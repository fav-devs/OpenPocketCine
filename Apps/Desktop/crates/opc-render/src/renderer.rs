//! The feed pipeline.
//!
//! 1. `ycbcr.frag` converts the decoder's planes to RGB **at the source raster**.
//! 2. `peaking_blur.frag` and `peaking_mask.frag` build the edge mask, when peaking is on.
//! 3. `feed.frag` grades through the colour cube and paints zebra and peaking.
//! 4. `blit.frag` stretches the result to the display raster.
//! 5. `overlay.frag` composites the shell's chrome over it, into the image or the
//!    swapchain. The chrome never goes through the cube, so a HUD reads true.
//!
//! The order is the phones' order and is not an accident: cubing after the upsample
//! blotched D-Log2 on Android. Every pass but the first is the Android shell's own shader.

use std::io::Cursor;

use ash::{vk, Device};
use opc_decode::Picture;
use raw_window_handle::{RawDisplayHandle, RawWindowHandle};

use crate::device::Gpu;
use crate::error::{Context, RenderError};
use crate::lut::Lut;
use crate::present::{Presented, Presenter, Surface, SurfaceSource};
use crate::resources::{transition, DeviceImage, HostBuffer};

pub use crate::options::{GradeOptions, Peaking, PeakingSense, Zebra};

const COLOR_FORMAT: vk::Format = vk::Format::R8G8B8A8_UNORM;
const PLANE_FORMAT: vk::Format = vk::Format::R8_UNORM;
const LUT_FORMAT: vk::Format = vk::Format::R32G32B32A32_SFLOAT;
/// `feed.frag`'s push-constant block: 32 floats.
const FEED_CONSTANTS: usize = 32;
const MAX_RASTER: u32 = 8192;

const PASS_YCBCR: usize = 0;
const PASS_FEED: usize = 1;
const PASS_BLIT: usize = 2;
const PASS_PEAK_BLUR: usize = 3;
const PASS_PEAK_MASK: usize = 4;
/// Composites the shell's chrome over the stretched picture. Separate from the grade so
/// the HUD never goes through the colour cube.
const PASS_OVERLAY: usize = 5;
const PASS_COUNT: usize = 6;

/// Where a picture of `source` shape sits inside a `display` of another, keeping its
/// proportions and centring the remainder: `(x, y, width, height)` in display pixels.
///
/// The blit draws into exactly this rectangle, so the bars around it are the render
/// pass's own clear. A shell mapping a click back to the picture must use the same
/// rectangle or it will point the camera somewhere near what the operator meant.
pub fn letterbox(source: (u32, u32), display: (u32, u32)) -> (u32, u32, u32, u32) {
    if source.0 == 0 || source.1 == 0 || display.0 == 0 || display.1 == 0 {
        return (0, 0, display.0, display.1);
    }
    let scale = (f64::from(display.0) / f64::from(source.0))
        .min(f64::from(display.1) / f64::from(source.1));
    let width = ((f64::from(source.0) * scale).round() as u32).clamp(1, display.0);
    let height = ((f64::from(source.1) * scale).round() as u32).clamp(1, display.1);
    (
        (display.0 - width) / 2,
        (display.1 - height) / 2,
        width,
        height,
    )
}

/// A rendered picture, 8-bit RGBA, tightly packed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rgba {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl Rgba {
    /// The pixel at `(x, y)` as `(r, g, b, a)`.
    pub fn pixel(&self, x: u32, y: u32) -> Option<(u8, u8, u8, u8)> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let at = ((y * self.width + x) * 4) as usize;
        Some((
            self.pixels[at],
            self.pixels[at + 1],
            self.pixels[at + 2],
            self.pixels[at + 3],
        ))
    }
}

struct Targets {
    source_width: u32,
    source_height: u32,
    display_width: u32,
    display_height: u32,
    planes: [DeviceImage; 3],
    plane_staging: [HostBuffer; 3],
    rgb: DeviceImage,
    graded: DeviceImage,
    output: DeviceImage,
    peaking_blur: DeviceImage,
    peaking_mask: DeviceImage,
    /// The graded picture at the display raster, before chrome.
    stretched: DeviceImage,
    /// The chrome, uploaded from the shell when it changes.
    overlay: DeviceImage,
    overlay_staging: HostBuffer,
    /// Indexed by the `PASS_*` constants; `PASS_BLIT`'s is the offscreen target.
    framebuffers: [vk::Framebuffer; PASS_COUNT],
    readback: HostBuffer,
}

impl Targets {
    unsafe fn destroy(&self, device: &Device) {
        unsafe {
            for framebuffer in self.framebuffers {
                device.destroy_framebuffer(framebuffer, None);
            }
            for plane in &self.planes {
                plane.destroy(device);
            }
            for staging in &self.plane_staging {
                staging.destroy(device);
            }
            for image in [
                &self.rgb,
                &self.graded,
                &self.output,
                &self.peaking_blur,
                &self.peaking_mask,
                &self.stretched,
                &self.overlay,
            ] {
                image.destroy(device);
            }
            self.overlay_staging.destroy(device);
            self.readback.destroy(device);
        }
    }
}

struct Pipelines {
    render_pass: vk::RenderPass,
    linear: vk::Sampler,
    /// Packed 16-bit blur values and the edge mask must not be interpolated.
    nearest: vk::Sampler,
    set_layouts: [vk::DescriptorSetLayout; PASS_COUNT],
    layouts: [vk::PipelineLayout; PASS_COUNT],
    pipelines: [vk::Pipeline; PASS_COUNT],
    pool: vk::DescriptorPool,
    sets: [vk::DescriptorSet; PASS_COUNT],
    /// Kept alive so a swapchain can build its own final pipeline for its own format.
    vertex_module: vk::ShaderModule,
    overlay_module: vk::ShaderModule,
}

impl Pipelines {
    unsafe fn destroy(&self, device: &Device) {
        unsafe {
            for pipeline in self.pipelines {
                device.destroy_pipeline(pipeline, None);
            }
            for layout in self.layouts {
                device.destroy_pipeline_layout(layout, None);
            }
            for layout in self.set_layouts {
                device.destroy_descriptor_set_layout(layout, None);
            }
            device.destroy_descriptor_pool(self.pool, None);
            device.destroy_sampler(self.linear, None);
            device.destroy_sampler(self.nearest, None);
            device.destroy_render_pass(self.render_pass, None);
            device.destroy_shader_module(self.vertex_module, None);
            device.destroy_shader_module(self.overlay_module, None);
        }
    }
}

/// Draws decoded pictures, offscreen or into a window.
pub struct FeedRenderer {
    gpu: Gpu,
    pipelines: Pipelines,
    presenter: Option<Presenter>,
    targets: Option<Targets>,
    lut: Option<DeviceImage>,
    lut_size: u32,
    dummy_2d: DeviceImage,
    dummy_3d: DeviceImage,
    /// Descriptor writes need an idle device, so they happen only when something the
    /// sets point at actually changed — not once a frame.
    descriptors_dirty: bool,
    peaking_bound: bool,
    /// The chrome the shell last handed over, at the raster it drew it at.
    overlay: Option<Rgba>,
    /// Cleared when the chrome or the display raster changes, so a still HUD is copied
    /// to the staging buffer once rather than every frame.
    overlay_staged: bool,
    overlay_opacity: f32,
}

impl std::fmt::Debug for FeedRenderer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FeedRenderer")
            .field("device", &self.gpu.device_name)
            .field("lut_size", &self.lut_size)
            .field("presenting", &self.presenter.is_some())
            .finish_non_exhaustive()
    }
}

impl FeedRenderer {
    /// Renders into an image. No surface, no swapchain.
    pub fn new() -> Result<Self, RenderError> {
        Self::build(Gpu::offscreen()?, None, 0, 0)
    }

    /// A real swapchain with no display behind it, for checking the present path.
    pub fn headless_window(width: u32, height: u32) -> Result<Self, RenderError> {
        let instance = [
            ash::khr::surface::NAME.as_ptr(),
            ash::ext::headless_surface::NAME.as_ptr(),
        ];
        let device = [ash::khr::swapchain::NAME.as_ptr()];
        let gpu = Gpu::new(&instance, &device)?;
        Self::build(gpu, Some(SurfaceSource::Headless), width, height)
    }

    /// Draws into a window.
    ///
    /// # Safety
    /// Both handles must stay valid for as long as this renderer lives.
    pub unsafe fn for_window(
        display: RawDisplayHandle,
        window: RawWindowHandle,
        width: u32,
        height: u32,
    ) -> Result<Self, RenderError> {
        let required = ash_window::enumerate_required_extensions(display)
            .context("vkEnumerateInstanceExtensionProperties")?
            .to_vec();
        let device = [ash::khr::swapchain::NAME.as_ptr()];
        let gpu = Gpu::new(&required, &device)?;
        Self::build(
            gpu,
            Some(SurfaceSource::Window { display, window }),
            width,
            height,
        )
    }

    fn build(
        gpu: Gpu,
        source: Option<SurfaceSource>,
        width: u32,
        height: u32,
    ) -> Result<Self, RenderError> {
        let pipelines = build_pipelines(&gpu)?;
        // `feed.frag` samples all five bindings unconditionally, so the ones an operator
        // has turned off still need something bound. A 1x1 texture with the matching
        // `*On` flag at zero keeps the shared shader untouched.
        let dummy_2d = DeviceImage::new_2d(
            &gpu,
            1,
            1,
            COLOR_FORMAT,
            vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST,
        )?;
        let dummy_3d = DeviceImage::new_3d(
            &gpu,
            1,
            LUT_FORMAT,
            vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST,
        )?;
        gpu.one_shot(|command| {
            // Safety: recording, and both images belong to this device.
            unsafe {
                for image in [dummy_2d.image, dummy_3d.image] {
                    transition(
                        &gpu.device,
                        command,
                        image,
                        vk::ImageLayout::UNDEFINED,
                        vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                    );
                }
            }
        })?;

        let presenter = match source {
            Some(source) => {
                let surface = Surface::new(&gpu, &source)?;
                Some(Presenter::new(
                    &gpu,
                    surface,
                    pipelines.layouts[PASS_OVERLAY],
                    pipelines.vertex_module,
                    pipelines.overlay_module,
                    width,
                    height,
                )?)
            }
            None => None,
        };

        Ok(Self {
            gpu,
            pipelines,
            presenter,
            targets: None,
            lut: None,
            lut_size: 0,
            dummy_2d,
            dummy_3d,
            descriptors_dirty: true,
            peaking_bound: false,
            overlay: None,
            overlay_staged: false,
            overlay_opacity: 1.0,
        })
    }

    /// Which device is drawing. A CPU device means software Vulkan, not a bug.
    pub fn device_name(&self) -> &str {
        &self.gpu.device_name
    }

    /// The swapchain's current size, when there is one.
    pub fn surface_size(&self) -> Option<(u32, u32)> {
        self.presenter
            .as_ref()
            .map(|presenter| (presenter.extent.width, presenter.extent.height))
    }

    /// Tells the swapchain the window changed size. Rebuilt on the next present.
    pub fn resize(&mut self, width: u32, height: u32) {
        if let Some(presenter) = self.presenter.as_mut() {
            presenter.resize(width, height);
        }
    }

    /// Uploads the cube the grade uses, or clears it. Passing `None` leaves the picture
    /// ungraded — the shader treats a lattice smaller than 2 as identity.
    pub fn set_lut(&mut self, lut: Option<&Lut>) -> Result<(), RenderError> {
        if let Some(existing) = self.lut.take() {
            // Safety: waiting for idle before destroying what a set may still point at.
            unsafe {
                let _ = self.gpu.device.device_wait_idle();
                existing.destroy(&self.gpu.device);
            }
        }
        self.lut_size = 0;
        self.descriptors_dirty = true;

        let Some(lut) = lut else { return Ok(()) };
        let size = lut.size();
        let components = lut.rgba();
        if size < 2 || components.len() != (size * size * size * 4) as usize {
            return Err(RenderError::Lut(
                "That cube has no usable lattice.".to_string(),
            ));
        }

        let image = DeviceImage::new_3d(
            &self.gpu,
            size,
            LUT_FORMAT,
            vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST,
        )?;
        let bytes = as_bytes(&components);
        let staging = HostBuffer::new(
            &self.gpu,
            bytes.len() as vk::DeviceSize,
            vk::BufferUsageFlags::TRANSFER_SRC,
        )?;
        staging.write_bytes(&self.gpu.device, bytes)?;

        let device = &self.gpu.device;
        let result = self.gpu.one_shot(|command| {
            // Safety: recording; the image and buffer belong to this device.
            unsafe {
                transition(
                    device,
                    command,
                    image.image,
                    vk::ImageLayout::UNDEFINED,
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                );
                copy_buffer_to_image(device, command, staging.buffer, &image, size, size, size);
                transition(
                    device,
                    command,
                    image.image,
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                );
            }
        });
        // Safety: the submission finished before `one_shot` returned.
        unsafe { staging.destroy(device) };
        result?;

        self.lut = Some(image);
        self.lut_size = size;
        Ok(())
    }

    /// Hands the shell's chrome over, to be composited after the stretch.
    ///
    /// The chrome is expected at the display raster. A mismatch is cropped or padded
    /// rather than refused: a window that resized between the shell drawing and this
    /// call should cost one slightly wrong frame, not an error the caller must handle.
    pub fn set_overlay(&mut self, chrome: &Rgba) {
        let matches = self
            .overlay
            .as_ref()
            .is_some_and(|current| current == chrome);
        if matches {
            return;
        }
        self.overlay = Some(chrome.clone());
        self.overlay_staged = false;
    }

    /// Drops the chrome. The next frame is the picture alone.
    pub fn clear_overlay(&mut self) {
        if self.overlay.is_none() {
            return;
        }
        self.overlay = None;
        self.overlay_staged = false;
    }

    /// How strongly the chrome is painted, `0.0`–`1.0`. The picture is never dimmed.
    pub fn set_overlay_opacity(&mut self, opacity: f32) {
        self.overlay_opacity = opacity.clamp(0.0, 1.0);
    }

    /// Converts, grades, and stretches one picture into an image.
    pub fn render(
        &mut self,
        picture: &Picture<'_>,
        display: (u32, u32),
        options: GradeOptions,
    ) -> Result<Rgba, RenderError> {
        let (display_width, display_height) = display;
        self.prepare(picture, display_width, display_height, options)?;

        let targets = self.targets.as_ref().expect("targets were just prepared");
        let device = &self.gpu.device;
        let pipelines = &self.pipelines;
        let lut_size = self.lut_size;
        let overlay_opacity = self.overlay_opacity;
        self.gpu.one_shot(|command| {
            record_frame(
                device,
                pipelines,
                targets,
                command,
                picture,
                options,
                lut_size,
                targets.framebuffers[PASS_OVERLAY],
                vk::Extent2D {
                    width: display_width,
                    height: display_height,
                },
                pipelines.pipelines[PASS_OVERLAY],
                overlay_opacity,
            );
            // Safety: the passes above left `output` readable by shaders; move it to a
            // transfer source and copy it back to host memory.
            unsafe {
                transition(
                    device,
                    command,
                    targets.output.image,
                    vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                    vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                );
                let region = vk::BufferImageCopy::default()
                    .image_subresource(
                        vk::ImageSubresourceLayers::default()
                            .aspect_mask(vk::ImageAspectFlags::COLOR)
                            .layer_count(1),
                    )
                    .image_extent(vk::Extent3D {
                        width: display_width,
                        height: display_height,
                        depth: 1,
                    });
                device.cmd_copy_image_to_buffer(
                    command,
                    targets.output.image,
                    vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                    targets.readback.buffer,
                    &[region],
                );
            }
        })?;

        let length = (display_width * display_height * 4) as usize;
        let pixels = targets.readback.read_bytes(&self.gpu.device, length)?;
        Ok(Rgba {
            width: display_width,
            height: display_height,
            pixels,
        })
    }

    /// Draws one picture to the window.
    ///
    /// `Presented::Rebuilt` means the swapchain changed and nothing was shown — call
    /// again with the same picture.
    pub fn present(
        &mut self,
        picture: &Picture<'_>,
        options: GradeOptions,
    ) -> Result<Presented, RenderError> {
        let Some((width, height)) = self.surface_size() else {
            return Err(RenderError::NotPresenting);
        };
        if width == 0 || height == 0 {
            return Ok(Presented::Rebuilt);
        }
        self.prepare(picture, width, height, options)?;

        let presenter = self
            .presenter
            .as_mut()
            .expect("a presenter was just checked");
        let Some((index, command)) = presenter.begin(&self.gpu)? else {
            return Ok(Presented::Rebuilt);
        };
        let framebuffer = presenter.framebuffer(index);
        let extent = presenter.extent;
        let pipeline = presenter.pipeline;

        let targets = self.targets.as_ref().expect("targets were just prepared");
        record_frame(
            &self.gpu.device,
            &self.pipelines,
            targets,
            command,
            picture,
            options,
            self.lut_size,
            framebuffer,
            extent,
            pipeline,
            self.overlay_opacity,
        );

        let presenter = self.presenter.as_mut().expect("still presenting");
        presenter.end(&self.gpu, index)
    }

    /// Sizes resources, uploads the planes' staging copies, and refreshes descriptors.
    fn prepare(
        &mut self,
        picture: &Picture<'_>,
        display_width: u32,
        display_height: u32,
        options: GradeOptions,
    ) -> Result<(), RenderError> {
        if picture.width == 0
            || picture.height == 0
            || picture.width > MAX_RASTER
            || picture.height > MAX_RASTER
        {
            return Err(RenderError::UnsupportedRaster {
                width: picture.width,
                height: picture.height,
            });
        }
        if display_width == 0
            || display_height == 0
            || display_width > MAX_RASTER
            || display_height > MAX_RASTER
        {
            return Err(RenderError::UnsupportedRaster {
                width: display_width,
                height: display_height,
            });
        }

        self.ensure_targets(picture, display_width, display_height)?;
        if options.peaking_on() != self.peaking_bound {
            self.peaking_bound = options.peaking_on();
            self.descriptors_dirty = true;
        }
        if self.descriptors_dirty {
            self.write_descriptors();
            self.descriptors_dirty = false;
        }
        // Plane and overlay staging are single shared host buffers. `prepare` runs
        // before the presenter waits its next frame fence, so writing them here while
        // an older submission is copying them is a host/GPU race. It showed up as a
        // corrupted bottom row on Windows. Keep the producer lifetime explicit until
        // staging is made per-frame; correctness beats sampling a half-written plane.
        unsafe {
            self.gpu
                .device
                .device_wait_idle()
                .context("vkDeviceWaitIdle before staging")?;
        }
        self.stage_planes(picture)?;
        self.stage_overlay()
    }

    fn ensure_targets(
        &mut self,
        picture: &Picture<'_>,
        display_width: u32,
        display_height: u32,
    ) -> Result<(), RenderError> {
        let matches = self.targets.as_ref().is_some_and(|targets| {
            targets.source_width == picture.width
                && targets.source_height == picture.height
                && targets.display_width == display_width
                && targets.display_height == display_height
        });
        if matches {
            return Ok(());
        }
        if let Some(old) = self.targets.take() {
            // Safety: waiting for idle before destroying what a set may still point at.
            unsafe {
                let _ = self.gpu.device.device_wait_idle();
                old.destroy(&self.gpu.device);
            }
        }
        self.targets = Some(build_targets(
            &self.gpu,
            &self.pipelines,
            picture,
            display_width,
            display_height,
        )?);
        self.descriptors_dirty = true;
        self.overlay_staged = false;
        Ok(())
    }

    /// Packs the decoder's padded rows into the staging buffers. The GPU copy itself is
    /// recorded with the frame so a present does not need a second submission.
    fn stage_planes(&self, picture: &Picture<'_>) -> Result<(), RenderError> {
        let targets = self.targets.as_ref().expect("targets were ensured");
        let (chroma_width, chroma_height) = picture.chroma_size();
        let planes: [(&[u8], usize, u32, u32); 3] = [
            (
                picture.luma,
                picture.luma_stride,
                picture.width,
                picture.height,
            ),
            (
                picture.chroma_blue,
                picture.chroma_stride,
                chroma_width,
                chroma_height,
            ),
            (
                picture.chroma_red,
                picture.chroma_stride,
                chroma_width,
                chroma_height,
            ),
        ];
        for (index, (source, stride, width, height)) in planes.iter().enumerate() {
            let mut packed = Vec::with_capacity((*width as usize) * (*height as usize));
            for row in 0..*height as usize {
                let start = row * stride;
                packed.extend_from_slice(&source[start..start + *width as usize]);
            }
            targets.plane_staging[index].write_bytes(&self.gpu.device, &packed)?;
        }
        Ok(())
    }

    /// Packs the chrome into its staging buffer at the display raster. Rows and columns
    /// the chrome does not reach stay transparent, so a stale HUD cannot smear.
    fn stage_overlay(&mut self) -> Result<(), RenderError> {
        if self.overlay_staged {
            return Ok(());
        }
        let targets = self.targets.as_ref().expect("targets were ensured");
        let width = targets.display_width as usize;
        let height = targets.display_height as usize;
        let mut packed = vec![0_u8; width * height * 4];
        if let Some(chrome) = self.overlay.as_ref() {
            let rows = height.min(chrome.height as usize);
            let columns = width.min(chrome.width as usize);
            for row in 0..rows {
                let from = row * chrome.width as usize * 4;
                let to = row * width * 4;
                packed[to..to + columns * 4]
                    .copy_from_slice(&chrome.pixels[from..from + columns * 4]);
            }
        }
        targets
            .overlay_staging
            .write_bytes(&self.gpu.device, &packed)?;
        self.overlay_staged = true;
        Ok(())
    }

    fn write_descriptors(&self) {
        let targets = self.targets.as_ref().expect("targets were ensured");
        let linear = self.pipelines.linear;
        let nearest = self.pipelines.nearest;
        let lut_view = self.lut.as_ref().unwrap_or(&self.dummy_3d).view;
        let mask_view = if self.peaking_bound {
            targets.peaking_mask.view
        } else {
            self.dummy_2d.view
        };

        let bind = |view: vk::ImageView, sampler: vk::Sampler| {
            [vk::DescriptorImageInfo::default()
                .sampler(sampler)
                .image_view(view)
                .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)]
        };

        let ycbcr: Vec<_> = targets
            .planes
            .iter()
            .map(|plane| bind(plane.view, linear))
            .collect();
        let feed = [
            bind(targets.rgb.view, linear),
            bind(lut_view, linear),
            bind(self.dummy_3d.view, linear),
            bind(self.dummy_3d.view, linear),
            bind(mask_view, nearest),
        ];
        let blit = bind(targets.graded.view, linear);
        // The chrome is rasterised at the display raster, so sampling it is 1:1 —
        // a linear filter would only soften text the operator needs to read.
        let overlay = [
            bind(targets.stretched.view, linear),
            bind(targets.overlay.view, nearest),
        ];
        let peak_blur = bind(targets.rgb.view, linear);
        let peak_mask = [
            bind(targets.rgb.view, linear),
            bind(targets.peaking_blur.view, nearest),
        ];

        // Built inline rather than through a helper: each `image_info` borrows the
        // array above it, and a closure would shorten that borrow to its own body.
        let mut writes = Vec::with_capacity(14);
        for (binding, image) in ycbcr.iter().enumerate() {
            writes.push(
                vk::WriteDescriptorSet::default()
                    .dst_set(self.pipelines.sets[PASS_YCBCR])
                    .dst_binding(binding as u32)
                    .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                    .image_info(image),
            );
        }
        for (binding, image) in feed.iter().enumerate() {
            writes.push(
                vk::WriteDescriptorSet::default()
                    .dst_set(self.pipelines.sets[PASS_FEED])
                    .dst_binding(binding as u32)
                    .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                    .image_info(image),
            );
        }
        for (binding, image) in overlay.iter().enumerate() {
            writes.push(
                vk::WriteDescriptorSet::default()
                    .dst_set(self.pipelines.sets[PASS_OVERLAY])
                    .dst_binding(binding as u32)
                    .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                    .image_info(image),
            );
        }
        for (binding, image) in peak_mask.iter().enumerate() {
            writes.push(
                vk::WriteDescriptorSet::default()
                    .dst_set(self.pipelines.sets[PASS_PEAK_MASK])
                    .dst_binding(binding as u32)
                    .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                    .image_info(image),
            );
        }
        writes.push(
            vk::WriteDescriptorSet::default()
                .dst_set(self.pipelines.sets[PASS_BLIT])
                .dst_binding(0)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .image_info(&blit),
        );
        writes.push(
            vk::WriteDescriptorSet::default()
                .dst_set(self.pipelines.sets[PASS_PEAK_BLUR])
                .dst_binding(0)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .image_info(&peak_blur),
        );

        // Safety: descriptors may not be rewritten while a frame that uses them is in
        // flight, so this waits first. It runs only when a bound resource changed.
        unsafe {
            let _ = self.gpu.device.device_wait_idle();
            self.gpu.device.update_descriptor_sets(&writes, &[]);
        }
    }
}

impl Drop for FeedRenderer {
    fn drop(&mut self) {
        // Safety: waiting for idle first, then destroying children before the device,
        // which `Gpu::drop` handles after this runs.
        unsafe {
            let _ = self.gpu.device.device_wait_idle();
            if let Some(mut presenter) = self.presenter.take() {
                presenter.destroy(&self.gpu);
            }
            if let Some(targets) = self.targets.take() {
                targets.destroy(&self.gpu.device);
            }
            if let Some(lut) = self.lut.take() {
                lut.destroy(&self.gpu.device);
            }
            self.dummy_2d.destroy(&self.gpu.device);
            self.dummy_3d.destroy(&self.gpu.device);
            self.pipelines.destroy(&self.gpu.device);
        }
    }
}

/// Records plane uploads and every pass into `command`, ending in `target`.
#[allow(clippy::too_many_arguments)]
fn record_frame(
    device: &Device,
    pipelines: &Pipelines,
    targets: &Targets,
    command: vk::CommandBuffer,
    picture: &Picture<'_>,
    options: GradeOptions,
    lut_size: u32,
    target: vk::Framebuffer,
    target_extent: vk::Extent2D,
    target_pipeline: vk::Pipeline,
    overlay_opacity: f32,
) {
    let (chroma_width, chroma_height) = picture.chroma_size();
    let plane_sizes = [
        (picture.width, picture.height),
        (chroma_width, chroma_height),
        (chroma_width, chroma_height),
    ];
    for (index, (width, height)) in plane_sizes.iter().enumerate() {
        let image = &targets.planes[index];
        // Safety: recording; every handle belongs to this device.
        unsafe {
            transition(
                device,
                command,
                image.image,
                vk::ImageLayout::UNDEFINED,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            );
            copy_buffer_to_image(
                device,
                command,
                targets.plane_staging[index].buffer,
                image,
                *width,
                *height,
                1,
            );
            transition(
                device,
                command,
                image.image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            );
        }
    }

    // Safety: recording; the chrome's buffer and image belong to this device. The copy
    // runs every frame even when the chrome did not change, because the image's contents
    // are not preserved across the transition out of `UNDEFINED`.
    unsafe {
        transition(
            device,
            command,
            targets.overlay.image,
            vk::ImageLayout::UNDEFINED,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
        );
        copy_buffer_to_image(
            device,
            command,
            targets.overlay_staging.buffer,
            &targets.overlay,
            targets.display_width,
            targets.display_height,
            1,
        );
        transition(
            device,
            command,
            targets.overlay.image,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        );
    }

    let source = vk::Extent2D {
        width: picture.width,
        height: picture.height,
    };
    let source_constants: [f32; 2] = [picture.width as f32, picture.height as f32];

    draw(
        device,
        pipelines,
        command,
        PASS_YCBCR,
        targets.framebuffers[PASS_YCBCR],
        source,
        pipelines.pipelines[PASS_YCBCR],
        // FFmpeg reports 8-bit 4:2:0 from this encoder as limited range.
        &[source_constants[0], source_constants[1], 0.0, 0.0],
        None,
    );

    if let Some(peaking) = options.peaking {
        draw(
            device,
            pipelines,
            command,
            PASS_PEAK_BLUR,
            targets.framebuffers[PASS_PEAK_BLUR],
            source,
            pipelines.pipelines[PASS_PEAK_BLUR],
            &source_constants,
            None,
        );
        draw(
            device,
            pipelines,
            command,
            PASS_PEAK_MASK,
            targets.framebuffers[PASS_PEAK_MASK],
            source,
            pipelines.pipelines[PASS_PEAK_MASK],
            &[
                source_constants[0],
                source_constants[1],
                peaking.sense.ratio_threshold(),
                peaking.sense.noise_gate(),
            ],
            None,
        );
    }

    draw(
        device,
        pipelines,
        command,
        PASS_FEED,
        targets.framebuffers[PASS_FEED],
        source,
        pipelines.pipelines[PASS_FEED],
        &feed_constants(picture, target_extent, options, lut_size),
        None,
    );

    // The picture keeps its proportions; the render pass's clear paints the bars. A
    // stretched face is a framing decision an operator would make wrongly.
    let (x, y, width, height) = letterbox(
        (picture.width, picture.height),
        (target_extent.width, target_extent.height),
    );
    let fit = vk::Rect2D {
        offset: vk::Offset2D {
            x: x as i32,
            y: y as i32,
        },
        extent: vk::Extent2D { width, height },
    };

    draw(
        device,
        pipelines,
        command,
        PASS_BLIT,
        targets.framebuffers[PASS_BLIT],
        target_extent,
        pipelines.pipelines[PASS_BLIT],
        &[1.0, 0.0],
        Some(fit),
    );

    draw(
        device,
        pipelines,
        command,
        PASS_OVERLAY,
        target,
        target_extent,
        target_pipeline,
        &[overlay_opacity, 0.0],
        None,
    );
}

fn feed_constants(
    picture: &Picture<'_>,
    display: vk::Extent2D,
    options: GradeOptions,
    lut_size: u32,
) -> [f32; FEED_CONSTANTS] {
    let mut out = [0.0_f32; FEED_CONSTANTS];
    out[0] = picture.width as f32;
    out[1] = picture.height as f32;
    out[2] = display.width as f32;
    out[3] = display.height as f32;
    out[4] = lut_size as f32;
    out[8] = f32::from(u8::from(options.split));
    out[9] = f32::from(u8::from(options.split_vertical));
    out[15] = f32::from(u8::from(options.upscale));
    out[16] = f32::from(u8::from(options.mirror));
    if let Some(zebra) = options.zebra.filter(|zebra| !zebra.is_off()) {
        if let Some(threshold) = zebra.highlight {
            out[10] = 1.0;
            out[11] = threshold;
        }
        if let Some((centre, half_width)) = zebra.midtone {
            out[12] = 1.0;
            out[13] = centre;
            out[14] = half_width;
        }
        out[20..24].copy_from_slice(&zebra.highlight_color);
        out[24..28].copy_from_slice(&zebra.midtone_color);
    }
    if let Some(peaking) = options.peaking {
        out[17] = 1.0;
        out[28..32].copy_from_slice(&peaking.color);
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn draw(
    device: &Device,
    pipelines: &Pipelines,
    command: vk::CommandBuffer,
    pass: usize,
    framebuffer: vk::Framebuffer,
    extent: vk::Extent2D,
    pipeline: vk::Pipeline,
    constants: &[f32],
    // Where inside `extent` the triangle lands. `None` fills it.
    into: Option<vk::Rect2D>,
) {
    let clear = [vk::ClearValue {
        color: vk::ClearColorValue {
            float32: [0.0, 0.0, 0.0, 1.0],
        },
    }];
    let area = vk::Rect2D {
        offset: vk::Offset2D { x: 0, y: 0 },
        extent,
    };
    let painted = into.unwrap_or(area);
    let begin = vk::RenderPassBeginInfo::default()
        .render_pass(pipelines.render_pass)
        .framebuffer(framebuffer)
        .render_area(area)
        .clear_values(&clear);
    let viewport = vk::Viewport {
        x: painted.offset.x as f32,
        y: painted.offset.y as f32,
        width: painted.extent.width as f32,
        height: painted.extent.height as f32,
        min_depth: 0.0,
        max_depth: 1.0,
    };
    // Safety: recording; every handle belongs to this device and the constants match the
    // range declared on this pass's pipeline layout.
    unsafe {
        device.cmd_begin_render_pass(command, &begin, vk::SubpassContents::INLINE);
        device.cmd_bind_pipeline(command, vk::PipelineBindPoint::GRAPHICS, pipeline);
        device.cmd_set_viewport(command, 0, &[viewport]);
        device.cmd_set_scissor(command, 0, &[painted]);
        device.cmd_bind_descriptor_sets(
            command,
            vk::PipelineBindPoint::GRAPHICS,
            pipelines.layouts[pass],
            0,
            &[pipelines.sets[pass]],
            &[],
        );
        device.cmd_push_constants(
            command,
            pipelines.layouts[pass],
            vk::ShaderStageFlags::FRAGMENT,
            0,
            as_bytes(constants),
        );
        device.cmd_draw(command, 3, 1, 0, 0);
        device.cmd_end_render_pass(command);
    }
}

/// Safety: `command` must be recording and every handle must belong to `device`.
unsafe fn copy_buffer_to_image(
    device: &Device,
    command: vk::CommandBuffer,
    buffer: vk::Buffer,
    image: &DeviceImage,
    width: u32,
    height: u32,
    depth: u32,
) {
    let region = vk::BufferImageCopy::default()
        .image_subresource(
            vk::ImageSubresourceLayers::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                .layer_count(1),
        )
        .image_extent(vk::Extent3D {
            width,
            height,
            depth,
        });
    unsafe {
        device.cmd_copy_buffer_to_image(
            command,
            buffer,
            image.image,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            &[region],
        );
    }
}

/// Reinterprets a float slice as the bytes a push constant or upload wants.
fn as_bytes(values: &[f32]) -> &[u8] {
    // Safety: `f32` has no padding or invalid bit patterns, and the result borrows the
    // same memory for the same lifetime with a smaller alignment requirement.
    unsafe {
        std::slice::from_raw_parts(values.as_ptr().cast::<u8>(), std::mem::size_of_val(values))
    }
}

fn shader_module(device: &Device, spirv: &[u8]) -> Result<vk::ShaderModule, RenderError> {
    let code = ash::util::read_spv(&mut Cursor::new(spirv))
        .map_err(|_| RenderError::NoVulkan("a feed shader is not valid SPIR-V".to_string()))?;
    let info = vk::ShaderModuleCreateInfo::default().code(&code);
    // Safety: `code` outlives the call and the device is open.
    unsafe { device.create_shader_module(&info, None) }.context("vkCreateShaderModule")
}

/// Builds the final pipeline against an arbitrary render pass, so a swapchain can have
/// one in its own colour format.
pub(crate) fn present_pipeline(
    device: &Device,
    render_pass: vk::RenderPass,
    layout: vk::PipelineLayout,
    vertex: vk::ShaderModule,
    fragment: vk::ShaderModule,
) -> Result<vk::Pipeline, RenderError> {
    let mut created = create_pipelines(device, render_pass, &[layout], vertex, &[fragment])?;
    Ok(created.remove(0))
}

fn sampler(device: &Device, filter: vk::Filter) -> Result<vk::Sampler, RenderError> {
    let info = vk::SamplerCreateInfo::default()
        .mag_filter(filter)
        .min_filter(filter)
        .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
        .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
        .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE);
    // Safety: the device is open.
    unsafe { device.create_sampler(&info, None) }.context("vkCreateSampler")
}

fn build_pipelines(gpu: &Gpu) -> Result<Pipelines, RenderError> {
    let device = &gpu.device;

    let attachment = [vk::AttachmentDescription::default()
        .format(COLOR_FORMAT)
        .samples(vk::SampleCountFlags::TYPE_1)
        .load_op(vk::AttachmentLoadOp::CLEAR)
        .store_op(vk::AttachmentStoreOp::STORE)
        .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
        .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
        .initial_layout(vk::ImageLayout::UNDEFINED)
        .final_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
    let reference = [vk::AttachmentReference::default()
        .attachment(0)
        .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)];
    let subpass = [vk::SubpassDescription::default()
        .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
        .color_attachments(&reference)];
    let pass_info = vk::RenderPassCreateInfo::default()
        .attachments(&attachment)
        .subpasses(&subpass);
    // Safety: every borrowed slice outlives the call.
    let render_pass =
        unsafe { device.create_render_pass(&pass_info, None) }.context("vkCreateRenderPass")?;

    let linear = sampler(device, vk::Filter::LINEAR)?;
    let nearest = sampler(device, vk::Filter::NEAREST)?;

    // Bindings per pass, indexed by `PASS_*`.
    let counts = [3_u32, 5, 1, 1, 2, 2];
    let mut set_layouts = [vk::DescriptorSetLayout::null(); PASS_COUNT];
    for (index, count) in counts.iter().enumerate() {
        let bindings: Vec<_> = (0..*count)
            .map(|binding| {
                vk::DescriptorSetLayoutBinding::default()
                    .binding(binding)
                    .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                    .descriptor_count(1)
                    .stage_flags(vk::ShaderStageFlags::FRAGMENT)
            })
            .collect();
        let info = vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings);
        // Safety: `bindings` outlives the call.
        set_layouts[index] = unsafe { device.create_descriptor_set_layout(&info, None) }
            .context("vkCreateDescriptorSetLayout")?;
    }

    let constant_sizes = [16_u32, (FEED_CONSTANTS * 4) as u32, 8, 8, 16, 8];
    let mut layouts = [vk::PipelineLayout::null(); PASS_COUNT];
    for (index, size) in constant_sizes.iter().enumerate() {
        let range = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::FRAGMENT)
            .offset(0)
            .size(*size)];
        let single = [set_layouts[index]];
        let info = vk::PipelineLayoutCreateInfo::default()
            .set_layouts(&single)
            .push_constant_ranges(&range);
        // Safety: both borrowed slices outlive the call.
        layouts[index] = unsafe { device.create_pipeline_layout(&info, None) }
            .context("vkCreatePipelineLayout")?;
    }

    let vertex_module = shader_module(
        device,
        include_bytes!(concat!(env!("OUT_DIR"), "/fullscreen.vert.spv")),
    )?;
    let overlay_module = shader_module(
        device,
        include_bytes!(concat!(env!("OUT_DIR"), "/overlay.frag.spv")),
    )?;
    let fragments = [
        shader_module(
            device,
            include_bytes!(concat!(env!("OUT_DIR"), "/ycbcr.frag.spv")),
        )?,
        shader_module(
            device,
            include_bytes!(concat!(env!("OUT_DIR"), "/feed.frag.spv")),
        )?,
        shader_module(
            device,
            include_bytes!(concat!(env!("OUT_DIR"), "/blit.frag.spv")),
        )?,
        shader_module(
            device,
            include_bytes!(concat!(env!("OUT_DIR"), "/peaking_blur.frag.spv")),
        )?,
        shader_module(
            device,
            include_bytes!(concat!(env!("OUT_DIR"), "/peaking_mask.frag.spv")),
        )?,
        overlay_module,
    ];

    let built = create_pipelines(device, render_pass, &layouts, vertex_module, &fragments);
    // Safety: modules other than the two kept for the swapchain may go once the
    // pipelines referencing them exist.
    unsafe {
        for (index, module) in fragments.iter().enumerate() {
            if index != PASS_OVERLAY {
                device.destroy_shader_module(*module, None);
            }
        }
    }
    let built = built?;
    let pipelines: [vk::Pipeline; PASS_COUNT] = built.try_into().expect("one pipeline per pass");

    let sizes = [vk::DescriptorPoolSize::default()
        .ty(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
        .descriptor_count(counts.iter().sum())];
    let pool_info = vk::DescriptorPoolCreateInfo::default()
        .pool_sizes(&sizes)
        .max_sets(PASS_COUNT as u32);
    // Safety: `sizes` outlives the call.
    let pool = unsafe { device.create_descriptor_pool(&pool_info, None) }
        .context("vkCreateDescriptorPool")?;
    let allocate = vk::DescriptorSetAllocateInfo::default()
        .descriptor_pool(pool)
        .set_layouts(&set_layouts);
    // Safety: the pool has room for exactly these sets.
    let allocated = unsafe { device.allocate_descriptor_sets(&allocate) }
        .context("vkAllocateDescriptorSets")?;
    let sets: [vk::DescriptorSet; PASS_COUNT] = allocated.try_into().expect("one set per pass");

    Ok(Pipelines {
        render_pass,
        linear,
        nearest,
        set_layouts,
        layouts,
        pipelines,
        pool,
        sets,
        vertex_module,
        overlay_module,
    })
}

fn create_pipelines(
    device: &Device,
    render_pass: vk::RenderPass,
    layouts: &[vk::PipelineLayout],
    vertex: vk::ShaderModule,
    fragments: &[vk::ShaderModule],
) -> Result<Vec<vk::Pipeline>, RenderError> {
    let vertex_input = vk::PipelineVertexInputStateCreateInfo::default();
    let assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
        .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
    let viewport = vk::PipelineViewportStateCreateInfo::default()
        .viewport_count(1)
        .scissor_count(1);
    let raster = vk::PipelineRasterizationStateCreateInfo::default()
        .polygon_mode(vk::PolygonMode::FILL)
        .cull_mode(vk::CullModeFlags::NONE)
        .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
        .line_width(1.0);
    let multisample = vk::PipelineMultisampleStateCreateInfo::default()
        .rasterization_samples(vk::SampleCountFlags::TYPE_1);
    let blend_attachment = [vk::PipelineColorBlendAttachmentState::default()
        .color_write_mask(vk::ColorComponentFlags::RGBA)];
    let blend = vk::PipelineColorBlendStateCreateInfo::default().attachments(&blend_attachment);
    let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
    let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);

    let stages: Vec<[vk::PipelineShaderStageCreateInfo<'_>; 2]> = fragments
        .iter()
        .map(|fragment| {
            [
                vk::PipelineShaderStageCreateInfo::default()
                    .stage(vk::ShaderStageFlags::VERTEX)
                    .module(vertex)
                    .name(c"main"),
                vk::PipelineShaderStageCreateInfo::default()
                    .stage(vk::ShaderStageFlags::FRAGMENT)
                    .module(*fragment)
                    .name(c"main"),
            ]
        })
        .collect();

    let infos: Vec<_> = stages
        .iter()
        .enumerate()
        .map(|(index, stage)| {
            vk::GraphicsPipelineCreateInfo::default()
                .stages(stage)
                .vertex_input_state(&vertex_input)
                .input_assembly_state(&assembly)
                .viewport_state(&viewport)
                .rasterization_state(&raster)
                .multisample_state(&multisample)
                .color_blend_state(&blend)
                .dynamic_state(&dynamic)
                .layout(layouts[index.min(layouts.len() - 1)])
                .render_pass(render_pass)
                .subpass(0)
        })
        .collect();

    // Safety: every borrowed state struct outlives this call.
    unsafe { device.create_graphics_pipelines(vk::PipelineCache::null(), &infos, None) }
        .map_err(|(_, result)| RenderError::Vulkan("vkCreateGraphicsPipelines", result))
}

fn build_targets(
    gpu: &Gpu,
    pipelines: &Pipelines,
    picture: &Picture<'_>,
    display_width: u32,
    display_height: u32,
) -> Result<Targets, RenderError> {
    let (chroma_width, chroma_height) = picture.chroma_size();
    let plane_sizes = [
        (picture.width, picture.height),
        (chroma_width, chroma_height),
        (chroma_width, chroma_height),
    ];
    let mut planes = Vec::with_capacity(3);
    let mut plane_staging = Vec::with_capacity(3);
    for (width, height) in plane_sizes {
        planes.push(DeviceImage::new_2d(
            gpu,
            width,
            height,
            PLANE_FORMAT,
            vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST,
        )?);
        plane_staging.push(HostBuffer::new(
            gpu,
            (width as vk::DeviceSize) * (height as vk::DeviceSize),
            vk::BufferUsageFlags::TRANSFER_SRC,
        )?);
    }

    let attachment_usage = vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::SAMPLED;
    let at_source = |gpu: &Gpu| {
        DeviceImage::new_2d(
            gpu,
            picture.width,
            picture.height,
            COLOR_FORMAT,
            attachment_usage,
        )
    };
    let rgb = at_source(gpu)?;
    let graded = at_source(gpu)?;
    let peaking_blur = at_source(gpu)?;
    let peaking_mask = at_source(gpu)?;
    let stretched = DeviceImage::new_2d(
        gpu,
        display_width,
        display_height,
        COLOR_FORMAT,
        attachment_usage,
    )?;
    let output = DeviceImage::new_2d(
        gpu,
        display_width,
        display_height,
        COLOR_FORMAT,
        attachment_usage | vk::ImageUsageFlags::TRANSFER_SRC,
    )?;
    // The chrome arrives already rasterised; the GPU only samples it.
    let overlay = DeviceImage::new_2d(
        gpu,
        display_width,
        display_height,
        COLOR_FORMAT,
        vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST,
    )?;
    let overlay_staging = HostBuffer::new(
        gpu,
        (display_width as vk::DeviceSize) * (display_height as vk::DeviceSize) * 4,
        vk::BufferUsageFlags::TRANSFER_SRC,
    )?;

    let mut framebuffers = [vk::Framebuffer::null(); PASS_COUNT];
    let attachments = [
        (PASS_YCBCR, &rgb),
        (PASS_FEED, &graded),
        (PASS_BLIT, &stretched),
        (PASS_PEAK_BLUR, &peaking_blur),
        (PASS_PEAK_MASK, &peaking_mask),
        (PASS_OVERLAY, &output),
    ];
    for (pass, image) in attachments {
        let views = [image.view];
        let info = vk::FramebufferCreateInfo::default()
            .render_pass(pipelines.render_pass)
            .attachments(&views)
            .width(image.width)
            .height(image.height)
            .layers(1);
        // Safety: `views` outlives the call and the view belongs to this device.
        framebuffers[pass] =
            unsafe { gpu.device.create_framebuffer(&info, None) }.context("vkCreateFramebuffer")?;
    }

    let readback = HostBuffer::new(
        gpu,
        (display_width as vk::DeviceSize) * (display_height as vk::DeviceSize) * 4,
        vk::BufferUsageFlags::TRANSFER_DST,
    )?;

    Ok(Targets {
        source_width: picture.width,
        source_height: picture.height,
        display_width,
        display_height,
        planes: [planes.remove(0), planes.remove(0), planes.remove(0)],
        plane_staging: [
            plane_staging.remove(0),
            plane_staging.remove(0),
            plane_staging.remove(0),
        ],
        rgb,
        graded,
        output,
        peaking_blur,
        peaking_mask,
        stretched,
        overlay,
        overlay_staging,
        framebuffers,
        readback,
    })
}

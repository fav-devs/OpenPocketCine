//! The three-pass feed pipeline.
//!
//! 1. `ycbcr.frag` converts the decoder's planes to RGB **at the source raster**.
//! 2. `feed.frag` grades that RGB through the colour cube, still at the source raster.
//! 3. `blit.frag` stretches the graded picture to the display raster.
//!
//! The order is the phones' order and is not an accident: cubing after the upsample
//! blotched D-Log2 on Android. Passes 2 and 3 are the Android shell's own shaders.

use std::io::Cursor;

use ash::{vk, Device};
use opc_decode::Picture;

use crate::device::Gpu;
use crate::error::{Context, RenderError};
use crate::lut::Lut;
use crate::resources::{transition, DeviceImage, HostBuffer};

const COLOR_FORMAT: vk::Format = vk::Format::R8G8B8A8_UNORM;
const PLANE_FORMAT: vk::Format = vk::Format::R8_UNORM;
const LUT_FORMAT: vk::Format = vk::Format::R32G32B32A32_SFLOAT;
/// `feed.frag`'s push-constant block: 32 floats.
const FEED_CONSTANTS: usize = 32;
const MAX_RASTER: u32 = 8192;

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

/// What the operator has turned on for this frame.
#[derive(Debug, Clone, Copy, Default)]
pub struct GradeOptions {
    /// Horizontal flip, the MIRROR assist.
    pub mirror: bool,
    /// Bicubic reconstruction when the display raster is larger than the source.
    pub upscale: bool,
    /// Show the cube on half the picture only.
    pub split: bool,
    /// Split down the middle rather than across it.
    pub split_vertical: bool,
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
    framebuffers: [vk::Framebuffer; 3],
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
            self.rgb.destroy(device);
            self.graded.destroy(device);
            self.output.destroy(device);
            self.readback.destroy(device);
        }
    }
}

struct Pipelines {
    render_pass: vk::RenderPass,
    sampler: vk::Sampler,
    set_layouts: [vk::DescriptorSetLayout; 3],
    layouts: [vk::PipelineLayout; 3],
    pipelines: [vk::Pipeline; 3],
    pool: vk::DescriptorPool,
    sets: [vk::DescriptorSet; 3],
}

/// Draws decoded pictures. Headless: the result is an image, not a window.
pub struct FeedRenderer {
    gpu: Gpu,
    pipelines: Pipelines,
    targets: Option<Targets>,
    lut: Option<DeviceImage>,
    lut_size: u32,
    dummy_2d: DeviceImage,
    dummy_3d: DeviceImage,
}

impl std::fmt::Debug for FeedRenderer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FeedRenderer")
            .field("device", &self.gpu.device_name)
            .field("lut_size", &self.lut_size)
            .finish_non_exhaustive()
    }
}

impl FeedRenderer {
    pub fn new() -> Result<Self, RenderError> {
        let gpu = Gpu::headless()?;
        let pipelines = build_pipelines(&gpu)?;
        // `feed.frag` samples all five bindings unconditionally, so the ones an operator
        // has turned off still need something bound. A 1x1 texture with the matching
        // `*On` flag at zero is the cheapest way to keep the shader untouched.
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
                transition(
                    &gpu.device,
                    command,
                    dummy_2d.image,
                    vk::ImageLayout::UNDEFINED,
                    vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                );
                transition(
                    &gpu.device,
                    command,
                    dummy_3d.image,
                    vk::ImageLayout::UNDEFINED,
                    vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                );
            }
        })?;

        Ok(Self {
            gpu,
            pipelines,
            targets: None,
            lut: None,
            lut_size: 0,
            dummy_2d,
            dummy_3d,
        })
    }

    /// Which device is drawing. A CPU device means software Vulkan, not a bug.
    pub fn device_name(&self) -> &str {
        &self.gpu.device_name
    }

    /// Uploads the cube the grade uses, or clears it. Passing `None` leaves the picture
    /// ungraded — the shader treats a lattice smaller than 2 as identity.
    pub fn set_lut(&mut self, lut: Option<&Lut>) -> Result<(), RenderError> {
        if let Some(existing) = self.lut.take() {
            // Safety: the queue is idle between renders.
            unsafe {
                let _ = self.gpu.device.device_wait_idle();
                existing.destroy(&self.gpu.device);
            }
        }
        self.lut_size = 0;

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
        let bytes: &[u8] = bytemuck_floats(&components);
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
                let region = vk::BufferImageCopy::default()
                    .image_subresource(
                        vk::ImageSubresourceLayers::default()
                            .aspect_mask(vk::ImageAspectFlags::COLOR)
                            .layer_count(1),
                    )
                    .image_extent(vk::Extent3D {
                        width: size,
                        height: size,
                        depth: size,
                    });
                device.cmd_copy_buffer_to_image(
                    command,
                    staging.buffer,
                    image.image,
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    &[region],
                );
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

    /// Converts, grades, and stretches one picture.
    pub fn render(
        &mut self,
        picture: &Picture<'_>,
        display: (u32, u32),
        options: GradeOptions,
    ) -> Result<Rgba, RenderError> {
        let (display_width, display_height) = display;
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
        self.upload_planes(picture)?;
        self.write_descriptors();
        self.record(picture, display_width, display_height, options)?;

        let targets = self.targets.as_ref().expect("targets were just ensured");
        let length = (display_width * display_height * 4) as usize;
        let pixels = targets.readback.read_bytes(&self.gpu.device, length)?;
        Ok(Rgba {
            width: display_width,
            height: display_height,
            pixels,
        })
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
            // Safety: the queue is idle between renders.
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
        Ok(())
    }

    fn upload_planes(&self, picture: &Picture<'_>) -> Result<(), RenderError> {
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
            // The decoder's rows are padded to its own stride; the upload is tight.
            let mut packed = Vec::with_capacity((*width as usize) * (*height as usize));
            for row in 0..*height as usize {
                let start = row * stride;
                packed.extend_from_slice(&source[start..start + *width as usize]);
            }
            targets.plane_staging[index].write_bytes(&self.gpu.device, &packed)?;
        }

        let device = &self.gpu.device;
        self.gpu.one_shot(|command| {
            for (index, (_, _, width, height)) in planes.iter().enumerate() {
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
                    let region = vk::BufferImageCopy::default()
                        .image_subresource(
                            vk::ImageSubresourceLayers::default()
                                .aspect_mask(vk::ImageAspectFlags::COLOR)
                                .layer_count(1),
                        )
                        .image_extent(vk::Extent3D {
                            width: *width,
                            height: *height,
                            depth: 1,
                        });
                    device.cmd_copy_buffer_to_image(
                        command,
                        targets.plane_staging[index].buffer,
                        image.image,
                        vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                        &[region],
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
        })
    }

    fn write_descriptors(&self) {
        let targets = self.targets.as_ref().expect("targets were ensured");
        let sampler = self.pipelines.sampler;
        let lut_view = self.lut.as_ref().unwrap_or(&self.dummy_3d).view;

        let info = |view: vk::ImageView| {
            vk::DescriptorImageInfo::default()
                .sampler(sampler)
                .image_view(view)
                .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
        };
        let ycbcr: Vec<_> = targets
            .planes
            .iter()
            .map(|plane| [info(plane.view)])
            .collect();
        let feed = [
            [info(targets.rgb.view)],
            [info(lut_view)],
            [info(self.dummy_3d.view)],
            [info(self.dummy_3d.view)],
            [info(self.dummy_2d.view)],
        ];
        let blit = [info(targets.graded.view)];

        let mut writes = Vec::with_capacity(9);
        for (binding, image) in ycbcr.iter().enumerate() {
            writes.push(
                vk::WriteDescriptorSet::default()
                    .dst_set(self.pipelines.sets[0])
                    .dst_binding(binding as u32)
                    .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                    .image_info(image),
            );
        }
        for (binding, image) in feed.iter().enumerate() {
            writes.push(
                vk::WriteDescriptorSet::default()
                    .dst_set(self.pipelines.sets[1])
                    .dst_binding(binding as u32)
                    .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                    .image_info(image),
            );
        }
        writes.push(
            vk::WriteDescriptorSet::default()
                .dst_set(self.pipelines.sets[2])
                .dst_binding(0)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .image_info(&blit),
        );

        // Safety: the queue is idle between renders, so no set is in use.
        unsafe {
            let _ = self.gpu.device.device_wait_idle();
            self.gpu.device.update_descriptor_sets(&writes, &[]);
        }
    }

    fn record(
        &self,
        picture: &Picture<'_>,
        display_width: u32,
        display_height: u32,
        options: GradeOptions,
    ) -> Result<(), RenderError> {
        let targets = self.targets.as_ref().expect("targets were ensured");
        let device = &self.gpu.device;
        let pipelines = &self.pipelines;

        let ycbcr_constants: [f32; 4] = [
            picture.width as f32,
            picture.height as f32,
            // FFmpeg reports 8-bit 4:2:0 from this encoder as limited range.
            0.0,
            0.0,
        ];
        let mut feed_constants = [0.0_f32; FEED_CONSTANTS];
        feed_constants[0] = picture.width as f32;
        feed_constants[1] = picture.height as f32;
        feed_constants[2] = display_width as f32;
        feed_constants[3] = display_height as f32;
        feed_constants[4] = self.lut_size as f32;
        feed_constants[8] = f32::from(u8::from(options.split));
        feed_constants[9] = f32::from(u8::from(options.split_vertical));
        feed_constants[15] = f32::from(u8::from(options.upscale));
        feed_constants[16] = f32::from(u8::from(options.mirror));
        let blit_constants: [f32; 2] = [1.0, 0.0];

        self.gpu.one_shot(|command| {
            let passes: [(usize, vk::Framebuffer, u32, u32, &[f32]); 3] = [
                (
                    0,
                    targets.framebuffers[0],
                    picture.width,
                    picture.height,
                    &ycbcr_constants,
                ),
                (
                    1,
                    targets.framebuffers[1],
                    picture.width,
                    picture.height,
                    &feed_constants,
                ),
                (
                    2,
                    targets.framebuffers[2],
                    display_width,
                    display_height,
                    &blit_constants,
                ),
            ];

            for (index, framebuffer, width, height, constants) in passes {
                let clear = [vk::ClearValue {
                    color: vk::ClearColorValue {
                        float32: [0.0, 0.0, 0.0, 1.0],
                    },
                }];
                let area = vk::Rect2D {
                    offset: vk::Offset2D { x: 0, y: 0 },
                    extent: vk::Extent2D { width, height },
                };
                let begin = vk::RenderPassBeginInfo::default()
                    .render_pass(pipelines.render_pass)
                    .framebuffer(framebuffer)
                    .render_area(area)
                    .clear_values(&clear);
                let viewport = vk::Viewport {
                    x: 0.0,
                    y: 0.0,
                    width: width as f32,
                    height: height as f32,
                    min_depth: 0.0,
                    max_depth: 1.0,
                };
                // Safety: recording; every handle belongs to this device and the push
                // constants match each pipeline layout's declared range.
                unsafe {
                    device.cmd_begin_render_pass(command, &begin, vk::SubpassContents::INLINE);
                    device.cmd_bind_pipeline(
                        command,
                        vk::PipelineBindPoint::GRAPHICS,
                        pipelines.pipelines[index],
                    );
                    device.cmd_set_viewport(command, 0, &[viewport]);
                    device.cmd_set_scissor(command, 0, &[area]);
                    device.cmd_bind_descriptor_sets(
                        command,
                        vk::PipelineBindPoint::GRAPHICS,
                        pipelines.layouts[index],
                        0,
                        &[pipelines.sets[index]],
                        &[],
                    );
                    device.cmd_push_constants(
                        command,
                        pipelines.layouts[index],
                        vk::ShaderStageFlags::FRAGMENT,
                        0,
                        bytemuck_floats(constants),
                    );
                    device.cmd_draw(command, 3, 1, 0, 0);
                    device.cmd_end_render_pass(command);
                }
            }

            // Safety: the render passes above left `output` readable by shaders; move it
            // to a transfer source and copy it back to host memory.
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
        })
    }
}

impl Drop for FeedRenderer {
    fn drop(&mut self) {
        // Safety: waiting for idle first, then destroying children before the device,
        // which `Gpu::drop` handles after this runs.
        unsafe {
            let _ = self.gpu.device.device_wait_idle();
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
            device.destroy_sampler(self.sampler, None);
            device.destroy_render_pass(self.render_pass, None);
        }
    }
}

/// Reinterprets a float slice as the bytes a push constant or upload wants.
fn bytemuck_floats(values: &[f32]) -> &[u8] {
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

    let sampler_info = vk::SamplerCreateInfo::default()
        .mag_filter(vk::Filter::LINEAR)
        .min_filter(vk::Filter::LINEAR)
        .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
        .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
        .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE);
    // Safety: the device is open.
    let sampler =
        unsafe { device.create_sampler(&sampler_info, None) }.context("vkCreateSampler")?;

    // Bindings per pass: three planes, the five `feed.frag` inputs, one for the stretch.
    let counts = [3_u32, 5, 1];
    let mut set_layouts = [vk::DescriptorSetLayout::null(); 3];
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

    let constant_sizes = [16_u32, (FEED_CONSTANTS * 4) as u32, 8];
    let mut layouts = [vk::PipelineLayout::null(); 3];
    for index in 0..3 {
        let range = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::FRAGMENT)
            .offset(0)
            .size(constant_sizes[index])];
        let single = [set_layouts[index]];
        let info = vk::PipelineLayoutCreateInfo::default()
            .set_layouts(&single)
            .push_constant_ranges(&range);
        // Safety: both borrowed slices outlive the call.
        layouts[index] = unsafe { device.create_pipeline_layout(&info, None) }
            .context("vkCreatePipelineLayout")?;
    }

    let vertex = shader_module(
        device,
        include_bytes!(concat!(env!("OUT_DIR"), "/fullscreen.vert.spv")),
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
    ];

    let pipelines = create_graphics_pipelines(device, render_pass, &layouts, vertex, &fragments);
    // Safety: modules may be destroyed once the pipelines referencing them exist.
    unsafe {
        device.destroy_shader_module(vertex, None);
        for module in fragments {
            device.destroy_shader_module(module, None);
        }
    }
    let pipelines = pipelines?;

    let sizes = [vk::DescriptorPoolSize::default()
        .ty(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
        .descriptor_count(counts.iter().sum())];
    let pool_info = vk::DescriptorPoolCreateInfo::default()
        .pool_sizes(&sizes)
        .max_sets(3);
    // Safety: `sizes` outlives the call.
    let pool = unsafe { device.create_descriptor_pool(&pool_info, None) }
        .context("vkCreateDescriptorPool")?;
    let allocate = vk::DescriptorSetAllocateInfo::default()
        .descriptor_pool(pool)
        .set_layouts(&set_layouts);
    // Safety: the pool has room for exactly these three sets.
    let allocated = unsafe { device.allocate_descriptor_sets(&allocate) }
        .context("vkAllocateDescriptorSets")?;

    Ok(Pipelines {
        render_pass,
        sampler,
        set_layouts,
        layouts,
        pipelines,
        pool,
        sets: [allocated[0], allocated[1], allocated[2]],
    })
}

fn create_graphics_pipelines(
    device: &Device,
    render_pass: vk::RenderPass,
    layouts: &[vk::PipelineLayout; 3],
    vertex: vk::ShaderModule,
    fragments: &[vk::ShaderModule; 3],
) -> Result<[vk::Pipeline; 3], RenderError> {
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
                .layout(layouts[index])
                .render_pass(render_pass)
                .subpass(0)
        })
        .collect();

    // Safety: every borrowed state struct outlives this call.
    let created =
        unsafe { device.create_graphics_pipelines(vk::PipelineCache::null(), &infos, None) }
            .map_err(|(_, result)| RenderError::Vulkan("vkCreateGraphicsPipelines", result))?;
    Ok([created[0], created[1], created[2]])
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
    let rgb = DeviceImage::new_2d(
        gpu,
        picture.width,
        picture.height,
        COLOR_FORMAT,
        attachment_usage,
    )?;
    let graded = DeviceImage::new_2d(
        gpu,
        picture.width,
        picture.height,
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

    let mut framebuffers = [vk::Framebuffer::null(); 3];
    for (index, image) in [&rgb, &graded, &output].into_iter().enumerate() {
        let views = [image.view];
        let info = vk::FramebufferCreateInfo::default()
            .render_pass(pipelines.render_pass)
            .attachments(&views)
            .width(image.width)
            .height(image.height)
            .layers(1);
        // Safety: `views` outlives the call and the view belongs to this device.
        framebuffers[index] =
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
        framebuffers,
        readback,
    })
}

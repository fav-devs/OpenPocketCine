//! Surface, swapchain, and the frame loop that puts a picture on a screen.
//!
//! The swapchain is exercised without a display through `VK_EXT_headless_surface`: the
//! acquire, submit, present, and recreate path is the same code a window drives, so the
//! part most likely to be wrong is the part that can be tested.
//!
//! Present mode prefers mailbox. A field monitor wants the freshest frame rather than a
//! queue of stale ones; FIFO is the guaranteed fallback.

use ash::khr::{surface, swapchain};
use ash::{vk, Device};
use raw_window_handle::{RawDisplayHandle, RawWindowHandle};

use crate::device::Gpu;
use crate::error::{Context, RenderError};

/// How many frames may be recorded before waiting on the oldest.
const FRAMES_IN_FLIGHT: usize = 2;

/// What happened to a present.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presented {
    /// The picture reached the screen.
    Shown,
    /// The swapchain was rebuilt — the window resized or the surface changed. The caller
    /// should draw again; nothing was shown this time.
    Rebuilt,
}

/// Where a surface comes from.
pub(crate) enum SurfaceSource {
    /// `VK_EXT_headless_surface`: a real swapchain with no display behind it.
    Headless,
    Window {
        display: RawDisplayHandle,
        window: RawWindowHandle,
    },
}

pub(crate) struct Surface {
    loader: surface::Instance,
    handle: vk::SurfaceKHR,
}

impl Surface {
    pub fn new(gpu: &Gpu, source: &SurfaceSource) -> Result<Self, RenderError> {
        let loader = surface::Instance::new(&gpu.entry, &gpu.instance);
        let handle = match source {
            SurfaceSource::Headless => {
                let headless = ash::ext::headless_surface::Instance::new(&gpu.entry, &gpu.instance);
                let info = vk::HeadlessSurfaceCreateInfoEXT::default();
                // Safety: the instance enabled `VK_EXT_headless_surface`.
                unsafe { headless.create_headless_surface(&info, None) }
                    .context("vkCreateHeadlessSurfaceEXT")?
            }
            SurfaceSource::Window { display, window } => {
                // Safety: the caller guarantees both handles outlive the surface, which
                // is the contract `FeedRenderer::for_window` passes on.
                unsafe {
                    ash_window::create_surface(&gpu.entry, &gpu.instance, *display, *window, None)
                }
                .context("vkCreateSurfaceKHR")?
            }
        };

        // Safety: `handle` and the queue family both belong to this instance.
        let supported = unsafe {
            loader.get_physical_device_surface_support(gpu.physical, gpu.queue_family, handle)
        }
        .context("vkGetPhysicalDeviceSurfaceSupportKHR")?;
        if !supported {
            // Safety: destroying the surface just created, before returning.
            unsafe { loader.destroy_surface(handle, None) };
            return Err(RenderError::NoPresentQueue);
        }
        Ok(Self { loader, handle })
    }

    fn format(&self, physical: vk::PhysicalDevice) -> Result<vk::SurfaceFormatKHR, RenderError> {
        // Safety: the surface belongs to the same instance as `physical`.
        let formats = unsafe {
            self.loader
                .get_physical_device_surface_formats(physical, self.handle)
        }
        .context("vkGetPhysicalDeviceSurfaceFormatsKHR")?;
        // Grade and blit both write plain 8-bit colour, so an sRGB swapchain would apply
        // a second transfer curve the phones do not.
        let preferred = [vk::Format::B8G8R8A8_UNORM, vk::Format::R8G8B8A8_UNORM];
        for wanted in preferred {
            if let Some(found) = formats.iter().find(|format| format.format == wanted) {
                return Ok(*found);
            }
        }
        formats.first().copied().ok_or(RenderError::NoSurfaceFormat)
    }

    fn present_mode(&self, physical: vk::PhysicalDevice) -> vk::PresentModeKHR {
        // Safety: the surface belongs to the same instance as `physical`.
        let modes = unsafe {
            self.loader
                .get_physical_device_surface_present_modes(physical, self.handle)
        }
        .unwrap_or_default();
        for wanted in [vk::PresentModeKHR::MAILBOX, vk::PresentModeKHR::IMMEDIATE] {
            if modes.contains(&wanted) {
                return wanted;
            }
        }
        // Always supported.
        vk::PresentModeKHR::FIFO
    }

    fn capabilities(
        &self,
        physical: vk::PhysicalDevice,
    ) -> Result<vk::SurfaceCapabilitiesKHR, RenderError> {
        // Safety: the surface belongs to the same instance as `physical`.
        unsafe {
            self.loader
                .get_physical_device_surface_capabilities(physical, self.handle)
        }
        .context("vkGetPhysicalDeviceSurfaceCapabilitiesKHR")
    }

    /// Safety: the device must be idle and nothing may still reference the surface.
    pub unsafe fn destroy(&self) {
        unsafe { self.loader.destroy_surface(self.handle, None) };
    }
}

struct Frame {
    image_available: vk::Semaphore,
    in_flight: vk::Fence,
    command: vk::CommandBuffer,
}

/// Owns the swapchain and the synchronisation around it.
pub(crate) struct Presenter {
    surface: Surface,
    loader: swapchain::Device,
    handle: vk::SwapchainKHR,
    pub render_pass: vk::RenderPass,
    pub pipeline: vk::Pipeline,
    pub extent: vk::Extent2D,
    format: vk::Format,
    views: Vec<vk::ImageView>,
    framebuffers: Vec<vk::Framebuffer>,
    /// One per swapchain image: present waits on the signal for *that* image, which a
    /// per-frame semaphore cannot promise once acquire starts returning them out of order.
    render_finished: Vec<vk::Semaphore>,
    image_in_flight: Vec<vk::Fence>,
    frames: Vec<Frame>,
    frame: usize,
    wanted: vk::Extent2D,
}

impl std::fmt::Debug for Presenter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Presenter")
            .field("extent", &self.extent)
            .field("images", &self.framebuffers.len())
            .finish_non_exhaustive()
    }
}

impl Presenter {
    pub fn new(
        gpu: &Gpu,
        surface: Surface,
        final_layout: vk::PipelineLayout,
        vertex: vk::ShaderModule,
        fragment: vk::ShaderModule,
        width: u32,
        height: u32,
    ) -> Result<Self, RenderError> {
        let loader = swapchain::Device::new(&gpu.instance, &gpu.device);
        let chosen = surface.format(gpu.physical)?;
        let render_pass = build_present_pass(&gpu.device, chosen.format)?;
        let pipeline = crate::renderer::present_pipeline(
            &gpu.device,
            render_pass,
            final_layout,
            vertex,
            fragment,
        )?;

        let mut presenter = Self {
            surface,
            loader,
            handle: vk::SwapchainKHR::null(),
            render_pass,
            pipeline,
            extent: vk::Extent2D { width, height },
            format: chosen.format,
            views: Vec::new(),
            framebuffers: Vec::new(),
            render_finished: Vec::new(),
            image_in_flight: Vec::new(),
            frames: Vec::new(),
            frame: 0,
            wanted: vk::Extent2D { width, height },
        };
        presenter.build_frames(gpu)?;
        presenter.rebuild(gpu)?;
        Ok(presenter)
    }

    fn build_frames(&mut self, gpu: &Gpu) -> Result<(), RenderError> {
        let allocate = vk::CommandBufferAllocateInfo::default()
            .command_pool(gpu.command_pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(FRAMES_IN_FLIGHT as u32);
        // Safety: the pool belongs to this device.
        let commands = unsafe { gpu.device.allocate_command_buffers(&allocate) }
            .context("vkAllocateCommandBuffers")?;
        for command in commands {
            let semaphore_info = vk::SemaphoreCreateInfo::default();
            let fence_info = vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED);
            // Safety: the device is open.
            let (image_available, in_flight) = unsafe {
                (
                    gpu.device
                        .create_semaphore(&semaphore_info, None)
                        .context("vkCreateSemaphore")?,
                    gpu.device
                        .create_fence(&fence_info, None)
                        .context("vkCreateFence")?,
                )
            };
            self.frames.push(Frame {
                image_available,
                in_flight,
                command,
            });
        }
        Ok(())
    }

    /// Asks for a new size. The swapchain is rebuilt on the next present.
    pub fn resize(&mut self, width: u32, height: u32) {
        self.wanted = vk::Extent2D { width, height };
    }

    pub fn needs_rebuild(&self) -> bool {
        self.wanted != self.extent
    }

    /// Tears down and recreates the swapchain and everything sized with it.
    pub fn rebuild(&mut self, gpu: &Gpu) -> Result<(), RenderError> {
        let capabilities = self.surface.capabilities(gpu.physical)?;
        // A surface that does not pin its own size lets the caller choose, clamped.
        let extent = if capabilities.current_extent.width == u32::MAX {
            vk::Extent2D {
                width: self.wanted.width.clamp(
                    capabilities.min_image_extent.width,
                    capabilities.max_image_extent.width,
                ),
                height: self.wanted.height.clamp(
                    capabilities.min_image_extent.height,
                    capabilities.max_image_extent.height,
                ),
            }
        } else {
            capabilities.current_extent
        };
        if extent.width == 0 || extent.height == 0 {
            // A minimised window: keep the old swapchain and draw nothing.
            self.extent = extent;
            return Ok(());
        }

        let mut count = capabilities.min_image_count + 1;
        if capabilities.max_image_count > 0 && count > capabilities.max_image_count {
            count = capabilities.max_image_count;
        }

        let old = self.handle;
        let info = vk::SwapchainCreateInfoKHR::default()
            .surface(self.surface.handle)
            .min_image_count(count)
            .image_format(self.format)
            .image_color_space(vk::ColorSpaceKHR::SRGB_NONLINEAR)
            .image_extent(extent)
            .image_array_layers(1)
            .image_usage(vk::ImageUsageFlags::COLOR_ATTACHMENT)
            .image_sharing_mode(vk::SharingMode::EXCLUSIVE)
            .pre_transform(capabilities.current_transform)
            .composite_alpha(vk::CompositeAlphaFlagsKHR::OPAQUE)
            .present_mode(self.surface.present_mode(gpu.physical))
            .clipped(true)
            .old_swapchain(old);

        // Safety: the device is idle here; every previous submission has been waited on.
        unsafe {
            let _ = gpu.device.device_wait_idle();
            self.release_swapchain_objects(&gpu.device);
        }
        // Safety: the create-info borrows nothing that dies before this call.
        let handle =
            unsafe { self.loader.create_swapchain(&info, None) }.context("vkCreateSwapchainKHR")?;
        if old != vk::SwapchainKHR::null() {
            // Safety: replaced above and no longer referenced.
            unsafe { self.loader.destroy_swapchain(old, None) };
        }
        self.handle = handle;
        self.extent = extent;
        self.wanted = extent;

        // Safety: the swapchain was just created.
        let images = unsafe { self.loader.get_swapchain_images(handle) }
            .context("vkGetSwapchainImagesKHR")?;
        for image in &images {
            let view_info = vk::ImageViewCreateInfo::default()
                .image(*image)
                .view_type(vk::ImageViewType::TYPE_2D)
                .format(self.format)
                .subresource_range(
                    vk::ImageSubresourceRange::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .level_count(1)
                        .layer_count(1),
                );
            // Safety: the image belongs to this swapchain.
            let view = unsafe { gpu.device.create_image_view(&view_info, None) }
                .context("vkCreateImageView")?;
            self.views.push(view);

            let attachments = [view];
            let framebuffer_info = vk::FramebufferCreateInfo::default()
                .render_pass(self.render_pass)
                .attachments(&attachments)
                .width(extent.width)
                .height(extent.height)
                .layers(1);
            // Safety: `attachments` outlives the call.
            let framebuffer = unsafe { gpu.device.create_framebuffer(&framebuffer_info, None) }
                .context("vkCreateFramebuffer")?;
            self.framebuffers.push(framebuffer);

            let semaphore_info = vk::SemaphoreCreateInfo::default();
            // Safety: the device is open.
            let semaphore = unsafe { gpu.device.create_semaphore(&semaphore_info, None) }
                .context("vkCreateSemaphore")?;
            self.render_finished.push(semaphore);
        }
        self.image_in_flight = vec![vk::Fence::null(); images.len()];
        Ok(())
    }

    /// Waits for the oldest frame in flight and takes the next swapchain image.
    ///
    /// `Ok(None)` means the swapchain was rebuilt and the caller should try again.
    pub fn begin(&mut self, gpu: &Gpu) -> Result<Option<(u32, vk::CommandBuffer)>, RenderError> {
        if self.needs_rebuild() || self.handle == vk::SwapchainKHR::null() {
            self.rebuild(gpu)?;
            return Ok(None);
        }
        if self.extent.width == 0 || self.extent.height == 0 {
            return Ok(None);
        }

        let frame = &self.frames[self.frame];
        // Safety: the fence belongs to this device and starts signalled.
        unsafe {
            gpu.device
                .wait_for_fences(&[frame.in_flight], true, u64::MAX)
        }
        .context("vkWaitForFences")?;

        // Safety: the swapchain and semaphore are live.
        let acquired = unsafe {
            self.loader.acquire_next_image(
                self.handle,
                u64::MAX,
                frame.image_available,
                vk::Fence::null(),
            )
        };
        let index = match acquired {
            Ok((index, _suboptimal)) => index,
            Err(vk::Result::ERROR_OUT_OF_DATE_KHR) | Err(vk::Result::SUBOPTIMAL_KHR) => {
                self.rebuild(gpu)?;
                return Ok(None);
            }
            Err(result) => return Err(RenderError::Vulkan("vkAcquireNextImageKHR", result)),
        };

        let waiting = self.image_in_flight[index as usize];
        if waiting != vk::Fence::null() {
            // Safety: a fence from an earlier submission that used this same image.
            unsafe { gpu.device.wait_for_fences(&[waiting], true, u64::MAX) }
                .context("vkWaitForFences")?;
        }
        self.image_in_flight[index as usize] = frame.in_flight;

        // Safety: nothing is pending on this fence or command buffer now.
        unsafe {
            gpu.device
                .reset_fences(&[frame.in_flight])
                .context("vkResetFences")?;
            let begin = vk::CommandBufferBeginInfo::default()
                .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
            gpu.device
                .begin_command_buffer(frame.command, &begin)
                .context("vkBeginCommandBuffer")?;
        }
        Ok(Some((index, frame.command)))
    }

    pub fn framebuffer(&self, index: u32) -> vk::Framebuffer {
        self.framebuffers[index as usize]
    }

    /// Submits the recorded frame and queues it for display.
    pub fn end(&mut self, gpu: &Gpu, index: u32) -> Result<Presented, RenderError> {
        let frame = &self.frames[self.frame];
        // Safety: recording finished before this call.
        unsafe { gpu.device.end_command_buffer(frame.command) }.context("vkEndCommandBuffer")?;

        let wait = [frame.image_available];
        let stages = [vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT];
        let commands = [frame.command];
        let signal = [self.render_finished[index as usize]];
        let submit = [vk::SubmitInfo::default()
            .wait_semaphores(&wait)
            .wait_dst_stage_mask(&stages)
            .command_buffers(&commands)
            .signal_semaphores(&signal)];
        // Safety: every borrowed slice outlives the submit.
        unsafe { gpu.device.queue_submit(gpu.queue, &submit, frame.in_flight) }
            .context("vkQueueSubmit")?;

        let swapchains = [self.handle];
        let indices = [index];
        let present = vk::PresentInfoKHR::default()
            .wait_semaphores(&signal)
            .swapchains(&swapchains)
            .image_indices(&indices);
        // Safety: same.
        let outcome = unsafe { self.loader.queue_present(gpu.queue, &present) };
        self.frame = (self.frame + 1) % FRAMES_IN_FLIGHT;
        match outcome {
            Ok(false) => Ok(Presented::Shown),
            Ok(true) | Err(vk::Result::ERROR_OUT_OF_DATE_KHR) | Err(vk::Result::SUBOPTIMAL_KHR) => {
                self.rebuild(gpu)?;
                Ok(Presented::Rebuilt)
            }
            Err(result) => Err(RenderError::Vulkan("vkQueuePresentKHR", result)),
        }
    }

    /// Safety: the device must be idle.
    unsafe fn release_swapchain_objects(&mut self, device: &Device) {
        unsafe {
            for framebuffer in self.framebuffers.drain(..) {
                device.destroy_framebuffer(framebuffer, None);
            }
            for view in self.views.drain(..) {
                device.destroy_image_view(view, None);
            }
            for semaphore in self.render_finished.drain(..) {
                device.destroy_semaphore(semaphore, None);
            }
        }
        self.image_in_flight.clear();
    }

    /// Safety: the device must be idle.
    pub unsafe fn destroy(&mut self, gpu: &Gpu) {
        unsafe {
            self.release_swapchain_objects(&gpu.device);
            if self.handle != vk::SwapchainKHR::null() {
                self.loader.destroy_swapchain(self.handle, None);
            }
            for frame in self.frames.drain(..) {
                gpu.device.destroy_semaphore(frame.image_available, None);
                gpu.device.destroy_fence(frame.in_flight, None);
                gpu.device
                    .free_command_buffers(gpu.command_pool, &[frame.command]);
            }
            gpu.device.destroy_pipeline(self.pipeline, None);
            gpu.device.destroy_render_pass(self.render_pass, None);
            self.surface.destroy();
        }
    }
}

fn build_present_pass(device: &Device, format: vk::Format) -> Result<vk::RenderPass, RenderError> {
    let attachment = [vk::AttachmentDescription::default()
        .format(format)
        .samples(vk::SampleCountFlags::TYPE_1)
        .load_op(vk::AttachmentLoadOp::CLEAR)
        .store_op(vk::AttachmentStoreOp::STORE)
        .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
        .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
        .initial_layout(vk::ImageLayout::UNDEFINED)
        .final_layout(vk::ImageLayout::PRESENT_SRC_KHR)];
    let reference = [vk::AttachmentReference::default()
        .attachment(0)
        .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)];
    let subpass = [vk::SubpassDescription::default()
        .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
        .color_attachments(&reference)];
    // The acquire semaphore is waited at COLOR_ATTACHMENT_OUTPUT, so the pass must not
    // write the attachment before that stage.
    let dependency = [vk::SubpassDependency::default()
        .src_subpass(vk::SUBPASS_EXTERNAL)
        .dst_subpass(0)
        .src_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
        .dst_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
        .dst_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)];
    let info = vk::RenderPassCreateInfo::default()
        .attachments(&attachment)
        .subpasses(&subpass)
        .dependencies(&dependency);
    // Safety: every borrowed slice outlives the call.
    unsafe { device.create_render_pass(&info, None) }.context("vkCreateRenderPass")
}

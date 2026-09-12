//! Images, buffers, and the transitions between them.
//!
//! Owners destroy explicitly rather than through `Drop`: every one of these needs the
//! device to clean up, and threading a device handle into each resource costs more than
//! it saves when the renderer already outlives all of them.

use ash::{vk, Device};

use crate::device::Gpu;
use crate::error::{Context, RenderError};

/// A device-local image with its view.
#[derive(Debug)]
pub(crate) struct DeviceImage {
    pub image: vk::Image,
    pub view: vk::ImageView,
    pub memory: vk::DeviceMemory,
    pub width: u32,
    pub height: u32,
}

impl DeviceImage {
    pub fn new(
        gpu: &Gpu,
        kind: vk::ImageType,
        extent: vk::Extent3D,
        format: vk::Format,
        usage: vk::ImageUsageFlags,
    ) -> Result<Self, RenderError> {
        let info = vk::ImageCreateInfo::default()
            .image_type(kind)
            .format(format)
            .extent(extent)
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(usage)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        // Safety: the device is open and the create-info is fully initialised.
        let image = unsafe { gpu.device.create_image(&info, None) }.context("vkCreateImage")?;

        // Safety: `image` was just created on this device.
        let requirements = unsafe { gpu.device.get_image_memory_requirements(image) };
        let index = gpu.memory_type(
            requirements.memory_type_bits,
            vk::MemoryPropertyFlags::DEVICE_LOCAL,
        )?;
        let allocate = vk::MemoryAllocateInfo::default()
            .allocation_size(requirements.size)
            .memory_type_index(index);
        // Safety: the allocation matches the image's stated requirements.
        let memory =
            unsafe { gpu.device.allocate_memory(&allocate, None) }.context("vkAllocateMemory")?;
        // Safety: freshly allocated memory bound to a freshly created image.
        unsafe { gpu.device.bind_image_memory(image, memory, 0) }.context("vkBindImageMemory")?;

        let view_type = match kind {
            vk::ImageType::TYPE_3D => vk::ImageViewType::TYPE_3D,
            _ => vk::ImageViewType::TYPE_2D,
        };
        let view_info = vk::ImageViewCreateInfo::default()
            .image(image)
            .view_type(view_type)
            .format(format)
            .subresource_range(
                vk::ImageSubresourceRange::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .level_count(1)
                    .layer_count(1),
            );
        // Safety: the image is bound to memory.
        let view = unsafe { gpu.device.create_image_view(&view_info, None) }
            .context("vkCreateImageView")?;

        Ok(Self {
            image,
            view,
            memory,
            width: extent.width,
            height: extent.height,
        })
    }

    pub fn new_2d(
        gpu: &Gpu,
        width: u32,
        height: u32,
        format: vk::Format,
        usage: vk::ImageUsageFlags,
    ) -> Result<Self, RenderError> {
        Self::new(
            gpu,
            vk::ImageType::TYPE_2D,
            vk::Extent3D {
                width,
                height,
                depth: 1,
            },
            format,
            usage,
        )
    }

    pub fn new_3d(
        gpu: &Gpu,
        size: u32,
        format: vk::Format,
        usage: vk::ImageUsageFlags,
    ) -> Result<Self, RenderError> {
        Self::new(
            gpu,
            vk::ImageType::TYPE_3D,
            vk::Extent3D {
                width: size,
                height: size,
                depth: size,
            },
            format,
            usage,
        )
    }

    /// Safety: the device must be idle with respect to this image.
    pub unsafe fn destroy(&self, device: &Device) {
        unsafe {
            device.destroy_image_view(self.view, None);
            device.destroy_image(self.image, None);
            device.free_memory(self.memory, None);
        }
    }
}

/// A host-visible staging buffer.
#[derive(Debug)]
pub(crate) struct HostBuffer {
    pub buffer: vk::Buffer,
    pub memory: vk::DeviceMemory,
    pub size: vk::DeviceSize,
}

impl HostBuffer {
    pub fn new(
        gpu: &Gpu,
        size: vk::DeviceSize,
        usage: vk::BufferUsageFlags,
    ) -> Result<Self, RenderError> {
        let info = vk::BufferCreateInfo::default()
            .size(size.max(1))
            .usage(usage)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        // Safety: the device is open.
        let buffer = unsafe { gpu.device.create_buffer(&info, None) }.context("vkCreateBuffer")?;
        // Safety: `buffer` was just created on this device.
        let requirements = unsafe { gpu.device.get_buffer_memory_requirements(buffer) };
        let index = gpu.memory_type(
            requirements.memory_type_bits,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
        )?;
        let allocate = vk::MemoryAllocateInfo::default()
            .allocation_size(requirements.size)
            .memory_type_index(index);
        // Safety: the allocation matches the buffer's stated requirements.
        let memory =
            unsafe { gpu.device.allocate_memory(&allocate, None) }.context("vkAllocateMemory")?;
        // Safety: freshly allocated memory bound to a freshly created buffer.
        unsafe { gpu.device.bind_buffer_memory(buffer, memory, 0) }
            .context("vkBindBufferMemory")?;
        Ok(Self {
            buffer,
            memory,
            size: requirements.size,
        })
    }

    pub fn write_bytes(&self, device: &Device, bytes: &[u8]) -> Result<(), RenderError> {
        if bytes.is_empty() {
            return Ok(());
        }
        // Safety: the memory is host-visible and coherent, and nothing else maps it.
        let pointer =
            unsafe { device.map_memory(self.memory, 0, self.size, vk::MemoryMapFlags::empty()) }
                .context("vkMapMemory")?;
        // Safety: the mapping covers `self.size`, and the copy is bounded by it.
        unsafe {
            let count = bytes.len().min(self.size as usize);
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), pointer.cast::<u8>(), count);
            device.unmap_memory(self.memory);
        }
        Ok(())
    }

    pub fn read_bytes(&self, device: &Device, length: usize) -> Result<Vec<u8>, RenderError> {
        // Safety: host-visible and coherent memory with no other mapping outstanding.
        let pointer =
            unsafe { device.map_memory(self.memory, 0, self.size, vk::MemoryMapFlags::empty()) }
                .context("vkMapMemory")?;
        let count = length.min(self.size as usize);
        let mut out = vec![0u8; count];
        // Safety: the mapping covers `count` bytes.
        unsafe {
            std::ptr::copy_nonoverlapping(pointer.cast::<u8>(), out.as_mut_ptr(), count);
            device.unmap_memory(self.memory);
        }
        Ok(out)
    }

    /// Safety: the device must be idle with respect to this buffer.
    pub unsafe fn destroy(&self, device: &Device) {
        unsafe {
            device.destroy_buffer(self.buffer, None);
            device.free_memory(self.memory, None);
        }
    }
}

/// Moves an image between layouts with a full barrier.
///
/// Safety: `command` must be recording, and `image` must belong to the same device.
pub(crate) unsafe fn transition(
    device: &Device,
    command: vk::CommandBuffer,
    image: vk::Image,
    from: vk::ImageLayout,
    to: vk::ImageLayout,
) {
    let barrier = vk::ImageMemoryBarrier::default()
        .old_layout(from)
        .new_layout(to)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .image(image)
        .subresource_range(
            vk::ImageSubresourceRange::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                .level_count(1)
                .layer_count(1),
        )
        .src_access_mask(vk::AccessFlags::MEMORY_WRITE)
        .dst_access_mask(vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE);
    unsafe {
        device.cmd_pipeline_barrier(
            command,
            vk::PipelineStageFlags::ALL_COMMANDS,
            vk::PipelineStageFlags::ALL_COMMANDS,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[barrier],
        );
    }
}

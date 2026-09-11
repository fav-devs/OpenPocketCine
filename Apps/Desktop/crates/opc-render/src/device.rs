//! Vulkan instance, device, and the one-shot command submission the pipeline uses.
//!
//! Headless on purpose: no surface, no swapchain. A window is the shell's job and sits
//! on top of this; rendering into an image is what can be checked without one.

use ash::{vk, Device, Entry, Instance};

use crate::error::{Context, RenderError};

/// An open Vulkan device with a graphics queue.
pub(crate) struct Gpu {
    pub device: Device,
    pub queue: vk::Queue,
    pub command_pool: vk::CommandPool,
    pub memory_properties: vk::PhysicalDeviceMemoryProperties,
    pub device_name: String,
    instance: Instance,
    _entry: Entry,
}

impl std::fmt::Debug for Gpu {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Gpu")
            .field("device_name", &self.device_name)
            .finish_non_exhaustive()
    }
}

impl Gpu {
    pub fn headless() -> Result<Self, RenderError> {
        // Safety: loads the system Vulkan loader. Nothing else has been initialised yet.
        let entry =
            unsafe { Entry::load() }.map_err(|error| RenderError::NoVulkan(error.to_string()))?;

        let application = vk::ApplicationInfo::default()
            .application_name(c"OpenPocketCine")
            .api_version(vk::make_api_version(0, 1, 1, 0));
        let instance_info = vk::InstanceCreateInfo::default().application_info(&application);
        // Safety: `instance_info` borrows `application`, which outlives this call.
        let instance = unsafe { entry.create_instance(&instance_info, None) }
            .map_err(|result| RenderError::NoVulkan(format!("{result:?}")))?;

        let Some((physical, queue_family)) = pick_device(&instance) else {
            // Safety: the instance was created above and is dropped exactly once.
            unsafe { instance.destroy_instance(None) };
            return Err(RenderError::NoGraphicsDevice);
        };

        let priorities = [1.0_f32];
        let queue_info = [vk::DeviceQueueCreateInfo::default()
            .queue_family_index(queue_family)
            .queue_priorities(&priorities)];
        let device_info = vk::DeviceCreateInfo::default().queue_create_infos(&queue_info);

        // Safety: the borrowed create-info outlives the call; failure destroys the instance.
        let device = match unsafe { instance.create_device(physical, &device_info, None) } {
            Ok(device) => device,
            Err(result) => {
                unsafe { instance.destroy_instance(None) };
                return Err(RenderError::Vulkan("vkCreateDevice", result));
            }
        };

        // Safety: the queue family and device were just created together.
        let queue = unsafe { device.get_device_queue(queue_family, 0) };
        let pool_info = vk::CommandPoolCreateInfo::default()
            .queue_family_index(queue_family)
            .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER);
        // Safety: the device is open.
        let command_pool = unsafe { device.create_command_pool(&pool_info, None) }
            .context("vkCreateCommandPool")?;

        // Safety: `physical` came from this instance.
        let memory_properties = unsafe { instance.get_physical_device_memory_properties(physical) };
        // Safety: same.
        let properties = unsafe { instance.get_physical_device_properties(physical) };
        let device_name = properties
            .device_name_as_c_str()
            .unwrap_or(c"unknown")
            .to_string_lossy()
            .into_owned();

        Ok(Self {
            device,
            queue,
            command_pool,
            memory_properties,
            device_name,
            instance,
            _entry: entry,
        })
    }

    pub fn memory_type(
        &self,
        supported: u32,
        wanted: vk::MemoryPropertyFlags,
    ) -> Result<u32, RenderError> {
        (0..self.memory_properties.memory_type_count)
            .find(|index| {
                let usable = supported & (1 << index) != 0;
                let kind = self.memory_properties.memory_types[*index as usize];
                usable && kind.property_flags.contains(wanted)
            })
            .ok_or(RenderError::NoSuitableMemory)
    }

    /// Records, submits, and waits on one command buffer.
    ///
    /// A fence per submission rather than a pipeline of them: the headless path renders
    /// one picture at a time, and a window's swapchain brings its own in-flight handling.
    pub fn one_shot<F>(&self, record: F) -> Result<(), RenderError>
    where
        F: FnOnce(vk::CommandBuffer),
    {
        let allocate = vk::CommandBufferAllocateInfo::default()
            .command_pool(self.command_pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(1);
        // Safety: the pool belongs to this device.
        let buffers = unsafe { self.device.allocate_command_buffers(&allocate) }
            .context("vkAllocateCommandBuffers")?;
        let buffer = buffers[0];

        let begin = vk::CommandBufferBeginInfo::default()
            .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
        // Safety: `buffer` was just allocated and is not in use.
        let outcome = unsafe { self.device.begin_command_buffer(buffer, &begin) }
            .context("vkBeginCommandBuffer")
            .and_then(|()| {
                record(buffer);
                // Safety: recording finished above.
                unsafe { self.device.end_command_buffer(buffer) }.context("vkEndCommandBuffer")
            })
            .and_then(|()| self.submit_and_wait(buffer));

        // Safety: the submission has completed or failed; either way the buffer is free.
        unsafe {
            self.device
                .free_command_buffers(self.command_pool, &buffers)
        };
        outcome
    }

    fn submit_and_wait(&self, buffer: vk::CommandBuffer) -> Result<(), RenderError> {
        let fence_info = vk::FenceCreateInfo::default();
        // Safety: the device is open.
        let fence =
            unsafe { self.device.create_fence(&fence_info, None) }.context("vkCreateFence")?;
        let buffers = [buffer];
        let submit = [vk::SubmitInfo::default().command_buffers(&buffers)];
        // Safety: `buffers` outlives the submit, and the fence is fresh.
        let result = unsafe { self.device.queue_submit(self.queue, &submit, fence) }
            .context("vkQueueSubmit")
            .and_then(|()| {
                // Safety: waiting on the fence just submitted, with no timeout.
                unsafe { self.device.wait_for_fences(&[fence], true, u64::MAX) }
                    .context("vkWaitForFences")
            });
        // Safety: the wait returned, so the fence is no longer in use.
        unsafe { self.device.destroy_fence(fence, None) };
        result
    }
}

fn pick_device(instance: &Instance) -> Option<(vk::PhysicalDevice, u32)> {
    // Safety: the instance is open.
    let devices = unsafe { instance.enumerate_physical_devices() }.ok()?;
    let mut best: Option<(vk::PhysicalDevice, u32, u32)> = None;
    for physical in devices {
        // Safety: `physical` came from this instance.
        let families = unsafe { instance.get_physical_device_queue_family_properties(physical) };
        let Some(family) = families
            .iter()
            .position(|family| family.queue_flags.contains(vk::QueueFlags::GRAPHICS))
        else {
            continue;
        };
        // Safety: same.
        let properties = unsafe { instance.get_physical_device_properties(physical) };
        // Prefer real hardware; a CPU device is a correct fallback, just a slow one.
        let rank = match properties.device_type {
            vk::PhysicalDeviceType::DISCRETE_GPU => 0,
            vk::PhysicalDeviceType::INTEGRATED_GPU => 1,
            vk::PhysicalDeviceType::VIRTUAL_GPU => 2,
            _ => 3,
        };
        if best.is_none_or(|(_, _, current)| rank < current) {
            best = Some((physical, family as u32, rank));
        }
    }
    best.map(|(physical, family, _)| (physical, family))
}

impl Drop for Gpu {
    fn drop(&mut self) {
        // Safety: every child object is destroyed by its own owner before the device.
        unsafe {
            let _ = self.device.device_wait_idle();
            self.device.destroy_command_pool(self.command_pool, None);
            self.device.destroy_device(None);
            self.instance.destroy_instance(None);
        }
    }
}

use ash::{Entry, Instance, vk};
use serde_json::{Value, json};
use std::ffi::CStr;

pub struct Device {
    _entry: Entry,
    instance: Instance,
    device: ash::Device,
    memory: vk::PhysicalDeviceMemoryProperties,
    properties: vk::PhysicalDeviceProperties,
}

impl Device {
    pub fn new() -> Result<Self, String> {
        let entry = unsafe { Entry::load() }.map_err(|error| error.to_string())?;
        let application = vk::ApplicationInfo::default()
            .application_name(c"streamed-fixture-profile")
            .api_version(vk::API_VERSION_1_3);
        let description = vk::InstanceCreateInfo::default().application_info(&application);
        let instance = unsafe { entry.create_instance(&description, None) }
            .map_err(|error| error.to_string())?;
        let result = (|| {
            for physical in unsafe { instance.enumerate_physical_devices() }
                .map_err(|error| error.to_string())?
            {
                let queues =
                    unsafe { instance.get_physical_device_queue_family_properties(physical) };
                let Some(index) = queues.iter().position(|queue| {
                    queue
                        .queue_flags
                        .contains(vk::QueueFlags::GRAPHICS | vk::QueueFlags::COMPUTE)
                }) else {
                    continue;
                };
                let priorities = [1.0];
                let queue = vk::DeviceQueueCreateInfo::default()
                    .queue_family_index(u32::try_from(index).map_err(|error| error.to_string())?)
                    .queue_priorities(&priorities);
                let queues = [queue];
                let description = vk::DeviceCreateInfo::default().queue_create_infos(&queues);
                let device = unsafe { instance.create_device(physical, &description, None) }
                    .map_err(|error| error.to_string())?;
                return Ok((
                    device,
                    unsafe { instance.get_physical_device_memory_properties(physical) },
                    unsafe { instance.get_physical_device_properties(physical) },
                ));
            }
            Err("no graphics/compute Vulkan device".to_string())
        })();
        match result {
            Ok((device, memory, properties)) => Ok(Self {
                _entry: entry,
                instance,
                device,
                memory,
                properties,
            }),
            Err(error) => {
                unsafe { instance.destroy_instance(None) };
                Err(error)
            }
        }
    }

    pub fn context(&self) -> Value {
        // SAFETY: Vulkan guarantees a NUL-terminated device_name array in its properties.
        let name =
            unsafe { CStr::from_ptr(self.properties.device_name.as_ptr()) }.to_string_lossy();
        json!({"kind": "device", "name": name, "vendor_id": self.properties.vendor_id,
            "device_id": self.properties.device_id, "driver_version": self.properties.driver_version,
            "api_version": self.properties.api_version,
            "max_memory_allocation_count": self.properties.limits.max_memory_allocation_count,
            "max_storage_buffer_range": self.properties.limits.max_storage_buffer_range})
    }

    pub fn allocate_buffer(
        &self,
        bytes: usize,
        usage: vk::BufferUsageFlags,
    ) -> Result<u64, String> {
        if bytes == 0 {
            return Ok(0);
        }
        let description = vk::BufferCreateInfo::default()
            .size(bytes as u64)
            .usage(usage)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        let buffer = unsafe { self.device.create_buffer(&description, None) }
            .map_err(|error| error.to_string())?;
        let mut allocated_memory = None;
        let result = (|| {
            let requirements = unsafe { self.device.get_buffer_memory_requirements(buffer) };
            let index = self
                .memory
                .memory_types
                .iter()
                .take(self.memory.memory_type_count as usize)
                .enumerate()
                .find(|(index, memory)| {
                    requirements.memory_type_bits & (1 << index) != 0
                        && memory.property_flags.contains(
                            vk::MemoryPropertyFlags::HOST_VISIBLE
                                | vk::MemoryPropertyFlags::HOST_COHERENT,
                        )
                })
                .map(|(index, _)| index as u32)
                .ok_or("no host-visible coherent buffer memory")?;
            let description = vk::MemoryAllocateInfo::default()
                .allocation_size(requirements.size)
                .memory_type_index(index);
            let memory = unsafe { self.device.allocate_memory(&description, None) }
                .map_err(|error| error.to_string())?;
            allocated_memory = Some(memory);
            let result = unsafe { self.device.bind_buffer_memory(buffer, memory, 0) }
                .map_err(|error| error.to_string());
            result.map(|()| requirements.size)
        })();
        // No commands use these buffers, so freeing needs no queue wait.
        unsafe { self.device.destroy_buffer(buffer, None) };
        if let Some(memory) = allocated_memory {
            unsafe { self.device.free_memory(memory, None) };
        }
        result
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        unsafe { self.device.destroy_device(None) };
        unsafe { self.instance.destroy_instance(None) };
    }
}

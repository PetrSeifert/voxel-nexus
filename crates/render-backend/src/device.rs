use super::VALIDATION_LAYER_NAME;
use super::configuration::{
    BackendError, DeviceCandidate, DeviceRejection, DeviceRequirement, DeviceSelectionError,
    QueueFamilyCapabilities, RenderPathDeviceCapabilities, SurfaceSupport,
};
use super::render_path::PresentationAdapter;
use ash::{Entry, Instance, vk};
use std::ffi::{CStr, c_char, c_void};
use std::ops::Deref;
use std::sync::atomic::{AtomicUsize, Ordering};

pub(super) struct LogicalDevice(ash::Device);

impl LogicalDevice {
    pub(super) fn new(
        instance: &Instance,
        selected_device: &InspectedDevice,
    ) -> Result<Self, BackendError> {
        let mut queue_family_indices = vec![selected_device.graphics_queue_family_index];
        if selected_device.presentation_queue_family_index
            != selected_device.graphics_queue_family_index
        {
            queue_family_indices.push(selected_device.presentation_queue_family_index);
        }
        let queue_priorities = [1.0];
        let queue_create_infos: Vec<_> = queue_family_indices
            .iter()
            .map(|queue_family_index| {
                vk::DeviceQueueCreateInfo::default()
                    .queue_family_index(*queue_family_index)
                    .queue_priorities(&queue_priorities)
            })
            .collect();
        let extension_names = [ash::khr::swapchain::NAME.as_ptr()];
        let device_create_info = vk::DeviceCreateInfo::default()
            .queue_create_infos(&queue_create_infos)
            .enabled_extension_names(&extension_names);
        let device = unsafe {
            instance.create_device(selected_device.physical_device, &device_create_info, None)
        }
        .map_err(BackendError::CreateDevice)?;
        Ok(Self(device))
    }
}

impl Deref for LogicalDevice {
    type Target = ash::Device;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Drop for LogicalDevice {
    fn drop(&mut self) {
        unsafe { self.0.destroy_device(None) };
    }
}

pub(super) struct InstanceSurface {
    _entry: Entry,
    pub(super) instance: Instance,
    debug_loader: Option<ash::ext::debug_utils::Instance>,
    debug_messenger: Option<vk::DebugUtilsMessengerEXT>,
    pub(super) validation_diagnostics: Box<ValidationDiagnostics>,
    surface_loader: ash::khr::surface::Instance,
    pub(super) surface: vk::SurfaceKHR,
}

impl Drop for InstanceSurface {
    fn drop(&mut self) {
        unsafe {
            self.surface_loader.destroy_surface(self.surface, None);
            if let (Some(debug_loader), Some(debug_messenger)) =
                (&self.debug_loader, self.debug_messenger)
            {
                debug_loader.destroy_debug_utils_messenger(debug_messenger, None);
            }
            self.instance.destroy_instance(None);
        }
    }
}

#[derive(Clone)]
pub(super) struct InspectedDevice {
    pub(super) physical_device: vk::PhysicalDevice,
    pub(super) candidate: DeviceCandidate,
    pub(super) graphics_queue_family_index: u32,
    pub(super) presentation_queue_family_index: u32,
    pub(super) timestamp_valid_bits: u32,
    pub(super) timestamp_period_nanoseconds: f64,
    pub(super) render_path_device_capabilities: RenderPathDeviceCapabilities,
}

#[derive(Default)]
pub(super) struct ValidationDiagnostics {
    errors: AtomicUsize,
    warnings: AtomicUsize,
}

impl ValidationDiagnostics {
    pub(super) fn warning_count(&self) -> usize {
        self.warnings.load(Ordering::SeqCst)
    }
    pub(super) fn error_count(&self) -> usize {
        self.errors.load(Ordering::SeqCst)
    }
}

pub(super) fn create_instance_surface(
    entry: Entry,
    application_name: &CStr,
    adapter: &impl PresentationAdapter,
    validation_enabled: bool,
) -> Result<InstanceSurface, BackendError> {
    let extension_names = adapter
        .required_instance_extensions()
        .map_err(BackendError::PlatformAdapter)?;
    let mut extension_name_pointers: Vec<*const c_char> = extension_names
        .iter()
        .map(|extension_name| extension_name.as_ptr())
        .collect();
    if validation_enabled {
        extension_name_pointers.push(ash::ext::debug_utils::NAME.as_ptr());
    }
    let layer_names = validation_enabled.then_some(VALIDATION_LAYER_NAME.as_ptr());
    let application_info = vk::ApplicationInfo::default()
        .application_name(application_name)
        .application_version(0)
        .engine_name(c"Voxel Nexus")
        .engine_version(0)
        .api_version(vk::API_VERSION_1_3);
    let instance_create_info = vk::InstanceCreateInfo::default()
        .application_info(&application_info)
        .enabled_extension_names(&extension_name_pointers)
        .enabled_layer_names(layer_names.as_slice());
    let instance = unsafe { entry.create_instance(&instance_create_info, None) }
        .map_err(BackendError::CreateInstance)?;
    let validation_diagnostics = Box::new(ValidationDiagnostics::default());
    let (debug_loader, debug_messenger) = if validation_enabled {
        let debug_loader = ash::ext::debug_utils::Instance::new(&entry, &instance);
        let validation_diagnostics_pointer = (&raw const *validation_diagnostics)
            .cast_mut()
            .cast::<c_void>();
        let messenger_info = validation_messenger_create_info(validation_diagnostics_pointer);
        let debug_messenger =
            match unsafe { debug_loader.create_debug_utils_messenger(&messenger_info, None) } {
                Ok(messenger) => messenger,
                Err(error) => {
                    unsafe { instance.destroy_instance(None) };
                    return Err(BackendError::CreateValidationMessenger(error));
                }
            };
        (Some(debug_loader), Some(debug_messenger))
    } else {
        (None, None)
    };
    let surface = match unsafe { adapter.create_surface(&entry, &instance) } {
        Ok(surface) => surface,
        Err(error) => {
            unsafe {
                if let (Some(debug_loader), Some(debug_messenger)) =
                    (&debug_loader, debug_messenger)
                {
                    debug_loader.destroy_debug_utils_messenger(debug_messenger, None);
                }
                instance.destroy_instance(None);
            }
            return Err(BackendError::PlatformAdapter(error));
        }
    };
    let surface_loader = ash::khr::surface::Instance::new(&entry, &instance);
    Ok(InstanceSurface {
        _entry: entry,
        instance,
        debug_loader,
        debug_messenger,
        validation_diagnostics,
        surface_loader,
        surface,
    })
}

fn validation_messenger_create_info(
    validation_diagnostics: *mut c_void,
) -> vk::DebugUtilsMessengerCreateInfoEXT<'static> {
    vk::DebugUtilsMessengerCreateInfoEXT::default()
        .message_severity(
            vk::DebugUtilsMessageSeverityFlagsEXT::WARNING
                | vk::DebugUtilsMessageSeverityFlagsEXT::ERROR,
        )
        .message_type(
            vk::DebugUtilsMessageTypeFlagsEXT::GENERAL
                | vk::DebugUtilsMessageTypeFlagsEXT::VALIDATION
                | vk::DebugUtilsMessageTypeFlagsEXT::PERFORMANCE,
        )
        .pfn_user_callback(Some(validation_callback))
        .user_data(validation_diagnostics)
}

unsafe extern "system" fn validation_callback(
    severity: vk::DebugUtilsMessageSeverityFlagsEXT,
    message_type: vk::DebugUtilsMessageTypeFlagsEXT,
    callback_data: *const vk::DebugUtilsMessengerCallbackDataEXT<'_>,
    user_data: *mut c_void,
) -> vk::Bool32 {
    if !user_data.is_null() {
        // SAFETY: `user_data` points at the boxed diagnostics, which the owner drops only after
        // destroying this messenger.
        let diagnostics = unsafe { &*user_data.cast::<ValidationDiagnostics>() };
        if severity.contains(vk::DebugUtilsMessageSeverityFlagsEXT::ERROR) {
            diagnostics.errors.fetch_add(1, Ordering::SeqCst);
        }
        if severity.contains(vk::DebugUtilsMessageSeverityFlagsEXT::WARNING) {
            diagnostics.warnings.fetch_add(1, Ordering::SeqCst);
        }
    }
    let message = if callback_data.is_null() {
        c"validation callback supplied no diagnostic data"
    } else {
        // SAFETY: Checked non-null above; Vulkan keeps callback data valid for the callback.
        let message_pointer = unsafe { (*callback_data).p_message };
        if message_pointer.is_null() {
            c"validation callback supplied no diagnostic message"
        } else {
            // SAFETY: Checked non-null above; Vulkan supplies a null-terminated message.
            unsafe { CStr::from_ptr(message_pointer) }
        }
    };
    eprintln!("Vulkan validation {severity:?} {message_type:?}: {message:?}");
    vk::FALSE
}

pub(super) fn require_vulkan_1_3_loader(entry: &Entry) -> Result<(), BackendError> {
    let loader_version = unsafe { entry.try_enumerate_instance_version() }
        .map_err(BackendError::QueryVulkanLoaderVersion)?
        .unwrap_or(vk::API_VERSION_1_0);
    if loader_version < vk::API_VERSION_1_3 {
        return Err(BackendError::VulkanLoaderTooOld {
            major: vk::api_version_major(loader_version),
            minor: vk::api_version_minor(loader_version),
        });
    }
    Ok(())
}

pub(super) fn require_validation_layer(entry: &Entry) -> Result<(), BackendError> {
    let layer_properties = unsafe { entry.enumerate_instance_layer_properties() }
        .map_err(BackendError::EnumerateInstanceLayers)?;
    let available = layer_properties.iter().any(|property| {
        // SAFETY: Vulkan null-terminates its fixed-size name arrays.
        let name = unsafe { CStr::from_ptr(property.layer_name.as_ptr()) };
        name == VALIDATION_LAYER_NAME
    });
    if !available {
        return Err(BackendError::ValidationLayerUnavailable);
    }
    Ok(())
}

pub(super) fn inspect_devices(
    presentation: &InstanceSurface,
) -> Result<Vec<InspectedDevice>, BackendError> {
    let physical_devices = unsafe { presentation.instance.enumerate_physical_devices() }
        .map_err(BackendError::EnumeratePhysicalDevices)?;
    physical_devices
        .into_iter()
        .map(|physical_device| inspect_device(presentation, physical_device))
        .collect()
}

fn inspect_device(
    presentation: &InstanceSurface,
    physical_device: vk::PhysicalDevice,
) -> Result<InspectedDevice, BackendError> {
    let properties = unsafe {
        presentation
            .instance
            .get_physical_device_properties(physical_device)
    };
    // SAFETY: Vulkan null-terminates its fixed-size name arrays.
    let name = unsafe { CStr::from_ptr(properties.device_name.as_ptr()) }
        .to_string_lossy()
        .into_owned();
    let extension_properties = unsafe {
        presentation
            .instance
            .enumerate_device_extension_properties(physical_device)
    }
    .map_err(BackendError::InspectPresentationSupport)?;
    let supports_swapchain = extension_properties.iter().any(|extension| {
        // SAFETY: Vulkan null-terminates its fixed-size name arrays.
        let extension_name = unsafe { CStr::from_ptr(extension.extension_name.as_ptr()) };
        extension_name == ash::khr::swapchain::NAME
    });
    let queue_properties = unsafe {
        presentation
            .instance
            .get_physical_device_queue_family_properties(physical_device)
    };
    let mut queue_families = Vec::with_capacity(queue_properties.len());
    for (queue_family_index, queue_property) in queue_properties.iter().enumerate() {
        let queue_family_index = u32::try_from(queue_family_index)
            .map_err(|_| BackendError::InspectPresentationSupport(vk::Result::ERROR_UNKNOWN))?;
        let supports_presentation = unsafe {
            presentation
                .surface_loader
                .get_physical_device_surface_support(
                    physical_device,
                    queue_family_index,
                    presentation.surface,
                )
        }
        .map_err(BackendError::InspectPresentationSupport)?;
        queue_families.push(QueueFamilyCapabilities {
            supports_graphics: queue_property
                .queue_flags
                .contains(vk::QueueFlags::GRAPHICS),
            supports_compute: queue_property.queue_flags.contains(vk::QueueFlags::COMPUTE),
            supports_presentation,
        });
    }
    let surface_support = query_surface_support(presentation, physical_device)?;
    let graphics_queue_family_index = select_graphics_queue_family(&queue_families)
        .and_then(|index| u32::try_from(index).ok())
        .unwrap_or(u32::MAX);
    let presentation_queue_family_index = queue_families
        .iter()
        .position(|queue_family| queue_family.supports_presentation)
        .and_then(|index| u32::try_from(index).ok())
        .unwrap_or(u32::MAX);
    let timestamp_valid_bits = usize::try_from(graphics_queue_family_index)
        .ok()
        .and_then(|index| queue_properties.get(index))
        .map(|properties| properties.timestamp_valid_bits)
        .unwrap_or(0);
    let command_queue_flags = usize::try_from(graphics_queue_family_index)
        .ok()
        .and_then(|index| queue_properties.get(index))
        .map(|properties| properties.queue_flags)
        .unwrap_or_else(vk::QueueFlags::empty);
    let rgba8_unorm_format_properties = unsafe {
        presentation
            .instance
            .get_physical_device_format_properties(physical_device, vk::Format::R8G8B8A8_UNORM)
    };
    let mut maintenance = vk::PhysicalDeviceMaintenance4Properties::default();
    if properties.api_version >= vk::API_VERSION_1_3 {
        let mut extended_properties =
            vk::PhysicalDeviceProperties2::default().push_next(&mut maintenance);
        unsafe {
            presentation
                .instance
                .get_physical_device_properties2(physical_device, &mut extended_properties)
        };
    }
    let limits = properties.limits;
    let render_path_device_capabilities = RenderPathDeviceCapabilities {
        api_version: properties.api_version,
        command_queue_flags,
        max_image_dimension_2d: limits.max_image_dimension2_d,
        max_bound_descriptor_sets: limits.max_bound_descriptor_sets,
        max_per_stage_descriptor_storage_images: limits.max_per_stage_descriptor_storage_images,
        max_per_stage_descriptor_storage_buffers: limits.max_per_stage_descriptor_storage_buffers,
        max_per_stage_descriptor_sampled_images: limits.max_per_stage_descriptor_sampled_images,
        max_per_stage_descriptor_samplers: limits.max_per_stage_descriptor_samplers,
        max_descriptor_set_storage_images: limits.max_descriptor_set_storage_images,
        max_descriptor_set_storage_buffers: limits.max_descriptor_set_storage_buffers,
        max_descriptor_set_sampled_images: limits.max_descriptor_set_sampled_images,
        max_descriptor_set_samplers: limits.max_descriptor_set_samplers,
        max_compute_work_group_count: limits.max_compute_work_group_count,
        max_compute_work_group_invocations: limits.max_compute_work_group_invocations,
        max_compute_work_group_size: limits.max_compute_work_group_size,
        max_storage_buffer_range: limits.max_storage_buffer_range,
        max_buffer_size: maintenance.max_buffer_size,
        rgba8_unorm_optimal_tiling_features: rgba8_unorm_format_properties.optimal_tiling_features,
    };

    Ok(InspectedDevice {
        physical_device,
        candidate: DeviceCandidate {
            name,
            api_version: properties.api_version,
            driver_version: properties.driver_version,
            supports_swapchain,
            has_surface_formats: !surface_support.formats.is_empty(),
            has_present_modes: !surface_support.present_modes.is_empty(),
            queue_families,
        },
        graphics_queue_family_index,
        presentation_queue_family_index,
        timestamp_valid_bits,
        timestamp_period_nanoseconds: f64::from(properties.limits.timestamp_period),
        render_path_device_capabilities,
    })
}

pub(super) fn select_graphics_queue_family(
    queue_families: &[QueueFamilyCapabilities],
) -> Option<usize> {
    queue_families
        .iter()
        .position(|queue_family| queue_family.supports_graphics && queue_family.supports_compute)
        .or_else(|| {
            queue_families
                .iter()
                .position(|queue_family| queue_family.supports_graphics)
        })
}

pub(super) fn query_surface_support(
    presentation: &InstanceSurface,
    physical_device: vk::PhysicalDevice,
) -> Result<SurfaceSupport, BackendError> {
    let capabilities = unsafe {
        presentation
            .surface_loader
            .get_physical_device_surface_capabilities(physical_device, presentation.surface)
    }
    .map_err(BackendError::InspectPresentationSupport)?;
    let formats = unsafe {
        presentation
            .surface_loader
            .get_physical_device_surface_formats(physical_device, presentation.surface)
    }
    .map_err(BackendError::InspectPresentationSupport)?;
    let present_modes = unsafe {
        presentation
            .surface_loader
            .get_physical_device_surface_present_modes(physical_device, presentation.surface)
    }
    .map_err(BackendError::InspectPresentationSupport)?;
    Ok(SurfaceSupport {
        capabilities,
        formats,
        present_modes,
    })
}

fn qualify_device(candidate: &DeviceCandidate) -> Result<(), DeviceRejection> {
    let mut unmet_requirements = Vec::new();
    if candidate.api_version < vk::API_VERSION_1_3 {
        unmet_requirements.push(DeviceRequirement::VulkanApi13 {
            available_version: candidate.api_version,
        });
    }
    if !candidate.supports_swapchain {
        unmet_requirements.push(DeviceRequirement::SwapchainExtension);
    }
    if !candidate.has_surface_formats {
        unmet_requirements.push(DeviceRequirement::SurfaceFormats);
    }
    if !candidate.has_present_modes {
        unmet_requirements.push(DeviceRequirement::PresentModes);
    }
    if !candidate
        .queue_families
        .iter()
        .any(|queue_family| queue_family.supports_graphics)
    {
        unmet_requirements.push(DeviceRequirement::GraphicsQueue);
    }
    if !candidate
        .queue_families
        .iter()
        .any(|queue_family| queue_family.supports_presentation)
    {
        unmet_requirements.push(DeviceRequirement::PresentationQueue);
    }
    if unmet_requirements.is_empty() {
        Ok(())
    } else {
        Err(DeviceRejection {
            device_name: candidate.name.clone(),
            unmet_requirements,
        })
    }
}

pub fn select_device(
    candidates: Vec<DeviceCandidate>,
) -> Result<DeviceCandidate, DeviceSelectionError> {
    select_qualified_device(candidates, |candidate| candidate)
}

pub(super) fn select_inspected_device(
    inspected_devices: Vec<InspectedDevice>,
) -> Result<InspectedDevice, DeviceSelectionError> {
    select_qualified_device(inspected_devices, |inspected_device| {
        &inspected_device.candidate
    })
}

fn select_qualified_device<Item>(
    items: Vec<Item>,
    device_candidate: impl Fn(&Item) -> &DeviceCandidate,
) -> Result<Item, DeviceSelectionError> {
    let mut rejections = Vec::new();
    for item in items {
        match qualify_device(device_candidate(&item)) {
            Ok(()) => return Ok(item),
            Err(rejection) => rejections.push(rejection),
        }
    }
    Err(DeviceSelectionError {
        candidates: rejections,
    })
}

use ash::{Entry, Instance, vk};
use std::ffi::{CStr, CString, c_char};
use std::fmt;
use thiserror::Error;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeContext {
    pub device_name: String,
    pub driver_version: u32,
    pub api_version: u32,
}

impl fmt::Display for RuntimeContext {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "Vulkan device: {}\nDriver version: {} ({:#010x})\nVulkan API version: {}.{}.{}",
            self.device_name,
            self.driver_version,
            self.driver_version,
            vk::api_version_major(self.api_version),
            vk::api_version_minor(self.api_version),
            vk::api_version_patch(self.api_version)
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueueFamilyCapabilities {
    pub supports_graphics: bool,
    pub supports_presentation: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceCandidate {
    pub name: String,
    pub api_version: u32,
    pub driver_version: u32,
    pub supports_swapchain: bool,
    pub has_surface_formats: bool,
    pub has_present_modes: bool,
    pub queue_families: Vec<QueueFamilyCapabilities>,
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum DeviceSelectionError {
    #[error("no Vulkan 1.3 device with presentation support is available")]
    NoSuitableDevice,
}

#[derive(Debug, Error)]
pub enum BackendError {
    #[error("could not load the Vulkan loader: {0}")]
    LoadVulkan(#[from] ash::LoadingError),
    #[error("the Vulkan loader supports API {major}.{minor}, but Vulkan 1.3 is required")]
    VulkanLoaderTooOld { major: u32, minor: u32 },
    #[error("could not query the Vulkan loader API version: {0}")]
    QueryVulkanLoaderVersion(vk::Result),
    #[error("the platform adapter could not prepare Vulkan presentation: {0}")]
    PlatformAdapter(String),
    #[error("could not create the Vulkan instance: {0}")]
    CreateInstance(vk::Result),
    #[error("could not enumerate Vulkan physical devices: {0}")]
    EnumeratePhysicalDevices(vk::Result),
    #[error("could not inspect Vulkan device presentation support: {0}")]
    InspectPresentationSupport(vk::Result),
    #[error(transparent)]
    SelectDevice(#[from] DeviceSelectionError),
    #[error("could not create the Vulkan logical device: {0}")]
    CreateDevice(vk::Result),
}

/// Supplies the platform-owned Vulkan instance extensions and presentation surface.
///
/// # Safety
///
/// Implementations must return a surface created from the supplied instance, and the
/// platform window behind that surface must outlive the Render Backend.
pub unsafe trait PresentationAdapter {
    fn required_instance_extensions(&self) -> Result<Vec<CString>, String>;

    /// # Safety
    ///
    /// The returned surface must belong to `instance` and the platform window must
    /// outlive the Render Backend that owns it.
    unsafe fn create_surface(
        &self,
        entry: &Entry,
        instance: &Instance,
    ) -> Result<vk::SurfaceKHR, String>;
}

pub struct RenderBackend {
    device: ash::Device,
    _presentation: InstanceSurface,
    runtime_context: RuntimeContext,
}

impl RenderBackend {
    pub fn initialize(
        application_name: &CStr,
        adapter: &impl PresentationAdapter,
    ) -> Result<Self, BackendError> {
        let entry = unsafe { Entry::load()? };
        require_vulkan_1_3_loader(&entry)?;

        let extension_names = adapter
            .required_instance_extensions()
            .map_err(BackendError::PlatformAdapter)?;
        let extension_name_pointers: Vec<*const c_char> = extension_names
            .iter()
            .map(|extension_name| extension_name.as_ptr())
            .collect();
        let application_info = vk::ApplicationInfo::default()
            .application_name(application_name)
            .application_version(0)
            .engine_name(c"Voxel Nexus")
            .engine_version(0)
            .api_version(vk::API_VERSION_1_3);
        let instance_create_info = vk::InstanceCreateInfo::default()
            .application_info(&application_info)
            .enabled_extension_names(&extension_name_pointers);
        let instance = unsafe { entry.create_instance(&instance_create_info, None) }
            .map_err(BackendError::CreateInstance)?;

        let surface = match unsafe { adapter.create_surface(&entry, &instance) } {
            Ok(surface) => surface,
            Err(error) => {
                unsafe { instance.destroy_instance(None) };
                return Err(BackendError::PlatformAdapter(error));
            }
        };
        let surface_loader = ash::khr::surface::Instance::new(&entry, &instance);
        let presentation = InstanceSurface {
            _entry: entry,
            instance,
            surface_loader,
            surface,
        };

        let inspected_devices = inspect_devices(&presentation)?;
        let selected_device = inspected_devices
            .into_iter()
            .find(|inspected_device| inspected_device.candidate.is_suitable())
            .ok_or(DeviceSelectionError::NoSuitableDevice)?;
        let device = create_logical_device(&presentation.instance, &selected_device)?;
        let runtime_context = RuntimeContext {
            device_name: selected_device.candidate.name,
            driver_version: selected_device.candidate.driver_version,
            api_version: selected_device.candidate.api_version,
        };

        Ok(Self {
            device,
            _presentation: presentation,
            runtime_context,
        })
    }

    pub fn runtime_context(&self) -> &RuntimeContext {
        &self.runtime_context
    }
}

impl Drop for RenderBackend {
    fn drop(&mut self) {
        if let Err(error) = unsafe { self.device.device_wait_idle() } {
            eprintln!("Vulkan device did not become idle during shutdown: {error}");
        }
        unsafe { self.device.destroy_device(None) };
    }
}

struct InstanceSurface {
    _entry: Entry,
    instance: Instance,
    surface_loader: ash::khr::surface::Instance,
    surface: vk::SurfaceKHR,
}

impl Drop for InstanceSurface {
    fn drop(&mut self) {
        unsafe {
            self.surface_loader.destroy_surface(self.surface, None);
            self.instance.destroy_instance(None);
        }
    }
}

struct InspectedDevice {
    physical_device: vk::PhysicalDevice,
    candidate: DeviceCandidate,
    graphics_queue_family_index: u32,
    presentation_queue_family_index: u32,
}

fn require_vulkan_1_3_loader(entry: &Entry) -> Result<(), BackendError> {
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

fn inspect_devices(presentation: &InstanceSurface) -> Result<Vec<InspectedDevice>, BackendError> {
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
            supports_presentation,
        });
    }
    let has_surface_formats = !unsafe {
        presentation
            .surface_loader
            .get_physical_device_surface_formats(physical_device, presentation.surface)
    }
    .map_err(BackendError::InspectPresentationSupport)?
    .is_empty();
    let has_present_modes = !unsafe {
        presentation
            .surface_loader
            .get_physical_device_surface_present_modes(physical_device, presentation.surface)
    }
    .map_err(BackendError::InspectPresentationSupport)?
    .is_empty();
    let graphics_queue_family_index = queue_families
        .iter()
        .position(|queue_family| queue_family.supports_graphics)
        .and_then(|index| u32::try_from(index).ok())
        .unwrap_or(u32::MAX);
    let presentation_queue_family_index = queue_families
        .iter()
        .position(|queue_family| queue_family.supports_presentation)
        .and_then(|index| u32::try_from(index).ok())
        .unwrap_or(u32::MAX);

    Ok(InspectedDevice {
        physical_device,
        candidate: DeviceCandidate {
            name,
            api_version: properties.api_version,
            driver_version: properties.driver_version,
            supports_swapchain,
            has_surface_formats,
            has_present_modes,
            queue_families,
        },
        graphics_queue_family_index,
        presentation_queue_family_index,
    })
}

fn create_logical_device(
    instance: &Instance,
    selected_device: &InspectedDevice,
) -> Result<ash::Device, BackendError> {
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
    unsafe { instance.create_device(selected_device.physical_device, &device_create_info, None) }
        .map_err(BackendError::CreateDevice)
}

impl DeviceCandidate {
    fn is_suitable(&self) -> bool {
        self.api_version >= vk::API_VERSION_1_3
            && self.supports_swapchain
            && self.has_surface_formats
            && self.has_present_modes
            && self
                .queue_families
                .iter()
                .any(|queue_family| queue_family.supports_graphics)
            && self
                .queue_families
                .iter()
                .any(|queue_family| queue_family.supports_presentation)
    }
}

pub fn select_device(
    candidates: Vec<DeviceCandidate>,
) -> Result<DeviceCandidate, DeviceSelectionError> {
    candidates
        .into_iter()
        .find(DeviceCandidate::is_suitable)
        .ok_or(DeviceSelectionError::NoSuitableDevice)
}

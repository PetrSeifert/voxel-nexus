use super::frame_observation::FrameObservationError;
use super::render_path::RenderPathPhase;
use ash::vk;
use std::fmt;
use thiserror::Error;

#[derive(Clone, Debug, PartialEq)]
pub struct RuntimeContext {
    pub device_name: String,
    pub driver_version: u32,
    pub api_version: u32,
    pub validation_enabled: bool,
    pub present_mode: vk::PresentModeKHR,
    pub timestamp_valid_bits: u32,
    pub timestamp_period_nanoseconds: f64,
}

impl fmt::Display for RuntimeContext {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "Vulkan device: {}\nDriver version: {} ({:#010x})\nVulkan API version: {}.{}.{}\nVulkan validation: {}\nVulkan present mode: {:?}\nGPU timestamp valid bits: {}\nGPU timestamp period nanoseconds: {}",
            self.device_name,
            self.driver_version,
            self.driver_version,
            vk::api_version_major(self.api_version),
            vk::api_version_minor(self.api_version),
            vk::api_version_patch(self.api_version),
            if self.validation_enabled {
                "enabled"
            } else {
                "disabled"
            },
            self.present_mode,
            self.timestamp_valid_bits,
            self.timestamp_period_nanoseconds,
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RenderBackendOptions {
    pub validation_enabled: bool,
    pub presentation_throttling_enabled: bool,
    pub gpu_timestamps_enabled: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RenderPathDeviceCapabilities {
    pub api_version: u32,
    pub command_queue_flags: vk::QueueFlags,
    pub max_image_dimension_2d: u32,
    pub max_bound_descriptor_sets: u32,
    pub max_per_stage_descriptor_storage_images: u32,
    pub max_per_stage_descriptor_storage_buffers: u32,
    pub max_per_stage_descriptor_sampled_images: u32,
    pub max_per_stage_descriptor_samplers: u32,
    pub max_descriptor_set_storage_images: u32,
    pub max_descriptor_set_storage_buffers: u32,
    pub max_descriptor_set_sampled_images: u32,
    pub max_descriptor_set_samplers: u32,
    pub max_compute_work_group_count: [u32; 3],
    pub max_compute_work_group_invocations: u32,
    pub max_compute_work_group_size: [u32; 3],
    pub max_storage_buffer_range: u32,
    pub max_buffer_size: u64,
    pub rgba8_unorm_optimal_tiling_features: vk::FormatFeatureFlags,
}

impl Default for RenderBackendOptions {
    fn default() -> Self {
        Self {
            validation_enabled: true,
            presentation_throttling_enabled: true,
            gpu_timestamps_enabled: false,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueueFamilyCapabilities {
    pub supports_graphics: bool,
    pub supports_compute: bool,
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

#[derive(Clone, Debug)]
pub struct SurfaceSupport {
    pub capabilities: vk::SurfaceCapabilitiesKHR,
    pub formats: Vec<vk::SurfaceFormatKHR>,
    pub present_modes: Vec<vk::PresentModeKHR>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SwapchainConfiguration {
    pub extent: vk::Extent2D,
    pub image_count: u32,
    pub format: vk::Format,
    pub color_space: vk::ColorSpaceKHR,
    pub present_mode: vk::PresentModeKHR,
    pub composite_alpha: vk::CompositeAlphaFlagsKHR,
    pub pre_transform: vk::SurfaceTransformFlagsKHR,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SwapchainConfigurationState {
    Suspended,
    Ready(SwapchainConfiguration),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DeviceRequirement {
    VulkanApi13 { available_version: u32 },
    SwapchainExtension,
    SurfaceFormats,
    PresentModes,
    GraphicsQueue,
    PresentationQueue,
}

impl fmt::Display for DeviceRequirement {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::VulkanApi13 { available_version } => write!(
                formatter,
                "supports Vulkan {}.{}.{}, but Vulkan 1.3 or newer is required",
                vk::api_version_major(*available_version),
                vk::api_version_minor(*available_version),
                vk::api_version_patch(*available_version)
            ),
            Self::SwapchainExtension => {
                write!(
                    formatter,
                    "the VK_KHR_swapchain device extension is unavailable"
                )
            }
            Self::SurfaceFormats => {
                write!(
                    formatter,
                    "the presentation surface exposes no image formats"
                )
            }
            Self::PresentModes => {
                write!(
                    formatter,
                    "the presentation surface exposes no presentation modes"
                )
            }
            Self::GraphicsQueue => write!(formatter, "no queue family supports graphics"),
            Self::PresentationQueue => {
                write!(
                    formatter,
                    "no queue family can present to the window surface"
                )
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceRejection {
    pub device_name: String,
    pub unmet_requirements: Vec<DeviceRequirement>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceSelectionError {
    pub candidates: Vec<DeviceRejection>,
}

impl fmt::Display for DeviceSelectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.candidates.is_empty() {
            return write!(
                formatter,
                "no Vulkan physical devices were found; install a Vulkan 1.3-capable graphics driver"
            );
        }
        write!(formatter, "no suitable Vulkan device was found:")?;
        for candidate in &self.candidates {
            write!(formatter, "\n- {}: ", candidate.device_name)?;
            for (index, requirement) in candidate.unmet_requirements.iter().enumerate() {
                if index > 0 {
                    write!(formatter, "; ")?;
                }
                write!(formatter, "{requirement}")?;
            }
            if candidate
                .unmet_requirements
                .iter()
                .any(|requirement| matches!(requirement, DeviceRequirement::VulkanApi13 { .. }))
            {
                write!(
                    formatter,
                    "; update the graphics driver or use a Vulkan 1.3-capable GPU"
                )?;
            }
            if candidate
                .unmet_requirements
                .iter()
                .any(|requirement| !matches!(requirement, DeviceRequirement::VulkanApi13 { .. }))
            {
                write!(
                    formatter,
                    "; update the graphics driver or use a GPU and desktop session with Vulkan presentation support"
                )?;
            }
        }
        Ok(())
    }
}

impl std::error::Error for DeviceSelectionError {}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum SwapchainConfigurationError {
    #[error("the presentation surface reported no usable formats")]
    NoSurfaceFormats,
    #[error("the presentation surface reported no usable presentation modes")]
    NoPresentModes,
    #[error(
        "presentation throttling was disabled, but VK_PRESENT_MODE_IMMEDIATE_KHR is unavailable"
    )]
    ImmediatePresentationUnavailable,
    #[error("the presentation surface reported no usable composite-alpha mode")]
    NoCompositeAlphaMode,
}

#[derive(Debug, Error)]
pub enum BackendError {
    #[error(
        "the Render Backend cannot render after a frame failure; shut it down and create a new backend"
    )]
    FrameFailureIsTerminal,
    #[error("could not load the Vulkan loader: {0}")]
    LoadVulkan(#[from] ash::LoadingError),
    #[error("the Vulkan loader supports API {major}.{minor}, but Vulkan 1.3 is required")]
    VulkanLoaderTooOld { major: u32, minor: u32 },
    #[error("could not query the Vulkan loader API version: {0}")]
    QueryVulkanLoaderVersion(vk::Result),
    #[error("could not enumerate Vulkan instance layers: {0}")]
    EnumerateInstanceLayers(vk::Result),
    #[error("Vulkan validation is required, but VK_LAYER_KHRONOS_validation is unavailable")]
    ValidationLayerUnavailable,
    #[error("the platform adapter could not prepare Vulkan presentation: {0}")]
    PlatformAdapter(String),
    #[error("could not create the Vulkan instance: {0}")]
    CreateInstance(vk::Result),
    #[error("could not create the Vulkan validation messenger: {0}")]
    CreateValidationMessenger(vk::Result),
    #[error("could not enumerate Vulkan physical devices: {0}")]
    EnumeratePhysicalDevices(vk::Result),
    #[error("could not inspect Vulkan device presentation support: {0}")]
    InspectPresentationSupport(vk::Result),
    #[error(transparent)]
    SelectDevice(#[from] DeviceSelectionError),
    #[error("could not create the Vulkan logical device: {0}")]
    CreateDevice(vk::Result),
    #[error(transparent)]
    ConfigureSwapchain(#[from] SwapchainConfigurationError),
    #[error("could not create the Vulkan swapchain: {0}")]
    CreateSwapchain(vk::Result),
    #[error("could not obtain the Vulkan swapchain images: {0}")]
    GetSwapchainImages(vk::Result),
    #[error("could not create a Vulkan image view: {0}")]
    CreateImageView(vk::Result),
    #[error("could not create the Vulkan command pool: {0}")]
    CreateCommandPool(vk::Result),
    #[error("could not allocate a Vulkan command buffer: {0}")]
    AllocateCommandBuffer(vk::Result),
    #[error("could not create Vulkan frame synchronization: {0}")]
    CreateFrameSynchronization(vk::Result),
    #[error("could not create the Vulkan timestamp query pool: {0}")]
    CreateTimestampQueryPool(vk::Result),
    #[error("could not read Vulkan timestamp query results: {0}")]
    ReadTimestampQueries(vk::Result),
    #[error(
        "GPU timestamps were requested, but the graphics queue exposes no valid timestamp bits"
    )]
    TimestampQueriesUnsupported,
    #[error(transparent)]
    FrameObservation(#[from] FrameObservationError),
    #[error("could not wait for the previous Vulkan frame: {0}")]
    WaitForFrame(vk::Result),
    #[error("could not wait for the Vulkan device before rebuilding presentation: {0}")]
    WaitForDevice(vk::Result),
    #[error("could not acquire the next Vulkan presentation image: {0}")]
    AcquireSwapchainImage(vk::Result),
    #[error("could not reset Vulkan frame synchronization: {0}")]
    ResetFrame(vk::Result),
    #[error("could not frame Vulkan command recording: {0}")]
    RecordCommands(vk::Result),
    #[error("could not submit the Vulkan frame: {0}")]
    SubmitFrame(vk::Result),
    #[error("could not present the Vulkan frame: {0}")]
    PresentFrame(vk::Result),
    #[error("Vulkan validation reported {count} error(s) during presentation")]
    ValidationErrors { count: usize },
    #[error("the Render Backend exhausted presentation configuration identities")]
    PresentationConfigurationIdentityExhausted,
    #[error("the Render Backend exhausted frame sequence identities")]
    FrameSequenceIdentityExhausted,
    #[error("Render Path {phase} failed: {source}")]
    RenderPath {
        phase: RenderPathPhase,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

impl BackendError {
    pub(super) fn boxed_render_path_failure(
        phase: RenderPathPhase,
        source: Box<dyn std::error::Error + Send + Sync>,
    ) -> Self {
        Self::RenderPath { phase, source }
    }
}

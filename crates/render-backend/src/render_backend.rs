use ash::{Entry, Instance, vk};
use std::ffi::{CStr, CString, c_char, c_void};
use std::fmt;
use std::marker::PhantomData;
use std::ops::Deref;
use std::sync::atomic::{AtomicUsize, Ordering};
use thiserror::Error;

mod render_path_switching;

pub use render_path_switching::{
    CameraStateRevision, RenderPathHandoffControl, RenderPathHandoffMismatch, RenderPathReadiness,
    RenderPathRetirement, RenderPathRoleStatus, RenderPathStamp, RenderPathStrategy,
    RenderPathSwitchDiagnostics, RenderPathSwitchEvent, RenderPathSwitchOwner,
    RenderPathSwitchRequestError, SwitchableRenderPath,
};

const VALIDATION_LAYER_NAME: &CStr = c"VK_LAYER_KHRONOS_validation";

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraState {
    eye: [f32; 3],
    target: [f32; 3],
    up: [f32; 3],
    field_of_view_degrees: f32,
    near_plane: f32,
    far_plane: f32,
}

impl CameraState {
    /// Validates the camera for every nonzero `u32` drawable extent.
    pub fn new(
        eye: [f32; 3],
        target: [f32; 3],
        up: [f32; 3],
        field_of_view_degrees: f32,
        near_plane: f32,
        far_plane: f32,
    ) -> Result<Self, CameraConfigurationError> {
        for (parameter, values) in [
            ("eye", eye.as_slice()),
            ("target", target.as_slice()),
            ("up", up.as_slice()),
            (
                "field of view",
                std::slice::from_ref(&field_of_view_degrees),
            ),
            ("near plane", std::slice::from_ref(&near_plane)),
            ("far plane", std::slice::from_ref(&far_plane)),
        ] {
            if values.iter().any(|value| !value.is_finite()) {
                return Err(CameraConfigurationError::NonfiniteParameter { parameter });
            }
        }
        if eye == target {
            return Err(CameraConfigurationError::CoincidentEyeAndTarget);
        }
        let direction = subtract(target, eye);
        let direction_length_squared = dot(direction, direction);
        if !direction_length_squared.is_finite() || direction_length_squared <= 0.0 {
            return Err(CameraConfigurationError::UnrepresentableTransform);
        }
        // Test collinearity before normalization can introduce rounding noise.
        let [direction_x, direction_y, direction_z] = direction.map(f64::from);
        let [up_x, up_y, up_z] = up.map(f64::from);
        if direction_y * up_z == direction_z * up_y
            && direction_z * up_x == direction_x * up_z
            && direction_x * up_y == direction_y * up_x
        {
            return Err(CameraConfigurationError::DegenerateUpDirection);
        }
        let side = cross(normalize(direction), up);
        let side_length_squared = dot(side, side);
        if side_length_squared == 0.0 {
            return Err(CameraConfigurationError::DegenerateUpDirection);
        }
        if !side_length_squared.is_finite() {
            return Err(CameraConfigurationError::UnrepresentableTransform);
        }
        if !(0.0 < field_of_view_degrees && field_of_view_degrees < 180.0) {
            return Err(CameraConfigurationError::InvalidFieldOfView);
        }
        if !(0.0 < near_plane && near_plane < far_plane) {
            return Err(CameraConfigurationError::InvalidClipPlanes);
        }
        let camera = Self {
            eye,
            target,
            up,
            field_of_view_degrees,
            near_plane,
            far_plane,
        };
        // Only the horizontal projection scale depends on extent. Its largest
        // magnitude occurs at the narrowest supported aspect ratio.
        camera.view_projection([1, u32::MAX])?;
        Ok(camera)
    }

    pub fn eye(self) -> [f32; 3] {
        self.eye
    }

    pub fn target(self) -> [f32; 3] {
        self.target
    }

    pub fn up(self) -> [f32; 3] {
        self.up
    }

    pub fn field_of_view_degrees(self) -> f32 {
        self.field_of_view_degrees
    }

    pub fn near_plane(self) -> f32 {
        self.near_plane
    }

    pub fn far_plane(self) -> f32 {
        self.far_plane
    }

    pub fn view_projection(
        self,
        drawable_dimensions: [u32; 2],
    ) -> Result<[f32; 16], CameraConfigurationError> {
        let [width, height] = drawable_dimensions;
        if width == 0 || height == 0 {
            return Err(CameraConfigurationError::ZeroDrawableExtent);
        }
        let aspect_ratio = width as f32 / height as f32;
        let projection = perspective(
            self.field_of_view_degrees.to_radians(),
            aspect_ratio,
            self.near_plane,
            self.far_plane,
        );
        let view = look_at(self.eye, self.target, self.up);
        let matrix = multiply_matrices(projection, view);
        if matrix.iter().any(|component| !component.is_finite()) {
            return Err(CameraConfigurationError::UnrepresentableTransform);
        }
        Ok(matrix)
    }
}

impl Default for CameraState {
    fn default() -> Self {
        Self {
            eye: [5.0, 4.0, 6.0],
            target: [0.0; 3],
            up: [0.0, 1.0, 0.0],
            field_of_view_degrees: 55.0,
            near_plane: 0.1,
            far_plane: 100.0,
        }
    }
}

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum CameraConfigurationError {
    #[error("camera {parameter} must contain only finite values")]
    NonfiniteParameter { parameter: &'static str },
    #[error("camera eye and target must be distinct")]
    CoincidentEyeAndTarget,
    #[error("camera up direction must be nonzero and not parallel to the view direction")]
    DegenerateUpDirection,
    #[error("camera vertical field of view must be greater than 0 and less than 180 degrees")]
    InvalidFieldOfView,
    #[error("camera clip planes must satisfy 0 < near < far")]
    InvalidClipPlanes,
    #[error(
        "camera transform exceeds f32 precision or range; reduce coordinate, up vector, or projection extremes"
    )]
    UnrepresentableTransform,
    #[error("camera projection requires a non-zero drawable extent")]
    ZeroDrawableExtent,
    #[error("a deterministic camera move requires at least one step")]
    ZeroMoveSteps,
    #[error("camera move step {step} exceeds the final step {total_steps}")]
    MoveStepOutOfRange { step: u32, total_steps: u32 },
}

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
    fn boxed_render_path_failure(
        phase: RenderPathPhase,
        source: Box<dyn std::error::Error + Send + Sync>,
    ) -> Self {
        Self::RenderPath { phase, source }
    }
}

pub fn run_render_path_phase<T>(
    phase: RenderPathPhase,
    operation: impl FnOnce() -> RenderPathResult<T>,
) -> Result<T, BackendError> {
    operation().map_err(|source| BackendError::boxed_render_path_failure(phase, source))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RenderPathPhase {
    Release,
    Configure,
    AdvanceFrameBoundary,
    Record,
    Shutdown,
}

impl fmt::Display for RenderPathPhase {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Release => "release",
            Self::Configure => "configure",
            Self::AdvanceFrameBoundary => "advance frame boundary",
            Self::Record => "record",
            Self::Shutdown => "shutdown",
        })
    }
}

pub type RenderPathResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync + 'static>>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PresentationConfigurationId(u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PresentationImage {
    image: vk::Image,
    view: vk::ImageView,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RenderPathAttachmentIdentity(usize);

pub struct RenderPathAttachment<'target> {
    identity: RenderPathAttachmentIdentity,
    _image: vk::Image,
    view: vk::ImageView,
    lifetime: PhantomData<&'target PresentationImage>,
}

impl RenderPathAttachment<'_> {
    pub fn identity(&self) -> RenderPathAttachmentIdentity {
        self.identity
    }
}

#[derive(Clone, Copy)]
pub struct RenderPathTarget<'target> {
    configuration_id: PresentationConfigurationId,
    format: vk::Format,
    extent: vk::Extent2D,
    images: &'target [PresentationImage],
}

impl<'target> RenderPathTarget<'target> {
    pub fn configuration_id(&self) -> PresentationConfigurationId {
        self.configuration_id
    }

    pub fn format(&self) -> vk::Format {
        self.format
    }

    pub fn extent(&self) -> vk::Extent2D {
        self.extent
    }

    pub fn attachments(&self) -> impl Iterator<Item = RenderPathAttachment<'_>> {
        self.images
            .iter()
            .enumerate()
            .map(|(index, image)| RenderPathAttachment {
                identity: RenderPathAttachmentIdentity(index),
                _image: image.image,
                view: image.view,
                lifetime: PhantomData,
            })
    }
}

pub struct RenderPathFrameTarget<'frame> {
    frame_sequence: u64,
    configuration_id: PresentationConfigurationId,
    attachment: RenderPathAttachment<'frame>,
    format: vk::Format,
    extent: vk::Extent2D,
}

impl RenderPathFrameTarget<'_> {
    pub fn frame_sequence(&self) -> u64 {
        self.frame_sequence
    }

    pub fn configuration_id(&self) -> PresentationConfigurationId {
        self.configuration_id
    }

    pub fn attachment(&self) -> &RenderPathAttachment<'_> {
        &self.attachment
    }

    pub fn format(&self) -> vk::Format {
        self.format
    }

    pub fn extent(&self) -> vk::Extent2D {
        self.extent
    }
}

#[derive(Clone, Copy)]
pub struct RenderPathDeviceContext<'device> {
    device: &'device ash::Device,
    memory_properties: vk::PhysicalDeviceMemoryProperties,
    capabilities: RenderPathDeviceCapabilities,
}

impl RenderPathDeviceContext<'_> {
    pub fn capabilities(&self) -> RenderPathDeviceCapabilities {
        self.capabilities
    }

    pub fn memory_type_index(
        &self,
        memory_type_bits: u32,
        required_properties: vk::MemoryPropertyFlags,
    ) -> Option<u32> {
        self.memory_properties
            .memory_types
            .iter()
            .take(usize::try_from(self.memory_properties.memory_type_count).ok()?)
            .enumerate()
            .find(|(index, memory_type)| {
                u32::try_from(*index)
                    .ok()
                    .and_then(|index| 1_u32.checked_shl(index))
                    .is_some_and(|bit| memory_type_bits & bit != 0)
                    && memory_type.property_flags.contains(required_properties)
            })
            .and_then(|(index, _)| u32::try_from(index).ok())
    }

    /// # Safety
    /// Every queue-family index and pointer in `create_info` must be valid for this device.
    pub unsafe fn create_buffer(
        &self,
        create_info: &vk::BufferCreateInfo<'_>,
    ) -> Result<vk::Buffer, vk::Result> {
        unsafe { self.device.create_buffer(create_info, None) }
    }

    /// # Safety
    /// `buffer` must be a live buffer created by this device.
    pub unsafe fn buffer_memory_requirements(&self, buffer: vk::Buffer) -> vk::MemoryRequirements {
        unsafe { self.device.get_buffer_memory_requirements(buffer) }
    }

    /// # Safety
    /// The allocation size and memory type must be valid for this device.
    pub unsafe fn allocate_memory(
        &self,
        allocate_info: &vk::MemoryAllocateInfo<'_>,
    ) -> Result<vk::DeviceMemory, vk::Result> {
        unsafe { self.device.allocate_memory(allocate_info, None) }
    }

    /// # Safety
    /// Both handles must belong to this device and satisfy the buffer memory requirements.
    pub unsafe fn bind_buffer_memory(
        &self,
        buffer: vk::Buffer,
        memory: vk::DeviceMemory,
    ) -> Result<(), vk::Result> {
        unsafe { self.device.bind_buffer_memory(buffer, memory, 0) }
    }

    /// # Safety
    /// `memory` must be host-visible, coherent, and allocated for at least `bytes.len()` bytes.
    pub unsafe fn write_memory(
        &self,
        memory: vk::DeviceMemory,
        bytes: &[u8],
    ) -> Result<(), vk::Result> {
        let size = u64::try_from(bytes.len()).map_err(|_| vk::Result::ERROR_OUT_OF_HOST_MEMORY)?;
        let destination = unsafe {
            self.device
                .map_memory(memory, 0, size, vk::MemoryMapFlags::empty())?
        };
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), destination.cast(), bytes.len()) };
        unsafe { self.device.unmap_memory(memory) };
        Ok(())
    }

    /// # Safety
    /// `memory` must be host-visible, coherent, and allocated for at least `size` bytes.
    /// No submitted work may access the memory until this call returns.
    pub unsafe fn write_memory_ranges(
        &self,
        memory: vk::DeviceMemory,
        size: u64,
        ranges: &[(usize, &[u8])],
    ) -> Result<(), vk::Result> {
        let length = usize::try_from(size).map_err(|_| vk::Result::ERROR_OUT_OF_HOST_MEMORY)?;
        if ranges.iter().any(|(offset, bytes)| {
            offset
                .checked_add(bytes.len())
                .is_none_or(|end| end > length)
        }) {
            return Err(vk::Result::ERROR_OUT_OF_HOST_MEMORY);
        }
        // Validate every range and map once so a failure cannot leave a partial revision.
        let destination = unsafe {
            self.device
                .map_memory(memory, 0, size, vk::MemoryMapFlags::empty())?
        };
        for (offset, bytes) in ranges {
            unsafe {
                std::ptr::copy_nonoverlapping(
                    bytes.as_ptr(),
                    destination.cast::<u8>().add(*offset),
                    bytes.len(),
                );
            }
        }
        unsafe { self.device.unmap_memory(memory) };
        Ok(())
    }

    /// # Safety
    /// `memory` must be host-visible, coherent, and allocated for at least `bytes.len()` bytes.
    /// Submitted writes to the range must be available to the host before this call.
    pub unsafe fn read_memory(
        &self,
        memory: vk::DeviceMemory,
        bytes: &mut [u8],
    ) -> Result<(), vk::Result> {
        let size = u64::try_from(bytes.len()).map_err(|_| vk::Result::ERROR_OUT_OF_HOST_MEMORY)?;
        let source = unsafe {
            self.device
                .map_memory(memory, 0, size, vk::MemoryMapFlags::empty())?
        };
        unsafe { std::ptr::copy_nonoverlapping(source.cast(), bytes.as_mut_ptr(), bytes.len()) };
        unsafe { self.device.unmap_memory(memory) };
        Ok(())
    }

    /// # Safety
    /// `buffer` must belong to this device and no submitted work may still use it.
    pub unsafe fn destroy_buffer(&self, buffer: vk::Buffer) {
        unsafe { self.device.destroy_buffer(buffer, None) };
    }

    /// # Safety
    /// `memory` must belong to this device and no live resource may remain bound to it.
    pub unsafe fn free_memory(&self, memory: vk::DeviceMemory) {
        unsafe { self.device.free_memory(memory, None) };
    }

    /// # Safety
    /// Every pointer and queue-family index in `create_info` must be valid for this device.
    pub unsafe fn create_image(
        &self,
        create_info: &vk::ImageCreateInfo<'_>,
    ) -> Result<vk::Image, vk::Result> {
        unsafe { self.device.create_image(create_info, None) }
    }

    /// # Safety
    /// `image` must be a live image created by this device.
    pub unsafe fn image_memory_requirements(&self, image: vk::Image) -> vk::MemoryRequirements {
        unsafe { self.device.get_image_memory_requirements(image) }
    }

    /// # Safety
    /// Both handles must belong to this device and satisfy the image memory requirements.
    pub unsafe fn bind_image_memory(
        &self,
        image: vk::Image,
        memory: vk::DeviceMemory,
    ) -> Result<(), vk::Result> {
        unsafe { self.device.bind_image_memory(image, memory, 0) }
    }

    /// # Safety
    /// The referenced image and subresource range must be valid for this device.
    pub unsafe fn create_image_view(
        &self,
        create_info: &vk::ImageViewCreateInfo<'_>,
    ) -> Result<vk::ImageView, vk::Result> {
        unsafe { self.device.create_image_view(create_info, None) }
    }

    /// # Safety
    /// `image_view` must belong to this device and no submitted work may still use it.
    pub unsafe fn destroy_image_view(&self, image_view: vk::ImageView) {
        unsafe { self.device.destroy_image_view(image_view, None) };
    }

    /// # Safety
    /// `image` must belong to this device and no submitted work or live view may use it.
    pub unsafe fn destroy_image(&self, image: vk::Image) {
        unsafe { self.device.destroy_image(image, None) };
    }
    /// # Safety
    ///
    /// Every handle and pointer referenced by `create_info` must be valid for this device.
    pub unsafe fn create_render_pass(
        &self,
        create_info: &vk::RenderPassCreateInfo<'_>,
    ) -> Result<vk::RenderPass, vk::Result> {
        unsafe { self.device.create_render_pass(create_info, None) }
    }

    /// # Safety
    ///
    /// The shader code referenced by `create_info` must satisfy Vulkan's validity rules.
    pub unsafe fn create_shader_module(
        &self,
        create_info: &vk::ShaderModuleCreateInfo<'_>,
    ) -> Result<vk::ShaderModule, vk::Result> {
        unsafe { self.device.create_shader_module(create_info, None) }
    }

    /// # Safety
    ///
    /// Every descriptor-set layout and push-constant range must satisfy Vulkan's validity rules.
    pub unsafe fn create_pipeline_layout(
        &self,
        create_info: &vk::PipelineLayoutCreateInfo<'_>,
    ) -> Result<vk::PipelineLayout, vk::Result> {
        unsafe { self.device.create_pipeline_layout(create_info, None) }
    }

    /// # Safety
    ///
    /// Every handle and pointer in `create_infos` must remain valid for the calls.
    pub unsafe fn create_graphics_pipelines(
        &self,
        pipeline_cache: vk::PipelineCache,
        create_infos: &[vk::GraphicsPipelineCreateInfo<'_>],
    ) -> Result<Vec<vk::Pipeline>, (Vec<vk::Pipeline>, vk::Result)> {
        unsafe {
            self.device
                .create_graphics_pipelines(pipeline_cache, create_infos, None)
        }
    }

    /// # Safety
    ///
    /// Every handle and pointer in `create_infos` must remain valid for the call.
    pub unsafe fn create_compute_pipelines(
        &self,
        pipeline_cache: vk::PipelineCache,
        create_infos: &[vk::ComputePipelineCreateInfo<'_>],
    ) -> Result<Vec<vk::Pipeline>, (Vec<vk::Pipeline>, vk::Result)> {
        unsafe {
            self.device
                .create_compute_pipelines(pipeline_cache, create_infos, None)
        }
    }

    /// # Safety
    ///
    /// Every binding in `create_info` must satisfy Vulkan's descriptor limits.
    pub unsafe fn create_descriptor_set_layout(
        &self,
        create_info: &vk::DescriptorSetLayoutCreateInfo<'_>,
    ) -> Result<vk::DescriptorSetLayout, vk::Result> {
        unsafe { self.device.create_descriptor_set_layout(create_info, None) }
    }

    /// # Safety
    ///
    /// `layout` must belong to this device and no live pipeline layout may depend on it.
    pub unsafe fn destroy_descriptor_set_layout(&self, layout: vk::DescriptorSetLayout) {
        unsafe { self.device.destroy_descriptor_set_layout(layout, None) };
    }

    /// # Safety
    ///
    /// Pool sizes and flags in `create_info` must satisfy Vulkan's validity rules.
    pub unsafe fn create_descriptor_pool(
        &self,
        create_info: &vk::DescriptorPoolCreateInfo<'_>,
    ) -> Result<vk::DescriptorPool, vk::Result> {
        unsafe { self.device.create_descriptor_pool(create_info, None) }
    }

    /// # Safety
    ///
    /// `pool` must belong to this device and no submitted work may use its descriptor sets.
    pub unsafe fn destroy_descriptor_pool(&self, pool: vk::DescriptorPool) {
        unsafe { self.device.destroy_descriptor_pool(pool, None) };
    }

    /// # Safety
    ///
    /// The pool and layouts referenced by `allocate_info` must be live and compatible.
    pub unsafe fn allocate_descriptor_sets(
        &self,
        allocate_info: &vk::DescriptorSetAllocateInfo<'_>,
    ) -> Result<Vec<vk::DescriptorSet>, vk::Result> {
        unsafe { self.device.allocate_descriptor_sets(allocate_info) }
    }

    /// # Safety
    ///
    /// Every descriptor and referenced resource must be valid for this device.
    pub unsafe fn update_descriptor_sets(&self, writes: &[vk::WriteDescriptorSet<'_>]) {
        unsafe { self.device.update_descriptor_sets(writes, &[]) };
    }

    /// # Safety
    ///
    /// Sampler parameters in `create_info` must satisfy the enabled device features and limits.
    pub unsafe fn create_sampler(
        &self,
        create_info: &vk::SamplerCreateInfo<'_>,
    ) -> Result<vk::Sampler, vk::Result> {
        unsafe { self.device.create_sampler(create_info, None) }
    }

    /// # Safety
    ///
    /// `sampler` must belong to this device and no submitted work may still use it.
    pub unsafe fn destroy_sampler(&self, sampler: vk::Sampler) {
        unsafe { self.device.destroy_sampler(sampler, None) };
    }

    /// # Safety
    ///
    /// `render_pass` must belong to this device and be compatible with `attachment`.
    pub unsafe fn create_framebuffer(
        &self,
        render_pass: vk::RenderPass,
        attachment: &RenderPathAttachment<'_>,
        depth_attachment: vk::ImageView,
        extent: vk::Extent2D,
    ) -> Result<vk::Framebuffer, vk::Result> {
        let attachments = [attachment.view, depth_attachment];
        let create_info = vk::FramebufferCreateInfo::default()
            .render_pass(render_pass)
            .attachments(&attachments)
            .width(extent.width)
            .height(extent.height)
            .layers(1);
        unsafe { self.device.create_framebuffer(&create_info, None) }
    }

    /// # Safety
    ///
    /// `render_pass` must belong to this device and be compatible with `attachment`.
    pub unsafe fn create_color_framebuffer(
        &self,
        render_pass: vk::RenderPass,
        attachment: &RenderPathAttachment<'_>,
        extent: vk::Extent2D,
    ) -> Result<vk::Framebuffer, vk::Result> {
        let attachments = [attachment.view];
        let create_info = vk::FramebufferCreateInfo::default()
            .render_pass(render_pass)
            .attachments(&attachments)
            .width(extent.width)
            .height(extent.height)
            .layers(1);
        unsafe { self.device.create_framebuffer(&create_info, None) }
    }

    /// # Safety
    ///
    /// `framebuffer` must belong to this device and no submitted work may still use it.
    pub unsafe fn destroy_framebuffer(&self, framebuffer: vk::Framebuffer) {
        unsafe { self.device.destroy_framebuffer(framebuffer, None) };
    }

    /// # Safety
    ///
    /// `pipeline` must belong to this device and no submitted work may still use it.
    pub unsafe fn destroy_pipeline(&self, pipeline: vk::Pipeline) {
        unsafe { self.device.destroy_pipeline(pipeline, None) };
    }

    /// # Safety
    ///
    /// `pipeline_layout` must belong to this device and no live object may depend on it.
    pub unsafe fn destroy_pipeline_layout(&self, pipeline_layout: vk::PipelineLayout) {
        unsafe { self.device.destroy_pipeline_layout(pipeline_layout, None) };
    }

    /// # Safety
    ///
    /// `render_pass` must belong to this device and no live object may depend on it.
    pub unsafe fn destroy_render_pass(&self, render_pass: vk::RenderPass) {
        unsafe { self.device.destroy_render_pass(render_pass, None) };
    }

    /// # Safety
    ///
    /// `shader_module` must belong to this device and not already have been destroyed.
    pub unsafe fn destroy_shader_module(&self, shader_module: vk::ShaderModule) {
        unsafe { self.device.destroy_shader_module(shader_module, None) };
    }
}

pub struct RenderPathFrameContext<'frame> {
    device: &'frame ash::Device,
    command_buffer: vk::CommandBuffer,
    target: RenderPathFrameTarget<'frame>,
}

impl RenderPathFrameContext<'_> {
    pub fn target(&self) -> &RenderPathFrameTarget<'_> {
        &self.target
    }

    /// # Safety
    ///
    /// The render pass and framebuffer must be compatible and valid for the current target.
    pub unsafe fn begin_render_pass(
        &self,
        begin_info: &vk::RenderPassBeginInfo<'_>,
        contents: vk::SubpassContents,
    ) {
        unsafe {
            self.device
                .cmd_begin_render_pass(self.command_buffer, begin_info, contents)
        };
    }

    /// # Safety
    ///
    /// `pipeline` must be valid, compatible with the active render pass, and use `bind_point`.
    pub unsafe fn bind_pipeline(&self, bind_point: vk::PipelineBindPoint, pipeline: vk::Pipeline) {
        unsafe {
            self.device
                .cmd_bind_pipeline(self.command_buffer, bind_point, pipeline)
        };
    }

    /// # Safety
    ///
    /// The descriptor sets must be live and compatible with `layout` and `bind_point`.
    pub unsafe fn bind_descriptor_sets(
        &self,
        bind_point: vk::PipelineBindPoint,
        layout: vk::PipelineLayout,
        descriptor_sets: &[vk::DescriptorSet],
    ) {
        unsafe {
            self.device.cmd_bind_descriptor_sets(
                self.command_buffer,
                bind_point,
                layout,
                0,
                descriptor_sets,
                &[],
            )
        };
    }

    /// # Safety
    ///
    /// The bound compute state must be valid and each group count must fit the device limit.
    pub unsafe fn dispatch(&self, group_count: [u32; 3]) {
        let [group_count_x, group_count_y, group_count_z] = group_count;
        unsafe {
            self.device.cmd_dispatch(
                self.command_buffer,
                group_count_x,
                group_count_y,
                group_count_z,
            )
        };
    }

    /// # Safety
    ///
    /// Each image barrier must describe a live image and valid old and new layouts.
    pub unsafe fn image_pipeline_barrier(
        &self,
        source_stage: vk::PipelineStageFlags,
        destination_stage: vk::PipelineStageFlags,
        barriers: &[vk::ImageMemoryBarrier<'_>],
    ) {
        unsafe {
            self.device.cmd_pipeline_barrier(
                self.command_buffer,
                source_stage,
                destination_stage,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                barriers,
            )
        };
    }

    /// # Safety
    /// Each buffer barrier must describe a live buffer and valid access masks.
    pub unsafe fn buffer_pipeline_barrier(
        &self,
        source_stage: vk::PipelineStageFlags,
        destination_stage: vk::PipelineStageFlags,
        barriers: &[vk::BufferMemoryBarrier<'_>],
    ) {
        unsafe {
            self.device.cmd_pipeline_barrier(
                self.command_buffer,
                source_stage,
                destination_stage,
                vk::DependencyFlags::empty(),
                &[],
                barriers,
                &[],
            )
        };
    }

    /// # Safety
    /// A compatible live vertex buffer must be supplied during command recording.
    pub unsafe fn bind_vertex_buffer(&self, buffer: vk::Buffer) {
        unsafe {
            self.device
                .cmd_bind_vertex_buffers(self.command_buffer, 0, &[buffer], &[0])
        };
    }

    /// # Safety
    /// A live `u32` index buffer must be supplied during command recording.
    pub unsafe fn bind_index_buffer(&self, buffer: vk::Buffer) {
        unsafe {
            self.device
                .cmd_bind_index_buffer(self.command_buffer, buffer, 0, vk::IndexType::UINT32)
        };
    }

    /// # Safety
    /// `layout` must declare a vertex push-constant range covering all supplied bytes.
    pub unsafe fn push_vertex_constants(&self, layout: vk::PipelineLayout, bytes: &[u8]) {
        unsafe {
            self.device.cmd_push_constants(
                self.command_buffer,
                layout,
                vk::ShaderStageFlags::VERTEX,
                0,
                bytes,
            )
        };
    }

    /// # Safety
    /// All graphics state and buffers required for the indexed draw must be valid and bound.
    pub unsafe fn draw_indexed(&self, index_count: u32) {
        unsafe {
            self.device
                .cmd_draw_indexed(self.command_buffer, index_count, 1, 0, 0, 0)
        };
    }

    /// # Safety
    ///
    /// Bound state must satisfy Vulkan's requirements for this draw.
    pub unsafe fn draw(
        &self,
        vertex_count: u32,
        instance_count: u32,
        first_vertex: u32,
        first_instance: u32,
    ) {
        unsafe {
            self.device.cmd_draw(
                self.command_buffer,
                vertex_count,
                instance_count,
                first_vertex,
                first_instance,
            )
        };
    }

    /// # Safety
    ///
    /// A render pass must be active in this frame context.
    pub unsafe fn end_render_pass(&self) {
        unsafe { self.device.cmd_end_render_pass(self.command_buffer) };
    }
}

pub trait RenderPath {
    fn submit_edit_outcome(
        &mut self,
        _outcome: voxel_frontend::VoxelEditOutcome,
    ) -> RenderPathResult<()> {
        Err(Box::new(RenderPathEditError::SubmissionUnavailable))
    }

    fn publish_camera_state(
        &mut self,
        _camera_state: CameraState,
        _camera_state_revision: CameraStateRevision,
    ) -> RenderPathResult<()> {
        Err(Box::new(RenderPathCameraStateError::PublicationUnavailable))
    }

    fn switch_diagnostics(&self) -> Option<RenderPathSwitchDiagnostics> {
        None
    }

    fn request_switch(
        &mut self,
        _replacement: Box<dyn SwitchableRenderPath>,
    ) -> Result<(), RenderPathSwitchRequestError> {
        Err(RenderPathSwitchRequestError::SwitchingUnavailable)
    }

    fn release(&mut self, device: RenderPathDeviceContext<'_>) -> RenderPathResult<()>;

    fn configure(
        &mut self,
        device: RenderPathDeviceContext<'_>,
        target: RenderPathTarget<'_>,
    ) -> RenderPathResult<()>;

    fn advance_frame_boundary(
        &mut self,
        _device: RenderPathDeviceContext<'_>,
        _target: RenderPathTarget<'_>,
    ) -> RenderPathResult<()> {
        Ok(())
    }

    fn shutdown(&mut self, _device: RenderPathDeviceContext<'_>) -> RenderPathResult<()> {
        Ok(())
    }

    fn record(&mut self, frame: RenderPathFrameContext<'_>) -> RenderPathResult<()>;
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum RenderPathEditError {
    #[error("the presenting Render Path does not accept Voxel Edit outcomes")]
    SubmissionUnavailable,
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum RenderPathCameraStateError {
    #[error("the active Render Path does not accept shared Camera State publication")]
    PublicationUnavailable,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrameOutcome {
    Presented,
    Recreate,
    RetryLater,
    Suspended,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameObservation {
    pub sequence: u64,
    pub gpu_frame_milliseconds: f64,
}

impl FrameObservation {
    pub fn from_gpu_timestamps(
        sequence: u64,
        start: u64,
        end: u64,
        timestamp_valid_bits: u32,
        timestamp_period_nanoseconds: f64,
    ) -> Result<Self, FrameObservationError> {
        if !timestamp_period_nanoseconds.is_finite() || timestamp_period_nanoseconds <= 0.0 {
            return Err(FrameObservationError::InvalidTimestampPeriod);
        }
        if timestamp_valid_bits == 0 || timestamp_valid_bits > 64 {
            return Err(FrameObservationError::InvalidTimestampValidBits);
        }
        let timestamp_mask = if timestamp_valid_bits == 64 {
            u64::MAX
        } else {
            (1_u64 << timestamp_valid_bits) - 1
        };
        let ticks = (end & timestamp_mask).wrapping_sub(start & timestamp_mask) & timestamp_mask;
        let gpu_frame_milliseconds = ticks as f64 * timestamp_period_nanoseconds / 1_000_000.0;
        if !gpu_frame_milliseconds.is_finite() {
            return Err(FrameObservationError::InvalidDuration);
        }
        Ok(Self {
            sequence,
            gpu_frame_milliseconds,
        })
    }
}

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum FrameObservationError {
    #[error("the graphics queue timestamp valid-bit count must be between 1 and 64")]
    InvalidTimestampValidBits,
    #[error("the Vulkan timestamp period must be finite and positive")]
    InvalidTimestampPeriod,
    #[error("the GPU timestamp duration is not finite")]
    InvalidDuration,
}

#[derive(Default)]
pub struct FrameObservationBuffer {
    latest: Option<FrameObservation>,
}

impl FrameObservationBuffer {
    pub fn publish(&mut self, observation: FrameObservation) -> Result<(), FrameObservationError> {
        if !observation.gpu_frame_milliseconds.is_finite()
            || observation.gpu_frame_milliseconds.is_sign_negative()
        {
            return Err(FrameObservationError::InvalidDuration);
        }
        self.latest = Some(observation);
        Ok(())
    }

    pub fn take(&mut self) -> Option<FrameObservation> {
        self.latest.take()
    }
}

pub struct RenderBackend {
    frame_failed: bool,
    rendering: Option<PresentationResources>,
    path: Box<dyn RenderPath>,
    device: LogicalDevice,
    presentation: InstanceSurface,
    selected_device: InspectedDevice,
    graphics_queue: vk::Queue,
    presentation_queue: vk::Queue,
    runtime_context: RuntimeContext,
    drawable_extent: vk::Extent2D,
    swapchain_needs_recreation: bool,
    next_configuration_id: u64,
    frame_sequences: BackendFrameSequences,
    path_is_configured: bool,
    path_is_shutdown: bool,
    memory_properties: vk::PhysicalDeviceMemoryProperties,
    render_path_device_capabilities: RenderPathDeviceCapabilities,
    options: RenderBackendOptions,
}

struct BackendFrameSequences {
    next: u64,
}

impl BackendFrameSequences {
    fn new() -> Self {
        Self { next: 1 }
    }

    fn pending(&self) -> u64 {
        self.next
    }

    fn record_submission(&mut self) -> Result<(), BackendError> {
        self.next = self
            .next
            .checked_add(1)
            .ok_or(BackendError::FrameSequenceIdentityExhausted)?;
        Ok(())
    }
}

impl RenderBackend {
    pub fn initialize(
        application_name: &CStr,
        adapter: &impl PresentationAdapter,
        initial_drawable_extent: vk::Extent2D,
        path: impl RenderPath + 'static,
    ) -> Result<Self, BackendError> {
        Self::initialize_with_options(
            application_name,
            adapter,
            initial_drawable_extent,
            path,
            RenderBackendOptions::default(),
        )
    }

    pub fn initialize_with_options(
        application_name: &CStr,
        adapter: &impl PresentationAdapter,
        initial_drawable_extent: vk::Extent2D,
        path: impl RenderPath + 'static,
        options: RenderBackendOptions,
    ) -> Result<Self, BackendError> {
        let entry = unsafe { Entry::load()? };
        require_vulkan_1_3_loader(&entry)?;
        if options.validation_enabled {
            require_validation_layer(&entry)?;
        }
        let presentation =
            create_instance_surface(entry, application_name, adapter, options.validation_enabled)?;

        let inspected_devices = inspect_devices(&presentation)?;
        let selected_device = select_inspected_device(inspected_devices)?;
        let memory_properties = unsafe {
            presentation
                .instance
                .get_physical_device_memory_properties(selected_device.physical_device)
        };
        let render_path_device_capabilities = selected_device.render_path_device_capabilities;
        let device = LogicalDevice::new(&presentation.instance, &selected_device)?;
        let graphics_queue =
            unsafe { device.get_device_queue(selected_device.graphics_queue_family_index, 0) };
        let presentation_queue =
            unsafe { device.get_device_queue(selected_device.presentation_queue_family_index, 0) };
        let rendering = PresentationResources::new(
            &presentation,
            &device,
            &selected_device,
            initial_drawable_extent,
            PresentationConfigurationId(0),
            options,
        )?;
        if options.gpu_timestamps_enabled && selected_device.timestamp_valid_bits == 0 {
            return Err(BackendError::TimestampQueriesUnsupported);
        }
        let present_mode = rendering
            .as_ref()
            .map(|rendering| rendering.present_mode)
            .unwrap_or(vk::PresentModeKHR::FIFO);
        let runtime_context = RuntimeContext {
            device_name: selected_device.candidate.name.clone(),
            driver_version: selected_device.candidate.driver_version,
            api_version: selected_device.candidate.api_version,
            validation_enabled: options.validation_enabled,
            present_mode,
            timestamp_valid_bits: selected_device.timestamp_valid_bits,
            timestamp_period_nanoseconds: selected_device.timestamp_period_nanoseconds,
        };

        let mut backend = Self {
            frame_failed: false,
            rendering,
            path: Box::new(path),
            device,
            presentation,
            selected_device,
            graphics_queue,
            presentation_queue,
            runtime_context,
            drawable_extent: initial_drawable_extent,
            swapchain_needs_recreation: false,
            next_configuration_id: 1,
            frame_sequences: BackendFrameSequences::new(),
            path_is_configured: false,
            path_is_shutdown: false,
            memory_properties,
            render_path_device_capabilities,
            options,
        };
        backend.configure_path()?;
        Ok(backend)
    }

    pub fn runtime_context(&self) -> &RuntimeContext {
        &self.runtime_context
    }

    pub fn render_path_switch_diagnostics(&self) -> Option<RenderPathSwitchDiagnostics> {
        self.path.switch_diagnostics()
    }

    pub fn submit_edit_outcome(
        &mut self,
        outcome: voxel_frontend::VoxelEditOutcome,
    ) -> RenderPathResult<()> {
        self.path.submit_edit_outcome(outcome)
    }

    pub fn request_render_path_switch(
        &mut self,
        replacement: Box<dyn SwitchableRenderPath>,
    ) -> Result<(), RenderPathSwitchRequestError> {
        self.path.request_switch(replacement)
    }

    pub fn publish_camera_state(
        &mut self,
        camera_state: CameraState,
        camera_state_revision: CameraStateRevision,
    ) -> RenderPathResult<()> {
        self.path
            .publish_camera_state(camera_state, camera_state_revision)
    }

    pub fn validation_error_count(&self) -> usize {
        self.presentation.validation_diagnostics.error_count()
    }

    pub fn take_frame_observation(&mut self) -> Option<FrameObservation> {
        self.rendering
            .as_mut()
            .and_then(PresentationResources::take_frame_observation)
    }

    pub fn last_submitted_frame_sequence(&self) -> Option<u64> {
        self.rendering
            .as_ref()
            .and_then(|rendering| rendering.last_submitted_frame_sequence)
    }

    pub fn presentation_extent(&self) -> Option<vk::Extent2D> {
        self.rendering.as_ref().map(|rendering| rendering.extent)
    }

    pub fn set_drawable_extent(&mut self, drawable_extent: vk::Extent2D) {
        self.drawable_extent = drawable_extent;
        self.swapchain_needs_recreation = true;
    }

    pub fn refresh_render_path(&mut self) -> Result<(), BackendError> {
        if self.frame_failed {
            return Err(BackendError::FrameFailureIsTerminal);
        }
        unsafe { self.device.device_wait_idle() }.map_err(BackendError::WaitForDevice)?;
        self.release_path()?;
        self.configure_path()
    }

    /// A frame error is terminal for this backend. The failing call preserves the original
    /// error; later calls return `FrameFailureIsTerminal`, including after resize or refresh.
    /// Call `shutdown` and create a new backend to resume rendering.
    pub fn draw_frame(&mut self) -> Result<FrameOutcome, BackendError> {
        if self.frame_failed {
            return Err(BackendError::FrameFailureIsTerminal);
        }
        let result = self.draw_frame_inner();
        // Acquisition may have signaled a semaphore without a submission consuming it.
        // Keep all presentation resources for idle shutdown, never for another frame.
        self.frame_failed = result.is_err();
        result
    }

    fn draw_frame_inner(&mut self) -> Result<FrameOutcome, BackendError> {
        if drawable_extent_is_zero(self.drawable_extent) {
            if self.path_is_configured || self.rendering.is_some() {
                unsafe { self.device.device_wait_idle() }.map_err(BackendError::WaitForDevice)?;
                self.release_path()?;
                self.rendering = None;
            }
            return self.ensure_validation_clean(FrameOutcome::Suspended);
        }
        if self.swapchain_needs_recreation || self.rendering.is_none() {
            let swapchain_ready = self.recreate_swapchain()?;
            if !swapchain_ready {
                return self.ensure_validation_clean(FrameOutcome::RetryLater);
            }
        }
        let Some(rendering) = &mut self.rendering else {
            return self.ensure_validation_clean(FrameOutcome::Suspended);
        };
        let submitted_frame_sequence = self.frame_sequences.pending();
        let outcome = match rendering.draw_frame(
            self.path.as_mut(),
            self.memory_properties,
            self.render_path_device_capabilities,
            self.graphics_queue,
            self.presentation_queue,
            submitted_frame_sequence,
        )? {
            PresentationOutcome::Presented => FrameOutcome::Presented,
            PresentationOutcome::Invalidated => {
                self.swapchain_needs_recreation = true;
                FrameOutcome::Recreate
            }
        };
        if rendering.last_submitted_frame_sequence == Some(submitted_frame_sequence) {
            self.frame_sequences.record_submission()?;
        }
        self.ensure_validation_clean(outcome)
    }

    fn recreate_swapchain(&mut self) -> Result<bool, BackendError> {
        unsafe { self.device.device_wait_idle() }.map_err(BackendError::WaitForDevice)?;
        self.release_path()?;
        self.rendering = None;
        let configuration_id = PresentationConfigurationId(self.next_configuration_id);
        self.next_configuration_id = self
            .next_configuration_id
            .checked_add(1)
            .ok_or(BackendError::PresentationConfigurationIdentityExhausted)?;
        let rendering = PresentationResources::new(
            &self.presentation,
            &self.device,
            &self.selected_device,
            self.drawable_extent,
            configuration_id,
            self.options,
        )?;
        let swapchain_ready = rendering.is_some();
        self.rendering = rendering;
        self.configure_path()?;
        self.swapchain_needs_recreation = !swapchain_ready;
        Ok(swapchain_ready)
    }

    fn configure_path(&mut self) -> Result<(), BackendError> {
        let Some(rendering) = &self.rendering else {
            return Ok(());
        };
        self.path_is_configured = true;
        run_render_path_phase(RenderPathPhase::Configure, || {
            self.path.configure(
                RenderPathDeviceContext {
                    device: &self.device,
                    memory_properties: self.memory_properties,
                    capabilities: self.render_path_device_capabilities,
                },
                rendering.render_path_target(),
            )
        })?;
        Ok(())
    }

    fn release_path(&mut self) -> Result<(), BackendError> {
        if !self.path_is_configured {
            return Ok(());
        }
        run_render_path_phase(RenderPathPhase::Release, || {
            self.path.release(RenderPathDeviceContext {
                device: &self.device,
                memory_properties: self.memory_properties,
                capabilities: self.render_path_device_capabilities,
            })
        })?;
        self.path_is_configured = false;
        Ok(())
    }

    fn shutdown_path(&mut self) -> Result<(), BackendError> {
        if self.path_is_shutdown {
            return Ok(());
        }
        run_render_path_phase(RenderPathPhase::Shutdown, || {
            self.path.shutdown(RenderPathDeviceContext {
                device: &self.device,
                memory_properties: self.memory_properties,
                capabilities: self.render_path_device_capabilities,
            })
        })?;
        self.path_is_shutdown = true;
        Ok(())
    }

    pub fn shutdown(&mut self) -> Result<(), BackendError> {
        unsafe { self.device.device_wait_idle() }.map_err(BackendError::WaitForDevice)?;
        self.release_path()?;
        self.shutdown_path()?;
        self.rendering = None;
        Ok(())
    }

    fn ensure_validation_clean(&self, outcome: FrameOutcome) -> Result<FrameOutcome, BackendError> {
        let validation_error_count = self.validation_error_count();
        if validation_error_count > 0 {
            return Err(BackendError::ValidationErrors {
                count: validation_error_count,
            });
        }
        Ok(outcome)
    }
}

impl Drop for RenderBackend {
    fn drop(&mut self) {
        if let Err(error) = unsafe { self.device.device_wait_idle() } {
            eprintln!("Vulkan device did not become idle during shutdown: {error}");
            return;
        }
        if let Err(error) = self.release_path() {
            eprintln!("{error}");
        }
        if let Err(error) = self.shutdown_path() {
            eprintln!("{error}");
        }
    }
}

struct LogicalDevice(ash::Device);

impl LogicalDevice {
    fn new(instance: &Instance, selected_device: &InspectedDevice) -> Result<Self, BackendError> {
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

struct InstanceSurface {
    _entry: Entry,
    instance: Instance,
    debug_loader: Option<ash::ext::debug_utils::Instance>,
    debug_messenger: Option<vk::DebugUtilsMessengerEXT>,
    validation_diagnostics: Box<ValidationDiagnostics>,
    surface_loader: ash::khr::surface::Instance,
    surface: vk::SurfaceKHR,
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
struct InspectedDevice {
    physical_device: vk::PhysicalDevice,
    candidate: DeviceCandidate,
    graphics_queue_family_index: u32,
    presentation_queue_family_index: u32,
    timestamp_valid_bits: u32,
    timestamp_period_nanoseconds: f64,
    render_path_device_capabilities: RenderPathDeviceCapabilities,
}

#[derive(Default)]
struct ValidationDiagnostics {
    errors: AtomicUsize,
}

impl ValidationDiagnostics {
    fn error_count(&self) -> usize {
        self.errors.load(Ordering::SeqCst)
    }
}

struct PresentationResources {
    device: ash::Device,
    swapchain_loader: ash::khr::swapchain::Device,
    swapchain: vk::SwapchainKHR,
    images: Vec<PresentationImage>,
    image_views: Vec<vk::ImageView>,
    command_pool: vk::CommandPool,
    command_buffer: vk::CommandBuffer,
    image_available: vk::Semaphore,
    render_finished: Vec<vk::Semaphore>,
    frame_fence: vk::Fence,
    extent: vk::Extent2D,
    format: vk::Format,
    configuration_id: PresentationConfigurationId,
    present_mode: vk::PresentModeKHR,
    timestamp_query_pool: vk::QueryPool,
    timestamp_valid_bits: u32,
    timestamp_period_nanoseconds: f64,
    pending_frame_sequence: Option<u64>,
    last_submitted_frame_sequence: Option<u64>,
    frame_observations: FrameObservationBuffer,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PresentationOutcome {
    Presented,
    Invalidated,
}

trait FrameBoundaryOperations {
    type Acquired;

    fn wait_for_preceding_frame(&mut self) -> Result<(), BackendError>;

    fn advance_render_path(&mut self) -> Result<(), BackendError>;

    fn acquire_image(&mut self) -> Result<Self::Acquired, BackendError>;
}

fn run_frame_boundary_operations<Operations: FrameBoundaryOperations>(
    operations: &mut Operations,
) -> Result<Operations::Acquired, BackendError> {
    operations.wait_for_preceding_frame()?;
    operations.advance_render_path()?;
    operations.acquire_image()
}

struct VulkanFrameBoundaryOperations<'frame> {
    presentation: &'frame mut PresentationResources,
    path: &'frame mut dyn RenderPath,
    memory_properties: vk::PhysicalDeviceMemoryProperties,
    capabilities: RenderPathDeviceCapabilities,
}

impl FrameBoundaryOperations for VulkanFrameBoundaryOperations<'_> {
    type Acquired = Option<(u32, bool)>;

    fn wait_for_preceding_frame(&mut self) -> Result<(), BackendError> {
        unsafe {
            self.presentation
                .device
                .wait_for_fences(&[self.presentation.frame_fence], true, u64::MAX)
                .map_err(BackendError::WaitForFrame)?;
        }
        self.presentation.collect_timestamp_observation()
    }

    fn advance_render_path(&mut self) -> Result<(), BackendError> {
        run_render_path_phase(RenderPathPhase::AdvanceFrameBoundary, || {
            self.path.advance_frame_boundary(
                RenderPathDeviceContext {
                    device: &self.presentation.device,
                    memory_properties: self.memory_properties,
                    capabilities: self.capabilities,
                },
                self.presentation.render_path_target(),
            )
        })
    }

    fn acquire_image(&mut self) -> Result<Self::Acquired, BackendError> {
        match unsafe {
            self.presentation.swapchain_loader.acquire_next_image(
                self.presentation.swapchain,
                u64::MAX,
                self.presentation.image_available,
                vk::Fence::null(),
            )
        } {
            Ok(acquired_image) => Ok(Some(acquired_image)),
            Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => Ok(None),
            Err(error) => Err(BackendError::AcquireSwapchainImage(error)),
        }
    }
}

impl PresentationResources {
    fn new(
        presentation: &InstanceSurface,
        device: &ash::Device,
        selected_device: &InspectedDevice,
        initial_drawable_extent: vk::Extent2D,
        configuration_id: PresentationConfigurationId,
        options: RenderBackendOptions,
    ) -> Result<Option<Self>, BackendError> {
        let surface_support = query_surface_support(presentation, selected_device.physical_device)?;
        let configuration = select_swapchain_configuration_for_mode(
            &surface_support,
            initial_drawable_extent,
            options.presentation_throttling_enabled,
        )?;
        let SwapchainConfigurationState::Ready(configuration) = configuration else {
            return Ok(None);
        };
        let swapchain_loader = ash::khr::swapchain::Device::new(&presentation.instance, device);
        let mut resources = Self {
            device: device.clone(),
            swapchain_loader,
            swapchain: vk::SwapchainKHR::null(),
            images: Vec::new(),
            image_views: Vec::new(),
            command_pool: vk::CommandPool::null(),
            command_buffer: vk::CommandBuffer::null(),
            image_available: vk::Semaphore::null(),
            render_finished: Vec::new(),
            frame_fence: vk::Fence::null(),
            extent: vk::Extent2D::default(),
            format: configuration.format,
            configuration_id,
            present_mode: configuration.present_mode,
            timestamp_query_pool: vk::QueryPool::null(),
            timestamp_valid_bits: selected_device.timestamp_valid_bits,
            timestamp_period_nanoseconds: selected_device.timestamp_period_nanoseconds,
            pending_frame_sequence: None,
            last_submitted_frame_sequence: None,
            frame_observations: FrameObservationBuffer::default(),
        };

        resources.extent = configuration.extent;
        resources.create_swapchain(presentation, selected_device, &configuration)?;
        let images = unsafe {
            resources
                .swapchain_loader
                .get_swapchain_images(resources.swapchain)
        }
        .map_err(BackendError::GetSwapchainImages)?;
        resources.create_image_views(&images, configuration.format)?;
        resources.create_commands(selected_device.graphics_queue_family_index)?;
        resources.create_synchronization()?;
        if options.gpu_timestamps_enabled {
            resources.create_timestamp_queries()?;
        }
        Ok(Some(resources))
    }

    fn create_timestamp_queries(&mut self) -> Result<(), BackendError> {
        let create_info = vk::QueryPoolCreateInfo::default()
            .query_type(vk::QueryType::TIMESTAMP)
            .query_count(2);
        self.timestamp_query_pool = unsafe { self.device.create_query_pool(&create_info, None) }
            .map_err(BackendError::CreateTimestampQueryPool)?;
        Ok(())
    }

    fn create_swapchain(
        &mut self,
        presentation: &InstanceSurface,
        selected_device: &InspectedDevice,
        configuration: &SwapchainConfiguration,
    ) -> Result<(), BackendError> {
        let queue_family_indices = [
            selected_device.graphics_queue_family_index,
            selected_device.presentation_queue_family_index,
        ];
        let mut create_info = vk::SwapchainCreateInfoKHR::default()
            .surface(presentation.surface)
            .min_image_count(configuration.image_count)
            .image_format(configuration.format)
            .image_color_space(configuration.color_space)
            .image_extent(configuration.extent)
            .image_array_layers(1)
            .image_usage(vk::ImageUsageFlags::COLOR_ATTACHMENT)
            .pre_transform(configuration.pre_transform)
            .composite_alpha(configuration.composite_alpha)
            .present_mode(configuration.present_mode)
            .clipped(true);
        if queue_family_indices[0] != queue_family_indices[1] {
            create_info = create_info
                .image_sharing_mode(vk::SharingMode::CONCURRENT)
                .queue_family_indices(&queue_family_indices);
        } else {
            create_info = create_info.image_sharing_mode(vk::SharingMode::EXCLUSIVE);
        }
        self.swapchain = unsafe { self.swapchain_loader.create_swapchain(&create_info, None) }
            .map_err(BackendError::CreateSwapchain)?;
        Ok(())
    }

    fn create_image_views(
        &mut self,
        images: &[vk::Image],
        format: vk::Format,
    ) -> Result<(), BackendError> {
        for image in images {
            let subresource_range = vk::ImageSubresourceRange::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                .level_count(1)
                .layer_count(1);
            let create_info = vk::ImageViewCreateInfo::default()
                .image(*image)
                .view_type(vk::ImageViewType::TYPE_2D)
                .format(format)
                .subresource_range(subresource_range);
            let image_view = unsafe { self.device.create_image_view(&create_info, None) }
                .map_err(BackendError::CreateImageView)?;
            self.images.push(PresentationImage {
                image: *image,
                view: image_view,
            });
            self.image_views.push(image_view);
        }
        Ok(())
    }

    fn create_commands(&mut self, queue_family_index: u32) -> Result<(), BackendError> {
        let pool_create_info = vk::CommandPoolCreateInfo::default()
            .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER)
            .queue_family_index(queue_family_index);
        self.command_pool = unsafe { self.device.create_command_pool(&pool_create_info, None) }
            .map_err(BackendError::CreateCommandPool)?;
        let allocate_info = vk::CommandBufferAllocateInfo::default()
            .command_pool(self.command_pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(1);
        let mut command_buffers = unsafe { self.device.allocate_command_buffers(&allocate_info) }
            .map_err(BackendError::AllocateCommandBuffer)?;
        self.command_buffer = command_buffers
            .pop()
            .ok_or(BackendError::AllocateCommandBuffer(
                vk::Result::ERROR_UNKNOWN,
            ))?;
        Ok(())
    }

    fn create_synchronization(&mut self) -> Result<(), BackendError> {
        let semaphore_info = vk::SemaphoreCreateInfo::default();
        self.image_available = unsafe { self.device.create_semaphore(&semaphore_info, None) }
            .map_err(BackendError::CreateFrameSynchronization)?;
        for _ in &self.images {
            let render_finished = unsafe { self.device.create_semaphore(&semaphore_info, None) }
                .map_err(BackendError::CreateFrameSynchronization)?;
            self.render_finished.push(render_finished);
        }
        let fence_info = vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED);
        self.frame_fence = unsafe { self.device.create_fence(&fence_info, None) }
            .map_err(BackendError::CreateFrameSynchronization)?;
        Ok(())
    }

    fn draw_frame(
        &mut self,
        path: &mut dyn RenderPath,
        memory_properties: vk::PhysicalDeviceMemoryProperties,
        capabilities: RenderPathDeviceCapabilities,
        graphics_queue: vk::Queue,
        presentation_queue: vk::Queue,
        submitted_frame_sequence: u64,
    ) -> Result<PresentationOutcome, BackendError> {
        self.last_submitted_frame_sequence = None;
        let acquired_image = run_frame_boundary_operations(&mut VulkanFrameBoundaryOperations {
            presentation: self,
            path,
            memory_properties,
            capabilities,
        })?;
        let Some((image_index, acquire_suboptimal)) = acquired_image else {
            return Ok(PresentationOutcome::Invalidated);
        };
        let image_index_usize = usize::try_from(image_index)
            .map_err(|_| BackendError::SubmitFrame(vk::Result::ERROR_UNKNOWN))?;
        let render_finished = self
            .render_finished
            .get(image_index_usize)
            .copied()
            .ok_or(BackendError::SubmitFrame(vk::Result::ERROR_UNKNOWN))?;
        unsafe {
            self.device
                .reset_command_buffer(self.command_buffer, vk::CommandBufferResetFlags::empty())
                .map_err(BackendError::ResetFrame)?;
        }
        self.record_commands(path, image_index, submitted_frame_sequence)?;

        let wait_semaphores = [self.image_available];
        let wait_stages = [vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT];
        let command_buffers = [self.command_buffer];
        let signal_semaphores = [render_finished];
        let submit_info = vk::SubmitInfo::default()
            .wait_semaphores(&wait_semaphores)
            .wait_dst_stage_mask(&wait_stages)
            .command_buffers(&command_buffers)
            .signal_semaphores(&signal_semaphores);
        unsafe {
            self.device
                .reset_fences(&[self.frame_fence])
                .map_err(BackendError::ResetFrame)?;
            self.device
                .queue_submit(graphics_queue, &[submit_info], self.frame_fence)
                .map_err(BackendError::SubmitFrame)?;
        }
        self.last_submitted_frame_sequence = Some(submitted_frame_sequence);
        self.pending_frame_sequence = (self.timestamp_query_pool != vk::QueryPool::null())
            .then_some(submitted_frame_sequence);

        let swapchains = [self.swapchain];
        let image_indices = [image_index];
        let present_info = vk::PresentInfoKHR::default()
            .wait_semaphores(&signal_semaphores)
            .swapchains(&swapchains)
            .image_indices(&image_indices);
        let present_suboptimal = match unsafe {
            self.swapchain_loader
                .queue_present(presentation_queue, &present_info)
        } {
            Ok(suboptimal) => suboptimal,
            Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => true,
            Err(error) => return Err(BackendError::PresentFrame(error)),
        };
        if acquire_suboptimal || present_suboptimal {
            Ok(PresentationOutcome::Invalidated)
        } else {
            Ok(PresentationOutcome::Presented)
        }
    }

    fn record_commands(
        &self,
        path: &mut dyn RenderPath,
        image_index: u32,
        frame_sequence: u64,
    ) -> Result<(), BackendError> {
        let target_index = usize::try_from(image_index)
            .map_err(|_| BackendError::RecordCommands(vk::Result::ERROR_UNKNOWN))?;
        let image = self
            .images
            .get(target_index)
            .copied()
            .ok_or(BackendError::RecordCommands(vk::Result::ERROR_UNKNOWN))?;
        let begin_info = vk::CommandBufferBeginInfo::default()
            .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
        unsafe {
            self.device
                .begin_command_buffer(self.command_buffer, &begin_info)
                .map_err(BackendError::RecordCommands)?;
            if self.timestamp_query_pool != vk::QueryPool::null() {
                self.device.cmd_reset_query_pool(
                    self.command_buffer,
                    self.timestamp_query_pool,
                    0,
                    2,
                );
                self.device.cmd_write_timestamp(
                    self.command_buffer,
                    vk::PipelineStageFlags::TOP_OF_PIPE,
                    self.timestamp_query_pool,
                    0,
                );
            }
        }
        run_render_path_phase(RenderPathPhase::Record, || {
            path.record(RenderPathFrameContext {
                device: &self.device,
                command_buffer: self.command_buffer,
                target: RenderPathFrameTarget {
                    frame_sequence,
                    configuration_id: self.configuration_id,
                    attachment: RenderPathAttachment {
                        identity: RenderPathAttachmentIdentity(target_index),
                        _image: image.image,
                        view: image.view,
                        lifetime: PhantomData,
                    },
                    format: self.format,
                    extent: self.extent,
                },
            })
        })?;
        unsafe {
            if self.timestamp_query_pool != vk::QueryPool::null() {
                self.device.cmd_write_timestamp(
                    self.command_buffer,
                    vk::PipelineStageFlags::BOTTOM_OF_PIPE,
                    self.timestamp_query_pool,
                    1,
                );
            }
            self.device
                .end_command_buffer(self.command_buffer)
                .map_err(BackendError::RecordCommands)?;
        }
        Ok(())
    }

    fn collect_timestamp_observation(&mut self) -> Result<(), BackendError> {
        let Some(frame_sequence) = self.pending_frame_sequence else {
            return Ok(());
        };
        let mut timestamps = [0_u64; 2];
        unsafe {
            self.device.get_query_pool_results(
                self.timestamp_query_pool,
                0,
                &mut timestamps,
                vk::QueryResultFlags::TYPE_64 | vk::QueryResultFlags::WAIT,
            )
        }
        .map_err(BackendError::ReadTimestampQueries)?;
        let observation = FrameObservation::from_gpu_timestamps(
            frame_sequence,
            timestamps[0],
            timestamps[1],
            self.timestamp_valid_bits,
            self.timestamp_period_nanoseconds,
        )?;
        self.frame_observations.publish(observation)?;
        self.pending_frame_sequence = None;
        Ok(())
    }

    fn take_frame_observation(&mut self) -> Option<FrameObservation> {
        self.frame_observations.take()
    }

    fn render_path_target(&self) -> RenderPathTarget<'_> {
        RenderPathTarget {
            configuration_id: self.configuration_id,
            format: self.format,
            extent: self.extent,
            images: &self.images,
        }
    }
}

impl Drop for PresentationResources {
    fn drop(&mut self) {
        unsafe {
            if self.timestamp_query_pool != vk::QueryPool::null() {
                self.device
                    .destroy_query_pool(self.timestamp_query_pool, None);
            }
            if self.frame_fence != vk::Fence::null() {
                self.device.destroy_fence(self.frame_fence, None);
            }
            for render_finished in &self.render_finished {
                self.device.destroy_semaphore(*render_finished, None);
            }
            if self.image_available != vk::Semaphore::null() {
                self.device.destroy_semaphore(self.image_available, None);
            }
            if self.command_pool != vk::CommandPool::null() {
                self.device.destroy_command_pool(self.command_pool, None);
            }
            for image_view in &self.image_views {
                self.device.destroy_image_view(*image_view, None);
            }
            if self.swapchain != vk::SwapchainKHR::null() {
                self.swapchain_loader
                    .destroy_swapchain(self.swapchain, None);
            }
        }
    }
}

fn create_instance_surface(
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
    if severity.contains(vk::DebugUtilsMessageSeverityFlagsEXT::ERROR) && !user_data.is_null() {
        let diagnostics = unsafe { &*user_data.cast::<ValidationDiagnostics>() };
        diagnostics.errors.fetch_add(1, Ordering::SeqCst);
    }
    let message = if callback_data.is_null() {
        c"validation callback supplied no diagnostic data"
    } else {
        let message_pointer = unsafe { (*callback_data).p_message };
        if message_pointer.is_null() {
            c"validation callback supplied no diagnostic message"
        } else {
            unsafe { CStr::from_ptr(message_pointer) }
        }
    };
    eprintln!("Vulkan validation {severity:?} {message_type:?}: {message:?}");
    vk::FALSE
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

fn require_validation_layer(entry: &Entry) -> Result<(), BackendError> {
    let layer_properties = unsafe { entry.enumerate_instance_layer_properties() }
        .map_err(BackendError::EnumerateInstanceLayers)?;
    let available = layer_properties.iter().any(|property| {
        let name = unsafe { CStr::from_ptr(property.layer_name.as_ptr()) };
        name == VALIDATION_LAYER_NAME
    });
    if !available {
        return Err(BackendError::ValidationLayerUnavailable);
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

fn select_graphics_queue_family(queue_families: &[QueueFamilyCapabilities]) -> Option<usize> {
    queue_families
        .iter()
        .position(|queue_family| queue_family.supports_graphics && queue_family.supports_compute)
        .or_else(|| {
            queue_families
                .iter()
                .position(|queue_family| queue_family.supports_graphics)
        })
}

fn query_surface_support(
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

fn select_inspected_device(
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

pub fn select_swapchain_configuration(
    support: &SurfaceSupport,
    drawable_extent: vk::Extent2D,
) -> Result<SwapchainConfigurationState, SwapchainConfigurationError> {
    select_swapchain_configuration_for_mode(support, drawable_extent, true)
}

fn select_swapchain_configuration_for_mode(
    support: &SurfaceSupport,
    drawable_extent: vk::Extent2D,
    presentation_throttling_enabled: bool,
) -> Result<SwapchainConfigurationState, SwapchainConfigurationError> {
    if drawable_extent_is_zero(drawable_extent) {
        return Ok(SwapchainConfigurationState::Suspended);
    }
    let surface_format = support
        .formats
        .iter()
        .copied()
        .find(|format| {
            format.format == vk::Format::B8G8R8A8_SRGB
                && format.color_space == vk::ColorSpaceKHR::SRGB_NONLINEAR
        })
        .or_else(|| support.formats.first().copied())
        .ok_or(SwapchainConfigurationError::NoSurfaceFormats)?;
    let required_present_mode = if presentation_throttling_enabled {
        vk::PresentModeKHR::FIFO
    } else {
        vk::PresentModeKHR::IMMEDIATE
    };
    let present_mode = support
        .present_modes
        .iter()
        .copied()
        .find(|mode| *mode == required_present_mode)
        .ok_or({
            if presentation_throttling_enabled {
                SwapchainConfigurationError::NoPresentModes
            } else {
                SwapchainConfigurationError::ImmediatePresentationUnavailable
            }
        })?;
    let extent = if support.capabilities.current_extent.width != u32::MAX {
        support.capabilities.current_extent
    } else {
        vk::Extent2D {
            width: drawable_extent.width.clamp(
                support.capabilities.min_image_extent.width,
                support.capabilities.max_image_extent.width,
            ),
            height: drawable_extent.height.clamp(
                support.capabilities.min_image_extent.height,
                support.capabilities.max_image_extent.height,
            ),
        }
    };
    let preferred_image_count = support.capabilities.min_image_count.saturating_add(1);
    let image_count = if support.capabilities.max_image_count == 0 {
        preferred_image_count
    } else {
        preferred_image_count.min(support.capabilities.max_image_count)
    };
    let composite_alpha = [
        vk::CompositeAlphaFlagsKHR::OPAQUE,
        vk::CompositeAlphaFlagsKHR::PRE_MULTIPLIED,
        vk::CompositeAlphaFlagsKHR::POST_MULTIPLIED,
        vk::CompositeAlphaFlagsKHR::INHERIT,
    ]
    .into_iter()
    .find(|mode| {
        support
            .capabilities
            .supported_composite_alpha
            .contains(*mode)
    })
    .ok_or(SwapchainConfigurationError::NoCompositeAlphaMode)?;
    let configuration = SwapchainConfiguration {
        extent,
        image_count,
        format: surface_format.format,
        color_space: surface_format.color_space,
        present_mode,
        composite_alpha,
        pre_transform: support.capabilities.current_transform,
    };
    if drawable_extent_is_zero(configuration.extent) {
        Ok(SwapchainConfigurationState::Suspended)
    } else {
        Ok(SwapchainConfigurationState::Ready(configuration))
    }
}

fn drawable_extent_is_zero(extent: vk::Extent2D) -> bool {
    extent.width == 0 || extent.height == 0
}

fn perspective(field_of_view: f32, aspect_ratio: f32, near: f32, far: f32) -> [f32; 16] {
    let focal_length = 1.0 / (field_of_view * 0.5).tan();
    [
        focal_length / aspect_ratio,
        0.0,
        0.0,
        0.0,
        0.0,
        -focal_length,
        0.0,
        0.0,
        0.0,
        0.0,
        far / (near - far),
        -1.0,
        0.0,
        0.0,
        near * far / (near - far),
        0.0,
    ]
}

fn look_at(eye: [f32; 3], center: [f32; 3], up: [f32; 3]) -> [f32; 16] {
    let forward = normalize(subtract(center, eye));
    let side = normalize(cross(forward, up));
    let upward = cross(side, forward);
    let [side_x, side_y, side_z] = side;
    let [upward_x, upward_y, upward_z] = upward;
    let [forward_x, forward_y, forward_z] = forward;
    [
        side_x,
        upward_x,
        -forward_x,
        0.0,
        side_y,
        upward_y,
        -forward_y,
        0.0,
        side_z,
        upward_z,
        -forward_z,
        0.0,
        -dot(side, eye),
        -dot(upward, eye),
        dot(forward, eye),
        1.0,
    ]
}

fn multiply_matrices(left: [f32; 16], right: [f32; 16]) -> [f32; 16] {
    let mut result = [0.0; 16];
    for column in 0..4 {
        for row in 0..4 {
            let Some(destination) = result.get_mut(column * 4 + row) else {
                continue;
            };
            *destination = (0..4)
                .filter_map(|inner| {
                    let left_value = left.get(inner * 4 + row)?;
                    let right_value = right.get(column * 4 + inner)?;
                    Some(left_value * right_value)
                })
                .sum();
        }
    }
    result
}

fn subtract(left: [f32; 3], right: [f32; 3]) -> [f32; 3] {
    let [left_x, left_y, left_z] = left;
    let [right_x, right_y, right_z] = right;
    [left_x - right_x, left_y - right_y, left_z - right_z]
}

fn dot(left: [f32; 3], right: [f32; 3]) -> f32 {
    let [left_x, left_y, left_z] = left;
    let [right_x, right_y, right_z] = right;
    left_x * right_x + left_y * right_y + left_z * right_z
}

fn cross(left: [f32; 3], right: [f32; 3]) -> [f32; 3] {
    let [left_x, left_y, left_z] = left;
    let [right_x, right_y, right_z] = right;
    [
        left_y * right_z - left_z * right_y,
        left_z * right_x - left_x * right_z,
        left_x * right_y - left_y * right_x,
    ]
}

fn normalize(vector: [f32; 3]) -> [f32; 3] {
    let length = dot(vector, vector).sqrt();
    let [vector_x, vector_y, vector_z] = vector;
    [vector_x / length, vector_y / length, vector_z / length]
}

#[cfg(test)]
mod tests {
    use super::{
        BackendError, BackendFrameSequences, FrameBoundaryOperations, QueueFamilyCapabilities,
        run_frame_boundary_operations, select_graphics_queue_family,
    };
    use ash::vk;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    struct TargetSnapshot {
        configuration_id: u64,
        format: vk::Format,
        extent: vk::Extent2D,
        attachment_count: usize,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum FrameBoundaryEvent {
        Wait,
        Advance(TargetSnapshot),
        Acquire,
    }

    struct ProofFrameBoundaryOperations {
        current_target: TargetSnapshot,
        events: Vec<FrameBoundaryEvent>,
        advance_error: Option<BackendError>,
    }

    impl FrameBoundaryOperations for ProofFrameBoundaryOperations {
        type Acquired = u32;

        fn wait_for_preceding_frame(&mut self) -> Result<(), BackendError> {
            self.events.push(FrameBoundaryEvent::Wait);
            Ok(())
        }

        fn advance_render_path(&mut self) -> Result<(), BackendError> {
            self.events
                .push(FrameBoundaryEvent::Advance(self.current_target));
            match self.advance_error.take() {
                Some(error) => Err(error),
                None => Ok(()),
            }
        }

        fn acquire_image(&mut self) -> Result<Self::Acquired, BackendError> {
            self.events.push(FrameBoundaryEvent::Acquire);
            Ok(2)
        }
    }

    fn target_snapshot(configuration_id: u64, width: u32, height: u32) -> TargetSnapshot {
        TargetSnapshot {
            configuration_id,
            format: vk::Format::B8G8R8A8_SRGB,
            extent: vk::Extent2D { width, height },
            attachment_count: 3,
        }
    }

    #[test]
    fn frame_boundary_waits_then_advances_the_current_complete_target_before_acquisition()
    -> Result<(), BackendError> {
        let mut operations = ProofFrameBoundaryOperations {
            current_target: target_snapshot(1, 800, 600),
            events: Vec::new(),
            advance_error: None,
        };
        let recreated_target = target_snapshot(2, 1200, 700);
        operations.current_target = recreated_target;

        assert_eq!(run_frame_boundary_operations(&mut operations)?, 2);
        assert_eq!(
            operations.events,
            vec![
                FrameBoundaryEvent::Wait,
                FrameBoundaryEvent::Advance(recreated_target),
                FrameBoundaryEvent::Acquire,
            ]
        );
        Ok(())
    }

    #[test]
    fn frame_boundary_failure_prevents_image_acquisition() {
        let mut operations = ProofFrameBoundaryOperations {
            current_target: target_snapshot(2, 1200, 700),
            events: Vec::new(),
            advance_error: Some(BackendError::FrameSequenceIdentityExhausted),
        };

        assert!(run_frame_boundary_operations(&mut operations).is_err());
        assert_eq!(
            operations.events,
            vec![
                FrameBoundaryEvent::Wait,
                FrameBoundaryEvent::Advance(target_snapshot(2, 1200, 700)),
            ]
        );
    }

    #[test]
    fn backend_frame_sequences_remain_monotonic_across_presentation_generations()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut sequences = BackendFrameSequences::new();
        assert_eq!(sequences.pending(), 1);
        sequences.record_submission()?;

        assert_eq!(sequences.pending(), 2);
        sequences.record_submission()?;
        assert_eq!(sequences.pending(), 3);
        Ok(())
    }

    #[test]
    fn graphics_queue_selection_prefers_a_compute_capable_graphics_family() {
        let queue_families = [
            QueueFamilyCapabilities {
                supports_graphics: true,
                supports_compute: false,
                supports_presentation: true,
            },
            QueueFamilyCapabilities {
                supports_graphics: true,
                supports_compute: true,
                supports_presentation: false,
            },
        ];

        assert_eq!(select_graphics_queue_family(&queue_families), Some(1));
    }
}

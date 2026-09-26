use ash::vk;
use render_backend::{
    CameraState, CameraStateRevision, PresentationConfigurationId, RenderPath,
    RenderPathAttachmentIdentity, RenderPathDeviceCapabilities, RenderPathDeviceContext,
    RenderPathFrameContext, RenderPathReadiness, RenderPathResult, RenderPathRetirement,
    RenderPathStamp, RenderPathStrategy, RenderPathTarget, SwitchableRenderPath,
};
use semantic_ray_oracle::{
    AxisNormal, SemanticRay, SemanticRayContact, SemanticRayContactClassification,
    SemanticRayError, SemanticRayObservation, SemanticRayProbe, SemanticRayResult,
};
use std::io::Cursor;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use thiserror::Error;
use voxel_frontend::{
    VoxelCoordinate, VoxelEditOutcome, VoxelSceneId, VoxelSceneRevision, VoxelSceneView,
};

mod compute_convergence;
mod compute_scene;

pub use compute_convergence::{
    ComputeCandidateDisposition, ComputeConvergenceAcceptance, ComputeConvergenceControlError,
    ComputeConvergenceController, ComputeConvergenceError, ComputeConvergenceEvent,
    ComputeConvergenceFailure, ComputeConvergenceFailurePhase, ComputeConvergenceGeneration,
    ComputeConvergenceRetry, ComputeConvergenceStatus, ComputeConvergenceWorkStamp,
    ComputePreparationBarrierObservation,
};
pub use compute_scene::{ComputeSceneBuildError, ComputeSceneBundle, ComputeVolumeHeader};

#[derive(Debug, Error)]
pub enum ComputeCameraRayError {
    #[error("compute camera rays require a nonzero drawable extent")]
    EmptyDrawableExtent,
    #[error("pixel {pixel:?} lies outside drawable extent {extent:?}")]
    PixelOutsideDrawable { pixel: [u32; 2], extent: [u32; 2] },
    #[error("the shared Camera State could not produce a Semantic Ray")]
    SemanticRay(#[from] SemanticRayError),
}

pub fn camera_semantic_ray(
    camera: CameraState,
    extent: vk::Extent2D,
    pixel: [u32; 2],
) -> Result<SemanticRay, ComputeCameraRayError> {
    if extent.width == 0 || extent.height == 0 {
        return Err(ComputeCameraRayError::EmptyDrawableExtent);
    }
    let [pixel_x, pixel_y] = pixel;
    if pixel_x >= extent.width || pixel_y >= extent.height {
        return Err(ComputeCameraRayError::PixelOutsideDrawable {
            pixel,
            extent: [extent.width, extent.height],
        });
    }
    let eye = camera.eye();
    let forward = normalize(subtract(camera.target(), eye));
    let right = normalize(cross(forward, camera.up()));
    let upward = cross(right, forward);
    let normalized_x = 2.0 * (pixel_x as f32 + 0.5) / extent.width as f32 - 1.0;
    let normalized_y = 1.0 - 2.0 * (pixel_y as f32 + 0.5) / extent.height as f32;
    let tangent = (camera.field_of_view_degrees().to_radians() * 0.5).tan();
    let aspect_ratio = extent.width as f32 / extent.height as f32;
    let direction = normalize(add(
        forward,
        add(
            scale(right, normalized_x * tangent * aspect_ratio),
            scale(upward, normalized_y * tangent),
        ),
    ));
    let forward_cosine = dot(direction, forward);
    Ok(SemanticRay::new(
        eye.map(f64::from),
        direction.map(f64::from),
        f64::from(camera.near_plane() / forward_cosine),
        f64::from(camera.far_plane() / forward_cosine),
    )?)
}

const OUTPUT_FORMAT: vk::Format = vk::Format::R8G8B8A8_UNORM;
const WORKGROUP_SIZE: [u32; 3] = [8, 8, 1];
const CAMERA_WORD_COUNT: usize = 20;
const CAMERA_BUFFER_SIZE: u32 = (CAMERA_WORD_COUNT * std::mem::size_of::<f32>()) as u32;
const MAXIMUM_SEMANTIC_RAY_PROBES: usize = 8;
const SEMANTIC_RAY_INPUT_WORD_COUNT: usize = 8;
const SEMANTIC_RAY_OUTPUT_WORD_COUNT: usize = 8;
const SEMANTIC_RAY_INPUT_START: usize = 1;
const SEMANTIC_RAY_OUTPUT_START: usize =
    SEMANTIC_RAY_INPUT_START + MAXIMUM_SEMANTIC_RAY_PROBES * SEMANTIC_RAY_INPUT_WORD_COUNT;
const SEMANTIC_RAY_BUFFER_WORD_COUNT: usize =
    SEMANTIC_RAY_OUTPUT_START + MAXIMUM_SEMANTIC_RAY_PROBES * SEMANTIC_RAY_OUTPUT_WORD_COUNT;
const SEMANTIC_RAY_BUFFER_SIZE: u32 =
    (SEMANTIC_RAY_BUFFER_WORD_COUNT * std::mem::size_of::<u32>()) as u32;

#[derive(Clone, Debug, PartialEq)]
pub struct ComputeSemanticRayProbeObservation {
    probe_identity: String,
    frame_sequence: u64,
    observation: SemanticRayObservation,
}

impl ComputeSemanticRayProbeObservation {
    pub fn probe_identity(&self) -> &str {
        &self.probe_identity
    }

    pub fn frame_sequence(&self) -> u64 {
        self.frame_sequence
    }

    pub fn observation(&self) -> &SemanticRayObservation {
        &self.observation
    }
}

#[derive(Clone, Debug)]
pub struct ComputeSemanticRayController {
    state: Arc<Mutex<ComputeSemanticRayControlState>>,
}

#[derive(Debug, Default)]
struct ComputeSemanticRayControlState {
    pending: Option<Vec<SemanticRayProbe>>,
    retained: Vec<ComputeSemanticRayProbeObservation>,
}

#[derive(Clone, Debug, Error, PartialEq)]
pub enum ComputeSemanticRayControlError {
    #[error("at most {maximum} compute Semantic Ray probes can be requested at once")]
    TooManyProbes { maximum: usize },
    #[error("compute Semantic Ray probe {probe_identity} cannot be represented as f32 GPU input")]
    InputNotRepresentable { probe_identity: String },
    #[error("a compute Semantic Ray observation request is already pending")]
    RequestPending,
    #[error("compute Semantic Ray observation control is unavailable")]
    Unavailable,
}

impl ComputeSemanticRayController {
    pub fn request(
        &self,
        probes: Vec<SemanticRayProbe>,
    ) -> Result<(), ComputeSemanticRayControlError> {
        if probes.len() > MAXIMUM_SEMANTIC_RAY_PROBES {
            return Err(ComputeSemanticRayControlError::TooManyProbes {
                maximum: MAXIMUM_SEMANTIC_RAY_PROBES,
            });
        }
        if let Some(probe) = probes.iter().find(|probe| !probe_is_representable(probe)) {
            return Err(ComputeSemanticRayControlError::InputNotRepresentable {
                probe_identity: probe.identity().to_owned(),
            });
        }
        let mut state = self
            .state
            .lock()
            .map_err(|_| ComputeSemanticRayControlError::Unavailable)?;
        if state.pending.is_some() {
            return Err(ComputeSemanticRayControlError::RequestPending);
        }
        state.pending = Some(probes);
        Ok(())
    }

    pub fn drain(
        &self,
    ) -> Result<Vec<ComputeSemanticRayProbeObservation>, ComputeSemanticRayControlError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| ComputeSemanticRayControlError::Unavailable)?;
        Ok(std::mem::take(&mut state.retained))
    }

    fn take_pending(
        &self,
    ) -> Result<Option<Vec<SemanticRayProbe>>, ComputeSemanticRayControlError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| ComputeSemanticRayControlError::Unavailable)?;
        Ok(state.pending.take())
    }

    fn retain(
        &self,
        observations: Vec<ComputeSemanticRayProbeObservation>,
    ) -> Result<(), ComputeSemanticRayControlError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| ComputeSemanticRayControlError::Unavailable)?;
        state.retained.extend(observations);
        Ok(())
    }
}

fn probe_is_representable(probe: &SemanticRayProbe) -> bool {
    let ray = probe.ray();
    ray.origin()
        .into_iter()
        .chain(ray.direction())
        .chain([ray.minimum_distance(), ray.maximum_distance()])
        .all(|value| (value as f32).is_finite())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ComputeDispatchConfiguration {
    output_extent: vk::Extent2D,
    output_format: vk::Format,
    workgroup_size: [u32; 3],
    group_count: [u32; 3],
}

impl ComputeDispatchConfiguration {
    pub fn output_extent(&self) -> vk::Extent2D {
        self.output_extent
    }

    pub fn output_format(&self) -> vk::Format {
        self.output_format
    }

    pub fn workgroup_size(&self) -> [u32; 3] {
        self.workgroup_size
    }

    pub fn group_count(&self) -> [u32; 3] {
        self.group_count
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ComputeCapabilityRecord {
    queried: RenderPathDeviceCapabilities,
    dispatch: ComputeDispatchConfiguration,
}

impl ComputeCapabilityRecord {
    pub fn queried(&self) -> RenderPathDeviceCapabilities {
        self.queried
    }

    pub fn dispatch(&self) -> ComputeDispatchConfiguration {
        self.dispatch
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ComputeDescriptorRequirement {
    BoundSet,
    StorageImagePerStage,
    StorageImagePerSet,
    StorageBufferPerStage,
    StorageBufferPerSet,
    SampledImagePerStage,
    SampledImagePerSet,
    SamplerPerStage,
    SamplerPerSet,
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum ComputeRenderPathRejection {
    #[error(
        "the device exposes Vulkan {major}.{minor}, but the compute Render Path requires Vulkan 1.3"
    )]
    VulkanApiVersion { major: u32, minor: u32 },
    #[error("the Render Backend command queue does not support graphics")]
    GraphicsQueueUnavailable,
    #[error("the Render Backend command queue does not support compute")]
    ComputeQueueUnavailable,
    #[error("the compute output extent must have nonzero width and height, got {width}x{height}")]
    EmptyOutputDimensions { width: u32, height: u32 },
    #[error(
        "the output extent {width}x{height} exceeds the maximum two-dimensional image dimension {maximum}"
    )]
    OutputDimensions {
        width: u32,
        height: u32,
        maximum: u32,
    },
    #[error(
        "the {requirement:?} descriptor limit is {available}, but {required} bindings are required"
    )]
    DescriptorBindingRange {
        requirement: ComputeDescriptorRequirement,
        required: u32,
        available: u32,
    },
    #[error(
        "the maximum storage-buffer binding range is {available} bytes, but {required} bytes are required"
    )]
    StorageBufferBindingRange { required: u32, available: u32 },
    #[error(
        "{format:?} lacks optimal-tiling storage-image and sampled-image support; available features are {available:?}"
    )]
    OutputFormat {
        format: vk::Format,
        available: vk::FormatFeatureFlags,
    },
    #[error(
        "the chosen workgroup size {chosen:?} with {chosen_invocations} invocations exceeds the device limits {maximum_size:?} and {maximum_invocations} invocations"
    )]
    WorkgroupSize {
        chosen: [u32; 3],
        chosen_invocations: u32,
        maximum_size: [u32; 3],
        maximum_invocations: u32,
    },
    #[error("the required dispatch group count {required:?} exceeds the device limit {maximum:?}")]
    DispatchGroupCount {
        required: [u32; 3],
        maximum: [u32; 3],
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ComputeCapabilityAssessment {
    Qualified(ComputeCapabilityRecord),
    Rejected {
        queried: RenderPathDeviceCapabilities,
        output_extent: vk::Extent2D,
        reason: ComputeRenderPathRejection,
    },
}

pub fn qualify_compute_render_path(
    queried: RenderPathDeviceCapabilities,
    output_extent: vk::Extent2D,
) -> Result<ComputeCapabilityRecord, ComputeRenderPathRejection> {
    if queried.api_version < vk::API_VERSION_1_3 {
        return Err(ComputeRenderPathRejection::VulkanApiVersion {
            major: vk::api_version_major(queried.api_version),
            minor: vk::api_version_minor(queried.api_version),
        });
    }
    if !queried
        .command_queue_flags
        .contains(vk::QueueFlags::GRAPHICS)
    {
        return Err(ComputeRenderPathRejection::GraphicsQueueUnavailable);
    }
    if !queried
        .command_queue_flags
        .contains(vk::QueueFlags::COMPUTE)
    {
        return Err(ComputeRenderPathRejection::ComputeQueueUnavailable);
    }
    if output_extent.width == 0 || output_extent.height == 0 {
        return Err(ComputeRenderPathRejection::EmptyOutputDimensions {
            width: output_extent.width,
            height: output_extent.height,
        });
    }
    if output_extent.width > queried.max_image_dimension_2d
        || output_extent.height > queried.max_image_dimension_2d
    {
        return Err(ComputeRenderPathRejection::OutputDimensions {
            width: output_extent.width,
            height: output_extent.height,
            maximum: queried.max_image_dimension_2d,
        });
    }

    let descriptor_limits = [
        (
            ComputeDescriptorRequirement::BoundSet,
            queried.max_bound_descriptor_sets,
            1,
        ),
        (
            ComputeDescriptorRequirement::StorageImagePerStage,
            queried.max_per_stage_descriptor_storage_images,
            1,
        ),
        (
            ComputeDescriptorRequirement::StorageImagePerSet,
            queried.max_descriptor_set_storage_images,
            1,
        ),
        (
            ComputeDescriptorRequirement::StorageBufferPerStage,
            queried.max_per_stage_descriptor_storage_buffers,
            3,
        ),
        (
            ComputeDescriptorRequirement::StorageBufferPerSet,
            queried.max_descriptor_set_storage_buffers,
            3,
        ),
        (
            ComputeDescriptorRequirement::SampledImagePerStage,
            queried.max_per_stage_descriptor_sampled_images,
            1,
        ),
        (
            ComputeDescriptorRequirement::SampledImagePerSet,
            queried.max_descriptor_set_sampled_images,
            1,
        ),
        (
            ComputeDescriptorRequirement::SamplerPerStage,
            queried.max_per_stage_descriptor_samplers,
            1,
        ),
        (
            ComputeDescriptorRequirement::SamplerPerSet,
            queried.max_descriptor_set_samplers,
            1,
        ),
    ];
    if let Some((requirement, available, required)) = descriptor_limits
        .into_iter()
        .find(|(_, available, required)| available < required)
    {
        return Err(ComputeRenderPathRejection::DescriptorBindingRange {
            requirement,
            required,
            available,
        });
    }
    let required_storage_buffer_range = CAMERA_BUFFER_SIZE.max(SEMANTIC_RAY_BUFFER_SIZE);
    if queried.max_storage_buffer_range < required_storage_buffer_range {
        return Err(ComputeRenderPathRejection::StorageBufferBindingRange {
            required: required_storage_buffer_range,
            available: queried.max_storage_buffer_range,
        });
    }

    let required_format_features =
        vk::FormatFeatureFlags::STORAGE_IMAGE | vk::FormatFeatureFlags::SAMPLED_IMAGE;
    if !queried
        .rgba8_unorm_optimal_tiling_features
        .contains(required_format_features)
    {
        return Err(ComputeRenderPathRejection::OutputFormat {
            format: OUTPUT_FORMAT,
            available: queried.rgba8_unorm_optimal_tiling_features,
        });
    }

    let chosen_invocations = WORKGROUP_SIZE.into_iter().product();
    if WORKGROUP_SIZE
        .into_iter()
        .zip(queried.max_compute_work_group_size)
        .any(|(chosen, maximum)| chosen > maximum)
        || chosen_invocations > queried.max_compute_work_group_invocations
    {
        return Err(ComputeRenderPathRejection::WorkgroupSize {
            chosen: WORKGROUP_SIZE,
            chosen_invocations,
            maximum_size: queried.max_compute_work_group_size,
            maximum_invocations: queried.max_compute_work_group_invocations,
        });
    }

    let [workgroup_width, workgroup_height, _] = WORKGROUP_SIZE;
    let group_count = [
        output_extent.width.div_ceil(workgroup_width),
        output_extent.height.div_ceil(workgroup_height),
        1,
    ];
    if group_count
        .into_iter()
        .zip(queried.max_compute_work_group_count)
        .any(|(required, maximum)| required > maximum)
    {
        return Err(ComputeRenderPathRejection::DispatchGroupCount {
            required: group_count,
            maximum: queried.max_compute_work_group_count,
        });
    }

    Ok(ComputeCapabilityRecord {
        queried,
        dispatch: ComputeDispatchConfiguration {
            output_extent,
            output_format: OUTPUT_FORMAT,
            workgroup_size: WORKGROUP_SIZE,
            group_count,
        },
    })
}

#[derive(Debug, Error)]
enum ComputeRenderPathError {
    #[error(transparent)]
    Convergence(#[from] ComputeConvergenceError),
    #[error(transparent)]
    ConvergenceControl(#[from] ComputeConvergenceControlError),
    #[error(transparent)]
    SemanticRayControl(#[from] ComputeSemanticRayControlError),
    #[error(transparent)]
    Rejected(#[from] ComputeRenderPathRejection),
    #[error("could not create the compute output image: {0}")]
    CreateOutputImage(vk::Result),
    #[error("no device-local memory type can hold the compute output image")]
    MissingOutputMemory,
    #[error("could not allocate compute output memory: {0}")]
    AllocateOutputMemory(vk::Result),
    #[error("could not bind compute output memory: {0}")]
    BindOutputMemory(vk::Result),
    #[error("could not create the compute output image view: {0}")]
    CreateOutputView(vk::Result),
    #[error("could not create the compute output sampler: {0}")]
    CreateSampler(vk::Result),
    #[error(
        "the compute Voxel Scene needs a {required}-byte storage-buffer binding, but the device limit is {available} bytes"
    )]
    SceneStorageBufferRange { required: u64, available: u32 },
    #[error("could not create the compute Voxel Scene buffer: {0}")]
    CreateSceneBuffer(vk::Result),
    #[error("no host-visible coherent memory type can hold the compute Voxel Scene buffer")]
    MissingSceneMemory,
    #[error("could not allocate compute Voxel Scene buffer memory: {0}")]
    AllocateSceneMemory(vk::Result),
    #[error("could not bind compute Voxel Scene buffer memory: {0}")]
    BindSceneMemory(vk::Result),
    #[error("could not write the compute Voxel Scene buffer: {0}")]
    WriteSceneMemory(vk::Result),
    #[error("could not create the compute Camera State buffer: {0}")]
    CreateCameraBuffer(vk::Result),
    #[error("no host-visible coherent memory type can hold the compute Camera State buffer")]
    MissingCameraMemory,
    #[error("could not allocate compute Camera State buffer memory: {0}")]
    AllocateCameraMemory(vk::Result),
    #[error("could not bind compute Camera State buffer memory: {0}")]
    BindCameraMemory(vk::Result),
    #[error("could not write the compute Camera State buffer: {0}")]
    WriteCameraMemory(vk::Result),
    #[error("could not create the compute Semantic Ray buffer: {0}")]
    CreateSemanticRayBuffer(vk::Result),
    #[error("no host-visible coherent memory type can hold the compute Semantic Ray buffer")]
    MissingSemanticRayMemory,
    #[error("could not allocate compute Semantic Ray buffer memory: {0}")]
    AllocateSemanticRayMemory(vk::Result),
    #[error("could not bind compute Semantic Ray buffer memory: {0}")]
    BindSemanticRayMemory(vk::Result),
    #[error("could not write compute Semantic Ray input: {0}")]
    WriteSemanticRayMemory(vk::Result),
    #[error("could not read compute Semantic Ray output: {0}")]
    ReadSemanticRayMemory(vk::Result),
    #[error("the compute Semantic Ray buffer is unavailable")]
    SemanticRayResourcesUnavailable,
    #[error("compute Semantic Ray probe {probe_identity} returned invalid GPU data: {reason}")]
    InvalidSemanticRayOutput {
        probe_identity: String,
        reason: &'static str,
    },
    #[error("the compute Camera State buffer is unavailable")]
    CameraResourcesUnavailable,
    #[error("could not create the compute descriptor-set layout: {0}")]
    CreateDescriptorSetLayout(vk::Result),
    #[error("could not create the compute descriptor pool: {0}")]
    CreateDescriptorPool(vk::Result),
    #[error("could not allocate the compute descriptor set: {0}")]
    AllocateDescriptorSet(vk::Result),
    #[error("the compute descriptor allocation returned no set")]
    MissingDescriptorSet,
    #[error("could not read compiled shader code: {0}")]
    ReadShader(#[from] std::io::Error),
    #[error("could not create a shader module: {0}")]
    CreateShaderModule(vk::Result),
    #[error("could not create a pipeline layout: {0}")]
    CreatePipelineLayout(vk::Result),
    #[error("could not create the compute pipeline: {0}")]
    CreateComputePipeline(vk::Result),
    #[error("could not create the composite render pass: {0}")]
    CreateRenderPass(vk::Result),
    #[error("could not create the composite graphics pipeline: {0}")]
    CreateGraphicsPipeline(vk::Result),
    #[error("could not create a composite framebuffer: {0}")]
    CreateFramebuffer(vk::Result),
    #[error("the frame target does not match the configured presentation")]
    StaleFrameTarget,
    #[error("the frame target has no configured composite framebuffer")]
    MissingFramebuffer,
    #[error("the compute convergence hidden candidate is unavailable")]
    MissingHiddenCandidate,
    #[error("injected compute convergence {0:?} failure")]
    InjectedConvergenceFailure(ComputeConvergenceFailurePhase),
    #[error("compute shutdown failed: {0}")]
    Shutdown(String),
    #[error(transparent)]
    Measurement(#[from] ComputeMeasurementControlError),
    #[error(transparent)]
    LifecycleControl(#[from] ComputeLifecycleControlError),
    #[error("compute resource byte accounting overflowed")]
    ResourceAccountingOverflow,
}

pub struct ComputeRayRenderPathAdapter {
    render_path: ComputeRayRenderPath,
    camera_state_revision: CameraStateRevision,
    published_camera_state_revision: CameraStateRevision,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ComputeTimingPhase {
    Preparation,
    Upload,
    Installation,
    Dispatch,
    Composite,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ComputeTimingEvent {
    phase: ComputeTimingPhase,
    scene_identity: VoxelSceneId,
    revision: VoxelSceneRevision,
    generation: u64,
    elapsed_milliseconds: f64,
}

impl ComputeTimingEvent {
    pub fn phase(&self) -> ComputeTimingPhase {
        self.phase
    }

    pub fn scene_identity(&self) -> &VoxelSceneId {
        &self.scene_identity
    }

    pub fn revision(&self) -> VoxelSceneRevision {
        self.revision
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn elapsed_milliseconds(&self) -> f64 {
        self.elapsed_milliseconds
    }
}

#[derive(Default)]
struct ComputeMeasurementState {
    events: Vec<ComputeTimingEvent>,
}

#[derive(Clone)]
pub struct ComputeMeasurementController {
    state: Arc<Mutex<ComputeMeasurementState>>,
}

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
#[error("the compute measurement state is unavailable")]
pub struct ComputeMeasurementControlError;

impl ComputeMeasurementController {
    fn with_initial_event(event: ComputeTimingEvent) -> Self {
        Self {
            state: Arc::new(Mutex::new(ComputeMeasurementState {
                events: vec![event],
            })),
        }
    }

    pub fn drain(&self) -> Result<Vec<ComputeTimingEvent>, ComputeMeasurementControlError> {
        self.state
            .lock()
            .map(|mut state| std::mem::take(&mut state.events))
            .map_err(|_| ComputeMeasurementControlError)
    }

    fn record(&self, event: ComputeTimingEvent) -> Result<(), ComputeMeasurementControlError> {
        self.state
            .lock()
            .map_err(|_| ComputeMeasurementControlError)?
            .events
            .push(event);
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ComputeOwnedResourceCounts {
    bytes: u64,
    objects: usize,
    allocations: usize,
    workers: usize,
    views: usize,
}

impl ComputeOwnedResourceCounts {
    pub fn bytes(self) -> u64 {
        self.bytes
    }

    pub fn objects(self) -> usize {
        self.objects
    }

    pub fn allocations(self) -> usize {
        self.allocations
    }

    pub fn workers(self) -> usize {
        self.workers
    }

    pub fn views(self) -> usize {
        self.views
    }

    pub fn is_zero(self) -> bool {
        self == Self::default()
    }
}

struct ComputeLifecycleControlState {
    shutdown_owned_resources: Option<ComputeOwnedResourceCounts>,
    resource_observations: Vec<ComputeResourceObservation>,
    next_resource_observation_sequence: u64,
    role: ComputeResourceRole,
    last_resource_state: Option<(
        ComputeResourceRole,
        VoxelSceneId,
        ComputeConvergenceStatus,
        ComputeOwnedResourceCounts,
    )>,
}

impl Default for ComputeLifecycleControlState {
    fn default() -> Self {
        Self {
            shutdown_owned_resources: None,
            resource_observations: Vec::new(),
            next_resource_observation_sequence: 0,
            role: ComputeResourceRole::Replacement,
            last_resource_state: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ComputeResourceRole {
    Presenting,
    Replacement,
    Retiring,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ComputeResourceObservationPoint {
    Configured,
    Convergence,
    Released,
    Presentation,
    Shutdown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComputeResourceObservation {
    sequence: u64,
    point: ComputeResourceObservationPoint,
    role: ComputeResourceRole,
    scene_identity: VoxelSceneId,
    status: ComputeConvergenceStatus,
    resources: ComputeOwnedResourceCounts,
}

impl ComputeResourceObservation {
    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    pub fn point(&self) -> ComputeResourceObservationPoint {
        self.point
    }

    pub fn role(&self) -> ComputeResourceRole {
        self.role
    }

    pub fn scene_identity(&self) -> &VoxelSceneId {
        &self.scene_identity
    }

    pub fn status(&self) -> ComputeConvergenceStatus {
        self.status
    }

    pub fn resources(&self) -> ComputeOwnedResourceCounts {
        self.resources
    }
}

#[derive(Clone)]
pub struct ComputeLifecycleController {
    state: Arc<Mutex<ComputeLifecycleControlState>>,
}

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
#[error("the compute lifecycle control state is unavailable")]
pub struct ComputeLifecycleControlError;

impl ComputeLifecycleController {
    fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(ComputeLifecycleControlState::default())),
        }
    }

    pub fn shutdown_owned_resources(
        &self,
    ) -> Result<Option<ComputeOwnedResourceCounts>, ComputeLifecycleControlError> {
        self.state
            .lock()
            .map(|state| state.shutdown_owned_resources)
            .map_err(|_| ComputeLifecycleControlError)
    }

    pub fn drain_resource_observations(
        &self,
    ) -> Result<Vec<ComputeResourceObservation>, ComputeLifecycleControlError> {
        self.state
            .lock()
            .map(|mut state| std::mem::take(&mut state.resource_observations))
            .map_err(|_| ComputeLifecycleControlError)
    }

    fn record_resources(
        &self,
        point: ComputeResourceObservationPoint,
        scene_identity: VoxelSceneId,
        status: ComputeConvergenceStatus,
        resources: ComputeOwnedResourceCounts,
    ) -> Result<(), ComputeLifecycleControlError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| ComputeLifecycleControlError)?;
        let role = state.role;
        let resource_state = (role, scene_identity.clone(), status, resources);
        if point != ComputeResourceObservationPoint::Shutdown
            && state.last_resource_state.as_ref() == Some(&resource_state)
        {
            return Ok(());
        }
        state.last_resource_state = Some(resource_state);
        let sequence = state.next_resource_observation_sequence;
        state.next_resource_observation_sequence = sequence
            .checked_add(1)
            .ok_or(ComputeLifecycleControlError)?;
        state
            .resource_observations
            .push(ComputeResourceObservation {
                sequence,
                point,
                role,
                scene_identity,
                status,
                resources,
            });
        Ok(())
    }

    fn set_role(&self, role: ComputeResourceRole) -> Result<(), ComputeLifecycleControlError> {
        self.state
            .lock()
            .map_err(|_| ComputeLifecycleControlError)?
            .role = role;
        Ok(())
    }

    fn record_shutdown(
        &self,
        scene_identity: VoxelSceneId,
        status: ComputeConvergenceStatus,
        owned_resources: ComputeOwnedResourceCounts,
    ) -> Result<(), ComputeLifecycleControlError> {
        self.record_resources(
            ComputeResourceObservationPoint::Shutdown,
            scene_identity,
            status,
            owned_resources,
        )?;
        self.state
            .lock()
            .map_err(|_| ComputeLifecycleControlError)?
            .shutdown_owned_resources = Some(owned_resources);
        Ok(())
    }
}

impl ComputeRayRenderPathAdapter {
    pub fn new(
        view: VoxelSceneView,
        camera_state: CameraState,
        camera_state_revision: CameraStateRevision,
    ) -> Result<Self, ComputeSceneBuildError> {
        let scene_bundle = ComputeSceneBundle::from_view(&view)?;
        Ok(Self {
            render_path: ComputeRayRenderPath::new(scene_bundle, camera_state, None),
            camera_state_revision,
            published_camera_state_revision: camera_state_revision,
        })
    }

    pub fn new_with_measurement(
        view: VoxelSceneView,
        camera_state: CameraState,
        camera_state_revision: CameraStateRevision,
    ) -> Result<(Self, ComputeMeasurementController), ComputeSceneBuildError> {
        let started_at = Instant::now();
        let scene_bundle = ComputeSceneBundle::from_view(&view)?;
        let measurement = ComputeMeasurementController::with_initial_event(ComputeTimingEvent {
            phase: ComputeTimingPhase::Preparation,
            scene_identity: scene_bundle.scene_identity().clone(),
            revision: scene_bundle.revision(),
            generation: 0,
            elapsed_milliseconds: started_at.elapsed().as_secs_f64() * 1_000.0,
        });
        Ok((
            Self {
                render_path: ComputeRayRenderPath::new(
                    scene_bundle,
                    camera_state,
                    Some(measurement.clone()),
                ),
                camera_state_revision,
                published_camera_state_revision: camera_state_revision,
            },
            measurement,
        ))
    }

    pub fn capability_assessment(&self) -> Option<&ComputeCapabilityAssessment> {
        self.render_path.capability_assessment.as_ref()
    }

    pub fn scene_bundle(&self) -> &ComputeSceneBundle {
        self.render_path.convergence.installed_bundle()
    }

    pub fn camera_state(&self) -> CameraState {
        self.render_path.camera_state
    }

    pub fn accept_edit_outcome(
        &mut self,
        outcome: VoxelEditOutcome,
    ) -> Result<ComputeConvergenceAcceptance, ComputeConvergenceError> {
        self.render_path.convergence.accept(outcome)
    }

    pub fn request_convergence_retry(
        &mut self,
    ) -> Result<ComputeConvergenceRetry, ComputeConvergenceError> {
        self.render_path.convergence.request_retry()
    }

    pub fn convergence_status(&self) -> ComputeConvergenceStatus {
        self.render_path.convergence.status()
    }

    pub fn drain_convergence_events(&mut self) -> Vec<ComputeConvergenceEvent> {
        self.render_path.convergence.drain_events()
    }

    pub fn enable_convergence_control(
        &mut self,
        hold_post_upload: bool,
    ) -> ComputeConvergenceController {
        let controller = self
            .render_path
            .convergence
            .enable_control(hold_post_upload);
        self.render_path.convergence_control = Some(controller.clone());
        controller
    }

    pub fn enable_semantic_ray_observation(&mut self) -> ComputeSemanticRayController {
        let controller = ComputeSemanticRayController {
            state: Arc::new(Mutex::new(ComputeSemanticRayControlState::default())),
        };
        self.render_path.semantic_ray_controller = Some(controller.clone());
        controller
    }

    pub fn enable_lifecycle_control(&mut self) -> ComputeLifecycleController {
        let controller = ComputeLifecycleController::new();
        self.render_path.lifecycle_controller = Some(controller.clone());
        controller
    }
}

impl RenderPath for ComputeRayRenderPathAdapter {
    fn publish_camera_state(
        &mut self,
        camera_state: CameraState,
        camera_state_revision: CameraStateRevision,
    ) -> RenderPathResult<()> {
        self.render_path.camera_state = camera_state;
        self.published_camera_state_revision = camera_state_revision;
        Ok(())
    }

    fn release(&mut self, device: RenderPathDeviceContext<'_>) -> RenderPathResult<()> {
        self.render_path.release(device)
    }

    fn configure(
        &mut self,
        device: RenderPathDeviceContext<'_>,
        target: RenderPathTarget<'_>,
    ) -> RenderPathResult<()> {
        self.render_path.configure(device, target)
    }

    fn shutdown(&mut self, device: RenderPathDeviceContext<'_>) -> RenderPathResult<()> {
        self.render_path.shutdown(device)
    }

    fn advance_frame_boundary(
        &mut self,
        device: RenderPathDeviceContext<'_>,
        target: RenderPathTarget<'_>,
    ) -> RenderPathResult<()> {
        if self.camera_state_revision != self.published_camera_state_revision {
            self.render_path
                .write_camera_state(&device, target.extent())?;
            self.camera_state_revision = self.published_camera_state_revision;
        }
        self.render_path.advance_frame_boundary(device, target)
    }

    fn record(&mut self, frame: RenderPathFrameContext<'_>) -> RenderPathResult<()> {
        if let Some(controller) = &self.render_path.lifecycle_controller {
            controller.set_role(ComputeResourceRole::Presenting)?;
        }
        self.render_path.record(frame)?;
        self.render_path
            .record_resource_observation(ComputeResourceObservationPoint::Presentation)?;
        Ok(())
    }
}

impl SwitchableRenderPath for ComputeRayRenderPathAdapter {
    fn stamp(&self) -> RenderPathStamp {
        let convergence = self.render_path.convergence.status();
        RenderPathStamp::new(
            RenderPathStrategy::ComputeRay,
            self.render_path
                .convergence
                .installed_bundle()
                .scene_identity()
                .clone(),
            convergence.required_revision(),
            convergence.visible_revision(),
            self.camera_state_revision,
            self.render_path.configuration_id,
            if self.render_path.configuration_id.is_some() {
                RenderPathReadiness::Recordable
            } else {
                RenderPathReadiness::Preparing
            },
        )
    }

    fn retire_at_frame_boundary(
        &mut self,
        device: RenderPathDeviceContext<'_>,
    ) -> RenderPathResult<RenderPathRetirement> {
        if let Some(controller) = &self.render_path.lifecycle_controller {
            controller.set_role(ComputeResourceRole::Retiring)?;
        }
        self.render_path.shutdown(device)?;
        Ok(RenderPathRetirement::Complete)
    }
}

struct ComputeRayRenderPath {
    convergence: compute_convergence::ComputeConvergence,
    camera_state: CameraState,
    capability_assessment: Option<ComputeCapabilityAssessment>,
    output_image: vk::Image,
    output_memory: vk::DeviceMemory,
    output_allocation_bytes: u64,
    output_view: vk::ImageView,
    sampler: vk::Sampler,
    scene_buffer: vk::Buffer,
    scene_memory: vk::DeviceMemory,
    scene_allocation_bytes: u64,
    scene_gpu_revision: VoxelSceneRevision,
    hidden_scene_gpu_resources: Option<ComputeHiddenSceneGpuResources>,
    convergence_control: Option<ComputeConvergenceController>,
    lifecycle_controller: Option<ComputeLifecycleController>,
    camera_buffer: vk::Buffer,
    camera_memory: vk::DeviceMemory,
    camera_allocation_bytes: u64,
    semantic_ray_buffer: vk::Buffer,
    semantic_ray_memory: vk::DeviceMemory,
    semantic_ray_allocation_bytes: u64,
    semantic_ray_controller: Option<ComputeSemanticRayController>,
    armed_semantic_ray_probes: Option<Vec<SemanticRayProbe>>,
    recorded_semantic_ray_frame: Option<(u64, VoxelSceneRevision)>,
    descriptor_set_layout: vk::DescriptorSetLayout,
    descriptor_pool: vk::DescriptorPool,
    descriptor_set: vk::DescriptorSet,
    compute_pipeline_layout: vk::PipelineLayout,
    compute_pipeline: vk::Pipeline,
    render_pass: vk::RenderPass,
    composite_pipeline_layout: vk::PipelineLayout,
    composite_pipeline: vk::Pipeline,
    framebuffers: Vec<vk::Framebuffer>,
    configured_attachments: Vec<RenderPathAttachmentIdentity>,
    configuration_id: Option<PresentationConfigurationId>,
    output_extent: vk::Extent2D,
    dispatch_group_count: [u32; 3],
    output_initialized: bool,
    measurement_controller: Option<ComputeMeasurementController>,
}

impl ComputeRayRenderPath {
    fn new(
        scene_bundle: ComputeSceneBundle,
        camera_state: CameraState,
        measurement_controller: Option<ComputeMeasurementController>,
    ) -> Self {
        let scene_gpu_revision = scene_bundle.revision();
        Self {
            convergence: compute_convergence::ComputeConvergence::new(scene_bundle),
            camera_state,
            capability_assessment: None,
            output_image: vk::Image::null(),
            output_memory: vk::DeviceMemory::null(),
            output_allocation_bytes: 0,
            output_view: vk::ImageView::null(),
            sampler: vk::Sampler::null(),
            scene_buffer: vk::Buffer::null(),
            scene_memory: vk::DeviceMemory::null(),
            scene_allocation_bytes: 0,
            scene_gpu_revision,
            hidden_scene_gpu_resources: None,
            convergence_control: None,
            lifecycle_controller: None,
            camera_buffer: vk::Buffer::null(),
            camera_memory: vk::DeviceMemory::null(),
            camera_allocation_bytes: 0,
            semantic_ray_buffer: vk::Buffer::null(),
            semantic_ray_memory: vk::DeviceMemory::null(),
            semantic_ray_allocation_bytes: 0,
            semantic_ray_controller: None,
            armed_semantic_ray_probes: None,
            recorded_semantic_ray_frame: None,
            descriptor_set_layout: vk::DescriptorSetLayout::null(),
            descriptor_pool: vk::DescriptorPool::null(),
            descriptor_set: vk::DescriptorSet::null(),
            compute_pipeline_layout: vk::PipelineLayout::null(),
            compute_pipeline: vk::Pipeline::null(),
            render_pass: vk::RenderPass::null(),
            composite_pipeline_layout: vk::PipelineLayout::null(),
            composite_pipeline: vk::Pipeline::null(),
            framebuffers: Vec::new(),
            configured_attachments: Vec::new(),
            configuration_id: None,
            output_extent: vk::Extent2D::default(),
            dispatch_group_count: [0; 3],
            output_initialized: false,
            measurement_controller,
        }
    }

    fn configure_resources(
        &mut self,
        device: &RenderPathDeviceContext<'_>,
        target: RenderPathTarget<'_>,
        qualification: ComputeCapabilityRecord,
    ) -> Result<(), ComputeRenderPathError> {
        let installation_started_at = Instant::now();
        if self.scene_buffer == vk::Buffer::null() || self.scene_memory == vk::DeviceMemory::null()
        {
            self.release_scene_resources(device);
            let upload_started_at = Instant::now();
            self.create_scene_buffer(device)?;
            self.record_timing(
                ComputeTimingPhase::Upload,
                self.convergence.status().installed(),
                upload_started_at,
            )?;
        }
        if self.semantic_ray_buffer == vk::Buffer::null()
            || self.semantic_ray_memory == vk::DeviceMemory::null()
        {
            self.create_semantic_ray_buffer(device)?;
        }
        self.create_output_resources(device, qualification.dispatch())?;
        self.create_descriptor_resources(device)?;
        self.create_compute_pipeline(device)?;
        self.create_render_pass(device, target.format())?;
        self.create_composite_pipeline(device, target.extent())?;
        for attachment in target.attachments() {
            let framebuffer = unsafe {
                device.create_color_framebuffer(self.render_pass, &attachment, target.extent())
            }
            .map_err(ComputeRenderPathError::CreateFramebuffer)?;
            self.framebuffers.push(framebuffer);
            self.configured_attachments.push(attachment.identity());
        }
        self.configuration_id = Some(target.configuration_id());
        self.output_extent = target.extent();
        self.dispatch_group_count = qualification.dispatch().group_count();
        self.record_timing(
            ComputeTimingPhase::Installation,
            self.convergence.status().installed(),
            installation_started_at,
        )?;
        Ok(())
    }

    fn record_timing(
        &self,
        phase: ComputeTimingPhase,
        stamp: ComputeConvergenceWorkStamp,
        started_at: Instant,
    ) -> Result<(), ComputeRenderPathError> {
        let Some(controller) = &self.measurement_controller else {
            return Ok(());
        };
        controller.record(ComputeTimingEvent {
            phase,
            scene_identity: self.convergence.installed_bundle().scene_identity().clone(),
            revision: stamp.revision(),
            generation: stamp.generation().value(),
            elapsed_milliseconds: started_at.elapsed().as_secs_f64() * 1_000.0,
        })?;
        Ok(())
    }

    fn create_output_resources(
        &mut self,
        device: &RenderPathDeviceContext<'_>,
        dispatch: ComputeDispatchConfiguration,
    ) -> Result<(), ComputeRenderPathError> {
        let extent = dispatch.output_extent();
        let image_info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(dispatch.output_format())
            .extent(vk::Extent3D {
                width: extent.width,
                height: extent.height,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(vk::ImageUsageFlags::STORAGE | vk::ImageUsageFlags::SAMPLED)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        self.output_image = unsafe { device.create_image(&image_info) }
            .map_err(ComputeRenderPathError::CreateOutputImage)?;
        let requirements = unsafe { device.image_memory_requirements(self.output_image) };
        let memory_type_index = device
            .memory_type_index(
                requirements.memory_type_bits,
                vk::MemoryPropertyFlags::DEVICE_LOCAL,
            )
            .ok_or(ComputeRenderPathError::MissingOutputMemory)?;
        let allocation_info = vk::MemoryAllocateInfo::default()
            .allocation_size(requirements.size)
            .memory_type_index(memory_type_index);
        self.output_memory = unsafe { device.allocate_memory(&allocation_info) }
            .map_err(ComputeRenderPathError::AllocateOutputMemory)?;
        self.output_allocation_bytes = requirements.size;
        unsafe { device.bind_image_memory(self.output_image, self.output_memory) }
            .map_err(ComputeRenderPathError::BindOutputMemory)?;
        let view_info = vk::ImageViewCreateInfo::default()
            .image(self.output_image)
            .view_type(vk::ImageViewType::TYPE_2D)
            .format(dispatch.output_format())
            .subresource_range(output_subresource_range());
        self.output_view = unsafe { device.create_image_view(&view_info) }
            .map_err(ComputeRenderPathError::CreateOutputView)?;
        let sampler_info = vk::SamplerCreateInfo::default()
            .mag_filter(vk::Filter::NEAREST)
            .min_filter(vk::Filter::NEAREST)
            .mipmap_mode(vk::SamplerMipmapMode::NEAREST)
            .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .min_lod(0.0)
            .max_lod(0.0);
        self.sampler = unsafe { device.create_sampler(&sampler_info) }
            .map_err(ComputeRenderPathError::CreateSampler)?;
        self.create_camera_buffer(device, extent)?;
        Ok(())
    }

    fn create_scene_buffer(
        &mut self,
        device: &RenderPathDeviceContext<'_>,
    ) -> Result<(), ComputeRenderPathError> {
        let resources = create_scene_gpu_resources(device, self.convergence.installed_bundle())?;
        self.scene_buffer = resources.buffer;
        self.scene_memory = resources.memory;
        self.scene_allocation_bytes = resources.allocation_bytes;
        self.scene_gpu_revision = self.convergence.installed_bundle().revision();
        Ok(())
    }

    fn create_camera_buffer(
        &mut self,
        device: &RenderPathDeviceContext<'_>,
        extent: vk::Extent2D,
    ) -> Result<(), ComputeRenderPathError> {
        let camera_words = camera_storage_words(self.camera_state, extent);
        let buffer_info = vk::BufferCreateInfo::default()
            .size(u64::from(CAMERA_BUFFER_SIZE))
            .usage(vk::BufferUsageFlags::STORAGE_BUFFER)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        self.camera_buffer = unsafe { device.create_buffer(&buffer_info) }
            .map_err(ComputeRenderPathError::CreateCameraBuffer)?;
        let requirements = unsafe { device.buffer_memory_requirements(self.camera_buffer) };
        let memory_type_index = device
            .memory_type_index(
                requirements.memory_type_bits,
                vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
            )
            .ok_or(ComputeRenderPathError::MissingCameraMemory)?;
        let allocation_info = vk::MemoryAllocateInfo::default()
            .allocation_size(requirements.size)
            .memory_type_index(memory_type_index);
        self.camera_memory = unsafe { device.allocate_memory(&allocation_info) }
            .map_err(ComputeRenderPathError::AllocateCameraMemory)?;
        self.camera_allocation_bytes = requirements.size;
        unsafe { device.bind_buffer_memory(self.camera_buffer, self.camera_memory) }
            .map_err(ComputeRenderPathError::BindCameraMemory)?;
        unsafe { device.write_memory(self.camera_memory, f32_bytes(&camera_words)) }
            .map_err(ComputeRenderPathError::WriteCameraMemory)?;
        Ok(())
    }

    fn write_camera_state(
        &self,
        device: &RenderPathDeviceContext<'_>,
        extent: vk::Extent2D,
    ) -> Result<(), ComputeRenderPathError> {
        if self.camera_buffer == vk::Buffer::null()
            || self.camera_memory == vk::DeviceMemory::null()
        {
            return Err(ComputeRenderPathError::CameraResourcesUnavailable);
        }
        let camera_words = camera_storage_words(self.camera_state, extent);
        unsafe { device.write_memory(self.camera_memory, f32_bytes(&camera_words)) }
            .map_err(ComputeRenderPathError::WriteCameraMemory)
    }

    fn create_semantic_ray_buffer(
        &mut self,
        device: &RenderPathDeviceContext<'_>,
    ) -> Result<(), ComputeRenderPathError> {
        let buffer_info = vk::BufferCreateInfo::default()
            .size(u64::from(SEMANTIC_RAY_BUFFER_SIZE))
            .usage(vk::BufferUsageFlags::STORAGE_BUFFER)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        self.semantic_ray_buffer = unsafe { device.create_buffer(&buffer_info) }
            .map_err(ComputeRenderPathError::CreateSemanticRayBuffer)?;
        let requirements = unsafe { device.buffer_memory_requirements(self.semantic_ray_buffer) };
        let Some(memory_type_index) = device.memory_type_index(
            requirements.memory_type_bits,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
        ) else {
            unsafe { device.destroy_buffer(self.semantic_ray_buffer) };
            self.semantic_ray_buffer = vk::Buffer::null();
            return Err(ComputeRenderPathError::MissingSemanticRayMemory);
        };
        let allocation_info = vk::MemoryAllocateInfo::default()
            .allocation_size(requirements.size)
            .memory_type_index(memory_type_index);
        self.semantic_ray_memory = match unsafe { device.allocate_memory(&allocation_info) } {
            Ok(memory) => memory,
            Err(error) => {
                unsafe { device.destroy_buffer(self.semantic_ray_buffer) };
                self.semantic_ray_buffer = vk::Buffer::null();
                return Err(ComputeRenderPathError::AllocateSemanticRayMemory(error));
            }
        };
        self.semantic_ray_allocation_bytes = requirements.size;
        if let Err(error) =
            unsafe { device.bind_buffer_memory(self.semantic_ray_buffer, self.semantic_ray_memory) }
        {
            unsafe {
                device.destroy_buffer(self.semantic_ray_buffer);
                device.free_memory(self.semantic_ray_memory);
            }
            self.semantic_ray_buffer = vk::Buffer::null();
            self.semantic_ray_memory = vk::DeviceMemory::null();
            self.semantic_ray_allocation_bytes = 0;
            return Err(ComputeRenderPathError::BindSemanticRayMemory(error));
        }
        let words = [0_u32; SEMANTIC_RAY_BUFFER_WORD_COUNT];
        unsafe { device.write_memory(self.semantic_ray_memory, u32_bytes(&words)) }
            .map_err(ComputeRenderPathError::WriteSemanticRayMemory)
    }

    fn stage_semantic_ray_request(
        &mut self,
        device: &RenderPathDeviceContext<'_>,
    ) -> Result<(), ComputeRenderPathError> {
        if self.armed_semantic_ray_probes.is_some() || self.recorded_semantic_ray_frame.is_some() {
            return Ok(());
        }
        let Some(controller) = &self.semantic_ray_controller else {
            return Ok(());
        };
        let Some(probes) = controller.take_pending()? else {
            return Ok(());
        };
        if self.semantic_ray_memory == vk::DeviceMemory::null() {
            return Err(ComputeRenderPathError::SemanticRayResourcesUnavailable);
        }
        let words = semantic_ray_input_words(&probes);
        unsafe { device.write_memory(self.semantic_ray_memory, u32_bytes(&words)) }
            .map_err(ComputeRenderPathError::WriteSemanticRayMemory)?;
        self.armed_semantic_ray_probes = Some(probes);
        Ok(())
    }

    fn collect_semantic_ray_observations(
        &mut self,
        device: &RenderPathDeviceContext<'_>,
    ) -> Result<(), ComputeRenderPathError> {
        let Some((frame_sequence, revision)) = self.recorded_semantic_ray_frame.take() else {
            return Ok(());
        };
        let probes = self
            .armed_semantic_ray_probes
            .take()
            .ok_or(ComputeRenderPathError::SemanticRayResourcesUnavailable)?;
        if self.semantic_ray_memory == vk::DeviceMemory::null() {
            return Err(ComputeRenderPathError::SemanticRayResourcesUnavailable);
        }
        let mut words = [0_u32; SEMANTIC_RAY_BUFFER_WORD_COUNT];
        unsafe { device.read_memory(self.semantic_ray_memory, u32_bytes_mut(&mut words)) }
            .map_err(ComputeRenderPathError::ReadSemanticRayMemory)?;
        let observations = decode_semantic_ray_output(
            &probes,
            &words,
            self.convergence.installed_bundle(),
            revision,
            frame_sequence,
        )?;
        if let Some(controller) = &self.semantic_ray_controller {
            controller.retain(observations)?;
        }
        Ok(())
    }

    fn create_descriptor_resources(
        &mut self,
        device: &RenderPathDeviceContext<'_>,
    ) -> Result<(), ComputeRenderPathError> {
        let bindings = [
            vk::DescriptorSetLayoutBinding::default()
                .binding(0)
                .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(1)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT),
            vk::DescriptorSetLayoutBinding::default()
                .binding(2)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(3)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(4)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
        ];
        let layout_info = vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings);
        self.descriptor_set_layout = unsafe { device.create_descriptor_set_layout(&layout_info) }
            .map_err(ComputeRenderPathError::CreateDescriptorSetLayout)?;

        let pool_sizes = [
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::STORAGE_IMAGE)
                .descriptor_count(1),
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(1),
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(3),
        ];
        let pool_info = vk::DescriptorPoolCreateInfo::default()
            .max_sets(1)
            .pool_sizes(&pool_sizes);
        self.descriptor_pool = unsafe { device.create_descriptor_pool(&pool_info) }
            .map_err(ComputeRenderPathError::CreateDescriptorPool)?;
        let layouts = [self.descriptor_set_layout];
        let allocation_info = vk::DescriptorSetAllocateInfo::default()
            .descriptor_pool(self.descriptor_pool)
            .set_layouts(&layouts);
        self.descriptor_set = unsafe { device.allocate_descriptor_sets(&allocation_info) }
            .map_err(ComputeRenderPathError::AllocateDescriptorSet)?
            .pop()
            .ok_or(ComputeRenderPathError::MissingDescriptorSet)?;

        let storage_image = [vk::DescriptorImageInfo::default()
            .image_view(self.output_view)
            .image_layout(vk::ImageLayout::GENERAL)];
        let sampled_image = [vk::DescriptorImageInfo::default()
            .sampler(self.sampler)
            .image_view(self.output_view)
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
        let scene_buffer = [vk::DescriptorBufferInfo::default()
            .buffer(self.scene_buffer)
            .offset(0)
            .range(scene_storage_byte_size(
                device,
                self.convergence.installed_bundle(),
            )?)];
        let camera_buffer = [vk::DescriptorBufferInfo::default()
            .buffer(self.camera_buffer)
            .offset(0)
            .range(u64::from(CAMERA_BUFFER_SIZE))];
        let semantic_ray_buffer = [vk::DescriptorBufferInfo::default()
            .buffer(self.semantic_ray_buffer)
            .offset(0)
            .range(u64::from(SEMANTIC_RAY_BUFFER_SIZE))];
        let writes = [
            vk::WriteDescriptorSet::default()
                .dst_set(self.descriptor_set)
                .dst_binding(0)
                .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
                .image_info(&storage_image),
            vk::WriteDescriptorSet::default()
                .dst_set(self.descriptor_set)
                .dst_binding(1)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .image_info(&sampled_image),
            vk::WriteDescriptorSet::default()
                .dst_set(self.descriptor_set)
                .dst_binding(2)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .buffer_info(&scene_buffer),
            vk::WriteDescriptorSet::default()
                .dst_set(self.descriptor_set)
                .dst_binding(3)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .buffer_info(&camera_buffer),
            vk::WriteDescriptorSet::default()
                .dst_set(self.descriptor_set)
                .dst_binding(4)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .buffer_info(&semantic_ray_buffer),
        ];
        unsafe { device.update_descriptor_sets(&writes) };
        Ok(())
    }

    fn create_compute_pipeline(
        &mut self,
        device: &RenderPathDeviceContext<'_>,
    ) -> Result<(), ComputeRenderPathError> {
        let shader_code = read_shader(include_bytes!(concat!(
            env!("OUT_DIR"),
            "/dense_dda.comp.spv"
        )))?;
        let shader_module = create_shader_module(device, &shader_code)?;
        let result = self.create_compute_pipeline_with_module(device, shader_module);
        unsafe { device.destroy_shader_module(shader_module) };
        result
    }

    fn create_compute_pipeline_with_module(
        &mut self,
        device: &RenderPathDeviceContext<'_>,
        shader_module: vk::ShaderModule,
    ) -> Result<(), ComputeRenderPathError> {
        let descriptor_set_layouts = [self.descriptor_set_layout];
        let layout_info =
            vk::PipelineLayoutCreateInfo::default().set_layouts(&descriptor_set_layouts);
        self.compute_pipeline_layout = unsafe { device.create_pipeline_layout(&layout_info) }
            .map_err(ComputeRenderPathError::CreatePipelineLayout)?;
        let stage = vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::COMPUTE)
            .module(shader_module)
            .name(c"main");
        let pipeline_info = vk::ComputePipelineCreateInfo::default()
            .stage(stage)
            .layout(self.compute_pipeline_layout);
        match unsafe {
            device.create_compute_pipelines(vk::PipelineCache::null(), &[pipeline_info])
        } {
            Ok(mut pipelines) => {
                self.compute_pipeline =
                    pipelines
                        .pop()
                        .ok_or(ComputeRenderPathError::CreateComputePipeline(
                            vk::Result::ERROR_UNKNOWN,
                        ))?;
                Ok(())
            }
            Err((pipelines, error)) => {
                for pipeline in pipelines {
                    unsafe { device.destroy_pipeline(pipeline) };
                }
                Err(ComputeRenderPathError::CreateComputePipeline(error))
            }
        }
    }

    fn create_render_pass(
        &mut self,
        device: &RenderPathDeviceContext<'_>,
        color_format: vk::Format,
    ) -> Result<(), ComputeRenderPathError> {
        let color_attachment = vk::AttachmentDescription::default()
            .format(color_format)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::DONT_CARE)
            .store_op(vk::AttachmentStoreOp::STORE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .final_layout(vk::ImageLayout::PRESENT_SRC_KHR);
        let color_reference = vk::AttachmentReference::default()
            .attachment(0)
            .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);
        let color_references = [color_reference];
        let subpass = vk::SubpassDescription::default()
            .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
            .color_attachments(&color_references);
        let dependency = vk::SubpassDependency::default()
            .src_subpass(vk::SUBPASS_EXTERNAL)
            .dst_subpass(0)
            .src_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
            .dst_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
            .dst_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE);
        let attachments = [color_attachment];
        let subpasses = [subpass];
        let dependencies = [dependency];
        let render_pass_info = vk::RenderPassCreateInfo::default()
            .attachments(&attachments)
            .subpasses(&subpasses)
            .dependencies(&dependencies);
        self.render_pass = unsafe { device.create_render_pass(&render_pass_info) }
            .map_err(ComputeRenderPathError::CreateRenderPass)?;
        Ok(())
    }

    fn create_composite_pipeline(
        &mut self,
        device: &RenderPathDeviceContext<'_>,
        extent: vk::Extent2D,
    ) -> Result<(), ComputeRenderPathError> {
        let vertex_code = read_shader(include_bytes!(concat!(
            env!("OUT_DIR"),
            "/composite.vert.spv"
        )))?;
        let fragment_code = read_shader(include_bytes!(concat!(
            env!("OUT_DIR"),
            "/composite.frag.spv"
        )))?;
        let vertex_module = create_shader_module(device, &vertex_code)?;
        let fragment_module = match create_shader_module(device, &fragment_code) {
            Ok(module) => module,
            Err(error) => {
                unsafe { device.destroy_shader_module(vertex_module) };
                return Err(error);
            }
        };
        let result = self.create_composite_pipeline_with_modules(
            device,
            extent,
            vertex_module,
            fragment_module,
        );
        unsafe {
            device.destroy_shader_module(fragment_module);
            device.destroy_shader_module(vertex_module);
        }
        result
    }

    fn create_composite_pipeline_with_modules(
        &mut self,
        device: &RenderPathDeviceContext<'_>,
        extent: vk::Extent2D,
        vertex_module: vk::ShaderModule,
        fragment_module: vk::ShaderModule,
    ) -> Result<(), ComputeRenderPathError> {
        let stages = [
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::VERTEX)
                .module(vertex_module)
                .name(c"main"),
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::FRAGMENT)
                .module(fragment_module)
                .name(c"main"),
        ];
        let vertex_input = vk::PipelineVertexInputStateCreateInfo::default();
        let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
        let viewports = [vk::Viewport {
            x: 0.0,
            y: 0.0,
            width: extent.width as f32,
            height: extent.height as f32,
            min_depth: 0.0,
            max_depth: 1.0,
        }];
        let scissors = [vk::Rect2D {
            offset: vk::Offset2D::default(),
            extent,
        }];
        let viewport_state = vk::PipelineViewportStateCreateInfo::default()
            .viewports(&viewports)
            .scissors(&scissors);
        let rasterization = vk::PipelineRasterizationStateCreateInfo::default()
            .polygon_mode(vk::PolygonMode::FILL)
            .cull_mode(vk::CullModeFlags::NONE)
            .line_width(1.0);
        let multisample = vk::PipelineMultisampleStateCreateInfo::default()
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);
        let color_attachments = [vk::PipelineColorBlendAttachmentState::default()
            .color_write_mask(vk::ColorComponentFlags::RGBA)];
        let color_blend =
            vk::PipelineColorBlendStateCreateInfo::default().attachments(&color_attachments);
        let descriptor_set_layouts = [self.descriptor_set_layout];
        let layout_info =
            vk::PipelineLayoutCreateInfo::default().set_layouts(&descriptor_set_layouts);
        self.composite_pipeline_layout = unsafe { device.create_pipeline_layout(&layout_info) }
            .map_err(ComputeRenderPathError::CreatePipelineLayout)?;
        let pipeline_info = vk::GraphicsPipelineCreateInfo::default()
            .stages(&stages)
            .vertex_input_state(&vertex_input)
            .input_assembly_state(&input_assembly)
            .viewport_state(&viewport_state)
            .rasterization_state(&rasterization)
            .multisample_state(&multisample)
            .color_blend_state(&color_blend)
            .layout(self.composite_pipeline_layout)
            .render_pass(self.render_pass)
            .subpass(0);
        match unsafe {
            device.create_graphics_pipelines(vk::PipelineCache::null(), &[pipeline_info])
        } {
            Ok(mut pipelines) => {
                self.composite_pipeline =
                    pipelines
                        .pop()
                        .ok_or(ComputeRenderPathError::CreateGraphicsPipeline(
                            vk::Result::ERROR_UNKNOWN,
                        ))?;
                Ok(())
            }
            Err((pipelines, error)) => {
                for pipeline in pipelines {
                    unsafe { device.destroy_pipeline(pipeline) };
                }
                Err(ComputeRenderPathError::CreateGraphicsPipeline(error))
            }
        }
    }

    fn record_frame(
        &mut self,
        frame: &RenderPathFrameContext<'_>,
    ) -> Result<(), ComputeRenderPathError> {
        let target = frame.target();
        if self.configuration_id != Some(target.configuration_id())
            || self.output_extent != target.extent()
        {
            return Err(ComputeRenderPathError::StaleFrameTarget);
        }
        let framebuffer_index = self
            .configured_attachments
            .iter()
            .position(|identity| *identity == target.attachment().identity())
            .ok_or(ComputeRenderPathError::MissingFramebuffer)?;
        let framebuffer = self
            .framebuffers
            .get(framebuffer_index)
            .copied()
            .ok_or(ComputeRenderPathError::MissingFramebuffer)?;

        let output_barrier = output_barrier_plan(self.output_initialized);
        let sampling_barrier = sampling_barrier_plan();
        let prepare_for_compute = [vk::ImageMemoryBarrier::default()
            .src_access_mask(output_barrier.source_access)
            .dst_access_mask(vk::AccessFlags::SHADER_WRITE)
            .old_layout(output_barrier.old_layout)
            .new_layout(vk::ImageLayout::GENERAL)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(self.output_image)
            .subresource_range(output_subresource_range())];
        let prepare_for_sampling = [vk::ImageMemoryBarrier::default()
            .src_access_mask(sampling_barrier.source_access)
            .dst_access_mask(sampling_barrier.destination_access)
            .old_layout(sampling_barrier.old_layout)
            .new_layout(sampling_barrier.new_layout)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(self.output_image)
            .subresource_range(output_subresource_range())];
        let render_pass_info = vk::RenderPassBeginInfo::default()
            .render_pass(self.render_pass)
            .framebuffer(framebuffer)
            .render_area(vk::Rect2D {
                offset: vk::Offset2D::default(),
                extent: target.extent(),
            });
        let descriptor_sets = [self.descriptor_set];
        let semantic_ray_host_write = [vk::BufferMemoryBarrier::default()
            .src_access_mask(vk::AccessFlags::HOST_WRITE)
            .dst_access_mask(vk::AccessFlags::SHADER_READ | vk::AccessFlags::SHADER_WRITE)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .buffer(self.semantic_ray_buffer)
            .offset(0)
            .size(u64::from(SEMANTIC_RAY_BUFFER_SIZE))];
        let semantic_ray_host_read = [vk::BufferMemoryBarrier::default()
            .src_access_mask(vk::AccessFlags::SHADER_WRITE)
            .dst_access_mask(vk::AccessFlags::HOST_READ)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .buffer(self.semantic_ray_buffer)
            .offset(0)
            .size(u64::from(SEMANTIC_RAY_BUFFER_SIZE))];
        let semantic_ray_request_is_armed = self.armed_semantic_ray_probes.is_some();

        let dispatch_started_at = Instant::now();
        unsafe {
            if semantic_ray_request_is_armed {
                frame.buffer_pipeline_barrier(
                    vk::PipelineStageFlags::HOST,
                    vk::PipelineStageFlags::COMPUTE_SHADER,
                    &semantic_ray_host_write,
                );
            }
            frame.image_pipeline_barrier(
                output_barrier.source_stage,
                vk::PipelineStageFlags::COMPUTE_SHADER,
                &prepare_for_compute,
            );
            frame.bind_pipeline(vk::PipelineBindPoint::COMPUTE, self.compute_pipeline);
            frame.bind_descriptor_sets(
                vk::PipelineBindPoint::COMPUTE,
                self.compute_pipeline_layout,
                &descriptor_sets,
            );
            frame.dispatch(self.dispatch_group_count);
            if semantic_ray_request_is_armed {
                frame.buffer_pipeline_barrier(
                    vk::PipelineStageFlags::COMPUTE_SHADER,
                    vk::PipelineStageFlags::HOST,
                    &semantic_ray_host_read,
                );
            }
            frame.image_pipeline_barrier(
                sampling_barrier.source_stage,
                sampling_barrier.destination_stage,
                &prepare_for_sampling,
            );
        }
        self.record_timing(
            ComputeTimingPhase::Dispatch,
            self.convergence.status().installed(),
            dispatch_started_at,
        )?;
        let composite_started_at = Instant::now();
        unsafe {
            frame.begin_render_pass(&render_pass_info, vk::SubpassContents::INLINE);
            frame.bind_pipeline(vk::PipelineBindPoint::GRAPHICS, self.composite_pipeline);
            frame.bind_descriptor_sets(
                vk::PipelineBindPoint::GRAPHICS,
                self.composite_pipeline_layout,
                &descriptor_sets,
            );
            frame.draw(3, 1, 0, 0);
            frame.end_render_pass();
        }
        self.record_timing(
            ComputeTimingPhase::Composite,
            self.convergence.status().installed(),
            composite_started_at,
        )?;
        if semantic_ray_request_is_armed && self.recorded_semantic_ray_frame.is_none() {
            self.recorded_semantic_ray_frame = Some((
                target.frame_sequence(),
                self.convergence.installed_bundle().revision(),
            ));
        }
        self.output_initialized = true;
        Ok(())
    }

    fn advance_convergence_at_frame_boundary(
        &mut self,
        device: &RenderPathDeviceContext<'_>,
        target: RenderPathTarget<'_>,
    ) -> Result<(), ComputeRenderPathError> {
        if self.configuration_id != Some(target.configuration_id())
            || self.output_extent != target.extent()
        {
            return Err(ComputeRenderPathError::StaleFrameTarget);
        }
        self.convergence.retain_ready_candidate();
        let hidden_stamp = self.convergence.status().hidden();
        let retained_gpu_stamp = self
            .hidden_scene_gpu_resources
            .as_ref()
            .map(|candidate| candidate.stamp);
        if retained_gpu_stamp.is_some() && retained_gpu_stamp != hidden_stamp {
            self.release_hidden_scene_gpu_resources_with(|resources| {
                release_scene_gpu_resources(device, resources);
            })?;
        }
        let Some(candidate_revision) = self
            .convergence
            .hidden_bundle()
            .map(ComputeSceneBundle::revision)
        else {
            return Ok(());
        };
        let candidate_stamp = hidden_stamp.ok_or(ComputeRenderPathError::MissingHiddenCandidate)?;
        if self.hidden_scene_gpu_resources.is_none() {
            if self
                .convergence
                .fail_hidden_if_injected(ComputeConvergenceFailurePhase::Upload)?
            {
                return Err(ComputeRenderPathError::InjectedConvergenceFailure(
                    ComputeConvergenceFailurePhase::Upload,
                ));
            }
            let candidate_bundle = self
                .convergence
                .hidden_bundle()
                .ok_or(ComputeRenderPathError::MissingHiddenCandidate)?;
            let candidate_range = match scene_storage_byte_size(device, candidate_bundle) {
                Ok(range) => range,
                Err(error) => {
                    self.convergence
                        .fail_hidden(ComputeConvergenceFailurePhase::Upload, error.to_string());
                    return Err(error);
                }
            };
            let upload_started_at = Instant::now();
            let candidate_resources = match create_scene_gpu_resources(device, candidate_bundle) {
                Ok(resources) => resources,
                Err(error) => {
                    self.convergence
                        .fail_hidden(ComputeConvergenceFailurePhase::Upload, error.to_string());
                    return Err(error);
                }
            };
            self.hidden_scene_gpu_resources = Some(ComputeHiddenSceneGpuResources {
                stamp: candidate_stamp,
                resources: candidate_resources,
                range: candidate_range,
            });
            self.record_timing(
                ComputeTimingPhase::Upload,
                candidate_stamp,
                upload_started_at,
            )?;
            self.convergence.mark_hidden_uploaded();
        }
        if let Some(control) = &self.convergence_control
            && control.hold_post_upload(candidate_revision)?
        {
            return Ok(());
        }
        let installation_started_at = Instant::now();
        if self
            .convergence
            .fail_hidden_if_injected(ComputeConvergenceFailurePhase::Installation)?
        {
            self.release_hidden_scene_gpu_resources_with(|resources| {
                release_scene_gpu_resources(device, resources);
            })?;
            return Err(ComputeRenderPathError::InjectedConvergenceFailure(
                ComputeConvergenceFailurePhase::Installation,
            ));
        }
        let retired_bundle = match self.convergence.install_hidden(self.scene_gpu_revision) {
            Ok(Some(bundle)) => bundle,
            Ok(None) => {
                self.release_hidden_scene_gpu_resources_with(|resources| {
                    release_scene_gpu_resources(device, resources);
                })?;
                return Ok(());
            }
            Err(error) => {
                self.release_hidden_scene_gpu_resources_with(|resources| {
                    release_scene_gpu_resources(device, resources);
                })?;
                self.convergence.fail_hidden(
                    ComputeConvergenceFailurePhase::Installation,
                    error.to_string(),
                );
                return Err(error.into());
            }
        };
        let candidate = self
            .hidden_scene_gpu_resources
            .take()
            .ok_or(ComputeRenderPathError::MissingHiddenCandidate)?;
        let scene_buffer = [vk::DescriptorBufferInfo::default()
            .buffer(candidate.resources.buffer)
            .offset(0)
            .range(candidate.range)];
        let writes = [vk::WriteDescriptorSet::default()
            .dst_set(self.descriptor_set)
            .dst_binding(2)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .buffer_info(&scene_buffer)];
        unsafe { device.update_descriptor_sets(&writes) };
        let retired_resources = ComputeSceneGpuResources {
            buffer: std::mem::replace(&mut self.scene_buffer, candidate.resources.buffer),
            memory: std::mem::replace(&mut self.scene_memory, candidate.resources.memory),
            allocation_bytes: std::mem::replace(
                &mut self.scene_allocation_bytes,
                candidate.resources.allocation_bytes,
            ),
        };
        self.scene_gpu_revision = candidate_revision;
        if let Some(control) = &self.convergence_control {
            control.clear_post_upload_revision(candidate_revision)?;
        }
        release_scene_gpu_resources(device, retired_resources);
        drop(retired_bundle);
        self.record_timing(
            ComputeTimingPhase::Installation,
            candidate_stamp,
            installation_started_at,
        )?;
        Ok(())
    }

    fn release_hidden_scene_gpu_resources_with(
        &mut self,
        mut release: impl FnMut(ComputeSceneGpuResources),
    ) -> Result<(), ComputeConvergenceControlError> {
        let Some(candidate) = self.hidden_scene_gpu_resources.take() else {
            return Ok(());
        };
        let control_result = self
            .convergence_control
            .as_ref()
            .map(|control| control.clear_post_upload_revision(candidate.stamp.revision()))
            .transpose();
        release(candidate.resources);
        control_result?;
        Ok(())
    }

    fn release_presentation_resources(&mut self, device: &RenderPathDeviceContext<'_>) {
        unsafe {
            for framebuffer in self.framebuffers.drain(..) {
                device.destroy_framebuffer(framebuffer);
            }
            if self.composite_pipeline != vk::Pipeline::null() {
                device.destroy_pipeline(self.composite_pipeline);
                self.composite_pipeline = vk::Pipeline::null();
            }
            if self.composite_pipeline_layout != vk::PipelineLayout::null() {
                device.destroy_pipeline_layout(self.composite_pipeline_layout);
                self.composite_pipeline_layout = vk::PipelineLayout::null();
            }
            if self.render_pass != vk::RenderPass::null() {
                device.destroy_render_pass(self.render_pass);
                self.render_pass = vk::RenderPass::null();
            }
            if self.compute_pipeline != vk::Pipeline::null() {
                device.destroy_pipeline(self.compute_pipeline);
                self.compute_pipeline = vk::Pipeline::null();
            }
            if self.compute_pipeline_layout != vk::PipelineLayout::null() {
                device.destroy_pipeline_layout(self.compute_pipeline_layout);
                self.compute_pipeline_layout = vk::PipelineLayout::null();
            }
            if self.descriptor_pool != vk::DescriptorPool::null() {
                device.destroy_descriptor_pool(self.descriptor_pool);
                self.descriptor_pool = vk::DescriptorPool::null();
                self.descriptor_set = vk::DescriptorSet::null();
            }
            if self.sampler != vk::Sampler::null() {
                device.destroy_sampler(self.sampler);
                self.sampler = vk::Sampler::null();
            }
            if self.descriptor_set_layout != vk::DescriptorSetLayout::null() {
                device.destroy_descriptor_set_layout(self.descriptor_set_layout);
                self.descriptor_set_layout = vk::DescriptorSetLayout::null();
            }
            if self.output_view != vk::ImageView::null() {
                device.destroy_image_view(self.output_view);
                self.output_view = vk::ImageView::null();
            }
            if self.output_image != vk::Image::null() {
                device.destroy_image(self.output_image);
                self.output_image = vk::Image::null();
            }
            if self.output_memory != vk::DeviceMemory::null() {
                device.free_memory(self.output_memory);
                self.output_memory = vk::DeviceMemory::null();
            }
            self.output_allocation_bytes = 0;
            if self.camera_buffer != vk::Buffer::null() {
                device.destroy_buffer(self.camera_buffer);
                self.camera_buffer = vk::Buffer::null();
            }
            if self.camera_memory != vk::DeviceMemory::null() {
                device.free_memory(self.camera_memory);
                self.camera_memory = vk::DeviceMemory::null();
            }
            self.camera_allocation_bytes = 0;
        }
        self.configured_attachments.clear();
        self.configuration_id = None;
        self.output_extent = vk::Extent2D::default();
        self.dispatch_group_count = [0; 3];
        self.output_initialized = false;
    }

    fn release_scene_resources(&mut self, device: &RenderPathDeviceContext<'_>) {
        if let Some(candidate) = self.hidden_scene_gpu_resources.take() {
            release_scene_gpu_resources(device, candidate.resources);
        }
        unsafe {
            if self.scene_buffer != vk::Buffer::null() {
                device.destroy_buffer(self.scene_buffer);
                self.scene_buffer = vk::Buffer::null();
            }
            if self.scene_memory != vk::DeviceMemory::null() {
                device.free_memory(self.scene_memory);
                self.scene_memory = vk::DeviceMemory::null();
            }
            self.scene_allocation_bytes = 0;
            if self.semantic_ray_buffer != vk::Buffer::null() {
                device.destroy_buffer(self.semantic_ray_buffer);
                self.semantic_ray_buffer = vk::Buffer::null();
            }
            if self.semantic_ray_memory != vk::DeviceMemory::null() {
                device.free_memory(self.semantic_ray_memory);
                self.semantic_ray_memory = vk::DeviceMemory::null();
            }
            self.semantic_ray_allocation_bytes = 0;
        }
        self.armed_semantic_ray_probes = None;
        self.recorded_semantic_ray_frame = None;
    }

    fn owned_resource_counts(&self) -> Result<ComputeOwnedResourceCounts, ComputeRenderPathError> {
        let hidden_resources = self
            .hidden_scene_gpu_resources
            .as_ref()
            .map(|candidate| &candidate.resources);
        let objects = [
            self.output_image != vk::Image::null(),
            self.output_view != vk::ImageView::null(),
            self.sampler != vk::Sampler::null(),
            self.scene_buffer != vk::Buffer::null(),
            hidden_resources.is_some_and(|resources| resources.buffer != vk::Buffer::null()),
            self.camera_buffer != vk::Buffer::null(),
            self.semantic_ray_buffer != vk::Buffer::null(),
            self.descriptor_set_layout != vk::DescriptorSetLayout::null(),
            self.descriptor_pool != vk::DescriptorPool::null(),
            self.descriptor_set != vk::DescriptorSet::null(),
            self.compute_pipeline_layout != vk::PipelineLayout::null(),
            self.compute_pipeline != vk::Pipeline::null(),
            self.render_pass != vk::RenderPass::null(),
            self.composite_pipeline_layout != vk::PipelineLayout::null(),
            self.composite_pipeline != vk::Pipeline::null(),
        ]
        .into_iter()
        .filter(|owned| *owned)
        .count()
            + self.framebuffers.len();
        let allocations = [
            self.output_memory != vk::DeviceMemory::null(),
            self.scene_memory != vk::DeviceMemory::null(),
            hidden_resources.is_some_and(|resources| resources.memory != vk::DeviceMemory::null()),
            self.camera_memory != vk::DeviceMemory::null(),
            self.semantic_ray_memory != vk::DeviceMemory::null(),
        ]
        .into_iter()
        .filter(|owned| *owned)
        .count();
        let hidden_allocation_bytes = hidden_resources
            .map(|resources| resources.allocation_bytes)
            .unwrap_or(0);
        let bytes = self
            .output_allocation_bytes
            .checked_add(self.scene_allocation_bytes)
            .and_then(|bytes| bytes.checked_add(hidden_allocation_bytes))
            .and_then(|bytes| bytes.checked_add(self.camera_allocation_bytes))
            .and_then(|bytes| bytes.checked_add(self.semantic_ray_allocation_bytes))
            .ok_or(ComputeRenderPathError::ResourceAccountingOverflow)?;
        Ok(ComputeOwnedResourceCounts {
            bytes,
            objects,
            allocations,
            workers: self.convergence.status().worker_count(),
            views: self.convergence.owned_view_count(),
        })
    }

    fn record_resource_observation(
        &self,
        point: ComputeResourceObservationPoint,
    ) -> Result<(), ComputeRenderPathError> {
        let Some(controller) = &self.lifecycle_controller else {
            return Ok(());
        };
        controller.record_resources(
            point,
            self.convergence.installed_bundle().scene_identity().clone(),
            self.convergence.status(),
            self.owned_resource_counts()?,
        )?;
        Ok(())
    }
}

impl RenderPath for ComputeRayRenderPath {
    fn release(&mut self, device: RenderPathDeviceContext<'_>) -> RenderPathResult<()> {
        self.release_presentation_resources(&device);
        self.record_resource_observation(ComputeResourceObservationPoint::Released)?;
        Ok(())
    }

    fn configure(
        &mut self,
        device: RenderPathDeviceContext<'_>,
        target: RenderPathTarget<'_>,
    ) -> RenderPathResult<()> {
        let queried = device.capabilities();
        let qualification = match qualify_compute_render_path(queried, target.extent()) {
            Ok(qualification) => qualification,
            Err(reason) => {
                self.capability_assessment = Some(ComputeCapabilityAssessment::Rejected {
                    queried,
                    output_extent: target.extent(),
                    reason: reason.clone(),
                });
                return Err(Box::new(reason));
            }
        };
        self.capability_assessment = Some(ComputeCapabilityAssessment::Qualified(qualification));
        if let Err(error) = self.configure_resources(&device, target, qualification) {
            self.release_presentation_resources(&device);
            return Err(Box::new(error));
        }
        self.record_resource_observation(ComputeResourceObservationPoint::Configured)?;
        Ok(())
    }

    fn shutdown(&mut self, device: RenderPathDeviceContext<'_>) -> RenderPathResult<()> {
        let mut failures = Vec::new();
        if let Err(error) = self.collect_semantic_ray_observations(&device) {
            failures.push(format!("Semantic Ray observation collection: {error}"));
        }
        if let Err(error) = self.convergence.shutdown() {
            failures.push(format!("convergence: {error}"));
        }
        if let Err(error) = self.release_hidden_scene_gpu_resources_with(|resources| {
            release_scene_gpu_resources(&device, resources);
        }) {
            failures.push(format!("hidden candidate control: {error}"));
        }
        self.release_presentation_resources(&device);
        self.release_scene_resources(&device);
        if let Some(controller) = self.convergence_control.clone()
            && let Err(error) =
                controller.synchronize(self.convergence.status(), self.convergence.drain_events())
        {
            failures.push(format!("convergence control: {error}"));
        }
        match self.owned_resource_counts() {
            Ok(owned_resources) => {
                if let Some(controller) = &self.lifecycle_controller
                    && let Err(error) = controller.record_shutdown(
                        self.convergence.installed_bundle().scene_identity().clone(),
                        self.convergence.status(),
                        owned_resources,
                    )
                {
                    failures.push(format!("lifecycle control: {error}"));
                }
            }
            Err(error) => failures.push(format!("resource accounting: {error}")),
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(Box::new(ComputeRenderPathError::Shutdown(
                failures.join("; "),
            )))
        }
    }

    fn advance_frame_boundary(
        &mut self,
        device: RenderPathDeviceContext<'_>,
        target: RenderPathTarget<'_>,
    ) -> RenderPathResult<()> {
        self.collect_semantic_ray_observations(&device)?;
        self.convergence
            .apply_controlled_request_at_frame_boundary()?;
        let convergence_result = self.advance_convergence_at_frame_boundary(&device, target);
        let control_result = self.convergence_control.as_ref().map(|control| {
            control.synchronize(self.convergence.status(), self.convergence.drain_events())
        });
        let resource_result =
            self.record_resource_observation(ComputeResourceObservationPoint::Convergence);
        convergence_result?;
        resource_result?;
        if let Some(control_result) = control_result {
            control_result?;
        }
        self.stage_semantic_ray_request(&device)?;
        Ok(())
    }

    fn record(&mut self, frame: RenderPathFrameContext<'_>) -> RenderPathResult<()> {
        self.record_frame(&frame)
            .map_err(|error| Box::new(error) as _)
    }
}

struct ComputeSceneGpuResources {
    buffer: vk::Buffer,
    memory: vk::DeviceMemory,
    allocation_bytes: u64,
}

struct ComputeHiddenSceneGpuResources {
    stamp: ComputeConvergenceWorkStamp,
    resources: ComputeSceneGpuResources,
    range: u64,
}

fn scene_storage_byte_size(
    device: &RenderPathDeviceContext<'_>,
    bundle: &ComputeSceneBundle,
) -> Result<u64, ComputeRenderPathError> {
    let byte_size = u64::try_from(u32_bytes(bundle.storage_words()).len()).map_err(|_| {
        ComputeRenderPathError::SceneStorageBufferRange {
            required: u64::MAX,
            available: device.capabilities().max_storage_buffer_range,
        }
    })?;
    if byte_size > u64::from(device.capabilities().max_storage_buffer_range) {
        return Err(ComputeRenderPathError::SceneStorageBufferRange {
            required: byte_size,
            available: device.capabilities().max_storage_buffer_range,
        });
    }
    Ok(byte_size)
}

fn create_scene_gpu_resources(
    device: &RenderPathDeviceContext<'_>,
    bundle: &ComputeSceneBundle,
) -> Result<ComputeSceneGpuResources, ComputeRenderPathError> {
    let bytes = u32_bytes(bundle.storage_words());
    let byte_size = scene_storage_byte_size(device, bundle)?;
    let buffer_info = vk::BufferCreateInfo::default()
        .size(byte_size)
        .usage(vk::BufferUsageFlags::STORAGE_BUFFER)
        .sharing_mode(vk::SharingMode::EXCLUSIVE);
    let buffer = unsafe { device.create_buffer(&buffer_info) }
        .map_err(ComputeRenderPathError::CreateSceneBuffer)?;
    let requirements = unsafe { device.buffer_memory_requirements(buffer) };
    let Some(memory_type_index) = device.memory_type_index(
        requirements.memory_type_bits,
        vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
    ) else {
        unsafe { device.destroy_buffer(buffer) };
        return Err(ComputeRenderPathError::MissingSceneMemory);
    };
    let allocation_info = vk::MemoryAllocateInfo::default()
        .allocation_size(requirements.size)
        .memory_type_index(memory_type_index);
    let memory = match unsafe { device.allocate_memory(&allocation_info) } {
        Ok(memory) => memory,
        Err(error) => {
            unsafe { device.destroy_buffer(buffer) };
            return Err(ComputeRenderPathError::AllocateSceneMemory(error));
        }
    };
    if let Err(error) = unsafe { device.bind_buffer_memory(buffer, memory) } {
        release_scene_gpu_resources(
            device,
            ComputeSceneGpuResources {
                buffer,
                memory,
                allocation_bytes: requirements.size,
            },
        );
        return Err(ComputeRenderPathError::BindSceneMemory(error));
    }
    if let Err(error) = unsafe { device.write_memory(memory, bytes) } {
        release_scene_gpu_resources(
            device,
            ComputeSceneGpuResources {
                buffer,
                memory,
                allocation_bytes: requirements.size,
            },
        );
        return Err(ComputeRenderPathError::WriteSceneMemory(error));
    }
    Ok(ComputeSceneGpuResources {
        buffer,
        memory,
        allocation_bytes: requirements.size,
    })
}

fn release_scene_gpu_resources(
    device: &RenderPathDeviceContext<'_>,
    resources: ComputeSceneGpuResources,
) {
    unsafe {
        if resources.buffer != vk::Buffer::null() {
            device.destroy_buffer(resources.buffer);
        }
        if resources.memory != vk::DeviceMemory::null() {
            device.free_memory(resources.memory);
        }
    }
}

fn output_subresource_range() -> vk::ImageSubresourceRange {
    vk::ImageSubresourceRange::default()
        .aspect_mask(vk::ImageAspectFlags::COLOR)
        .base_mip_level(0)
        .level_count(1)
        .base_array_layer(0)
        .layer_count(1)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct OutputBarrierPlan {
    source_stage: vk::PipelineStageFlags,
    source_access: vk::AccessFlags,
    old_layout: vk::ImageLayout,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SamplingBarrierPlan {
    source_stage: vk::PipelineStageFlags,
    destination_stage: vk::PipelineStageFlags,
    source_access: vk::AccessFlags,
    destination_access: vk::AccessFlags,
    old_layout: vk::ImageLayout,
    new_layout: vk::ImageLayout,
}

fn output_barrier_plan(output_initialized: bool) -> OutputBarrierPlan {
    if output_initialized {
        OutputBarrierPlan {
            source_stage: vk::PipelineStageFlags::FRAGMENT_SHADER,
            source_access: vk::AccessFlags::SHADER_READ,
            old_layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        }
    } else {
        OutputBarrierPlan {
            source_stage: vk::PipelineStageFlags::TOP_OF_PIPE,
            source_access: vk::AccessFlags::empty(),
            old_layout: vk::ImageLayout::UNDEFINED,
        }
    }
}

fn sampling_barrier_plan() -> SamplingBarrierPlan {
    SamplingBarrierPlan {
        source_stage: vk::PipelineStageFlags::COMPUTE_SHADER,
        destination_stage: vk::PipelineStageFlags::FRAGMENT_SHADER,
        source_access: vk::AccessFlags::SHADER_WRITE,
        destination_access: vk::AccessFlags::SHADER_READ,
        old_layout: vk::ImageLayout::GENERAL,
        new_layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
    }
}

fn read_shader(bytes: &[u8]) -> Result<Vec<u32>, std::io::Error> {
    ash::util::read_spv(&mut Cursor::new(bytes))
}

fn f32_bytes(values: &[f32]) -> &[u8] {
    let byte_length = std::mem::size_of_val(values);
    // Every f32 bit pattern is initialized data and valid to read as bytes.
    unsafe { std::slice::from_raw_parts(values.as_ptr().cast(), byte_length) }
}

fn u32_bytes(values: &[u32]) -> &[u8] {
    let byte_length = std::mem::size_of_val(values);
    // Every u32 bit pattern is initialized data and valid to read as bytes.
    unsafe { std::slice::from_raw_parts(values.as_ptr().cast(), byte_length) }
}

fn u32_bytes_mut(values: &mut [u32]) -> &mut [u8] {
    let byte_length = std::mem::size_of_val(values);
    // Every u32 bit pattern is valid and the mapped read initializes all requested bytes.
    unsafe { std::slice::from_raw_parts_mut(values.as_mut_ptr().cast(), byte_length) }
}

fn semantic_ray_input_words(probes: &[SemanticRayProbe]) -> [u32; SEMANTIC_RAY_BUFFER_WORD_COUNT] {
    let mut words = [0_u32; SEMANTIC_RAY_BUFFER_WORD_COUNT];
    words[0] = u32::try_from(probes.len()).unwrap_or(0);
    for (index, probe) in probes.iter().enumerate() {
        let offset = SEMANTIC_RAY_INPUT_START + index * SEMANTIC_RAY_INPUT_WORD_COUNT;
        let ray = probe.ray();
        let [origin_x, origin_y, origin_z] = ray.origin().map(|value| (value as f32).to_bits());
        let [direction_x, direction_y, direction_z] =
            ray.direction().map(|value| (value as f32).to_bits());
        words[offset..offset + SEMANTIC_RAY_INPUT_WORD_COUNT].copy_from_slice(&[
            origin_x,
            origin_y,
            origin_z,
            (ray.minimum_distance() as f32).to_bits(),
            direction_x,
            direction_y,
            direction_z,
            (ray.maximum_distance() as f32).to_bits(),
        ]);
    }
    words
}

fn decode_semantic_ray_output(
    probes: &[SemanticRayProbe],
    words: &[u32; SEMANTIC_RAY_BUFFER_WORD_COUNT],
    bundle: &ComputeSceneBundle,
    revision: VoxelSceneRevision,
    frame_sequence: u64,
) -> Result<Vec<ComputeSemanticRayProbeObservation>, ComputeRenderPathError> {
    if bundle.revision() != revision {
        return Err(ComputeRenderPathError::InvalidSemanticRayOutput {
            probe_identity: probes
                .first()
                .map(|probe| probe.identity().to_owned())
                .unwrap_or_else(|| "empty-request".to_owned()),
            reason: "the installed CPU bundle no longer matches the recorded GPU revision",
        });
    }
    probes
        .iter()
        .enumerate()
        .map(|(index, probe)| {
            let offset = SEMANTIC_RAY_OUTPUT_START + index * SEMANTIC_RAY_OUTPUT_WORD_COUNT;
            let output = words
                .get(offset..offset + SEMANTIC_RAY_OUTPUT_WORD_COUNT)
                .ok_or(ComputeRenderPathError::InvalidSemanticRayOutput {
                    probe_identity: probe.identity().to_owned(),
                    reason: "the output record is outside the readback buffer",
                })?;
            let result = match output[0] {
                0 => SemanticRayResult::Miss,
                1 => {
                    let volume_index = usize::try_from(output[1]).map_err(|_| {
                        ComputeRenderPathError::InvalidSemanticRayOutput {
                            probe_identity: probe.identity().to_owned(),
                            reason: "the volume index cannot address host memory",
                        }
                    })?;
                    let volume = bundle.volume_headers().get(volume_index).ok_or(
                        ComputeRenderPathError::InvalidSemanticRayOutput {
                            probe_identity: probe.identity().to_owned(),
                            reason: "the volume index is outside the installed bundle",
                        },
                    )?;
                    let material_index = output[5].checked_sub(1).ok_or(
                        ComputeRenderPathError::InvalidSemanticRayOutput {
                            probe_identity: probe.identity().to_owned(),
                            reason: "a contact returned the empty material word",
                        },
                    )?;
                    let material_index = usize::try_from(material_index).map_err(|_| {
                        ComputeRenderPathError::InvalidSemanticRayOutput {
                            probe_identity: probe.identity().to_owned(),
                            reason: "the material index cannot address host memory",
                        }
                    })?;
                    let material = bundle.material_identities().get(material_index).ok_or(
                        ComputeRenderPathError::InvalidSemanticRayOutput {
                            probe_identity: probe.identity().to_owned(),
                            reason: "the material index is outside the installed bundle",
                        },
                    )?;
                    let distance = f32::from_bits(output[6]);
                    if !distance.is_finite() {
                        return Err(ComputeRenderPathError::InvalidSemanticRayOutput {
                            probe_identity: probe.identity().to_owned(),
                            reason: "the contact distance is not finite",
                        });
                    }
                    let classification = semantic_ray_classification(output[7]).ok_or(
                        ComputeRenderPathError::InvalidSemanticRayOutput {
                            probe_identity: probe.identity().to_owned(),
                            reason: "the contact normal code is invalid",
                        },
                    )?;
                    SemanticRayResult::Contact(SemanticRayContact::new(
                        volume.identity().clone(),
                        VoxelCoordinate::new(
                            i32::from_ne_bytes(output[2].to_ne_bytes()),
                            i32::from_ne_bytes(output[3].to_ne_bytes()),
                            i32::from_ne_bytes(output[4].to_ne_bytes()),
                        ),
                        material.clone(),
                        f64::from(distance),
                        classification,
                    ))
                }
                _ => {
                    return Err(ComputeRenderPathError::InvalidSemanticRayOutput {
                        probe_identity: probe.identity().to_owned(),
                        reason: "the hit flag is invalid",
                    });
                }
            };
            Ok(ComputeSemanticRayProbeObservation {
                probe_identity: probe.identity().to_owned(),
                frame_sequence,
                observation: SemanticRayObservation::new(
                    bundle.scene_identity().clone(),
                    revision,
                    result,
                ),
            })
        })
        .collect()
}

fn semantic_ray_classification(code: u32) -> Option<SemanticRayContactClassification> {
    let normal = match code {
        0 => return Some(SemanticRayContactClassification::StartedInside),
        1 => AxisNormal::NegativeX,
        2 => AxisNormal::PositiveX,
        3 => AxisNormal::NegativeY,
        4 => AxisNormal::PositiveY,
        5 => AxisNormal::NegativeZ,
        6 => AxisNormal::PositiveZ,
        _ => return None,
    };
    Some(SemanticRayContactClassification::Entered(normal))
}

fn camera_storage_words(camera: CameraState, extent: vk::Extent2D) -> [f32; CAMERA_WORD_COUNT] {
    let eye = camera.eye();
    let forward = normalize(subtract(camera.target(), eye));
    let right = normalize(cross(forward, camera.up()));
    let upward = cross(right, forward);
    let mut words = [0.0; CAMERA_WORD_COUNT];
    words[0..3].copy_from_slice(&eye);
    words[4..7].copy_from_slice(&forward);
    words[8..11].copy_from_slice(&right);
    words[12..15].copy_from_slice(&upward);
    words[16] = camera.near_plane();
    words[17] = camera.far_plane();
    words[18] = (camera.field_of_view_degrees().to_radians() * 0.5).tan();
    words[19] = extent.width as f32 / extent.height as f32;
    words
}

fn subtract(left: [f32; 3], right: [f32; 3]) -> [f32; 3] {
    let [left_x, left_y, left_z] = left;
    let [right_x, right_y, right_z] = right;
    [left_x - right_x, left_y - right_y, left_z - right_z]
}

fn add(left: [f32; 3], right: [f32; 3]) -> [f32; 3] {
    let [left_x, left_y, left_z] = left;
    let [right_x, right_y, right_z] = right;
    [left_x + right_x, left_y + right_y, left_z + right_z]
}

fn scale(vector: [f32; 3], factor: f32) -> [f32; 3] {
    let [x, y, z] = vector;
    [x * factor, y * factor, z * factor]
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

fn create_shader_module(
    device: &RenderPathDeviceContext<'_>,
    code: &[u32],
) -> Result<vk::ShaderModule, ComputeRenderPathError> {
    let create_info = vk::ShaderModuleCreateInfo::default().code(code);
    unsafe { device.create_shader_module(&create_info) }
        .map_err(ComputeRenderPathError::CreateShaderModule)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_gpu_candidate_release_clears_control_and_owned_resources()
    -> Result<(), Box<dyn std::error::Error>> {
        use voxel_frontend::{DenseVoxelScene, VoxelFrontend, VoxelSceneId};

        let view = VoxelFrontend::new().publish(DenseVoxelScene::new(
            VoxelSceneId::new("hidden-release"),
            VoxelSceneRevision::new(1),
            Vec::new(),
            Vec::new(),
        ))?;
        let bundle = ComputeSceneBundle::from_view(&view)?;
        let mut render_path = ComputeRayRenderPath::new(
            bundle,
            CameraState::new(
                [2.0, 2.0, 2.0],
                [0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                50.0,
                0.1,
                100.0,
            )?,
            None,
        );
        let controller = render_path.convergence.enable_control(true);
        render_path.convergence_control = Some(controller.clone());
        let installed = render_path.convergence.status().installed();
        render_path.hidden_scene_gpu_resources = Some(ComputeHiddenSceneGpuResources {
            stamp: installed,
            resources: ComputeSceneGpuResources {
                buffer: vk::Buffer::null(),
                memory: vk::DeviceMemory::null(),
                allocation_bytes: 0,
            },
            range: 0,
        });
        assert!(controller.hold_post_upload(installed.revision())?);
        let mut release_count = 0;

        render_path.release_hidden_scene_gpu_resources_with(|resources| {
            assert_eq!(resources.buffer, vk::Buffer::null());
            assert_eq!(resources.memory, vk::DeviceMemory::null());
            release_count += 1;
        })?;

        assert_eq!(release_count, 1);
        assert!(render_path.hidden_scene_gpu_resources.is_none());
        assert_eq!(controller.post_upload_revision()?, None);
        Ok(())
    }

    #[test]
    fn first_dispatch_discards_undefined_contents_before_compute_writes() {
        assert_eq!(
            output_barrier_plan(false),
            OutputBarrierPlan {
                source_stage: vk::PipelineStageFlags::TOP_OF_PIPE,
                source_access: vk::AccessFlags::empty(),
                old_layout: vk::ImageLayout::UNDEFINED,
            }
        );
    }

    #[test]
    fn later_dispatch_waits_for_the_previous_composite_read() {
        assert_eq!(
            output_barrier_plan(true),
            OutputBarrierPlan {
                source_stage: vk::PipelineStageFlags::FRAGMENT_SHADER,
                source_access: vk::AccessFlags::SHADER_READ,
                old_layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            }
        );
    }

    #[test]
    fn compute_writes_become_visible_to_fragment_sampling() {
        assert_eq!(
            sampling_barrier_plan(),
            SamplingBarrierPlan {
                source_stage: vk::PipelineStageFlags::COMPUTE_SHADER,
                destination_stage: vk::PipelineStageFlags::FRAGMENT_SHADER,
                source_access: vk::AccessFlags::SHADER_WRITE,
                destination_access: vk::AccessFlags::SHADER_READ,
                old_layout: vk::ImageLayout::GENERAL,
                new_layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            }
        );
    }

    #[test]
    fn compute_shader_guards_only_out_of_range_invocations_and_writes_every_valid_pixel() {
        let shader = include_str!("../shaders/dense_dda.comp");
        assert!(shader.contains("writeonly image2D output_image"));
        assert!(shader.contains("greaterThanEqual(pixel, dimensions)"));
        assert!(shader.contains("readonly buffer SceneData"));
        assert!(shader.contains("Hit trace_volume"));
        assert!(shader.contains("bvec3 tied"));
        assert!(shader.contains("imageStore(output_image, pixel"));
        assert!(shader.contains("binding = 4) buffer SemanticRayData"));
        assert!(shader.contains("Hit nearest = trace_scene"));
        assert!(shader.contains("observe_semantic_ray(probe_index)"));
    }

    #[test]
    fn semantic_ray_readback_decodes_installed_scene_identities_and_gpu_contact_data()
    -> Result<(), Box<dyn std::error::Error>> {
        use semantic_ray_oracle::{SemanticRayDistanceTolerance, observe};
        use voxel_frontend::{
            DenseVoxelBatch, DenseVoxelScene, DenseVoxelVolume, VoxelExtent, VoxelFrontend,
            VoxelMaterial, VoxelMaterialId, VoxelRegion, VoxelSceneId, VoxelValue, VoxelVolumeId,
            VoxelVolumeMetadata,
        };

        let material_identity = VoxelMaterialId::new("stone");
        let view = VoxelFrontend::new().publish(DenseVoxelScene::new(
            VoxelSceneId::new("gpu-observation"),
            VoxelSceneRevision::new(9),
            vec![VoxelMaterial::new(material_identity.clone(), [1.0; 4])],
            vec![DenseVoxelVolume::new(
                VoxelVolumeMetadata::new(
                    VoxelVolumeId::new("volume"),
                    VoxelExtent::new(1, 1, 1),
                    [0.0; 3],
                    1.0,
                ),
                vec![DenseVoxelBatch::new(
                    VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), VoxelExtent::new(1, 1, 1)),
                    vec![VoxelValue::Occupied(material_identity)],
                )],
            )],
        ))?;
        let bundle = ComputeSceneBundle::from_view(&view)?;
        let probe = SemanticRayProbe::new(
            "entered",
            SemanticRay::new([-1.0, 0.5, 0.5], [1.0, 0.0, 0.0], 0.0, 3.0)?,
        )?;
        let mut words = semantic_ray_input_words(std::slice::from_ref(&probe));
        let output = &mut words
            [SEMANTIC_RAY_OUTPUT_START..SEMANTIC_RAY_OUTPUT_START + SEMANTIC_RAY_OUTPUT_WORD_COUNT];
        output.copy_from_slice(&[1, 0, 0, 0, 0, 1, 1.0_f32.to_bits(), 1]);

        let decoded = decode_semantic_ray_output(
            std::slice::from_ref(&probe),
            &words,
            &bundle,
            view.revision(),
            37,
        )?;
        let observation = decoded.first().ok_or("missing decoded observation")?;
        let oracle = observe(&view, probe.ray())?;
        let tolerance = SemanticRayDistanceTolerance::new(1.0e-6)?;

        assert_eq!(observation.probe_identity(), "entered");
        assert_eq!(observation.frame_sequence(), 37);
        assert!(observation.observation().agrees_with(&oracle, tolerance));
        Ok(())
    }

    #[test]
    fn composite_samples_the_compute_output_over_a_full_screen_triangle() {
        let vertex_shader = include_str!("../shaders/composite.vert");
        let fragment_shader = include_str!("../shaders/composite.frag");
        assert!(vertex_shader.contains("vec2( 3.0, -1.0)"));
        assert!(vertex_shader.contains("vec2(-1.0,  3.0)"));
        assert!(fragment_shader.contains("sampler2D computed_image"));
        assert!(fragment_shader.contains("texture(computed_image, texture_coordinate)"));
    }
}

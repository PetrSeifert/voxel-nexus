use ash::vk;
use render_backend::{
    CameraState, CameraStateRevision, PresentationConfigurationId, RenderPath,
    RenderPathAttachmentIdentity, RenderPathDeviceCapabilities, RenderPathDeviceContext,
    RenderPathFrameContext, RenderPathReadiness, RenderPathResult, RenderPathRetirement,
    RenderPathStamp, RenderPathStrategy, RenderPathTarget, SwitchableRenderPath,
};
use semantic_ray_oracle::{SemanticRay, SemanticRayError};
use std::io::Cursor;
use thiserror::Error;
use voxel_frontend::{VoxelEditOutcome, VoxelSceneRevision, VoxelSceneView};

mod compute_convergence;
mod compute_scene;

pub use compute_convergence::{
    ComputeCandidateDisposition, ComputeConvergenceAcceptance, ComputeConvergenceError,
    ComputeConvergenceEvent, ComputeConvergenceFailure, ComputeConvergenceFailurePhase,
    ComputeConvergenceGeneration, ComputeConvergenceRetry, ComputeConvergenceStatus,
    ComputeConvergenceWorkStamp,
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
            2,
        ),
        (
            ComputeDescriptorRequirement::StorageBufferPerSet,
            queried.max_descriptor_set_storage_buffers,
            2,
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
    if queried.max_storage_buffer_range < CAMERA_BUFFER_SIZE {
        return Err(ComputeRenderPathRejection::StorageBufferBindingRange {
            required: CAMERA_BUFFER_SIZE,
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
    #[error("compute convergence shutdown failed: {0}")]
    ConvergenceShutdown(String),
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
}

pub struct ComputeRayRenderPathAdapter {
    render_path: ComputeRayRenderPath,
    camera_state_revision: CameraStateRevision,
    published_camera_state_revision: CameraStateRevision,
}

impl ComputeRayRenderPathAdapter {
    pub fn new(
        view: VoxelSceneView,
        camera_state: CameraState,
        camera_state_revision: CameraStateRevision,
    ) -> Result<Self, ComputeSceneBuildError> {
        let scene_bundle = ComputeSceneBundle::from_view(&view)?;
        Ok(Self {
            render_path: ComputeRayRenderPath::new(scene_bundle, camera_state),
            camera_state_revision,
            published_camera_state_revision: camera_state_revision,
        })
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
        self.render_path.record(frame)
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
    output_view: vk::ImageView,
    sampler: vk::Sampler,
    scene_buffer: vk::Buffer,
    scene_memory: vk::DeviceMemory,
    scene_gpu_revision: VoxelSceneRevision,
    camera_buffer: vk::Buffer,
    camera_memory: vk::DeviceMemory,
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
}

impl ComputeRayRenderPath {
    fn new(scene_bundle: ComputeSceneBundle, camera_state: CameraState) -> Self {
        let scene_gpu_revision = scene_bundle.revision();
        Self {
            convergence: compute_convergence::ComputeConvergence::new(scene_bundle),
            camera_state,
            capability_assessment: None,
            output_image: vk::Image::null(),
            output_memory: vk::DeviceMemory::null(),
            output_view: vk::ImageView::null(),
            sampler: vk::Sampler::null(),
            scene_buffer: vk::Buffer::null(),
            scene_memory: vk::DeviceMemory::null(),
            scene_gpu_revision,
            camera_buffer: vk::Buffer::null(),
            camera_memory: vk::DeviceMemory::null(),
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
        }
    }

    fn configure_resources(
        &mut self,
        device: &RenderPathDeviceContext<'_>,
        target: RenderPathTarget<'_>,
        qualification: ComputeCapabilityRecord,
    ) -> Result<(), ComputeRenderPathError> {
        if self.scene_buffer == vk::Buffer::null() || self.scene_memory == vk::DeviceMemory::null()
        {
            self.release_scene_resources(device);
            self.create_scene_buffer(device)?;
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
                .descriptor_count(2),
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

        unsafe {
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
            frame.image_pipeline_barrier(
                sampling_barrier.source_stage,
                sampling_barrier.destination_stage,
                &prepare_for_sampling,
            );
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
        let Some(candidate_bundle) = self.convergence.hidden_bundle() else {
            return Ok(());
        };
        let candidate_revision = candidate_bundle.revision();
        let candidate_range = match scene_storage_byte_size(device, candidate_bundle) {
            Ok(range) => range,
            Err(error) => {
                self.convergence
                    .fail_hidden(ComputeConvergenceFailurePhase::Upload, error.to_string());
                return Err(error);
            }
        };
        let candidate_resources = match create_scene_gpu_resources(device, candidate_bundle) {
            Ok(resources) => resources,
            Err(error) => {
                self.convergence
                    .fail_hidden(ComputeConvergenceFailurePhase::Upload, error.to_string());
                return Err(error);
            }
        };
        self.convergence.mark_hidden_uploaded();
        let retired_bundle = match self.convergence.install_hidden(self.scene_gpu_revision) {
            Ok(Some(bundle)) => bundle,
            Ok(None) => {
                release_scene_gpu_resources(device, candidate_resources);
                return Ok(());
            }
            Err(error) => {
                release_scene_gpu_resources(device, candidate_resources);
                self.convergence.fail_hidden(
                    ComputeConvergenceFailurePhase::Installation,
                    error.to_string(),
                );
                return Err(error.into());
            }
        };
        let scene_buffer = [vk::DescriptorBufferInfo::default()
            .buffer(candidate_resources.buffer)
            .offset(0)
            .range(candidate_range)];
        let writes = [vk::WriteDescriptorSet::default()
            .dst_set(self.descriptor_set)
            .dst_binding(2)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .buffer_info(&scene_buffer)];
        unsafe { device.update_descriptor_sets(&writes) };
        let retired_resources = ComputeSceneGpuResources {
            buffer: std::mem::replace(&mut self.scene_buffer, candidate_resources.buffer),
            memory: std::mem::replace(&mut self.scene_memory, candidate_resources.memory),
        };
        self.scene_gpu_revision = candidate_revision;
        release_scene_gpu_resources(device, retired_resources);
        drop(retired_bundle);
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
            if self.camera_buffer != vk::Buffer::null() {
                device.destroy_buffer(self.camera_buffer);
                self.camera_buffer = vk::Buffer::null();
            }
            if self.camera_memory != vk::DeviceMemory::null() {
                device.free_memory(self.camera_memory);
                self.camera_memory = vk::DeviceMemory::null();
            }
        }
        self.configured_attachments.clear();
        self.configuration_id = None;
        self.output_extent = vk::Extent2D::default();
        self.dispatch_group_count = [0; 3];
        self.output_initialized = false;
    }

    fn release_scene_resources(&mut self, device: &RenderPathDeviceContext<'_>) {
        unsafe {
            if self.scene_buffer != vk::Buffer::null() {
                device.destroy_buffer(self.scene_buffer);
                self.scene_buffer = vk::Buffer::null();
            }
            if self.scene_memory != vk::DeviceMemory::null() {
                device.free_memory(self.scene_memory);
                self.scene_memory = vk::DeviceMemory::null();
            }
        }
    }
}

impl RenderPath for ComputeRayRenderPath {
    fn release(&mut self, device: RenderPathDeviceContext<'_>) -> RenderPathResult<()> {
        self.release_presentation_resources(&device);
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
        Ok(())
    }

    fn shutdown(&mut self, device: RenderPathDeviceContext<'_>) -> RenderPathResult<()> {
        let convergence_error = self.convergence.shutdown().err();
        self.release_presentation_resources(&device);
        self.release_scene_resources(&device);
        match convergence_error {
            Some(error) => Err(Box::new(ComputeRenderPathError::ConvergenceShutdown(error))),
            None => Ok(()),
        }
    }

    fn advance_frame_boundary(
        &mut self,
        device: RenderPathDeviceContext<'_>,
        target: RenderPathTarget<'_>,
    ) -> RenderPathResult<()> {
        self.advance_convergence_at_frame_boundary(&device, target)
            .map_err(|error| Box::new(error) as _)
    }

    fn record(&mut self, frame: RenderPathFrameContext<'_>) -> RenderPathResult<()> {
        self.record_frame(&frame)
            .map_err(|error| Box::new(error) as _)
    }
}

struct ComputeSceneGpuResources {
    buffer: vk::Buffer,
    memory: vk::DeviceMemory,
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
        release_scene_gpu_resources(device, ComputeSceneGpuResources { buffer, memory });
        return Err(ComputeRenderPathError::BindSceneMemory(error));
    }
    if let Err(error) = unsafe { device.write_memory(memory, bytes) } {
        release_scene_gpu_resources(device, ComputeSceneGpuResources { buffer, memory });
        return Err(ComputeRenderPathError::WriteSceneMemory(error));
    }
    Ok(ComputeSceneGpuResources { buffer, memory })
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

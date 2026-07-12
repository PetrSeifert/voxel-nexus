use ash::vk;
use render_backend::{
    PresentationConfigurationId, RenderPath, RenderPathAttachmentIdentity, RenderPathDeviceContext,
    RenderPathFrameContext, RenderPathResult, RenderPathTarget,
};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::io::Cursor;
use std::mem::size_of;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::thread::{self, JoinHandle};

use thiserror::Error;
use voxel_frontend::{
    VoxelChangeSet, VoxelCoordinate, VoxelEditOutcome, VoxelExtent, VoxelFrontendError,
    VoxelMaterialId, VoxelRegion, VoxelSceneId, VoxelSceneRevision, VoxelSceneView, VoxelValue,
    VoxelVolumeId, VoxelVolumeMetadata,
};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum AxisNormal {
    NegativeX,
    PositiveX,
    NegativeY,
    PositiveY,
    NegativeZ,
    PositiveZ,
}

impl AxisNormal {
    pub fn vector(self) -> [f32; 3] {
        match self {
            Self::NegativeX => [-1.0, 0.0, 0.0],
            Self::PositiveX => [1.0, 0.0, 0.0],
            Self::NegativeY => [0.0, -1.0, 0.0],
            Self::PositiveY => [0.0, 1.0, 0.0],
            Self::NegativeZ => [0.0, 0.0, -1.0],
            Self::PositiveZ => [0.0, 0.0, 1.0],
        }
    }

    fn offset(self) -> [i32; 3] {
        match self {
            Self::NegativeX => [-1, 0, 0],
            Self::PositiveX => [1, 0, 0],
            Self::NegativeY => [0, -1, 0],
            Self::PositiveY => [0, 1, 0],
            Self::NegativeZ => [0, 0, -1],
            Self::PositiveZ => [0, 0, 1],
        }
    }
}

const AXIS_NORMALS: [AxisNormal; 6] = [
    AxisNormal::NegativeX,
    AxisNormal::PositiveX,
    AxisNormal::NegativeY,
    AxisNormal::PositiveY,
    AxisNormal::NegativeZ,
    AxisNormal::PositiveZ,
];

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraPose {
    eye: [f32; 3],
    target: [f32; 3],
    up: [f32; 3],
    field_of_view_degrees: f32,
    near_plane: f32,
    far_plane: f32,
}

impl CameraPose {
    pub const fn new(
        eye: [f32; 3],
        target: [f32; 3],
        up: [f32; 3],
        field_of_view_degrees: f32,
        near_plane: f32,
        far_plane: f32,
    ) -> Self {
        Self {
            eye,
            target,
            up,
            field_of_view_degrees,
            near_plane,
            far_plane,
        }
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
        Ok(multiply_matrices(projection, view))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DeterministicCameraMove {
    start: CameraPose,
    end: CameraPose,
    total_steps: u32,
}

impl DeterministicCameraMove {
    pub fn new(
        start: CameraPose,
        end: CameraPose,
        total_steps: u32,
    ) -> Result<Self, CameraConfigurationError> {
        if total_steps == 0 {
            return Err(CameraConfigurationError::ZeroMoveSteps);
        }
        Ok(Self {
            start,
            end,
            total_steps,
        })
    }

    pub fn total_steps(self) -> u32 {
        self.total_steps
    }

    pub fn pose_at_step(self, step: u32) -> Result<CameraPose, CameraConfigurationError> {
        if step > self.total_steps {
            return Err(CameraConfigurationError::MoveStepOutOfRange {
                step,
                total_steps: self.total_steps,
            });
        }
        let progress = step as f32 / self.total_steps as f32;
        Ok(CameraPose {
            eye: interpolate_vector(self.start.eye, self.end.eye, progress),
            target: interpolate_vector(self.start.target, self.end.target, progress),
            up: interpolate_vector(self.start.up, self.end.up, progress),
            field_of_view_degrees: interpolate_scalar(
                self.start.field_of_view_degrees,
                self.end.field_of_view_degrees,
                progress,
            ),
            near_plane: interpolate_scalar(self.start.near_plane, self.end.near_plane, progress),
            far_plane: interpolate_scalar(self.start.far_plane, self.end.far_plane, progress),
        })
    }
}

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum CameraConfigurationError {
    #[error("camera projection requires a non-zero drawable extent")]
    ZeroDrawableExtent,
    #[error("a deterministic camera move requires at least one step")]
    ZeroMoveSteps,
    #[error("camera move step {step} exceeds the final step {total_steps}")]
    MoveStepOutOfRange { step: u32, total_steps: u32 },
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct SemanticFace {
    volume_identity: VoxelVolumeId,
    occupied_coordinate: VoxelCoordinate,
    outward_normal: AxisNormal,
    material_identity: VoxelMaterialId,
}

impl SemanticFace {
    pub fn new(
        volume_identity: VoxelVolumeId,
        occupied_coordinate: VoxelCoordinate,
        outward_normal: AxisNormal,
        material_identity: VoxelMaterialId,
    ) -> Self {
        Self {
            volume_identity,
            occupied_coordinate,
            outward_normal,
            material_identity,
        }
    }

    pub fn volume_identity(&self) -> &VoxelVolumeId {
        &self.volume_identity
    }

    pub fn occupied_coordinate(&self) -> VoxelCoordinate {
        self.occupied_coordinate
    }

    pub fn outward_normal(&self) -> AxisNormal {
        self.outward_normal
    }

    pub fn material_identity(&self) -> &VoxelMaterialId {
        &self.material_identity
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RasterVertex {
    position: [f32; 3],
    normal: [f32; 3],
    linear_base_color: [f32; 4],
}

const _: () = assert!(size_of::<RasterVertex>() == 10 * size_of::<f32>());

impl RasterVertex {
    pub fn position(&self) -> [f32; 3] {
        self.position
    }

    pub fn normal(&self) -> [f32; 3] {
        self.normal
    }

    pub fn linear_base_color(&self) -> [f32; 4] {
        self.linear_base_color
    }
}

#[derive(Clone, Debug)]
pub struct RasterArtifact {
    scene_identity: VoxelSceneId,
    source_revision: VoxelSceneRevision,
    region_extent: Option<VoxelExtent>,
    volume_identity: Option<VoxelVolumeId>,
    vertices: Vec<RasterVertex>,
    indices: Vec<u32>,
    semantic_faces: Vec<SemanticFace>,
    vertex_byte_size: usize,
    index_byte_size: usize,
    regions: Vec<RasterRegionResult>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct RasterRegionIdentity {
    volume_identity: VoxelVolumeId,
    core_origin: VoxelCoordinate,
}

impl RasterRegionIdentity {
    pub fn volume_identity(&self) -> &VoxelVolumeId {
        &self.volume_identity
    }

    pub fn core_origin(&self) -> VoxelCoordinate {
        self.core_origin
    }
}

#[derive(Clone, Debug)]
pub struct RasterRegionResult {
    identity: RasterRegionIdentity,
    core: VoxelRegion,
    source_revision: VoxelSceneRevision,
    vertices: Vec<RasterVertex>,
    indices: Vec<u32>,
    semantic_faces: Vec<SemanticFace>,
}

impl RasterRegionResult {
    pub fn identity(&self) -> &RasterRegionIdentity {
        &self.identity
    }

    pub fn core(&self) -> VoxelRegion {
        self.core
    }

    pub fn source_revision(&self) -> VoxelSceneRevision {
        self.source_revision
    }

    pub fn vertices(&self) -> &[RasterVertex] {
        &self.vertices
    }

    pub fn indices(&self) -> &[u32] {
        &self.indices
    }

    pub fn semantic_faces(&self) -> &[SemanticFace] {
        &self.semantic_faces
    }

    pub fn is_empty(&self) -> bool {
        self.semantic_faces.is_empty()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RasterRegionResourceOwnership {
    None,
    VertexAndIndex,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RasterRegionInstallation {
    identity: RasterRegionIdentity,
    resource_ownership: RasterRegionResourceOwnership,
    installation_generation: RasterRegionInstallationGeneration,
    gpu_resource_identity: Option<RasterRegionGpuResourceIdentity>,
    activity: RasterRegionActivity,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct RasterRegionGpuResourceIdentity {
    region_identity: RasterRegionIdentity,
    installation_generation: RasterRegionInstallationGeneration,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct RasterRegionInstallationGeneration(u64);

impl RasterRegionInstallationGeneration {
    pub fn new(generation: u64) -> Self {
        Self(generation)
    }

    pub fn checked_successor(self) -> Option<Self> {
        self.0.checked_add(1).map(Self)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RasterRegionActivity {
    scheduling_events: u64,
    derivation_events: u64,
    upload_events: u64,
    replacement_events: u64,
}

impl RasterRegionInstallation {
    pub fn identity(&self) -> &RasterRegionIdentity {
        &self.identity
    }

    pub fn resource_ownership(&self) -> RasterRegionResourceOwnership {
        self.resource_ownership
    }

    pub fn installation_generation(&self) -> RasterRegionInstallationGeneration {
        self.installation_generation
    }

    pub fn gpu_resource_identity(&self) -> Option<&RasterRegionGpuResourceIdentity> {
        self.gpu_resource_identity.as_ref()
    }

    pub fn activity(&self) -> RasterRegionActivity {
        self.activity
    }

    fn new(
        region: &RasterRegionResult,
        has_gpu_resources: bool,
        installation_generation: RasterRegionInstallationGeneration,
    ) -> Self {
        Self {
            identity: region.identity().clone(),
            resource_ownership: if has_gpu_resources {
                RasterRegionResourceOwnership::VertexAndIndex
            } else {
                RasterRegionResourceOwnership::None
            },
            gpu_resource_identity: has_gpu_resources.then(|| RasterRegionGpuResourceIdentity {
                region_identity: region.identity().clone(),
                installation_generation,
            }),
            installation_generation,
            activity: RasterRegionActivity::default(),
        }
    }
}

impl RasterRegionActivity {
    pub fn scheduling_events(self) -> u64 {
        self.scheduling_events
    }

    pub fn derivation_events(self) -> u64 {
        self.derivation_events
    }

    pub fn upload_events(self) -> u64 {
        self.upload_events
    }

    pub fn replacement_events(self) -> u64 {
        self.replacement_events
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RasterAdjacentChangeMismatch {
    SceneIdentity {
        installed: Option<VoxelSceneId>,
        change_set: VoxelSceneId,
        successor_view: VoxelSceneId,
    },
    SuccessorRevision {
        change_set: VoxelSceneRevision,
        successor_view: VoxelSceneRevision,
    },
    PredecessorRevision {
        installed: Option<VoxelSceneRevision>,
        change_set: VoxelSceneRevision,
    },
    Adjacency {
        installed: Option<VoxelSceneRevision>,
        successor: VoxelSceneRevision,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RasterAdjacentChangeOutcome {
    Applied {
        scene_identity: VoxelSceneId,
        predecessor_revision: VoxelSceneRevision,
        successor_revision: VoxelSceneRevision,
        affected_regions: Vec<RasterRegionIdentity>,
    },
    Inapplicable {
        mismatches: Vec<RasterAdjacentChangeMismatch>,
    },
}

#[derive(Debug, Error)]
pub enum RasterAdjacentChangeError {
    #[error(transparent)]
    Derivation(#[from] RasterArtifactBuildError),
    #[error("Raster Region installation generation overflow for {identity:?}")]
    InstallationGenerationOverflow { identity: RasterRegionIdentity },
    #[error("no complete raster artifact is installed")]
    MissingInstallation,
    #[error("the successor artifact is missing installed Raster Region {identity:?}")]
    MissingSuccessorRegion { identity: RasterRegionIdentity },
    #[error("the installed artifact does not define a Raster Region grid")]
    MissingRegionGrid,
    #[error("configured Raster Region resources require a device context for replacement")]
    ConfiguredResourcesRequireDevice,
    #[error("configured GPU resources are missing Raster Region {identity:?}")]
    MissingConfiguredRegion { identity: RasterRegionIdentity },
    #[error("Raster Region resource bookkeeping could not be allocated")]
    ResourceBookkeepingAllocation,
    #[error("GPU upload failed for Raster Region {identity:?}: {source}")]
    Upload {
        identity: RasterRegionIdentity,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RasterArtifactInstallationPhase {
    Upload,
    PresentationConfiguration,
    Record,
}

impl fmt::Display for RasterArtifactInstallationPhase {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Upload => "upload",
            Self::PresentationConfiguration => "presentation configuration",
            Self::Record => "record",
        })
    }
}

#[derive(Debug, Error)]
#[error("raster artifact {phase} failed for Voxel Scene Revision {source_revision}: {source}")]
pub struct RasterArtifactInstallationError {
    phase: RasterArtifactInstallationPhase,
    source_revision: VoxelSceneRevision,
    #[source]
    source: Box<dyn std::error::Error + Send + Sync>,
}

struct RasterArtifactInstallationState {
    expected_revision: VoxelSceneRevision,
    staged_artifact: Option<RasterArtifact>,
    artifact_was_published: bool,
    installed_revision: Option<VoxelSceneRevision>,
    inject_upload_failure: bool,
}

#[derive(Clone)]
pub struct RasterArtifactInstaller {
    state: Arc<Mutex<RasterArtifactInstallationState>>,
}

#[derive(Clone)]
pub struct RasterCameraController {
    camera_pose: Arc<Mutex<CameraPose>>,
}

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
#[error("the raster camera control state is unavailable")]
pub struct RasterCameraControlError;

impl RasterCameraController {
    pub fn set_pose(&self, camera_pose: CameraPose) -> Result<(), RasterCameraControlError> {
        let mut current_pose = self
            .camera_pose
            .lock()
            .map_err(|_| RasterCameraControlError)?;
        *current_pose = camera_pose;
        Ok(())
    }

    pub fn pose(&self) -> Result<CameraPose, RasterCameraControlError> {
        self.camera_pose
            .lock()
            .map(|camera_pose| *camera_pose)
            .map_err(|_| RasterCameraControlError)
    }
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum RasterArtifactInstallerError {
    #[error(
        "complete raster artifact revision mismatch: expected Voxel Scene Revision {expected}, received {actual}"
    )]
    RevisionMismatch {
        expected: VoxelSceneRevision,
        actual: VoxelSceneRevision,
    },
    #[error(
        "a complete raster artifact was already published for Voxel Scene Revision {source_revision}"
    )]
    AlreadyPublished { source_revision: VoxelSceneRevision },
    #[error("the raster artifact installation state is unavailable")]
    StateUnavailable,
}

impl RasterArtifactInstaller {
    pub fn inject_next_upload_failure(&self) -> Result<(), RasterArtifactInstallerError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| RasterArtifactInstallerError::StateUnavailable)?;
        state.inject_upload_failure = true;
        Ok(())
    }

    pub fn publish_complete(
        &self,
        artifact: RasterArtifact,
    ) -> Result<(), RasterArtifactInstallerError> {
        let actual = artifact.source_revision();
        let mut state = self
            .state
            .lock()
            .map_err(|_| RasterArtifactInstallerError::StateUnavailable)?;
        if actual != state.expected_revision {
            return Err(RasterArtifactInstallerError::RevisionMismatch {
                expected: state.expected_revision,
                actual,
            });
        }
        if state.artifact_was_published {
            return Err(RasterArtifactInstallerError::AlreadyPublished {
                source_revision: actual,
            });
        }
        state.staged_artifact = Some(artifact);
        state.artifact_was_published = true;
        Ok(())
    }

    pub fn staged_source_revision(
        &self,
    ) -> Result<Option<VoxelSceneRevision>, RasterArtifactInstallerError> {
        let state = self
            .state
            .lock()
            .map_err(|_| RasterArtifactInstallerError::StateUnavailable)?;
        Ok(state
            .staged_artifact
            .as_ref()
            .map(RasterArtifact::source_revision))
    }

    pub fn installed_source_revision(
        &self,
    ) -> Result<Option<VoxelSceneRevision>, RasterArtifactInstallerError> {
        let state = self
            .state
            .lock()
            .map_err(|_| RasterArtifactInstallerError::StateUnavailable)?;
        Ok(state.installed_revision)
    }
}

impl RasterArtifactInstallationError {
    pub fn new(
        phase: RasterArtifactInstallationPhase,
        source_revision: VoxelSceneRevision,
        source: Box<dyn std::error::Error + Send + Sync>,
    ) -> Self {
        Self {
            phase,
            source_revision,
            source,
        }
    }

    pub fn phase(&self) -> RasterArtifactInstallationPhase {
        self.phase
    }

    pub fn source_revision(&self) -> VoxelSceneRevision {
        self.source_revision
    }
}

pub struct RasterRenderPath {
    artifact: Option<RasterArtifact>,
    installation: Option<RasterArtifactInstaller>,
    expected_source_revision: Option<VoxelSceneRevision>,
    installed_source_revision: Option<VoxelSceneRevision>,
    camera_control: RasterCameraController,
    region_resources: Vec<RasterRegionGpuResources>,
    depth_image: vk::Image,
    depth_memory: vk::DeviceMemory,
    depth_view: vk::ImageView,
    render_pass: vk::RenderPass,
    pipeline_layout: vk::PipelineLayout,
    pipeline: vk::Pipeline,
    framebuffers: Vec<vk::Framebuffer>,
    configured_attachments: Vec<RenderPathAttachmentIdentity>,
    configuration_id: Option<PresentationConfigurationId>,
    camera_constants: [f32; 16],
    installed_regions: Vec<RasterRegionInstallation>,
    convergence: Option<RasterConvergence>,
}

impl Default for RasterRenderPath {
    fn default() -> Self {
        let camera_pose = CameraPose::new(
            [5.0, 4.0, 6.0],
            [0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            55.0,
            0.1,
            100.0,
        );
        Self {
            artifact: None,
            installation: None,
            expected_source_revision: None,
            installed_source_revision: None,
            camera_control: RasterCameraController {
                camera_pose: Arc::new(Mutex::new(camera_pose)),
            },
            region_resources: Vec::new(),
            depth_image: vk::Image::null(),
            depth_memory: vk::DeviceMemory::null(),
            depth_view: vk::ImageView::null(),
            render_pass: vk::RenderPass::null(),
            pipeline_layout: vk::PipelineLayout::null(),
            pipeline: vk::Pipeline::null(),
            framebuffers: Vec::new(),
            configured_attachments: Vec::new(),
            configuration_id: None,
            camera_constants: [0.0; 16],
            installed_regions: Vec::new(),
            convergence: None,
        }
    }
}

impl RasterRenderPath {
    pub fn new() -> Self {
        Self::default()
    }

    pub const fn front_face() -> vk::FrontFace {
        vk::FrontFace::COUNTER_CLOCKWISE
    }

    pub fn with_camera_pose(camera_pose: CameraPose) -> Self {
        Self {
            camera_control: RasterCameraController {
                camera_pose: Arc::new(Mutex::new(camera_pose)),
            },
            ..Self::default()
        }
    }

    pub fn awaiting_artifact(
        camera_pose: CameraPose,
        expected_source_revision: VoxelSceneRevision,
    ) -> (Self, RasterArtifactInstaller) {
        let (render_path, installer, _) =
            Self::awaiting_artifact_with_camera_control(camera_pose, expected_source_revision);
        (render_path, installer)
    }

    pub fn awaiting_artifact_with_camera_control(
        camera_pose: CameraPose,
        expected_source_revision: VoxelSceneRevision,
    ) -> (Self, RasterArtifactInstaller, RasterCameraController) {
        let installer = RasterArtifactInstaller {
            state: Arc::new(Mutex::new(RasterArtifactInstallationState {
                expected_revision: expected_source_revision,
                staged_artifact: None,
                artifact_was_published: false,
                installed_revision: None,
                inject_upload_failure: false,
            })),
        };
        let camera_control = RasterCameraController {
            camera_pose: Arc::new(Mutex::new(camera_pose)),
        };
        let render_path = Self {
            camera_control: camera_control.clone(),
            installation: Some(installer.clone()),
            expected_source_revision: Some(expected_source_revision),
            ..Self::default()
        };
        (render_path, installer, camera_control)
    }

    pub fn camera_pose(&self) -> Result<CameraPose, RasterCameraControlError> {
        self.camera_control.pose()
    }

    pub fn install_artifact(&mut self, artifact: RasterArtifact) {
        self.expected_source_revision = Some(artifact.source_revision());
        self.installed_source_revision = Some(artifact.source_revision());
        self.installed_regions = artifact
            .regions()
            .iter()
            .map(|region| {
                RasterRegionInstallation::new(
                    region,
                    !region.is_empty(),
                    RasterRegionInstallationGeneration::new(1),
                )
            })
            .collect();
        self.artifact = Some(artifact);
    }

    pub fn installed_source_revision(&self) -> Option<VoxelSceneRevision> {
        self.installed_source_revision
    }

    pub fn installed_regions(&self) -> &[RasterRegionInstallation] {
        &self.installed_regions
    }

    pub fn installed_artifact(&self) -> Option<&RasterArtifact> {
        self.artifact.as_ref()
    }

    pub fn begin_convergence(&mut self) -> Result<(), RasterConvergenceError> {
        if self.convergence.is_some() {
            return Err(RasterConvergenceError::AlreadyStarted);
        }
        self.convergence = Some(RasterConvergence::from_visible(self)?);
        Ok(())
    }

    pub fn accept_edit_outcome(
        &mut self,
        outcome: VoxelEditOutcome,
    ) -> Result<RasterConvergenceAcceptance, RasterConvergenceError> {
        self.convergence
            .as_mut()
            .ok_or(RasterConvergenceError::NotStarted)?
            .accept(outcome)
    }

    pub fn request_convergence_retry(
        &mut self,
    ) -> Result<RasterConvergenceRetry, RasterConvergenceError> {
        self.convergence
            .as_mut()
            .ok_or(RasterConvergenceError::NotStarted)?
            .request_retry()
    }

    pub fn drain_convergence_events(
        &mut self,
    ) -> Result<Vec<RasterConvergenceEvent>, RasterConvergenceError> {
        self.convergence
            .as_mut()
            .ok_or(RasterConvergenceError::NotStarted)?
            .drain_events()
    }

    pub fn visible_revision(&self) -> Option<VoxelSceneRevision> {
        self.convergence
            .as_ref()
            .map(RasterConvergence::visible_revision)
    }

    pub fn required_revision(&self) -> Option<VoxelSceneRevision> {
        self.convergence
            .as_ref()
            .map(RasterConvergence::required_revision)
    }

    fn advance_convergence_at_frame_boundary(
        &mut self,
        device: Option<&RenderPathDeviceContext<'_>>,
    ) -> Result<(), RasterConvergenceError> {
        let Some(mut convergence) = self.convergence.take() else {
            return Ok(());
        };
        let result = (|| {
            convergence.upload_ready_with_optional_device(device, self)?;
            let commit = convergence.commit_at_frame_boundary(self)?;
            let retirement = match commit {
                RasterConvergenceCommit::NoCandidate => return Ok(()),
                RasterConvergenceCommit::Failed { retirement, .. }
                | RasterConvergenceCommit::Rejected { retirement, .. }
                | RasterConvergenceCommit::Committed { retirement, .. } => retirement,
            };
            if retirement.resource_count() == 0 {
                return Ok(());
            }
            let device = device.ok_or(RasterConvergenceError::ConfiguredResourcesRequireDevice)?;
            // The backend invokes this hook only after its sole in-flight frame fence has completed.
            unsafe { retirement.release_after_gpu_completion(device) };
            Ok(())
        })();
        self.convergence = Some(convergence);
        result
    }

    pub fn apply_adjacent_change(
        &mut self,
        successor_view: &VoxelSceneView,
        change_set: &VoxelChangeSet,
    ) -> Result<RasterAdjacentChangeOutcome, RasterAdjacentChangeError> {
        self.apply_adjacent_change_with_optional_device(None, successor_view, change_set)
    }

    pub fn apply_adjacent_change_with_device(
        &mut self,
        device: RenderPathDeviceContext<'_>,
        successor_view: &VoxelSceneView,
        change_set: &VoxelChangeSet,
    ) -> Result<RasterAdjacentChangeOutcome, RasterAdjacentChangeError> {
        self.apply_adjacent_change_with_optional_device(Some(&device), successor_view, change_set)
    }

    fn apply_adjacent_change_with_optional_device(
        &mut self,
        device: Option<&RenderPathDeviceContext<'_>>,
        successor_view: &VoxelSceneView,
        change_set: &VoxelChangeSet,
    ) -> Result<RasterAdjacentChangeOutcome, RasterAdjacentChangeError> {
        let mut mismatches = Vec::new();
        let installed_scene_identity = self
            .artifact
            .as_ref()
            .map(|artifact| &artifact.scene_identity);
        if installed_scene_identity != Some(change_set.scene_identity())
            || installed_scene_identity != Some(successor_view.scene_id())
        {
            mismatches.push(RasterAdjacentChangeMismatch::SceneIdentity {
                installed: installed_scene_identity.cloned(),
                change_set: change_set.scene_identity().clone(),
                successor_view: successor_view.scene_id().clone(),
            });
        }
        if change_set.successor_revision() != successor_view.revision() {
            mismatches.push(RasterAdjacentChangeMismatch::SuccessorRevision {
                change_set: change_set.successor_revision(),
                successor_view: successor_view.revision(),
            });
        }
        if self.installed_source_revision != Some(change_set.predecessor_revision()) {
            mismatches.push(RasterAdjacentChangeMismatch::PredecessorRevision {
                installed: self.installed_source_revision,
                change_set: change_set.predecessor_revision(),
            });
        }
        if self
            .installed_source_revision
            .and_then(VoxelSceneRevision::checked_successor)
            != Some(change_set.successor_revision())
        {
            mismatches.push(RasterAdjacentChangeMismatch::Adjacency {
                installed: self.installed_source_revision,
                successor: change_set.successor_revision(),
            });
        }
        if !mismatches.is_empty() {
            return Ok(RasterAdjacentChangeOutcome::Inapplicable { mismatches });
        }

        let installed_artifact = self
            .artifact
            .as_ref()
            .ok_or(RasterAdjacentChangeError::MissingInstallation)?;
        let region_extent = installed_artifact
            .region_extent
            .ok_or(RasterAdjacentChangeError::MissingRegionGrid)?;
        let affected_region_identities =
            affected_raster_region_identities(successor_view, change_set, region_extent)?;
        let mut replacement_regions = HashMap::new();
        for region in installed_artifact
            .regions()
            .iter()
            .filter(|region| affected_region_identities.contains(region.identity()))
        {
            let metadata = successor_view
                .volumes()
                .iter()
                .find(|metadata| metadata.identity() == region.identity().volume_identity())
                .ok_or_else(|| {
                    build_error(
                        successor_view.revision(),
                        RasterArtifactBuildPhase::Metadata,
                        RasterArtifactBuildCause::UnknownVolume(
                            region.identity().volume_identity().clone(),
                        ),
                    )
                })?;
            replacement_regions.insert(
                region.identity().clone(),
                derive_raster_region(successor_view, metadata, region.core())?,
            );
        }

        let mut successor_regions = installed_artifact.regions().to_vec();
        for region in &mut successor_regions {
            if let Some(replacement) = replacement_regions.remove(region.identity()) {
                *region = replacement;
            }
        }
        let successor_artifact = assemble_raster_artifact(
            successor_view.scene_id().clone(),
            successor_view.revision(),
            region_extent,
            successor_regions,
        )?;

        let mut successor_installations = self.installed_regions.clone();
        for installation in &mut successor_installations {
            if !affected_region_identities.contains(installation.identity()) {
                installation.activity = RasterRegionActivity::default();
                continue;
            }
            let successor_region = successor_artifact
                .regions()
                .iter()
                .find(|region| region.identity() == installation.identity())
                .ok_or_else(|| RasterAdjacentChangeError::MissingSuccessorRegion {
                    identity: installation.identity().clone(),
                })?;
            installation.installation_generation = installation
                .installation_generation
                .checked_successor()
                .ok_or_else(
                    || RasterAdjacentChangeError::InstallationGenerationOverflow {
                        identity: installation.identity().clone(),
                    },
                )?;
            installation.resource_ownership = if successor_region.is_empty() {
                RasterRegionResourceOwnership::None
            } else {
                RasterRegionResourceOwnership::VertexAndIndex
            };
            installation.gpu_resource_identity = if successor_region.is_empty() {
                None
            } else {
                Some(RasterRegionGpuResourceIdentity {
                    region_identity: installation.identity().clone(),
                    installation_generation: installation.installation_generation,
                })
            };
            installation.activity = RasterRegionActivity {
                scheduling_events: 1,
                derivation_events: 1,
                upload_events: 1,
                replacement_events: 1,
            };
        }

        let configured_resources = self.configuration_id.is_some();
        if configured_resources && device.is_none() {
            return Err(RasterAdjacentChangeError::ConfiguredResourcesRequireDevice);
        }
        if configured_resources {
            for identity in &affected_region_identities {
                if !self
                    .region_resources
                    .iter()
                    .any(|resources| &resources.identity == identity)
                {
                    return Err(RasterAdjacentChangeError::MissingConfiguredRegion {
                        identity: identity.clone(),
                    });
                }
            }
        }

        let mut successor_gpu_resources = Vec::new();
        let mut retired_gpu_resources = Vec::new();
        let mut replacement_gpu_resources = Vec::new();
        if configured_resources {
            successor_gpu_resources
                .try_reserve_exact(self.region_resources.len())
                .map_err(|_| RasterAdjacentChangeError::ResourceBookkeepingAllocation)?;
            retired_gpu_resources
                .try_reserve_exact(affected_region_identities.len())
                .map_err(|_| RasterAdjacentChangeError::ResourceBookkeepingAllocation)?;
            replacement_gpu_resources
                .try_reserve_exact(affected_region_identities.len())
                .map_err(|_| RasterAdjacentChangeError::ResourceBookkeepingAllocation)?;
            let device =
                device.ok_or(RasterAdjacentChangeError::ConfiguredResourcesRequireDevice)?;
            for region in successor_artifact
                .regions()
                .iter()
                .filter(|region| affected_region_identities.contains(region.identity()))
            {
                match upload_raster_region_resources(device, region) {
                    Ok(resources) => replacement_gpu_resources.push(resources),
                    Err(source) => {
                        for resources in replacement_gpu_resources.drain(..) {
                            release_raster_region_resources(device, resources);
                        }
                        return Err(RasterAdjacentChangeError::Upload {
                            identity: region.identity().clone(),
                            source: Box::new(source),
                        });
                    }
                }
            }
        }

        let affected_regions = successor_artifact
            .regions()
            .iter()
            .filter(|region| affected_region_identities.contains(region.identity()))
            .map(|region| region.identity().clone())
            .collect();
        if configured_resources {
            for resources in std::mem::take(&mut self.region_resources) {
                if affected_region_identities.contains(&resources.identity) {
                    retired_gpu_resources.push(resources);
                } else {
                    successor_gpu_resources.push(resources);
                }
            }
            successor_gpu_resources.append(&mut replacement_gpu_resources);
            self.region_resources = successor_gpu_resources;
        }
        self.expected_source_revision = Some(successor_view.revision());
        self.installed_source_revision = Some(successor_view.revision());
        self.installed_regions = successor_installations;
        self.artifact = Some(successor_artifact);
        if let Some(device) = device {
            for resources in retired_gpu_resources {
                release_raster_region_resources(device, resources);
            }
        }
        Ok(RasterAdjacentChangeOutcome::Applied {
            scene_identity: successor_view.scene_id().clone(),
            predecessor_revision: change_set.predecessor_revision(),
            successor_revision: successor_view.revision(),
            affected_regions,
        })
    }
}

impl RasterArtifact {
    pub fn scene_identity(&self) -> &VoxelSceneId {
        &self.scene_identity
    }

    pub fn source_revision(&self) -> VoxelSceneRevision {
        self.source_revision
    }

    pub fn volume_identity(&self) -> Option<&VoxelVolumeId> {
        self.volume_identity.as_ref()
    }

    pub fn vertices(&self) -> &[RasterVertex] {
        &self.vertices
    }

    pub fn indices(&self) -> &[u32] {
        &self.indices
    }

    pub fn semantic_faces(&self) -> &[SemanticFace] {
        &self.semantic_faces
    }

    pub fn quad_vertices(&self, face: &SemanticFace) -> Option<&[RasterVertex]> {
        let face_index = self
            .semantic_faces
            .iter()
            .position(|candidate| candidate == face)?;
        let start = face_index.checked_mul(4)?;
        let end = start.checked_add(4)?;
        self.vertices.get(start..end)
    }

    pub fn vertex_byte_size(&self) -> usize {
        self.vertex_byte_size
    }

    pub fn index_byte_size(&self) -> usize {
        self.index_byte_size
    }

    pub fn regions(&self) -> &[RasterRegionResult] {
        &self.regions
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RasterArtifactBuildPhase {
    Metadata,
    MaterialResolution,
    VoxelRead,
    FaceExtraction,
    Geometry,
}

impl fmt::Display for RasterArtifactBuildPhase {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Metadata => "metadata",
            Self::MaterialResolution => "material resolution",
            Self::VoxelRead => "Voxel Region read",
            Self::FaceExtraction => "face extraction",
            Self::Geometry => "geometry",
        };
        formatter.write_str(name)
    }
}

#[derive(Debug, Error)]
pub enum RasterArtifactBuildCause {
    #[error("unknown Voxel Volume identity {0:?}")]
    UnknownVolume(VoxelVolumeId),
    #[error("Voxel Volume dimensions cannot be represented as logical coordinates")]
    UnrepresentableVolumeDimensions,
    #[error("count or byte-size arithmetic overflow")]
    ArithmeticOverflow,
    #[error("memory allocation failed")]
    AllocationFailed,
    #[error("logical Voxel Region read failed: {0}")]
    VoxelRead(#[source] VoxelFrontendError),
    #[error("logical Voxel Region read returned an invalid or incomplete volume")]
    InvalidVoxelRead,
    #[error("occupied coordinate references unknown Voxel Material {0:?}")]
    UnknownMaterial(VoxelMaterialId),
    #[error("scene-space coordinate transform produced a non-finite value")]
    InvalidSceneTransform,
    #[error("geometry needs an index that cannot be represented as u32")]
    IndexOverflow,
    #[error("Raster Region extent must be non-empty")]
    EmptyRasterRegionExtent,
}

#[derive(Debug, Error)]
#[error("raster artifact {phase} failed for Voxel Scene Revision {source_revision:?}: {cause}")]
pub struct RasterArtifactBuildError {
    phase: RasterArtifactBuildPhase,
    source_revision: VoxelSceneRevision,
    #[source]
    cause: RasterArtifactBuildCause,
}

impl RasterArtifactBuildError {
    pub fn phase(&self) -> RasterArtifactBuildPhase {
        self.phase
    }

    pub fn source_revision(&self) -> VoxelSceneRevision {
        self.source_revision
    }

    pub fn cause_detail(&self) -> &RasterArtifactBuildCause {
        &self.cause
    }
}

struct PendingFace {
    coordinate: VoxelCoordinate,
    normal: AxisNormal,
    material_identity: VoxelMaterialId,
    linear_base_color: [f32; 4],
}

pub fn derive_raster_artifact(
    view: &VoxelSceneView,
    volume_identity: &VoxelVolumeId,
) -> Result<RasterArtifact, RasterArtifactBuildError> {
    let source_revision = view.revision();
    let metadata = view
        .volumes()
        .iter()
        .find(|metadata| metadata.identity() == volume_identity)
        .ok_or_else(|| {
            build_error(
                source_revision,
                RasterArtifactBuildPhase::Metadata,
                RasterArtifactBuildCause::UnknownVolume(volume_identity.clone()),
            )
        })?;
    let dimensions = checked_dimensions(metadata.extent()).ok_or_else(|| {
        build_error(
            source_revision,
            RasterArtifactBuildPhase::Metadata,
            RasterArtifactBuildCause::UnrepresentableVolumeDimensions,
        )
    })?;
    let value_count = dimensions
        .iter()
        .try_fold(1_usize, |count, dimension| count.checked_mul(*dimension))
        .ok_or_else(|| {
            build_error(
                source_revision,
                RasterArtifactBuildPhase::Metadata,
                RasterArtifactBuildCause::ArithmeticOverflow,
            )
        })?;

    let mut values = Vec::new();
    values.try_reserve_exact(value_count).map_err(|_| {
        build_error(
            source_revision,
            RasterArtifactBuildPhase::VoxelRead,
            RasterArtifactBuildCause::AllocationFailed,
        )
    })?;
    values.resize(value_count, None);
    let samples = view
        .read_region(
            volume_identity,
            VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), metadata.extent()),
        )
        .map_err(|error| {
            build_error(
                source_revision,
                RasterArtifactBuildPhase::VoxelRead,
                RasterArtifactBuildCause::VoxelRead(error),
            )
        })?;
    if samples.len() != value_count {
        return Err(build_error(
            source_revision,
            RasterArtifactBuildPhase::VoxelRead,
            RasterArtifactBuildCause::InvalidVoxelRead,
        ));
    }
    for sample in samples {
        let index = dense_index(dimensions, sample.coordinate()).ok_or_else(|| {
            build_error(
                source_revision,
                RasterArtifactBuildPhase::VoxelRead,
                RasterArtifactBuildCause::InvalidVoxelRead,
            )
        })?;
        let destination = values.get_mut(index).ok_or_else(|| {
            build_error(
                source_revision,
                RasterArtifactBuildPhase::VoxelRead,
                RasterArtifactBuildCause::InvalidVoxelRead,
            )
        })?;
        if destination.is_some() {
            return Err(build_error(
                source_revision,
                RasterArtifactBuildPhase::VoxelRead,
                RasterArtifactBuildCause::InvalidVoxelRead,
            ));
        }
        *destination = Some(sample.value().clone());
    }
    if values.iter().any(Option::is_none) {
        return Err(build_error(
            source_revision,
            RasterArtifactBuildPhase::VoxelRead,
            RasterArtifactBuildCause::InvalidVoxelRead,
        ));
    }

    let mut pending_faces = Vec::new();
    for (index, value) in values.iter().enumerate() {
        let Some(VoxelValue::Occupied(material_identity)) = value else {
            continue;
        };
        let coordinate = coordinate_from_index(dimensions, index).ok_or_else(|| {
            build_error(
                source_revision,
                RasterArtifactBuildPhase::FaceExtraction,
                RasterArtifactBuildCause::ArithmeticOverflow,
            )
        })?;
        let linear_base_color = view
            .materials()
            .iter()
            .find(|material| material.identity() == material_identity)
            .map(|material| material.linear_base_color())
            .ok_or_else(|| {
                build_error(
                    source_revision,
                    RasterArtifactBuildPhase::MaterialResolution,
                    RasterArtifactBuildCause::UnknownMaterial(material_identity.clone()),
                )
            })?;
        for normal in AXIS_NORMALS {
            if face_is_exposed(&values, dimensions, coordinate, normal) {
                pending_faces.try_reserve(1).map_err(|_| {
                    build_error(
                        source_revision,
                        RasterArtifactBuildPhase::FaceExtraction,
                        RasterArtifactBuildCause::AllocationFailed,
                    )
                })?;
                pending_faces.push(PendingFace {
                    coordinate,
                    normal,
                    material_identity: material_identity.clone(),
                    linear_base_color,
                });
            }
        }
    }

    build_geometry(
        view.scene_id(),
        source_revision,
        volume_identity,
        metadata,
        pending_faces,
    )
}

pub fn derive_raster_regions(
    view: &VoxelSceneView,
    region_extent: VoxelExtent,
) -> Result<RasterArtifact, RasterArtifactBuildError> {
    let source_revision = view.revision();
    let mut regions = Vec::new();
    visit_raster_region_cores(view, region_extent, |metadata, core| {
        regions.push(derive_raster_region(view, metadata, core)?);
        Ok(true)
    })?;
    assemble_raster_artifact(
        view.scene_id().clone(),
        source_revision,
        region_extent,
        regions,
    )
}

fn visit_raster_region_cores(
    view: &VoxelSceneView,
    region_extent: VoxelExtent,
    mut visit: impl FnMut(&VoxelVolumeMetadata, VoxelRegion) -> Result<bool, RasterArtifactBuildError>,
) -> Result<bool, RasterArtifactBuildError> {
    let source_revision = view.revision();
    let [region_width, region_height, region_depth] = region_extent.dimensions();
    if region_width == 0 || region_height == 0 || region_depth == 0 {
        return Err(build_error(
            source_revision,
            RasterArtifactBuildPhase::Metadata,
            RasterArtifactBuildCause::EmptyRasterRegionExtent,
        ));
    }
    for metadata in view.volumes() {
        let [volume_width, volume_height, volume_depth] = metadata.extent().dimensions();
        checked_dimensions(metadata.extent()).ok_or_else(|| {
            build_error(
                source_revision,
                RasterArtifactBuildPhase::Metadata,
                RasterArtifactBuildCause::UnrepresentableVolumeDimensions,
            )
        })?;
        let mut origin_z = 0_u32;
        while origin_z < volume_depth {
            let mut origin_y = 0_u32;
            while origin_y < volume_height {
                let mut origin_x = 0_u32;
                while origin_x < volume_width {
                    let core = VoxelRegion::new(
                        raster_region_origin(source_revision, origin_x, origin_y, origin_z)?,
                        VoxelExtent::new(
                            region_width.min(volume_width - origin_x),
                            region_height.min(volume_height - origin_y),
                            region_depth.min(volume_depth - origin_z),
                        ),
                    );
                    if !visit(metadata, core)? {
                        return Ok(false);
                    }
                    origin_x = origin_x
                        .checked_add(region_width)
                        .ok_or_else(|| metadata_dimensions_error(source_revision))?;
                }
                origin_y = origin_y
                    .checked_add(region_height)
                    .ok_or_else(|| metadata_dimensions_error(source_revision))?;
            }
            origin_z = origin_z
                .checked_add(region_depth)
                .ok_or_else(|| metadata_dimensions_error(source_revision))?;
        }
    }
    Ok(true)
}

fn affected_raster_region_identities(
    view: &VoxelSceneView,
    change_set: &VoxelChangeSet,
    region_extent: VoxelExtent,
) -> Result<HashSet<RasterRegionIdentity>, RasterArtifactBuildError> {
    let source_revision = view.revision();
    let [region_width, region_height, region_depth] = region_extent.dimensions();
    if region_width == 0 || region_height == 0 || region_depth == 0 {
        return Err(build_error(
            source_revision,
            RasterArtifactBuildPhase::Metadata,
            RasterArtifactBuildCause::EmptyRasterRegionExtent,
        ));
    }
    let region_dimensions = [region_width, region_height, region_depth];
    let mut affected = HashSet::new();
    for changed_region in change_set.changed_regions() {
        let metadata = view
            .volumes()
            .iter()
            .find(|metadata| metadata.identity() == changed_region.volume_identity())
            .ok_or_else(|| {
                build_error(
                    source_revision,
                    RasterArtifactBuildPhase::Metadata,
                    RasterArtifactBuildCause::UnknownVolume(
                        changed_region.volume_identity().clone(),
                    ),
                )
            })?;
        let [origin_x, origin_y, origin_z] = changed_region.region().origin().components();
        let [width, height, depth] = changed_region.region().extent().dimensions();
        for z_offset in 0..depth {
            for y_offset in 0..height {
                for x_offset in 0..width {
                    let coordinate = VoxelCoordinate::new(
                        origin_x
                            .checked_add(
                                i32::try_from(x_offset)
                                    .map_err(|_| metadata_dimensions_error(source_revision))?,
                            )
                            .ok_or_else(|| metadata_dimensions_error(source_revision))?,
                        origin_y
                            .checked_add(
                                i32::try_from(y_offset)
                                    .map_err(|_| metadata_dimensions_error(source_revision))?,
                            )
                            .ok_or_else(|| metadata_dimensions_error(source_revision))?,
                        origin_z
                            .checked_add(
                                i32::try_from(z_offset)
                                    .map_err(|_| metadata_dimensions_error(source_revision))?,
                            )
                            .ok_or_else(|| metadata_dimensions_error(source_revision))?,
                    );
                    for offset in [
                        [0, 0, 0],
                        [-1, 0, 0],
                        [1, 0, 0],
                        [0, -1, 0],
                        [0, 1, 0],
                        [0, 0, -1],
                        [0, 0, 1],
                    ] {
                        let [x, y, z] = coordinate.components();
                        let Some(x) = x.checked_add(offset[0]) else {
                            continue;
                        };
                        let Some(y) = y.checked_add(offset[1]) else {
                            continue;
                        };
                        let Some(z) = z.checked_add(offset[2]) else {
                            continue;
                        };
                        let neighbor = [x, y, z];
                        let [volume_width, volume_height, volume_depth] =
                            metadata.extent().dimensions();
                        let volume_dimensions = [volume_width, volume_height, volume_depth];
                        if neighbor
                            .iter()
                            .zip(volume_dimensions)
                            .any(|(component, dimension)| {
                                *component < 0
                                    || u32::try_from(*component)
                                        .map_or(true, |component| component >= dimension)
                            })
                        {
                            continue;
                        }
                        let core_components = neighbor
                            .into_iter()
                            .zip(region_dimensions)
                            .map(|(component, dimension)| {
                                u32::try_from(component)
                                    .ok()
                                    .and_then(|component| {
                                        component
                                            .checked_div(dimension)
                                            .and_then(|index| index.checked_mul(dimension))
                                    })
                                    .and_then(|origin| i32::try_from(origin).ok())
                            })
                            .collect::<Option<Vec<_>>>();
                        let Some(core_components) = core_components else {
                            return Err(metadata_dimensions_error(source_revision));
                        };
                        let [core_x, core_y, core_z] = core_components.as_slice() else {
                            return Err(metadata_dimensions_error(source_revision));
                        };
                        affected.insert(RasterRegionIdentity {
                            volume_identity: changed_region.volume_identity().clone(),
                            core_origin: VoxelCoordinate::new(*core_x, *core_y, *core_z),
                        });
                    }
                }
            }
        }
    }
    Ok(affected)
}

fn derive_raster_region(
    view: &VoxelSceneView,
    metadata: &VoxelVolumeMetadata,
    core: VoxelRegion,
) -> Result<RasterRegionResult, RasterArtifactBuildError> {
    let source_revision = view.revision();
    let [core_x, core_y, core_z] = core.origin().components();
    let [core_width, core_height, core_depth] = core.extent().dimensions();
    let core_end_x = core_x
        .checked_add(
            i32::try_from(core_width).map_err(|_| metadata_dimensions_error(source_revision))?,
        )
        .ok_or_else(|| metadata_dimensions_error(source_revision))?;
    let core_end_y = core_y
        .checked_add(
            i32::try_from(core_height).map_err(|_| metadata_dimensions_error(source_revision))?,
        )
        .ok_or_else(|| metadata_dimensions_error(source_revision))?;
    let core_end_z = core_z
        .checked_add(
            i32::try_from(core_depth).map_err(|_| metadata_dimensions_error(source_revision))?,
        )
        .ok_or_else(|| metadata_dimensions_error(source_revision))?;
    let face_neighbor_regions = [
        core,
        VoxelRegion::new(
            VoxelCoordinate::new(core_x - 1, core_y, core_z),
            VoxelExtent::new(1, core_height, core_depth),
        ),
        VoxelRegion::new(
            VoxelCoordinate::new(core_end_x, core_y, core_z),
            VoxelExtent::new(1, core_height, core_depth),
        ),
        VoxelRegion::new(
            VoxelCoordinate::new(core_x, core_y - 1, core_z),
            VoxelExtent::new(core_width, 1, core_depth),
        ),
        VoxelRegion::new(
            VoxelCoordinate::new(core_x, core_end_y, core_z),
            VoxelExtent::new(core_width, 1, core_depth),
        ),
        VoxelRegion::new(
            VoxelCoordinate::new(core_x, core_y, core_z - 1),
            VoxelExtent::new(core_width, core_height, 1),
        ),
        VoxelRegion::new(
            VoxelCoordinate::new(core_x, core_y, core_end_z),
            VoxelExtent::new(core_width, core_height, 1),
        ),
    ];
    let mut values = HashMap::new();
    for region in face_neighbor_regions {
        let samples = view
            .read_region(metadata.identity(), region)
            .map_err(|error| {
                build_error(
                    source_revision,
                    RasterArtifactBuildPhase::VoxelRead,
                    RasterArtifactBuildCause::VoxelRead(error),
                )
            })?;
        values.extend(
            samples
                .into_iter()
                .map(|sample| (sample.coordinate(), sample.value().clone())),
        );
    }
    let mut pending_faces = Vec::new();
    for z_offset in 0..core_depth {
        for y_offset in 0..core_height {
            for x_offset in 0..core_width {
                let coordinate = VoxelCoordinate::new(
                    core_x
                        .checked_add(
                            i32::try_from(x_offset)
                                .map_err(|_| metadata_dimensions_error(source_revision))?,
                        )
                        .ok_or_else(|| metadata_dimensions_error(source_revision))?,
                    core_y
                        .checked_add(
                            i32::try_from(y_offset)
                                .map_err(|_| metadata_dimensions_error(source_revision))?,
                        )
                        .ok_or_else(|| metadata_dimensions_error(source_revision))?,
                    core_z
                        .checked_add(
                            i32::try_from(z_offset)
                                .map_err(|_| metadata_dimensions_error(source_revision))?,
                        )
                        .ok_or_else(|| metadata_dimensions_error(source_revision))?,
                );
                let Some(VoxelValue::Occupied(material_identity)) = values.get(&coordinate) else {
                    continue;
                };
                let linear_base_color = view
                    .materials()
                    .iter()
                    .find(|material| material.identity() == material_identity)
                    .map(|material| material.linear_base_color())
                    .ok_or_else(|| {
                        build_error(
                            source_revision,
                            RasterArtifactBuildPhase::MaterialResolution,
                            RasterArtifactBuildCause::UnknownMaterial(material_identity.clone()),
                        )
                    })?;
                for normal in AXIS_NORMALS {
                    let [offset_x, offset_y, offset_z] = normal.offset();
                    let neighbor = VoxelCoordinate::new(
                        coordinate.components()[0] + offset_x,
                        coordinate.components()[1] + offset_y,
                        coordinate.components()[2] + offset_z,
                    );
                    if !matches!(values.get(&neighbor), Some(VoxelValue::Occupied(_))) {
                        pending_faces.push(PendingFace {
                            coordinate,
                            normal,
                            material_identity: material_identity.clone(),
                            linear_base_color,
                        });
                    }
                }
            }
        }
    }
    let artifact = build_geometry(
        view.scene_id(),
        source_revision,
        metadata.identity(),
        metadata,
        pending_faces,
    )?;
    let mut region = artifact.regions.into_iter().next().ok_or_else(|| {
        build_error(
            source_revision,
            RasterArtifactBuildPhase::Geometry,
            RasterArtifactBuildCause::ArithmeticOverflow,
        )
    })?;
    region.identity = RasterRegionIdentity {
        volume_identity: metadata.identity().clone(),
        core_origin: core.origin(),
    };
    region.core = core;
    Ok(region)
}

fn assemble_raster_artifact(
    scene_identity: VoxelSceneId,
    source_revision: VoxelSceneRevision,
    region_extent: VoxelExtent,
    regions: Vec<RasterRegionResult>,
) -> Result<RasterArtifact, RasterArtifactBuildError> {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    let mut semantic_faces = Vec::new();
    for region in &regions {
        let first_vertex =
            u32::try_from(vertices.len()).map_err(|_| geometry_overflow(source_revision))?;
        vertices.extend_from_slice(&region.vertices);
        for index in &region.indices {
            indices.push(
                first_vertex
                    .checked_add(*index)
                    .ok_or_else(|| geometry_overflow(source_revision))?,
            );
        }
        semantic_faces.extend_from_slice(&region.semantic_faces);
    }
    let vertex_byte_size = vertices
        .len()
        .checked_mul(size_of::<RasterVertex>())
        .ok_or_else(|| geometry_overflow(source_revision))?;
    let index_byte_size = indices
        .len()
        .checked_mul(size_of::<u32>())
        .ok_or_else(|| geometry_overflow(source_revision))?;
    Ok(RasterArtifact {
        scene_identity,
        source_revision,
        region_extent: Some(region_extent),
        volume_identity: None,
        vertices,
        indices,
        semantic_faces,
        vertex_byte_size,
        index_byte_size,
        regions,
    })
}

fn metadata_dimensions_error(source_revision: VoxelSceneRevision) -> RasterArtifactBuildError {
    build_error(
        source_revision,
        RasterArtifactBuildPhase::Metadata,
        RasterArtifactBuildCause::UnrepresentableVolumeDimensions,
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RasterPreparationDisposition {
    SupersededBeforeUpload,
    SupersededAfterUpload,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RasterConvergenceAcceptance {
    Unchanged { revision: VoxelSceneRevision },
    NotNewer { revision: VoxelSceneRevision },
    Accepted { revision: VoxelSceneRevision },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RasterConvergenceRetry {
    NoRequiredWork,
    Requested { revision: VoxelSceneRevision },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RasterConvergenceFailurePhase {
    Derivation,
    Upload,
    Commit,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RasterConvergenceFailure {
    scene_identity: VoxelSceneId,
    failed_revision: VoxelSceneRevision,
    phase: RasterConvergenceFailurePhase,
    source: String,
    region_identity: Option<RasterRegionIdentity>,
}

impl RasterConvergenceFailure {
    pub fn scene_identity(&self) -> &VoxelSceneId {
        &self.scene_identity
    }

    pub fn failed_revision(&self) -> VoxelSceneRevision {
        self.failed_revision
    }

    pub fn phase(&self) -> RasterConvergenceFailurePhase {
        self.phase
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn region_identity(&self) -> Option<&RasterRegionIdentity> {
        self.region_identity.as_ref()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RasterConvergenceEvent {
    EventsCompacted {
        discarded: u64,
    },
    CandidateRejectionsCompacted {
        first_revision: VoxelSceneRevision,
        last_revision: VoxelSceneRevision,
        discarded: u64,
        disposition: RasterPreparationDisposition,
    },
    PreparationStarted {
        revision: VoxelSceneRevision,
    },
    PreparationReady {
        revision: VoxelSceneRevision,
    },
    PreparationDiscarded {
        revision: VoxelSceneRevision,
        disposition: RasterPreparationDisposition,
    },
    CandidateUploaded {
        revision: VoxelSceneRevision,
    },
    CandidateRejected {
        revision: VoxelSceneRevision,
        disposition: RasterPreparationDisposition,
    },
    CandidateCommitted {
        revision: VoxelSceneRevision,
    },
    Failure {
        failure: RasterConvergenceFailure,
    },
}

const RASTER_CONVERGENCE_EVENT_CAPACITY: usize = 16;

struct RasterConvergenceEvents {
    retained: VecDeque<RasterConvergenceEvent>,
    discarded: u64,
    compacted_candidate_rejections: Option<RasterCompactedCandidateRejections>,
}

struct RasterCompactedCandidateRejections {
    first_revision: VoxelSceneRevision,
    last_revision: VoxelSceneRevision,
    discarded: u64,
    disposition: RasterPreparationDisposition,
}

impl RasterConvergenceEvents {
    fn new() -> Self {
        Self {
            retained: VecDeque::new(),
            discarded: 0,
            compacted_candidate_rejections: None,
        }
    }

    fn push(&mut self, event: RasterConvergenceEvent) {
        if self.retained.len() == RASTER_CONVERGENCE_EVENT_CAPACITY
            && let Some(discarded) = self.retained.pop_front()
        {
            self.record_discarded(discarded);
        }
        self.retained.push_back(event);
    }

    fn record_discarded(&mut self, event: RasterConvergenceEvent) {
        let RasterConvergenceEvent::CandidateRejected {
            revision,
            disposition,
        } = event
        else {
            self.discarded = self.discarded.saturating_add(1);
            return;
        };
        match &mut self.compacted_candidate_rejections {
            Some(compacted) => {
                compacted.last_revision = revision;
                compacted.discarded = compacted.discarded.saturating_add(1);
            }
            None => {
                self.compacted_candidate_rejections = Some(RasterCompactedCandidateRejections {
                    first_revision: revision,
                    last_revision: revision,
                    discarded: 1,
                    disposition,
                });
            }
        }
    }

    fn append(&mut self, other: &mut Self) {
        if other.discarded > 0 {
            self.discarded = self.discarded.saturating_add(other.discarded);
            other.discarded = 0;
        }
        if let Some(compacted) = other.compacted_candidate_rejections.take() {
            match &mut self.compacted_candidate_rejections {
                Some(retained) => {
                    retained.last_revision = compacted.last_revision;
                    retained.discarded = retained.discarded.saturating_add(compacted.discarded);
                }
                None => self.compacted_candidate_rejections = Some(compacted),
            }
        }
        for event in other.retained.drain(..) {
            self.push(event);
        }
    }

    fn drain(&mut self) -> Vec<RasterConvergenceEvent> {
        let mut events = Vec::new();
        if self.discarded > 0 {
            events.push(RasterConvergenceEvent::EventsCompacted {
                discarded: self.discarded,
            });
            self.discarded = 0;
        }
        if let Some(compacted) = self.compacted_candidate_rejections.take() {
            events.push(RasterConvergenceEvent::CandidateRejectionsCompacted {
                first_revision: compacted.first_revision,
                last_revision: compacted.last_revision,
                discarded: compacted.discarded,
                disposition: compacted.disposition,
            });
        }
        events.extend(self.retained.drain(..));
        events
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RasterConvergenceUpload {
    NoReadyPreparation,
    CandidateAlreadyRetained { revision: VoxelSceneRevision },
    Uploaded { revision: VoxelSceneRevision },
}

#[must_use = "retired GPU resources must be handed to safe retirement"]
struct RasterCandidateRetirement {
    resources: Vec<RasterRegionGpuResources>,
}

impl RasterCandidateRetirement {
    fn resource_count(&self) -> usize {
        self.resources.len()
    }

    /// # Safety
    ///
    /// No submitted GPU work may still reference any resource in this handoff.
    unsafe fn release_after_gpu_completion(self, device: &RenderPathDeviceContext<'_>) {
        for resources in self.resources {
            release_raster_region_resources(device, resources);
        }
    }
}

#[must_use = "the commit outcome contains resources that must be handed to safe retirement"]
enum RasterConvergenceCommit {
    NoCandidate,
    Failed {
        retirement: RasterCandidateRetirement,
    },
    Rejected {
        retirement: RasterCandidateRetirement,
    },
    Committed {
        retirement: RasterCandidateRetirement,
    },
}

type RasterResourceUploader<'uploader> = dyn FnMut(&RasterRegionResult) -> Result<RasterRegionGpuResources, RasterConvergenceError>
    + 'uploader;

#[derive(Debug, Error)]
pub enum RasterConvergenceError {
    #[error("Raster Convergence has already been started")]
    AlreadyStarted,
    #[error("Raster Convergence has not been started")]
    NotStarted,
    #[error("Raster Convergence requires a complete visible installation")]
    MissingVisibleInstallation,
    #[error("the visible installation does not support convergence")]
    UnsupportedVisibleInstallation,
    #[error(
        "changed outcome Voxel Scene identity mismatch: expected {expected:?}, change set {change_set:?}, view {view:?}"
    )]
    SceneIdentityMismatch {
        expected: VoxelSceneId,
        change_set: VoxelSceneId,
        view: VoxelSceneId,
    },
    #[error(
        "changed outcome successor mismatch: change set revision {change_set}, view revision {view}"
    )]
    SuccessorRevisionMismatch {
        change_set: VoxelSceneRevision,
        view: VoxelSceneRevision,
    },
    #[error("Raster Convergence generation identity overflow")]
    GenerationOverflow,
    #[error("visible installation revision mismatch: expected {expected}, received {actual:?}")]
    VisibleRevisionMismatch {
        expected: VoxelSceneRevision,
        actual: Option<VoxelSceneRevision>,
    },
    #[error("the visible installation is missing Raster Region {identity:?}")]
    MissingVisibleRegion { identity: RasterRegionIdentity },
    #[error("the visible Raster Region installation changed before candidate commit")]
    VisibleInstallationChanged,
    #[error("Raster Region installation generation overflow for {identity:?}")]
    InstallationGenerationOverflow { identity: RasterRegionIdentity },
    #[error("configured Raster Region resources require a device context for candidate upload")]
    ConfiguredResourcesRequireDevice,
    #[error("configured GPU resources are missing Raster Region {identity:?}")]
    MissingConfiguredRegion { identity: RasterRegionIdentity },
    #[error("Raster candidate resource bookkeeping could not be allocated")]
    ResourceBookkeepingAllocation,
    #[error("GPU upload failed for Raster Region {identity:?}: {source}")]
    Upload {
        identity: RasterRegionIdentity,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    #[error(transparent)]
    Bookkeeping(#[from] RasterArtifactBuildError),
    #[error("could not start preparation for Voxel Scene Revision {revision}: {source}")]
    PreparationStart {
        revision: VoxelSceneRevision,
        #[source]
        source: std::io::Error,
    },
    #[error("preparation terminated for Voxel Scene Revision {revision}")]
    PreparationTerminated { revision: VoxelSceneRevision },
    #[error("preparation failed for Voxel Scene Revision {revision}: {source}")]
    Preparation {
        revision: VoxelSceneRevision,
        #[source]
        source: RasterArtifactBuildError,
    },
}

#[derive(Clone)]
enum RasterPreparationTargetScope {
    Localized(HashSet<RasterRegionIdentity>),
    FullRebuild,
}

#[derive(Clone)]
struct RasterPreparationTarget {
    view: VoxelSceneView,
    region_extent: VoxelExtent,
    scope: RasterPreparationTargetScope,
}

enum RasterPreparationCompletion {
    Completed(Result<Vec<RasterRegionResult>, RasterDerivationFailure>),
    Cancelled,
}

struct RasterDerivationFailure {
    region_identity: Option<RasterRegionIdentity>,
    source: RasterArtifactBuildError,
}

enum RasterActivePreparationStatus {
    Running,
    Ready { regions: Vec<RasterRegionResult> },
}

struct RasterActivePreparation {
    generation: RasterConvergenceGeneration,
    target: RasterPreparationTarget,
    cancellation: Arc<AtomicBool>,
    completion_receiver: mpsc::Receiver<RasterPreparationCompletion>,
    worker: Option<JoinHandle<()>>,
    status: RasterActivePreparationStatus,
}

struct RasterHiddenCandidate {
    generation: RasterConvergenceGeneration,
    target: RasterPreparationTarget,
    artifact: RasterArtifact,
    installations: Vec<RasterRegionInstallation>,
    visible_installations: Vec<RasterRegionInstallation>,
    affected_regions: HashSet<RasterRegionIdentity>,
    successor_gpu_resources: Vec<RasterRegionGpuResources>,
    retired_gpu_resources: Vec<RasterRegionGpuResources>,
    configured_resources: bool,
}

#[derive(Clone, Copy, Eq, PartialEq)]
struct RasterConvergenceGeneration(u64);

impl RasterConvergenceGeneration {
    fn initial() -> Self {
        Self(0)
    }

    fn checked_successor(self) -> Option<Self> {
        self.0.checked_add(1).map(Self)
    }
}

pub struct RasterConvergence {
    scene_identity: VoxelSceneId,
    visible_revision: VoxelSceneRevision,
    required_revision: VoxelSceneRevision,
    region_extent: VoxelExtent,
    required_generation: RasterConvergenceGeneration,
    active: Option<RasterActivePreparation>,
    pending: Option<RasterPreparationTarget>,
    paused: Option<RasterPreparationTarget>,
    hidden_candidate: Option<RasterHiddenCandidate>,
    events: RasterConvergenceEvents,
}

impl RasterConvergence {
    pub fn from_visible(render_path: &RasterRenderPath) -> Result<Self, RasterConvergenceError> {
        let artifact = render_path
            .installed_artifact()
            .ok_or(RasterConvergenceError::MissingVisibleInstallation)?;
        let region_extent = artifact
            .region_extent
            .ok_or(RasterConvergenceError::UnsupportedVisibleInstallation)?;
        let scene_identity = artifact.scene_identity.clone();
        let visible_revision = artifact.source_revision();
        Ok(Self {
            scene_identity,
            visible_revision,
            required_revision: visible_revision,
            region_extent,
            required_generation: RasterConvergenceGeneration::initial(),
            active: None,
            pending: None,
            paused: None,
            hidden_candidate: None,
            events: RasterConvergenceEvents::new(),
        })
    }

    pub fn visible_revision(&self) -> VoxelSceneRevision {
        self.visible_revision
    }

    pub fn required_revision(&self) -> VoxelSceneRevision {
        self.required_revision
    }

    pub fn accept(
        &mut self,
        outcome: VoxelEditOutcome,
    ) -> Result<RasterConvergenceAcceptance, RasterConvergenceError> {
        let (view, change_set) = match outcome {
            VoxelEditOutcome::Unchanged(view) => {
                return Ok(RasterConvergenceAcceptance::Unchanged {
                    revision: view.revision(),
                });
            }
            VoxelEditOutcome::Changed { view, change_set } => (view, change_set),
        };
        if change_set.scene_identity() != &self.scene_identity
            || view.scene_id() != &self.scene_identity
        {
            return Err(RasterConvergenceError::SceneIdentityMismatch {
                expected: self.scene_identity.clone(),
                change_set: change_set.scene_identity().clone(),
                view: view.scene_id().clone(),
            });
        }
        if change_set.successor_revision() != view.revision() {
            return Err(RasterConvergenceError::SuccessorRevisionMismatch {
                change_set: change_set.successor_revision(),
                view: view.revision(),
            });
        }
        if !view.revision().is_newer_than(self.required_revision) {
            return Ok(RasterConvergenceAcceptance::NotNewer {
                revision: view.revision(),
            });
        }

        let target_scope = if change_set.predecessor_revision() == self.required_revision {
            let mut affected = match self.newest_target().map(|target| &target.scope) {
                Some(RasterPreparationTargetScope::Localized(affected)) => affected.clone(),
                Some(RasterPreparationTargetScope::FullRebuild) => HashSet::new(),
                None => HashSet::new(),
            };
            if matches!(
                self.newest_target().map(|target| &target.scope),
                Some(RasterPreparationTargetScope::FullRebuild)
            ) {
                RasterPreparationTargetScope::FullRebuild
            } else {
                affected.extend(affected_raster_region_identities(
                    &view,
                    &change_set,
                    self.region_extent,
                )?);
                RasterPreparationTargetScope::Localized(affected)
            }
        } else {
            RasterPreparationTargetScope::FullRebuild
        };
        let target = RasterPreparationTarget {
            view,
            region_extent: self.region_extent,
            scope: target_scope,
        };
        let generation = self
            .required_generation
            .checked_successor()
            .ok_or(RasterConvergenceError::GenerationOverflow)?;
        self.schedule_target(generation, target)?;
        self.required_generation = generation;
        self.required_revision = change_set.successor_revision();
        Ok(RasterConvergenceAcceptance::Accepted {
            revision: self.required_revision,
        })
    }

    pub fn request_retry(&mut self) -> Result<RasterConvergenceRetry, RasterConvergenceError> {
        let Some(target) = self.newest_target().cloned() else {
            return Ok(RasterConvergenceRetry::NoRequiredWork);
        };
        let generation = self
            .required_generation
            .checked_successor()
            .ok_or(RasterConvergenceError::GenerationOverflow)?;
        self.schedule_target(generation, target)?;
        self.required_generation = generation;
        Ok(RasterConvergenceRetry::Requested {
            revision: self.required_revision,
        })
    }

    pub fn drain_events(&mut self) -> Result<Vec<RasterConvergenceEvent>, RasterConvergenceError> {
        self.poll_preparation()?;
        Ok(self.events.drain())
    }

    fn upload_ready_with_optional_device(
        &mut self,
        device: Option<&RenderPathDeviceContext<'_>>,
        render_path: &RasterRenderPath,
    ) -> Result<RasterConvergenceUpload, RasterConvergenceError> {
        self.upload_ready_with_resource_adapter(device, render_path, None)
    }

    #[cfg(test)]
    fn upload_ready_with_test_resources(
        &mut self,
        render_path: &RasterRenderPath,
        uploader: &mut RasterResourceUploader<'_>,
    ) -> Result<RasterConvergenceUpload, RasterConvergenceError> {
        self.upload_ready_with_resource_adapter(None, render_path, Some(uploader))
    }

    fn upload_ready_with_resource_adapter(
        &mut self,
        device: Option<&RenderPathDeviceContext<'_>>,
        render_path: &RasterRenderPath,
        mut test_uploader: Option<&mut RasterResourceUploader<'_>>,
    ) -> Result<RasterConvergenceUpload, RasterConvergenceError> {
        self.poll_preparation()?;
        if let Some(candidate) = &self.hidden_candidate {
            return Ok(RasterConvergenceUpload::CandidateAlreadyRetained {
                revision: candidate.target.view.revision(),
            });
        }
        let Some(active) = self.active.as_ref() else {
            return Ok(RasterConvergenceUpload::NoReadyPreparation);
        };
        let RasterActivePreparationStatus::Ready { regions } = &active.status else {
            return Ok(RasterConvergenceUpload::NoReadyPreparation);
        };
        let revision = active.target.view.revision();
        let target = active.target.clone();
        if active.generation != self.required_generation || revision != self.required_revision {
            return Ok(RasterConvergenceUpload::NoReadyPreparation);
        }
        macro_rules! retain_after_upload_failure {
            ($result:expr, $region_identity:expr) => {
                match $result {
                    Ok(value) => value,
                    Err(error) => {
                        self.pause_after_upload_failure(
                            target,
                            revision,
                            error.to_string(),
                            $region_identity,
                        );
                        return Ok(RasterConvergenceUpload::NoReadyPreparation);
                    }
                }
            };
        }
        if render_path.installed_source_revision() != Some(self.visible_revision) {
            retain_after_upload_failure!(
                Err::<(), _>(RasterConvergenceError::VisibleRevisionMismatch {
                    expected: self.visible_revision,
                    actual: render_path.installed_source_revision(),
                }),
                None
            );
        }
        let installed_artifact = retain_after_upload_failure!(
            render_path
                .installed_artifact()
                .ok_or(RasterConvergenceError::MissingVisibleInstallation),
            None
        );
        let affected_regions = regions
            .iter()
            .map(|region| region.identity().clone())
            .collect::<HashSet<_>>();
        let successor_regions = match &active.target.scope {
            RasterPreparationTargetScope::FullRebuild => regions.clone(),
            RasterPreparationTargetScope::Localized(_) => {
                let replacements = regions
                    .iter()
                    .map(|region| (region.identity().clone(), region.clone()))
                    .collect::<HashMap<_, _>>();
                installed_artifact
                    .regions()
                    .iter()
                    .map(|region| {
                        replacements
                            .get(region.identity())
                            .cloned()
                            .unwrap_or_else(|| region.clone())
                    })
                    .collect()
            }
        };
        let artifact = retain_after_upload_failure!(
            assemble_raster_artifact(
                self.scene_identity.clone(),
                revision,
                self.region_extent,
                successor_regions,
            ),
            None
        );
        let mut installations = Vec::new();
        retain_after_upload_failure!(
            installations
                .try_reserve_exact(artifact.regions().len())
                .map_err(|_| RasterConvergenceError::ResourceBookkeepingAllocation),
            None
        );
        for region in artifact.regions() {
            let prior = render_path
                .installed_regions()
                .iter()
                .find(|installation| installation.identity() == region.identity());
            if affected_regions.contains(region.identity()) {
                let installation_generation = match prior {
                    Some(prior) => retain_after_upload_failure!(
                        prior
                            .installation_generation()
                            .checked_successor()
                            .ok_or_else(|| {
                                RasterConvergenceError::InstallationGenerationOverflow {
                                    identity: region.identity().clone(),
                                }
                            }),
                        Some(region.identity().clone())
                    ),
                    None => RasterRegionInstallationGeneration::new(1),
                };
                let mut installation = RasterRegionInstallation::new(
                    region,
                    !region.is_empty(),
                    installation_generation,
                );
                installation.activity = RasterRegionActivity {
                    scheduling_events: 1,
                    derivation_events: 1,
                    upload_events: 1,
                    replacement_events: 1,
                };
                installations.push(installation);
            } else {
                let mut installation = retain_after_upload_failure!(
                    prior
                        .cloned()
                        .ok_or_else(|| RasterConvergenceError::MissingVisibleRegion {
                            identity: region.identity().clone(),
                        }),
                    Some(region.identity().clone())
                );
                installation.activity = RasterRegionActivity::default();
                installations.push(installation);
            }
        }

        let configured_resources = !render_path.region_resources.is_empty();
        if configured_resources && device.is_none() && test_uploader.is_none() {
            retain_after_upload_failure!(
                Err::<(), _>(RasterConvergenceError::ConfiguredResourcesRequireDevice),
                None
            );
        }
        if configured_resources {
            for identity in &affected_regions {
                if !render_path
                    .region_resources
                    .iter()
                    .any(|resources| &resources.identity == identity)
                {
                    retain_after_upload_failure!(
                        Err::<(), _>(RasterConvergenceError::MissingConfiguredRegion {
                            identity: identity.clone(),
                        }),
                        Some(identity.clone())
                    );
                }
            }
        }
        let mut successor_gpu_resources = Vec::new();
        let mut retired_gpu_resources = Vec::new();
        if configured_resources {
            retain_after_upload_failure!(
                successor_gpu_resources
                    .try_reserve_exact(render_path.region_resources.len())
                    .map_err(|_| RasterConvergenceError::ResourceBookkeepingAllocation),
                None
            );
            retain_after_upload_failure!(
                retired_gpu_resources
                    .try_reserve_exact(affected_regions.len())
                    .map_err(|_| RasterConvergenceError::ResourceBookkeepingAllocation),
                None
            );
            for region in artifact
                .regions()
                .iter()
                .filter(|region| affected_regions.contains(region.identity()))
            {
                let upload = match test_uploader.as_mut() {
                    Some(uploader) => uploader(region),
                    None => match device {
                        Some(device) => {
                            upload_raster_region_resources(device, region).map_err(|source| {
                                RasterConvergenceError::Upload {
                                    identity: region.identity().clone(),
                                    source: Box::new(source),
                                }
                            })
                        }
                        None => Err(RasterConvergenceError::ConfiguredResourcesRequireDevice),
                    },
                };
                match upload {
                    Ok(resources) => successor_gpu_resources.push(resources),
                    Err(error) => {
                        if let Some(device) = device {
                            for resources in successor_gpu_resources.drain(..) {
                                release_raster_region_resources(device, resources);
                            }
                        }
                        self.pause_after_upload_failure(
                            target,
                            revision,
                            error.to_string(),
                            Some(region.identity().clone()),
                        );
                        return Ok(RasterConvergenceUpload::NoReadyPreparation);
                    }
                }
            }
        }
        let generation = active.generation;
        self.active = None;
        self.hidden_candidate = Some(RasterHiddenCandidate {
            generation,
            target,
            artifact,
            installations,
            visible_installations: render_path.installed_regions.clone(),
            affected_regions,
            successor_gpu_resources,
            retired_gpu_resources,
            configured_resources,
        });
        self.events
            .push(RasterConvergenceEvent::CandidateUploaded { revision });
        Ok(RasterConvergenceUpload::Uploaded { revision })
    }

    fn commit_at_frame_boundary(
        &mut self,
        render_path: &mut RasterRenderPath,
    ) -> Result<RasterConvergenceCommit, RasterConvergenceError> {
        let Some(candidate) = self.hidden_candidate.as_ref() else {
            return Ok(RasterConvergenceCommit::NoCandidate);
        };
        let revision = candidate.target.view.revision();
        let is_current =
            candidate.generation == self.required_generation && revision == self.required_revision;
        if !is_current {
            let candidate = self
                .hidden_candidate
                .take()
                .ok_or(RasterConvergenceError::PreparationTerminated { revision })?;
            self.events.push(RasterConvergenceEvent::CandidateRejected {
                revision,
                disposition: RasterPreparationDisposition::SupersededAfterUpload,
            });
            return Ok(RasterConvergenceCommit::Rejected {
                retirement: RasterCandidateRetirement {
                    resources: candidate.successor_gpu_resources,
                },
            });
        }
        if render_path.installed_source_revision() != Some(self.visible_revision) {
            let error = RasterConvergenceError::VisibleRevisionMismatch {
                expected: self.visible_revision,
                actual: render_path.installed_source_revision(),
            };
            return self.fail_hidden_candidate(error.to_string());
        }
        if render_path.installed_regions != candidate.visible_installations {
            return self.fail_hidden_candidate(
                RasterConvergenceError::VisibleInstallationChanged.to_string(),
            );
        }
        if candidate.configured_resources != !render_path.region_resources.is_empty() {
            return self.fail_hidden_candidate(
                RasterConvergenceError::UnsupportedVisibleInstallation.to_string(),
            );
        }
        let mut candidate = self
            .hidden_candidate
            .take()
            .ok_or(RasterConvergenceError::PreparationTerminated { revision })?;
        if candidate.configured_resources {
            for resources in std::mem::take(&mut render_path.region_resources) {
                if candidate.affected_regions.contains(&resources.identity) {
                    candidate.retired_gpu_resources.push(resources);
                } else {
                    candidate.successor_gpu_resources.push(resources);
                }
            }
            render_path.region_resources = candidate.successor_gpu_resources;
        }
        render_path.expected_source_revision = Some(revision);
        render_path.installed_source_revision = Some(revision);
        render_path.installed_regions = candidate.installations;
        render_path.artifact = Some(candidate.artifact);
        self.visible_revision = revision;
        self.events
            .push(RasterConvergenceEvent::CandidateCommitted { revision });
        Ok(RasterConvergenceCommit::Committed {
            retirement: RasterCandidateRetirement {
                resources: candidate.retired_gpu_resources,
            },
        })
    }

    fn fail_hidden_candidate(
        &mut self,
        source: String,
    ) -> Result<RasterConvergenceCommit, RasterConvergenceError> {
        let candidate =
            self.hidden_candidate
                .take()
                .ok_or(RasterConvergenceError::PreparationTerminated {
                    revision: self.required_revision,
                })?;
        let revision = candidate.target.view.revision();
        self.paused = Some(candidate.target);
        self.record_failure(
            revision,
            RasterConvergenceFailurePhase::Commit,
            source,
            None,
        );
        Ok(RasterConvergenceCommit::Failed {
            retirement: RasterCandidateRetirement {
                resources: candidate.successor_gpu_resources,
            },
        })
    }

    fn newest_target(&self) -> Option<&RasterPreparationTarget> {
        self.pending
            .as_ref()
            .or_else(|| self.active.as_ref().map(|active| &active.target))
            .or_else(|| {
                self.hidden_candidate
                    .as_ref()
                    .map(|candidate| &candidate.target)
            })
            .or(self.paused.as_ref())
    }

    fn record_failure(
        &mut self,
        failed_revision: VoxelSceneRevision,
        phase: RasterConvergenceFailurePhase,
        source: String,
        region_identity: Option<RasterRegionIdentity>,
    ) {
        self.events.push(RasterConvergenceEvent::Failure {
            failure: RasterConvergenceFailure {
                scene_identity: self.scene_identity.clone(),
                failed_revision,
                phase,
                source,
                region_identity,
            },
        });
    }

    fn pause_after_upload_failure(
        &mut self,
        target: RasterPreparationTarget,
        failed_revision: VoxelSceneRevision,
        source: String,
        region_identity: Option<RasterRegionIdentity>,
    ) {
        self.active = None;
        self.pending = None;
        self.paused = Some(target);
        self.record_failure(
            failed_revision,
            RasterConvergenceFailurePhase::Upload,
            source,
            region_identity,
        );
    }

    fn take_hidden_resources_for_release(&mut self) -> Vec<RasterRegionGpuResources> {
        let Some(candidate) = self.hidden_candidate.take() else {
            return Vec::new();
        };
        let candidate_is_current = candidate.generation == self.required_generation
            && candidate.target.view.revision() == self.required_revision;
        if candidate_is_current && self.active.is_none() && self.pending.is_none() {
            self.pending = Some(candidate.target);
        }
        candidate.successor_gpu_resources
    }

    fn active_is_ready(&self) -> bool {
        self.active.as_ref().is_some_and(|active| {
            matches!(active.status, RasterActivePreparationStatus::Ready { .. })
        })
    }

    fn schedule_target(
        &mut self,
        generation: RasterConvergenceGeneration,
        target: RasterPreparationTarget,
    ) -> Result<(), RasterConvergenceError> {
        let result = if self.active_is_ready() {
            self.replace_ready_preparation(generation, target)
        } else if let Some(active) = &self.active {
            active.cancellation.store(true, Ordering::Release);
            self.pending = Some(target);
            Ok(())
        } else {
            let preparation = Self::start_preparation(generation, target, &mut self.events)?;
            self.pending = None;
            self.active = Some(preparation);
            Ok(())
        };
        if result.is_ok() {
            self.paused = None;
        }
        result
    }

    fn discard_ready_preparation(&mut self) {
        let Some(active) = self.active.take() else {
            return;
        };
        self.events
            .push(RasterConvergenceEvent::PreparationDiscarded {
                revision: active.target.view.revision(),
                disposition: RasterPreparationDisposition::SupersededBeforeUpload,
            });
    }

    fn replace_ready_preparation(
        &mut self,
        generation: RasterConvergenceGeneration,
        target: RasterPreparationTarget,
    ) -> Result<(), RasterConvergenceError> {
        let mut started_events = RasterConvergenceEvents::new();
        let replacement = Self::start_preparation(generation, target, &mut started_events)?;
        self.discard_ready_preparation();
        self.active = Some(replacement);
        self.events.append(&mut started_events);
        Ok(())
    }

    fn start_preparation(
        generation: RasterConvergenceGeneration,
        target: RasterPreparationTarget,
        events: &mut RasterConvergenceEvents,
    ) -> Result<RasterActivePreparation, RasterConvergenceError> {
        let revision = target.view.revision();
        let cancellation = Arc::new(AtomicBool::new(false));
        let (completion_sender, completion_receiver) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name(format!("raster-convergence-{revision}"))
            .spawn({
                let target = target.clone();
                let cancellation = cancellation.clone();
                move || {
                    let completion = derive_convergence_target(&target, &cancellation);
                    if completion_sender.send(completion).is_err() {
                        eprintln!(
                            "Raster Convergence result receiver closed for Voxel Scene Revision {revision}"
                        );
                    }
                }
            })
            .map_err(|source| RasterConvergenceError::PreparationStart { revision, source })?;
        events.push(RasterConvergenceEvent::PreparationStarted { revision });
        Ok(RasterActivePreparation {
            generation,
            target,
            cancellation,
            completion_receiver,
            worker: Some(worker),
            status: RasterActivePreparationStatus::Running,
        })
    }

    fn poll_preparation(&mut self) -> Result<(), RasterConvergenceError> {
        let completion = {
            let Some(active) = self.active.as_mut() else {
                return Ok(());
            };
            if matches!(active.status, RasterActivePreparationStatus::Ready { .. }) {
                return Ok(());
            }
            match active.completion_receiver.try_recv() {
                Ok(completion) => completion,
                Err(mpsc::TryRecvError::Empty) => return Ok(()),
                Err(mpsc::TryRecvError::Disconnected) => {
                    return self.retain_target_after_termination();
                }
            }
        };
        let mut active =
            self.active
                .take()
                .ok_or(RasterConvergenceError::PreparationTerminated {
                    revision: self.required_revision,
                })?;
        let revision = active.target.view.revision();
        let worker = active
            .worker
            .take()
            .ok_or(RasterConvergenceError::PreparationTerminated { revision })?;
        if worker.join().is_err() {
            return Err(RasterConvergenceError::PreparationTerminated { revision });
        }
        let is_current =
            active.generation == self.required_generation && revision == self.required_revision;
        if let RasterPreparationCompletion::Completed(Err(failure)) = completion {
            self.record_failure(
                revision,
                RasterConvergenceFailurePhase::Derivation,
                failure.source.to_string(),
                failure.region_identity,
            );
            if is_current {
                self.pending = None;
                self.paused = Some(active.target);
            } else {
                self.start_pending_preparation()?;
            }
            return Ok(());
        }
        if !is_current || matches!(completion, RasterPreparationCompletion::Cancelled) {
            self.events
                .push(RasterConvergenceEvent::PreparationDiscarded {
                    revision,
                    disposition: RasterPreparationDisposition::SupersededBeforeUpload,
                });
            self.start_pending_preparation()?;
            return Ok(());
        }
        let RasterPreparationCompletion::Completed(result) = completion else {
            return Ok(());
        };
        let regions = result.map_err(|source| RasterConvergenceError::Preparation {
            revision,
            source: source.source,
        })?;
        active.status = RasterActivePreparationStatus::Ready { regions };
        self.active = Some(active);
        self.events
            .push(RasterConvergenceEvent::PreparationReady { revision });
        Ok(())
    }

    fn start_pending_preparation(&mut self) -> Result<(), RasterConvergenceError> {
        let Some(target) = self.pending.take() else {
            return Ok(());
        };
        let preparation =
            Self::start_preparation(self.required_generation, target.clone(), &mut self.events);
        match preparation {
            Ok(preparation) => {
                self.active = Some(preparation);
                Ok(())
            }
            Err(error) => {
                self.pending = Some(target);
                Err(error)
            }
        }
    }

    fn retain_target_after_termination(&mut self) -> Result<(), RasterConvergenceError> {
        let mut active =
            self.active
                .take()
                .ok_or(RasterConvergenceError::PreparationTerminated {
                    revision: self.required_revision,
                })?;
        let revision = active.target.view.revision();
        if let Some(worker) = active.worker.take()
            && worker.join().is_err()
        {
            eprintln!("preparation panicked for Voxel Scene Revision {revision}");
        }
        if self.pending.is_none() {
            self.pending = Some(active.target);
        }
        Err(RasterConvergenceError::PreparationTerminated { revision })
    }
}

impl Drop for RasterConvergence {
    fn drop(&mut self) {
        if let Some(active) = &self.active {
            active.cancellation.store(true, Ordering::Release);
        }
    }
}

fn derive_convergence_target(
    target: &RasterPreparationTarget,
    cancellation: &AtomicBool,
) -> RasterPreparationCompletion {
    let mut regions = Vec::new();
    let mut failed_region_identity = None;
    let traversal =
        visit_raster_region_cores(&target.view, target.region_extent, |metadata, core| {
            if cancellation.load(Ordering::Acquire) {
                return Ok(false);
            }
            let identity = RasterRegionIdentity {
                volume_identity: metadata.identity().clone(),
                core_origin: core.origin(),
            };
            let should_derive = match &target.scope {
                RasterPreparationTargetScope::Localized(affected) => affected.contains(&identity),
                RasterPreparationTargetScope::FullRebuild => true,
            };
            if should_derive {
                match derive_raster_region(&target.view, metadata, core) {
                    Ok(region) => regions.push(region),
                    Err(source) => {
                        failed_region_identity = Some(identity);
                        return Err(source);
                    }
                }
            }
            Ok(true)
        });
    match traversal {
        Ok(true) => RasterPreparationCompletion::Completed(Ok(regions)),
        Ok(false) => RasterPreparationCompletion::Cancelled,
        Err(source) => RasterPreparationCompletion::Completed(Err(RasterDerivationFailure {
            region_identity: failed_region_identity,
            source,
        })),
    }
}

fn raster_region_origin(
    source_revision: VoxelSceneRevision,
    origin_x: u32,
    origin_y: u32,
    origin_z: u32,
) -> Result<VoxelCoordinate, RasterArtifactBuildError> {
    Ok(VoxelCoordinate::new(
        i32::try_from(origin_x).map_err(|_| metadata_dimensions_error(source_revision))?,
        i32::try_from(origin_y).map_err(|_| metadata_dimensions_error(source_revision))?,
        i32::try_from(origin_z).map_err(|_| metadata_dimensions_error(source_revision))?,
    ))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RasterArtifactPreparationEvent {
    PausedAtBarrier { source_revision: VoxelSceneRevision },
    Completed { source_revision: VoxelSceneRevision },
}

#[derive(Debug, Error)]
pub enum RasterArtifactPreparationError {
    #[error("background derivation failed for Voxel Scene Revision {source_revision:?}: {source}")]
    Derivation {
        source_revision: VoxelSceneRevision,
        #[source]
        source: RasterArtifactBuildError,
    },
    #[error(
        "background preparation synchronization failed for Voxel Scene Revision {source_revision:?}"
    )]
    Synchronization { source_revision: VoxelSceneRevision },
    #[error(
        "background preparation worker terminated unexpectedly for Voxel Scene Revision {source_revision:?}"
    )]
    WorkerTerminated { source_revision: VoxelSceneRevision },
    #[error(
        "could not start background preparation for Voxel Scene Revision {source_revision:?}: {source}"
    )]
    WorkerStart {
        source_revision: VoxelSceneRevision,
        #[source]
        source: std::io::Error,
    },
}

impl RasterArtifactPreparationError {
    pub fn source_revision(&self) -> VoxelSceneRevision {
        match self {
            Self::Derivation {
                source_revision, ..
            }
            | Self::Synchronization { source_revision }
            | Self::WorkerTerminated { source_revision }
            | Self::WorkerStart {
                source_revision, ..
            } => *source_revision,
        }
    }
}

#[derive(Default)]
struct RasterPreparationBarrierState {
    reached: bool,
    released: bool,
}

struct RasterPreparationBarrierShared {
    state: Mutex<RasterPreparationBarrierState>,
    released: Condvar,
}

pub struct RasterPreparationBarrier {
    shared: Arc<RasterPreparationBarrierShared>,
}

pub struct RasterPreparationBarrierRelease {
    shared: Arc<RasterPreparationBarrierShared>,
}

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
#[error("the raster preparation barrier state is unavailable")]
pub struct RasterPreparationBarrierError;

impl RasterPreparationBarrier {
    pub fn held() -> (Self, RasterPreparationBarrierRelease) {
        let shared = Arc::new(RasterPreparationBarrierShared {
            state: Mutex::new(RasterPreparationBarrierState::default()),
            released: Condvar::new(),
        });
        (
            Self {
                shared: shared.clone(),
            },
            RasterPreparationBarrierRelease { shared },
        )
    }

    fn reach_and_wait(
        &self,
        source_revision: VoxelSceneRevision,
        notify: &impl Fn(RasterArtifactPreparationEvent),
    ) -> Result<(), RasterArtifactPreparationError> {
        let mut state = self
            .shared
            .state
            .lock()
            .map_err(|_| RasterArtifactPreparationError::Synchronization { source_revision })?;
        state.reached = true;
        notify(RasterArtifactPreparationEvent::PausedAtBarrier { source_revision });
        while !state.released {
            state =
                self.shared.released.wait(state).map_err(|_| {
                    RasterArtifactPreparationError::Synchronization { source_revision }
                })?;
        }
        Ok(())
    }
}

impl RasterPreparationBarrierRelease {
    pub fn release(&self) -> Result<(), RasterPreparationBarrierError> {
        let mut state = self
            .shared
            .state
            .lock()
            .map_err(|_| RasterPreparationBarrierError)?;
        state.released = true;
        self.shared.released.notify_one();
        Ok(())
    }

    pub fn was_reached(&self) -> Result<bool, RasterPreparationBarrierError> {
        let state = self
            .shared
            .state
            .lock()
            .map_err(|_| RasterPreparationBarrierError)?;
        Ok(state.reached)
    }
}

impl Drop for RasterPreparationBarrierRelease {
    fn drop(&mut self) {
        let mut state = match self.shared.state.lock() {
            Ok(state) => state,
            Err(poisoned) => {
                eprintln!("raster preparation barrier was poisoned during implicit release");
                poisoned.into_inner()
            }
        };
        state.released = true;
        self.shared.released.notify_one();
    }
}

pub struct RasterArtifactPreparation {
    source_revision: VoxelSceneRevision,
    result_receiver: mpsc::Receiver<Result<RasterArtifact, RasterArtifactPreparationError>>,
    worker: Option<JoinHandle<()>>,
}

impl RasterArtifactPreparation {
    pub fn start_regions(
        view: VoxelSceneView,
        region_extent: VoxelExtent,
        barrier: Option<RasterPreparationBarrier>,
        notify: impl Fn(RasterArtifactPreparationEvent) + Send + 'static,
    ) -> Result<Self, RasterArtifactPreparationError> {
        let source_revision = view.revision();
        Self::start_with_derivation(source_revision, barrier, notify, move || {
            derive_raster_regions(&view, region_extent)
        })
    }

    pub fn start(
        view: VoxelSceneView,
        volume_identity: VoxelVolumeId,
        barrier: Option<RasterPreparationBarrier>,
        notify: impl Fn(RasterArtifactPreparationEvent) + Send + 'static,
    ) -> Result<Self, RasterArtifactPreparationError> {
        let source_revision = view.revision();
        Self::start_with_derivation(source_revision, barrier, notify, move || {
            derive_raster_artifact(&view, &volume_identity)
        })
    }

    fn start_with_derivation(
        source_revision: VoxelSceneRevision,
        barrier: Option<RasterPreparationBarrier>,
        notify: impl Fn(RasterArtifactPreparationEvent) + Send + 'static,
        derive: impl FnOnce() -> Result<RasterArtifact, RasterArtifactBuildError> + Send + 'static,
    ) -> Result<Self, RasterArtifactPreparationError> {
        let (result_sender, result_receiver) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name(format!("raster-preparation-{source_revision}"))
            .spawn(move || {
                let result = (|| {
                    if let Some(barrier) = barrier {
                        barrier.reach_and_wait(source_revision, &notify)?;
                    }
                    derive().map_err(|source| {
                        RasterArtifactPreparationError::Derivation {
                            source_revision,
                            source,
                        }
                    })
                })();
                if result_sender.send(result).is_err() {
                    eprintln!(
                        "background raster preparation result receiver closed for Voxel Scene Revision {source_revision}"
                    );
                    return;
                }
                notify(RasterArtifactPreparationEvent::Completed { source_revision });
            })
            .map_err(|source| RasterArtifactPreparationError::WorkerStart {
                source_revision,
                source,
            })?;
        Ok(Self {
            source_revision,
            result_receiver,
            worker: Some(worker),
        })
    }

    pub fn source_revision(&self) -> VoxelSceneRevision {
        self.source_revision
    }

    pub fn try_complete(
        &mut self,
    ) -> Result<Option<RasterArtifact>, RasterArtifactPreparationError> {
        let result = match self.result_receiver.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return Ok(None),
            Err(mpsc::TryRecvError::Disconnected) => {
                return Err(RasterArtifactPreparationError::WorkerTerminated {
                    source_revision: self.source_revision,
                });
            }
        };
        let worker =
            self.worker
                .take()
                .ok_or(RasterArtifactPreparationError::WorkerTerminated {
                    source_revision: self.source_revision,
                })?;
        if worker.join().is_err() {
            return Err(RasterArtifactPreparationError::WorkerTerminated {
                source_revision: self.source_revision,
            });
        }
        result.map(Some)
    }
}

fn build_geometry(
    scene_identity: &VoxelSceneId,
    source_revision: VoxelSceneRevision,
    volume_identity: &VoxelVolumeId,
    metadata: &VoxelVolumeMetadata,
    pending_faces: Vec<PendingFace>,
) -> Result<RasterArtifact, RasterArtifactBuildError> {
    let face_count = pending_faces.len();
    let vertex_count = face_count
        .checked_mul(4)
        .ok_or_else(|| geometry_overflow(source_revision))?;
    let index_count = face_count
        .checked_mul(6)
        .ok_or_else(|| geometry_overflow(source_revision))?;
    let vertex_byte_size = vertex_count
        .checked_mul(size_of::<RasterVertex>())
        .ok_or_else(|| geometry_overflow(source_revision))?;
    let index_byte_size = index_count
        .checked_mul(size_of::<u32>())
        .ok_or_else(|| geometry_overflow(source_revision))?;
    u32::try_from(vertex_count).map_err(|_| {
        build_error(
            source_revision,
            RasterArtifactBuildPhase::Geometry,
            RasterArtifactBuildCause::IndexOverflow,
        )
    })?;

    let mut vertices = Vec::new();
    vertices
        .try_reserve_exact(vertex_count)
        .map_err(|_| geometry_allocation(source_revision))?;
    let mut indices = Vec::new();
    indices
        .try_reserve_exact(index_count)
        .map_err(|_| geometry_allocation(source_revision))?;
    let mut semantic_faces = Vec::new();
    semantic_faces
        .try_reserve_exact(face_count)
        .map_err(|_| geometry_allocation(source_revision))?;

    for pending_face in pending_faces {
        let first_vertex = u32::try_from(vertices.len()).map_err(|_| {
            build_error(
                source_revision,
                RasterArtifactBuildPhase::Geometry,
                RasterArtifactBuildCause::IndexOverflow,
            )
        })?;
        let positions = face_positions(metadata, pending_face.coordinate, pending_face.normal)
            .ok_or_else(|| {
                build_error(
                    source_revision,
                    RasterArtifactBuildPhase::Geometry,
                    RasterArtifactBuildCause::InvalidSceneTransform,
                )
            })?;
        for position in positions {
            vertices.push(RasterVertex {
                position,
                normal: pending_face.normal.vector(),
                linear_base_color: pending_face.linear_base_color,
            });
        }
        for local_index in [0_u32, 1, 2, 0, 2, 3] {
            indices.push(first_vertex.checked_add(local_index).ok_or_else(|| {
                build_error(
                    source_revision,
                    RasterArtifactBuildPhase::Geometry,
                    RasterArtifactBuildCause::IndexOverflow,
                )
            })?);
        }
        semantic_faces.push(SemanticFace::new(
            volume_identity.clone(),
            pending_face.coordinate,
            pending_face.normal,
            pending_face.material_identity,
        ));
    }

    let region = RasterRegionResult {
        identity: RasterRegionIdentity {
            volume_identity: volume_identity.clone(),
            core_origin: VoxelCoordinate::new(0, 0, 0),
        },
        core: VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), metadata.extent()),
        source_revision,
        vertices: vertices.clone(),
        indices: indices.clone(),
        semantic_faces: semantic_faces.clone(),
    };
    Ok(RasterArtifact {
        scene_identity: scene_identity.clone(),
        source_revision,
        region_extent: None,
        volume_identity: Some(volume_identity.clone()),
        vertices,
        indices,
        semantic_faces,
        vertex_byte_size,
        index_byte_size,
        regions: vec![region],
    })
}

fn checked_dimensions(extent: VoxelExtent) -> Option<[usize; 3]> {
    let [width, height, depth] = extent.dimensions();
    if width > i32::MAX as u32 || height > i32::MAX as u32 || depth > i32::MAX as u32 {
        return None;
    }
    Some([
        usize::try_from(width).ok()?,
        usize::try_from(height).ok()?,
        usize::try_from(depth).ok()?,
    ])
}

fn dense_index(dimensions: [usize; 3], coordinate: VoxelCoordinate) -> Option<usize> {
    let [coordinate_x, coordinate_y, coordinate_z] = coordinate.components();
    let coordinate_x = usize::try_from(coordinate_x).ok()?;
    let coordinate_y = usize::try_from(coordinate_y).ok()?;
    let coordinate_z = usize::try_from(coordinate_z).ok()?;
    let [width, height, depth] = dimensions;
    if coordinate_x >= width || coordinate_y >= height || coordinate_z >= depth {
        return None;
    }
    coordinate_z
        .checked_mul(height)?
        .checked_add(coordinate_y)?
        .checked_mul(width)?
        .checked_add(coordinate_x)
}

fn coordinate_from_index(dimensions: [usize; 3], index: usize) -> Option<VoxelCoordinate> {
    let [width, height, depth] = dimensions;
    let plane_size = width.checked_mul(height)?;
    if plane_size == 0 || index >= plane_size.checked_mul(depth)? {
        return None;
    }
    let coordinate_z = index / plane_size;
    let within_plane = index % plane_size;
    let coordinate_y = within_plane / width;
    let coordinate_x = within_plane % width;
    Some(VoxelCoordinate::new(
        i32::try_from(coordinate_x).ok()?,
        i32::try_from(coordinate_y).ok()?,
        i32::try_from(coordinate_z).ok()?,
    ))
}

fn offset_coordinate(coordinate: VoxelCoordinate, normal: AxisNormal) -> Option<VoxelCoordinate> {
    let [coordinate_x, coordinate_y, coordinate_z] = coordinate.components();
    let [offset_x, offset_y, offset_z] = normal.offset();
    Some(VoxelCoordinate::new(
        coordinate_x.checked_add(offset_x)?,
        coordinate_y.checked_add(offset_y)?,
        coordinate_z.checked_add(offset_z)?,
    ))
}

fn face_is_exposed(
    values: &[Option<VoxelValue>],
    dimensions: [usize; 3],
    coordinate: VoxelCoordinate,
    normal: AxisNormal,
) -> bool {
    !offset_coordinate(coordinate, normal)
        .and_then(|neighbor| dense_index(dimensions, neighbor))
        .and_then(|neighbor_index| values.get(neighbor_index))
        .is_some_and(|value| matches!(value, Some(VoxelValue::Occupied(_))))
}

fn face_positions(
    metadata: &VoxelVolumeMetadata,
    coordinate: VoxelCoordinate,
    normal: AxisNormal,
) -> Option<[[f32; 3]; 4]> {
    let [coordinate_x, coordinate_y, coordinate_z] = coordinate.components();
    let [origin_x, origin_y, origin_z] = metadata.scene_origin();
    let minimum_x = scene_component(origin_x, metadata.voxel_size(), coordinate_x)?;
    let minimum_y = scene_component(origin_y, metadata.voxel_size(), coordinate_y)?;
    let minimum_z = scene_component(origin_z, metadata.voxel_size(), coordinate_z)?;
    let maximum_x = scene_component(
        origin_x,
        metadata.voxel_size(),
        coordinate_x.checked_add(1)?,
    )?;
    let maximum_y = scene_component(
        origin_y,
        metadata.voxel_size(),
        coordinate_y.checked_add(1)?,
    )?;
    let maximum_z = scene_component(
        origin_z,
        metadata.voxel_size(),
        coordinate_z.checked_add(1)?,
    )?;
    Some(match normal {
        AxisNormal::NegativeX => [
            [minimum_x, minimum_y, minimum_z],
            [minimum_x, minimum_y, maximum_z],
            [minimum_x, maximum_y, maximum_z],
            [minimum_x, maximum_y, minimum_z],
        ],
        AxisNormal::PositiveX => [
            [maximum_x, minimum_y, minimum_z],
            [maximum_x, maximum_y, minimum_z],
            [maximum_x, maximum_y, maximum_z],
            [maximum_x, minimum_y, maximum_z],
        ],
        AxisNormal::NegativeY => [
            [minimum_x, minimum_y, minimum_z],
            [maximum_x, minimum_y, minimum_z],
            [maximum_x, minimum_y, maximum_z],
            [minimum_x, minimum_y, maximum_z],
        ],
        AxisNormal::PositiveY => [
            [minimum_x, maximum_y, minimum_z],
            [minimum_x, maximum_y, maximum_z],
            [maximum_x, maximum_y, maximum_z],
            [maximum_x, maximum_y, minimum_z],
        ],
        AxisNormal::NegativeZ => [
            [minimum_x, minimum_y, minimum_z],
            [minimum_x, maximum_y, minimum_z],
            [maximum_x, maximum_y, minimum_z],
            [maximum_x, minimum_y, minimum_z],
        ],
        AxisNormal::PositiveZ => [
            [minimum_x, minimum_y, maximum_z],
            [maximum_x, minimum_y, maximum_z],
            [maximum_x, maximum_y, maximum_z],
            [minimum_x, maximum_y, maximum_z],
        ],
    })
}

fn scene_component(origin: f32, voxel_size: f32, coordinate: i32) -> Option<f32> {
    let value = origin + coordinate as f32 * voxel_size;
    value.is_finite().then_some(value)
}

fn build_error(
    source_revision: VoxelSceneRevision,
    phase: RasterArtifactBuildPhase,
    cause: RasterArtifactBuildCause,
) -> RasterArtifactBuildError {
    RasterArtifactBuildError {
        phase,
        source_revision,
        cause,
    }
}

fn geometry_overflow(source_revision: VoxelSceneRevision) -> RasterArtifactBuildError {
    build_error(
        source_revision,
        RasterArtifactBuildPhase::Geometry,
        RasterArtifactBuildCause::ArithmeticOverflow,
    )
}

fn geometry_allocation(source_revision: VoxelSceneRevision) -> RasterArtifactBuildError {
    build_error(
        source_revision,
        RasterArtifactBuildPhase::Geometry,
        RasterArtifactBuildCause::AllocationFailed,
    )
}

#[derive(Debug, Error)]
enum RasterResourceError {
    #[error("no complete raster artifact is installed")]
    MissingArtifact,
    #[error(transparent)]
    ArtifactGate(#[from] RasterArtifactInstallerError),
    #[error(transparent)]
    CameraControl(#[from] RasterCameraControlError),
    #[error("injected GPU upload failure")]
    InjectedUploadFailure,
    #[error("the raster artifact index count cannot be represented for indexed drawing")]
    IndexCount,
    #[error("the Raster Vertex stride cannot be represented for graphics state")]
    VertexStride,
    #[error("could not configure the raster camera: {0}")]
    Camera(#[from] CameraConfigurationError),
    #[error("the static {0} buffer byte length cannot be represented by Vulkan")]
    BufferSize(&'static str),
    #[error("could not create the static {kind} buffer: {source}")]
    CreateBuffer {
        kind: &'static str,
        source: vk::Result,
    },
    #[error("no host-visible coherent memory type can hold the static {0} buffer")]
    MissingBufferMemory(&'static str),
    #[error("could not allocate the static {kind} buffer memory: {source}")]
    AllocateBufferMemory {
        kind: &'static str,
        source: vk::Result,
    },
    #[error("could not bind the static {kind} buffer memory: {source}")]
    BindBufferMemory {
        kind: &'static str,
        source: vk::Result,
    },
    #[error("could not upload the static {kind} data: {source}")]
    UploadBuffer {
        kind: &'static str,
        source: vk::Result,
    },
    #[error("could not create the depth image: {0}")]
    CreateDepthImage(vk::Result),
    #[error("no device-local memory type can hold the depth image")]
    MissingDepthMemory,
    #[error("could not allocate the depth image memory: {0}")]
    AllocateDepthMemory(vk::Result),
    #[error("could not bind the depth image memory: {0}")]
    BindDepthMemory(vk::Result),
    #[error("could not create the depth image view: {0}")]
    CreateDepthView(vk::Result),
    #[error("could not create the raster render pass: {0}")]
    CreateRenderPass(vk::Result),
    #[error("could not read a reproducibly built raster shader artifact: {0}")]
    ReadShaderArtifact(#[from] std::io::Error),
    #[error("could not create a raster shader module: {0}")]
    CreateShaderModule(vk::Result),
    #[error("could not create the raster graphics pipeline layout: {0}")]
    CreatePipelineLayout(vk::Result),
    #[error("could not create the raster graphics pipeline: {0}")]
    CreateGraphicsPipeline(vk::Result),
    #[error("could not create a raster framebuffer: {0}")]
    CreateFramebuffer(vk::Result),
    #[error("the frame target does not belong to the configured raster presentation target")]
    StaleFrameTarget,
    #[error("the configured raster presentation target has no framebuffer for the acquired image")]
    MissingFramebuffer,
}

struct RasterRegionGpuResources {
    identity: RasterRegionIdentity,
    vertex_buffer: vk::Buffer,
    vertex_memory: vk::DeviceMemory,
    index_buffer: vk::Buffer,
    index_memory: vk::DeviceMemory,
    index_count: u32,
}

impl RasterResourceError {
    fn installation_phase(&self) -> RasterArtifactInstallationPhase {
        match self {
            Self::CreateBuffer { .. }
            | Self::MissingBufferMemory(_)
            | Self::AllocateBufferMemory { .. }
            | Self::BindBufferMemory { .. }
            | Self::UploadBuffer { .. }
            | Self::IndexCount
            | Self::VertexStride
            | Self::BufferSize(_) => RasterArtifactInstallationPhase::Upload,
            Self::ArtifactGate(_) => RasterArtifactInstallationPhase::Upload,
            Self::InjectedUploadFailure => RasterArtifactInstallationPhase::Upload,
            Self::CameraControl(_) => RasterArtifactInstallationPhase::Record,
            _ => RasterArtifactInstallationPhase::PresentationConfiguration,
        }
    }
}

impl RenderPath for RasterRenderPath {
    fn release(&mut self, device: RenderPathDeviceContext<'_>) -> RenderPathResult<()> {
        if let Some(convergence) = &mut self.convergence {
            for resources in convergence.take_hidden_resources_for_release() {
                release_raster_region_resources(&device, resources);
            }
        }
        self.release_resources(&device);
        Ok(())
    }

    fn configure(
        &mut self,
        device: RenderPathDeviceContext<'_>,
        target: RenderPathTarget<'_>,
    ) -> RenderPathResult<()> {
        let source_revision = self
            .expected_source_revision
            .or_else(|| self.artifact.as_ref().map(RasterArtifact::source_revision))
            .ok_or_else(|| {
                Box::new(RasterResourceError::MissingArtifact)
                    as Box<dyn std::error::Error + Send + Sync>
            })?;
        let result = self
            .accept_staged_artifact()
            .and_then(|()| self.inject_upload_failure())
            .and_then(|()| self.configure_resources(&device, target))
            .and_then(|()| self.mark_artifact_installed());
        if let Err(error) = result {
            let phase = error.installation_phase();
            self.release_resources(&device);
            return Err(Box::new(RasterArtifactInstallationError::new(
                phase,
                source_revision,
                Box::new(error),
            )));
        }
        Ok(())
    }

    fn commit_frame_boundary(
        &mut self,
        device: RenderPathDeviceContext<'_>,
    ) -> RenderPathResult<()> {
        self.advance_convergence_at_frame_boundary(Some(&device))?;
        Ok(())
    }

    fn record(&mut self, frame: RenderPathFrameContext<'_>) -> RenderPathResult<()> {
        let source_revision = self
            .expected_source_revision
            .or_else(|| self.artifact.as_ref().map(RasterArtifact::source_revision))
            .ok_or(RasterResourceError::MissingArtifact)?;
        let result = self.record_frame(&frame);
        result.map_err(|error| {
            Box::new(RasterArtifactInstallationError::new(
                RasterArtifactInstallationPhase::Record,
                source_revision,
                Box::new(error),
            )) as Box<dyn std::error::Error + Send + Sync>
        })
    }
}

impl RasterRenderPath {
    fn inject_upload_failure(&self) -> Result<(), RasterResourceError> {
        if self.artifact.is_none() || self.installed_source_revision.is_some() {
            return Ok(());
        }
        let Some(installer) = &self.installation else {
            return Ok(());
        };
        let mut state = installer
            .state
            .lock()
            .map_err(|_| RasterArtifactInstallerError::StateUnavailable)?;
        if !state.inject_upload_failure {
            return Ok(());
        }
        state.inject_upload_failure = false;
        Err(RasterResourceError::InjectedUploadFailure)
    }

    fn accept_staged_artifact(&mut self) -> Result<(), RasterResourceError> {
        let Some(installer) = &self.installation else {
            return Ok(());
        };
        let mut state = installer
            .state
            .lock()
            .map_err(|_| RasterArtifactInstallerError::StateUnavailable)?;
        if let Some(artifact) = state.staged_artifact.take() {
            self.artifact = Some(artifact);
        }
        Ok(())
    }

    fn mark_artifact_installed(&mut self) -> Result<(), RasterResourceError> {
        let Some(artifact) = &self.artifact else {
            return Ok(());
        };
        let source_revision = artifact.source_revision();
        if self.expected_source_revision != Some(source_revision) {
            return Err(RasterArtifactInstallerError::RevisionMismatch {
                expected: self.expected_source_revision.unwrap_or(source_revision),
                actual: source_revision,
            }
            .into());
        }
        self.installed_source_revision = Some(source_revision);
        self.installed_regions = artifact
            .regions()
            .iter()
            .map(|region| {
                let has_gpu_resources = self.region_resources.iter().any(|resources| {
                    resources.identity == *region.identity() && resources.index_count > 0
                });
                RasterRegionInstallation::new(
                    region,
                    has_gpu_resources,
                    RasterRegionInstallationGeneration::new(1),
                )
            })
            .collect();
        if let Some(installer) = &self.installation {
            let mut state = installer
                .state
                .lock()
                .map_err(|_| RasterArtifactInstallerError::StateUnavailable)?;
            state.installed_revision = Some(source_revision);
        }
        Ok(())
    }

    fn configure_resources(
        &mut self,
        device: &RenderPathDeviceContext<'_>,
        target: RenderPathTarget<'_>,
    ) -> Result<(), RasterResourceError> {
        if let Some(artifact) = &self.artifact {
            for region in artifact.regions() {
                self.region_resources
                    .push(upload_raster_region_resources(device, region)?);
            }
        }
        self.create_depth_resources(device, target.extent())?;
        self.create_render_pass(device, target.format())?;
        self.create_graphics_pipeline(device, target.extent())?;
        for attachment in target.attachments() {
            let framebuffer = unsafe {
                device.create_framebuffer(
                    self.render_pass,
                    &attachment,
                    self.depth_view,
                    target.extent(),
                )
            }
            .map_err(RasterResourceError::CreateFramebuffer)?;
            self.framebuffers.push(framebuffer);
            self.configured_attachments.push(attachment.identity());
        }
        self.configuration_id = Some(target.configuration_id());
        self.camera_constants = self
            .camera_control
            .pose()?
            .view_projection([target.extent().width, target.extent().height])?;
        Ok(())
    }

    fn create_depth_resources(
        &mut self,
        device: &RenderPathDeviceContext<'_>,
        extent: vk::Extent2D,
    ) -> Result<(), RasterResourceError> {
        let create_info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(vk::Format::D32_SFLOAT)
            .extent(vk::Extent3D {
                width: extent.width,
                height: extent.height,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        self.depth_image = unsafe { device.create_image(&create_info) }
            .map_err(RasterResourceError::CreateDepthImage)?;
        let requirements = unsafe { device.image_memory_requirements(self.depth_image) };
        let memory_type_index = device
            .memory_type_index(
                requirements.memory_type_bits,
                vk::MemoryPropertyFlags::DEVICE_LOCAL,
            )
            .ok_or(RasterResourceError::MissingDepthMemory)?;
        let allocate_info = vk::MemoryAllocateInfo::default()
            .allocation_size(requirements.size)
            .memory_type_index(memory_type_index);
        self.depth_memory = unsafe { device.allocate_memory(&allocate_info) }
            .map_err(RasterResourceError::AllocateDepthMemory)?;
        unsafe { device.bind_image_memory(self.depth_image, self.depth_memory) }
            .map_err(RasterResourceError::BindDepthMemory)?;
        let view_info = vk::ImageViewCreateInfo::default()
            .image(self.depth_image)
            .view_type(vk::ImageViewType::TYPE_2D)
            .format(vk::Format::D32_SFLOAT)
            .subresource_range(
                vk::ImageSubresourceRange::default()
                    .aspect_mask(vk::ImageAspectFlags::DEPTH)
                    .base_mip_level(0)
                    .level_count(1)
                    .base_array_layer(0)
                    .layer_count(1),
            );
        self.depth_view = unsafe { device.create_image_view(&view_info) }
            .map_err(RasterResourceError::CreateDepthView)?;
        Ok(())
    }

    fn create_render_pass(
        &mut self,
        device: &RenderPathDeviceContext<'_>,
        color_format: vk::Format,
    ) -> Result<(), RasterResourceError> {
        let color = vk::AttachmentDescription::default()
            .format(color_format)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::STORE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .final_layout(vk::ImageLayout::PRESENT_SRC_KHR);
        let depth = vk::AttachmentDescription::default()
            .format(vk::Format::D32_SFLOAT)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::DONT_CARE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .final_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL);
        let color_reference = vk::AttachmentReference::default()
            .attachment(0)
            .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);
        let depth_reference = vk::AttachmentReference::default()
            .attachment(1)
            .layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL);
        let color_references = [color_reference];
        let subpass = vk::SubpassDescription::default()
            .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
            .color_attachments(&color_references)
            .depth_stencil_attachment(&depth_reference);
        let dependency = vk::SubpassDependency::default()
            .src_subpass(vk::SUBPASS_EXTERNAL)
            .dst_subpass(0)
            .src_stage_mask(
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT
                    | vk::PipelineStageFlags::EARLY_FRAGMENT_TESTS,
            )
            .dst_stage_mask(
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT
                    | vk::PipelineStageFlags::EARLY_FRAGMENT_TESTS,
            )
            .dst_access_mask(
                vk::AccessFlags::COLOR_ATTACHMENT_WRITE
                    | vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE,
            );
        let attachments = [color, depth];
        let subpasses = [subpass];
        let dependencies = [dependency];
        let create_info = vk::RenderPassCreateInfo::default()
            .attachments(&attachments)
            .subpasses(&subpasses)
            .dependencies(&dependencies);
        self.render_pass = unsafe { device.create_render_pass(&create_info) }
            .map_err(RasterResourceError::CreateRenderPass)?;
        Ok(())
    }

    fn create_graphics_pipeline(
        &mut self,
        device: &RenderPathDeviceContext<'_>,
        extent: vk::Extent2D,
    ) -> Result<(), RasterResourceError> {
        let vertex_code = ash::util::read_spv(&mut Cursor::new(include_bytes!(concat!(
            env!("OUT_DIR"),
            "/raster.vert.spv"
        ))))?;
        let fragment_code = ash::util::read_spv(&mut Cursor::new(include_bytes!(concat!(
            env!("OUT_DIR"),
            "/raster.frag.spv"
        ))))?;
        let vertex_module = create_shader_module(device, &vertex_code)?;
        let fragment_module = match create_shader_module(device, &fragment_code) {
            Ok(module) => module,
            Err(error) => {
                unsafe { device.destroy_shader_module(vertex_module) };
                return Err(error);
            }
        };
        let result =
            self.create_pipeline_with_modules(device, extent, vertex_module, fragment_module);
        unsafe {
            device.destroy_shader_module(fragment_module);
            device.destroy_shader_module(vertex_module);
        }
        result
    }

    fn create_pipeline_with_modules(
        &mut self,
        device: &RenderPathDeviceContext<'_>,
        extent: vk::Extent2D,
        vertex_module: vk::ShaderModule,
        fragment_module: vk::ShaderModule,
    ) -> Result<(), RasterResourceError> {
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
        let binding = [vk::VertexInputBindingDescription {
            binding: 0,
            stride: u32::try_from(size_of::<RasterVertex>())
                .map_err(|_| RasterResourceError::VertexStride)?,
            input_rate: vk::VertexInputRate::VERTEX,
        }];
        let attributes = [
            vk::VertexInputAttributeDescription {
                location: 0,
                binding: 0,
                format: vk::Format::R32G32B32_SFLOAT,
                offset: 0,
            },
            vk::VertexInputAttributeDescription {
                location: 1,
                binding: 0,
                format: vk::Format::R32G32B32_SFLOAT,
                offset: 12,
            },
            vk::VertexInputAttributeDescription {
                location: 2,
                binding: 0,
                format: vk::Format::R32G32B32A32_SFLOAT,
                offset: 24,
            },
        ];
        let vertex_input = vk::PipelineVertexInputStateCreateInfo::default()
            .vertex_binding_descriptions(&binding)
            .vertex_attribute_descriptions(&attributes);
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
            .cull_mode(vk::CullModeFlags::BACK)
            .front_face(Self::front_face())
            .line_width(1.0);
        let multisample = vk::PipelineMultisampleStateCreateInfo::default()
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);
        let depth_stencil = vk::PipelineDepthStencilStateCreateInfo::default()
            .depth_test_enable(true)
            .depth_write_enable(true)
            .depth_compare_op(vk::CompareOp::LESS);
        let color_attachment = [vk::PipelineColorBlendAttachmentState::default()
            .color_write_mask(vk::ColorComponentFlags::RGBA)];
        let color_blend =
            vk::PipelineColorBlendStateCreateInfo::default().attachments(&color_attachment);
        let push_constant_range = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::VERTEX)
            .offset(0)
            .size(64)];
        let layout_info =
            vk::PipelineLayoutCreateInfo::default().push_constant_ranges(&push_constant_range);
        self.pipeline_layout = unsafe { device.create_pipeline_layout(&layout_info) }
            .map_err(RasterResourceError::CreatePipelineLayout)?;
        let pipeline_info = vk::GraphicsPipelineCreateInfo::default()
            .stages(&stages)
            .vertex_input_state(&vertex_input)
            .input_assembly_state(&input_assembly)
            .viewport_state(&viewport_state)
            .rasterization_state(&rasterization)
            .multisample_state(&multisample)
            .depth_stencil_state(&depth_stencil)
            .color_blend_state(&color_blend)
            .layout(self.pipeline_layout)
            .render_pass(self.render_pass)
            .subpass(0);
        match unsafe {
            device.create_graphics_pipelines(vk::PipelineCache::null(), &[pipeline_info])
        } {
            Ok(mut pipelines) => {
                self.pipeline =
                    pipelines
                        .pop()
                        .ok_or(RasterResourceError::CreateGraphicsPipeline(
                            vk::Result::ERROR_UNKNOWN,
                        ))?;
                Ok(())
            }
            Err((pipelines, error)) => {
                for pipeline in pipelines {
                    unsafe { device.destroy_pipeline(pipeline) };
                }
                Err(RasterResourceError::CreateGraphicsPipeline(error))
            }
        }
    }

    fn record_frame(
        &mut self,
        frame: &RenderPathFrameContext<'_>,
    ) -> Result<(), RasterResourceError> {
        let target = frame.target();
        if self.configuration_id != Some(target.configuration_id()) {
            return Err(RasterResourceError::StaleFrameTarget);
        }
        let framebuffer_index = self
            .configured_attachments
            .iter()
            .position(|identity| *identity == target.attachment().identity())
            .ok_or(RasterResourceError::MissingFramebuffer)?;
        let framebuffer = self
            .framebuffers
            .get(framebuffer_index)
            .copied()
            .ok_or(RasterResourceError::MissingFramebuffer)?;
        let clear_values = [
            vk::ClearValue {
                color: vk::ClearColorValue {
                    float32: [0.025, 0.035, 0.06, 1.0],
                },
            },
            vk::ClearValue {
                depth_stencil: vk::ClearDepthStencilValue {
                    depth: 1.0,
                    stencil: 0,
                },
            },
        ];
        self.camera_constants = self
            .camera_control
            .pose()?
            .view_projection([target.extent().width, target.extent().height])?;
        let render_pass_info = vk::RenderPassBeginInfo::default()
            .render_pass(self.render_pass)
            .framebuffer(framebuffer)
            .render_area(vk::Rect2D {
                offset: vk::Offset2D::default(),
                extent: target.extent(),
            })
            .clear_values(&clear_values);
        unsafe {
            frame.begin_render_pass(&render_pass_info, vk::SubpassContents::INLINE);
            frame.bind_pipeline(vk::PipelineBindPoint::GRAPHICS, self.pipeline);
            for resources in &self.region_resources {
                if resources.index_count == 0 {
                    continue;
                }
                frame.bind_vertex_buffer(resources.vertex_buffer);
                frame.bind_index_buffer(resources.index_buffer);
                frame
                    .push_vertex_constants(self.pipeline_layout, f32_bytes(&self.camera_constants));
                frame.draw_indexed(resources.index_count);
            }
            frame.end_render_pass();
        }
        Ok(())
    }

    fn release_resources(&mut self, device: &RenderPathDeviceContext<'_>) {
        unsafe {
            for framebuffer in self.framebuffers.drain(..) {
                device.destroy_framebuffer(framebuffer);
            }
            if self.pipeline != vk::Pipeline::null() {
                device.destroy_pipeline(self.pipeline);
                self.pipeline = vk::Pipeline::null();
            }
            if self.pipeline_layout != vk::PipelineLayout::null() {
                device.destroy_pipeline_layout(self.pipeline_layout);
                self.pipeline_layout = vk::PipelineLayout::null();
            }
            if self.render_pass != vk::RenderPass::null() {
                device.destroy_render_pass(self.render_pass);
                self.render_pass = vk::RenderPass::null();
            }
            if self.depth_view != vk::ImageView::null() {
                device.destroy_image_view(self.depth_view);
                self.depth_view = vk::ImageView::null();
            }
            if self.depth_image != vk::Image::null() {
                device.destroy_image(self.depth_image);
                self.depth_image = vk::Image::null();
            }
            if self.depth_memory != vk::DeviceMemory::null() {
                device.free_memory(self.depth_memory);
                self.depth_memory = vk::DeviceMemory::null();
            }
            for resources in self.region_resources.drain(..) {
                release_raster_region_resources(device, resources);
            }
        }
        self.configured_attachments.clear();
        self.configuration_id = None;
    }
}

struct StaticBuffer {
    buffer: vk::Buffer,
    memory: vk::DeviceMemory,
}

fn upload_raster_region_resources(
    device: &RenderPathDeviceContext<'_>,
    region: &RasterRegionResult,
) -> Result<RasterRegionGpuResources, RasterResourceError> {
    let index_count =
        u32::try_from(region.indices().len()).map_err(|_| RasterResourceError::IndexCount)?;
    let vertex = if region.vertices().is_empty() {
        None
    } else {
        Some(create_static_buffer(
            device,
            raster_vertex_bytes(region.vertices()),
            vk::BufferUsageFlags::VERTEX_BUFFER,
            "vertex",
        )?)
    };
    let index = if region.indices().is_empty() {
        None
    } else {
        match create_static_buffer(
            device,
            u32_bytes(region.indices()),
            vk::BufferUsageFlags::INDEX_BUFFER,
            "index",
        ) {
            Ok(index) => Some(index),
            Err(error) => {
                if let Some(vertex) = vertex {
                    unsafe {
                        device.destroy_buffer(vertex.buffer);
                        device.free_memory(vertex.memory);
                    }
                }
                return Err(error);
            }
        }
    };
    Ok(RasterRegionGpuResources {
        identity: region.identity().clone(),
        vertex_buffer: vertex
            .as_ref()
            .map_or(vk::Buffer::null(), |vertex| vertex.buffer),
        vertex_memory: vertex
            .as_ref()
            .map_or(vk::DeviceMemory::null(), |vertex| vertex.memory),
        index_buffer: index
            .as_ref()
            .map_or(vk::Buffer::null(), |index| index.buffer),
        index_memory: index
            .as_ref()
            .map_or(vk::DeviceMemory::null(), |index| index.memory),
        index_count,
    })
}

fn release_raster_region_resources(
    device: &RenderPathDeviceContext<'_>,
    resources: RasterRegionGpuResources,
) {
    unsafe {
        if resources.index_buffer != vk::Buffer::null() {
            device.destroy_buffer(resources.index_buffer);
        }
        if resources.index_memory != vk::DeviceMemory::null() {
            device.free_memory(resources.index_memory);
        }
        if resources.vertex_buffer != vk::Buffer::null() {
            device.destroy_buffer(resources.vertex_buffer);
        }
        if resources.vertex_memory != vk::DeviceMemory::null() {
            device.free_memory(resources.vertex_memory);
        }
    }
}

fn create_static_buffer(
    device: &RenderPathDeviceContext<'_>,
    bytes: &[u8],
    usage: vk::BufferUsageFlags,
    kind: &'static str,
) -> Result<StaticBuffer, RasterResourceError> {
    let size = u64::try_from(bytes.len()).map_err(|_| RasterResourceError::BufferSize(kind))?;
    let create_info = vk::BufferCreateInfo::default()
        .size(size)
        .usage(usage)
        .sharing_mode(vk::SharingMode::EXCLUSIVE);
    let buffer = unsafe { device.create_buffer(&create_info) }
        .map_err(|source| RasterResourceError::CreateBuffer { kind, source })?;
    let requirements = unsafe { device.buffer_memory_requirements(buffer) };
    let memory_type_index = match device.memory_type_index(
        requirements.memory_type_bits,
        vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
    ) {
        Some(index) => index,
        None => {
            unsafe { device.destroy_buffer(buffer) };
            return Err(RasterResourceError::MissingBufferMemory(kind));
        }
    };
    let allocate_info = vk::MemoryAllocateInfo::default()
        .allocation_size(requirements.size)
        .memory_type_index(memory_type_index);
    let memory = match unsafe { device.allocate_memory(&allocate_info) } {
        Ok(memory) => memory,
        Err(source) => {
            unsafe { device.destroy_buffer(buffer) };
            return Err(RasterResourceError::AllocateBufferMemory { kind, source });
        }
    };
    if let Err(source) = unsafe { device.bind_buffer_memory(buffer, memory) } {
        unsafe {
            device.free_memory(memory);
            device.destroy_buffer(buffer);
        }
        return Err(RasterResourceError::BindBufferMemory { kind, source });
    }
    if let Err(source) = unsafe { device.write_memory(memory, bytes) } {
        unsafe {
            device.destroy_buffer(buffer);
            device.free_memory(memory);
        }
        return Err(RasterResourceError::UploadBuffer { kind, source });
    }
    Ok(StaticBuffer { buffer, memory })
}

fn create_shader_module(
    device: &RenderPathDeviceContext<'_>,
    code: &[u32],
) -> Result<vk::ShaderModule, RasterResourceError> {
    let create_info = vk::ShaderModuleCreateInfo::default().code(code);
    unsafe { device.create_shader_module(&create_info) }
        .map_err(RasterResourceError::CreateShaderModule)
}

fn raster_vertex_bytes(values: &[RasterVertex]) -> &[u8] {
    let byte_length = std::mem::size_of_val(values);
    // RasterVertex is repr(C), contains only f32 arrays, and has no padding at its checked size.
    unsafe { std::slice::from_raw_parts(values.as_ptr().cast(), byte_length) }
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod convergence_tests {
    use super::*;
    use ash::vk::Handle;
    use voxel_frontend::{
        DenseVoxelBatch, DenseVoxelScene, DenseVoxelVolume, VoxelEditCommand, VoxelFrontend,
        VoxelMaterial,
    };

    fn frontend(revision: u64, width: u32) -> Result<VoxelFrontend, Box<dyn std::error::Error>> {
        let extent = VoxelExtent::new(width, 1, 1);
        let material_identity = VoxelMaterialId::new("stone");
        let frontend = VoxelFrontend::new();
        frontend.publish(DenseVoxelScene::new(
            VoxelSceneId::new("convergence-unit"),
            VoxelSceneRevision::new(revision),
            vec![VoxelMaterial::new(material_identity, [0.2, 0.3, 0.4, 1.0])],
            vec![DenseVoxelVolume::new(
                VoxelVolumeMetadata::new(
                    VoxelVolumeId::new("terrain"),
                    extent,
                    [0.0, 0.0, 0.0],
                    1.0,
                ),
                vec![DenseVoxelBatch::new(
                    VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), extent),
                    vec![VoxelValue::Empty; usize::try_from(width)?],
                )],
            )],
        ))?;
        Ok(frontend)
    }

    fn convergence(
        frontend: &VoxelFrontend,
    ) -> Result<RasterConvergence, Box<dyn std::error::Error>> {
        let render_path = render_path(frontend)?;
        Ok(RasterConvergence::from_visible(&render_path)?)
    }

    fn render_path(
        frontend: &VoxelFrontend,
    ) -> Result<RasterRenderPath, Box<dyn std::error::Error>> {
        let mut render_path = RasterRenderPath::new();
        render_path.install_artifact(derive_raster_regions(
            &frontend.scene_view()?,
            VoxelExtent::new(1, 1, 1),
        )?);
        Ok(render_path)
    }

    fn changed(frontend: &VoxelFrontend, x: i32) -> Result<VoxelEditOutcome, VoxelFrontendError> {
        frontend.edit(VoxelEditCommand::new(
            VoxelVolumeId::new("terrain"),
            VoxelCoordinate::new(x, 0, 0),
            VoxelValue::Occupied(VoxelMaterialId::new("stone")),
        ))
    }

    fn wait_until_ready(
        convergence: &mut RasterConvergence,
    ) -> Result<(), Box<dyn std::error::Error>> {
        for _ in 0..10_000 {
            if convergence
                .drain_events()?
                .iter()
                .any(|event| matches!(event, RasterConvergenceEvent::PreparationReady { .. }))
            {
                return Ok(());
            }
            thread::yield_now();
        }
        Err("preparation did not become ready".into())
    }

    fn wait_until_ready_without_draining(
        convergence: &mut RasterConvergence,
    ) -> Result<(), Box<dyn std::error::Error>> {
        for _ in 0..10_000 {
            convergence.poll_preparation()?;
            if convergence.active_is_ready() {
                return Ok(());
            }
            thread::yield_now();
        }
        Err("preparation did not become ready".into())
    }

    fn fake_resources(
        identity: RasterRegionIdentity,
        raw_identity: u64,
    ) -> RasterRegionGpuResources {
        RasterRegionGpuResources {
            identity,
            vertex_buffer: vk::Buffer::from_raw(raw_identity),
            vertex_memory: vk::DeviceMemory::from_raw(raw_identity + 1_000),
            index_buffer: vk::Buffer::from_raw(raw_identity + 2_000),
            index_memory: vk::DeviceMemory::from_raw(raw_identity + 3_000),
            index_count: 6,
        }
    }

    #[test]
    fn private_target_bookkeeping_accumulates_adjacent_work_and_marks_discontinuities()
    -> Result<(), Box<dyn std::error::Error>> {
        let adjacent_frontend = frontend(10, 4)?;
        let discontinuous_frontend = frontend(30, 4)?;
        let mut convergence = convergence(&adjacent_frontend)?;

        convergence.accept(changed(&adjacent_frontend, 0)?)?;
        convergence.accept(changed(&adjacent_frontend, 3)?)?;
        let localized = convergence
            .pending
            .as_ref()
            .ok_or("missing newest pending target")?;
        let RasterPreparationTargetScope::Localized(affected) = &localized.scope else {
            return Err("adjacent outcomes did not retain localized work".into());
        };
        assert!(affected.len() >= 2);

        convergence.accept(changed(&discontinuous_frontend, 1)?)?;
        assert!(matches!(
            convergence
                .pending
                .as_ref()
                .ok_or("missing discontinuous pending target")?
                .scope,
            RasterPreparationTargetScope::FullRebuild
        ));
        Ok(())
    }

    #[test]
    fn failed_preparation_clears_dead_active_state_and_retry_progresses()
    -> Result<(), Box<dyn std::error::Error>> {
        let frontend = frontend(40, 1)?;
        let mut convergence = convergence(&frontend)?;
        convergence.accept(changed(&frontend, 0)?)?;
        wait_until_ready(&mut convergence)?;

        let active = convergence
            .active
            .as_mut()
            .ok_or("missing ready preparation")?;
        let (completion_sender, completion_receiver) = mpsc::sync_channel(1);
        completion_sender.send(RasterPreparationCompletion::Completed(Err(
            RasterDerivationFailure {
                region_identity: None,
                source: metadata_dimensions_error(VoxelSceneRevision::new(41)),
            },
        )))?;
        active.status = RasterActivePreparationStatus::Running;
        active.completion_receiver = completion_receiver;
        active.worker = Some(thread::spawn(|| {}));

        let events = convergence.drain_events()?;
        assert!(events.iter().any(|event| matches!(
            event,
            RasterConvergenceEvent::Failure { failure }
                if failure.scene_identity() == &VoxelSceneId::new("convergence-unit")
                    && failure.failed_revision() == VoxelSceneRevision::new(41)
                    && failure.phase() == RasterConvergenceFailurePhase::Derivation
                    && failure.region_identity().is_none()
                    && failure.source().contains("dimensions")
        )));
        assert!(convergence.active.is_none());
        assert!(convergence.pending.is_none());
        assert_eq!(
            convergence.request_retry()?,
            RasterConvergenceRetry::Requested {
                revision: VoxelSceneRevision::new(41),
            }
        );
        wait_until_ready(&mut convergence)?;
        Ok(())
    }

    #[test]
    fn terminated_preparation_clears_dead_active_state_and_retry_progresses()
    -> Result<(), Box<dyn std::error::Error>> {
        let frontend = frontend(50, 1)?;
        let mut convergence = convergence(&frontend)?;
        convergence.accept(changed(&frontend, 0)?)?;
        wait_until_ready(&mut convergence)?;

        let active = convergence
            .active
            .as_mut()
            .ok_or("missing ready preparation")?;
        let (completion_sender, completion_receiver) = mpsc::channel();
        drop(completion_sender);
        active.status = RasterActivePreparationStatus::Running;
        active.completion_receiver = completion_receiver;

        assert!(matches!(
            convergence.drain_events(),
            Err(RasterConvergenceError::PreparationTerminated { .. })
        ));
        assert!(convergence.active.is_none());
        convergence.request_retry()?;
        wait_until_ready(&mut convergence)?;
        Ok(())
    }

    #[test]
    fn frame_boundary_hook_commits_all_affected_entries_and_visible_revision_together()
    -> Result<(), Box<dyn std::error::Error>> {
        let frontend = frontend(100, 3)?;
        let mut render_path = render_path(&frontend)?;
        let before = render_path.installed_regions().to_vec();
        render_path.begin_convergence()?;
        render_path.accept_edit_outcome(changed(&frontend, 0)?)?;
        wait_until_ready_without_draining(
            render_path
                .convergence
                .as_mut()
                .ok_or("missing convergence")?,
        )?;

        let mut convergence = render_path
            .convergence
            .take()
            .ok_or("missing convergence")?;
        assert!(matches!(
            convergence.upload_ready_with_optional_device(None, &render_path)?,
            RasterConvergenceUpload::Uploaded { revision }
                if revision == VoxelSceneRevision::new(101)
        ));
        render_path.convergence = Some(convergence);
        assert_eq!(
            render_path.visible_revision(),
            Some(VoxelSceneRevision::new(100))
        );
        assert_eq!(
            render_path.installed_source_revision(),
            Some(VoxelSceneRevision::new(100))
        );

        render_path.advance_convergence_at_frame_boundary(None)?;
        assert_eq!(
            render_path.visible_revision(),
            Some(VoxelSceneRevision::new(101))
        );
        assert_eq!(
            render_path.installed_source_revision(),
            Some(VoxelSceneRevision::new(101))
        );
        assert!(
            render_path
                .installed_artifact()
                .ok_or("missing committed artifact")?
                .regions()
                .iter()
                .filter(|region| region.identity().core_origin() != VoxelCoordinate::new(2, 0, 0))
                .all(|region| region.source_revision() == VoxelSceneRevision::new(101))
        );
        for installation in render_path.installed_regions() {
            let prior = before
                .iter()
                .find(|prior| prior.identity() == installation.identity())
                .ok_or("missing prior installation")?;
            if installation.identity().core_origin() == VoxelCoordinate::new(2, 0, 0) {
                assert_eq!(installation, prior);
            } else {
                assert_eq!(
                    installation.installation_generation(),
                    prior
                        .installation_generation()
                        .checked_successor()
                        .ok_or("test installation generation overflow")?
                );
            }
        }
        Ok(())
    }

    #[test]
    fn configured_candidate_swaps_affected_resources_and_preserves_unaffected_resources()
    -> Result<(), Box<dyn std::error::Error>> {
        let frontend = frontend(110, 3)?;
        let mut render_path = render_path(&frontend)?;
        render_path.region_resources = render_path
            .installed_regions()
            .iter()
            .enumerate()
            .map(|(index, installation)| {
                Ok(fake_resources(
                    installation.identity().clone(),
                    u64::try_from(index)? + 10,
                ))
            })
            .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;
        let unaffected_identity = RasterRegionIdentity {
            volume_identity: VoxelVolumeId::new("terrain"),
            core_origin: VoxelCoordinate::new(2, 0, 0),
        };
        let unaffected_buffer = render_path
            .region_resources
            .iter()
            .find(|resources| resources.identity == unaffected_identity)
            .ok_or("missing unaffected resource")?
            .vertex_buffer;
        let before_installations = render_path.installed_regions().to_vec();
        let mut convergence = RasterConvergence::from_visible(&render_path)?;
        convergence.accept(changed(&frontend, 0)?)?;
        wait_until_ready_without_draining(&mut convergence)?;
        let mut next_resource_identity = 100_u64;
        convergence.upload_ready_with_test_resources(&render_path, &mut |region| {
            let resources = fake_resources(region.identity().clone(), next_resource_identity);
            next_resource_identity += 1;
            Ok(resources)
        })?;
        assert!(matches!(
            convergence.upload_ready_with_test_resources(&render_path, &mut |_| {
                Err(RasterConvergenceError::ResourceBookkeepingAllocation)
            })?,
            RasterConvergenceUpload::CandidateAlreadyRetained { .. }
        ));
        assert_eq!(
            render_path.installed_source_revision(),
            Some(VoxelSceneRevision::new(110))
        );

        let RasterConvergenceCommit::Committed { retirement, .. } =
            convergence.commit_at_frame_boundary(&mut render_path)?
        else {
            return Err("configured candidate was not committed".into());
        };
        assert_eq!(retirement.resource_count(), 2);
        assert_eq!(
            render_path
                .region_resources
                .iter()
                .find(|resources| resources.identity == unaffected_identity)
                .ok_or("missing retained unaffected resource")?
                .vertex_buffer,
            unaffected_buffer
        );
        let unaffected_installation = render_path
            .installed_regions()
            .iter()
            .find(|installation| installation.identity() == &unaffected_identity)
            .ok_or("missing unaffected installation")?;
        let prior_unaffected = before_installations
            .iter()
            .find(|installation| installation.identity() == &unaffected_identity)
            .ok_or("missing prior unaffected installation")?;
        assert_eq!(unaffected_installation, prior_unaffected);
        assert!(render_path.region_resources.iter().all(|resources| {
            resources.identity == unaffected_identity || resources.vertex_buffer.as_raw() >= 100
        }));
        Ok(())
    }

    #[test]
    fn upload_failure_pauses_the_required_target_and_preserves_the_visible_installation()
    -> Result<(), Box<dyn std::error::Error>> {
        let frontend = frontend(150, 1)?;
        let mut render_path = render_path(&frontend)?;
        render_path.region_resources = render_path
            .installed_regions()
            .iter()
            .map(|installation| fake_resources(installation.identity().clone(), 10))
            .collect();
        let visible_installations = render_path.installed_regions().to_vec();
        let mut convergence = RasterConvergence::from_visible(&render_path)?;
        convergence.accept(changed(&frontend, 0)?)?;
        wait_until_ready_without_draining(&mut convergence)?;

        assert!(matches!(
            convergence.upload_ready_with_test_resources(&render_path, &mut |_| {
                Err(RasterConvergenceError::ResourceBookkeepingAllocation)
            })?,
            RasterConvergenceUpload::NoReadyPreparation
        ));
        assert_eq!(render_path.installed_regions(), visible_installations);
        assert!(convergence.active.is_none());
        assert!(convergence.pending.is_none());
        assert!(convergence.events.retained.iter().any(|event| matches!(
            event,
            RasterConvergenceEvent::Failure { failure }
                if failure.scene_identity() == &VoxelSceneId::new("convergence-unit")
                    && failure.failed_revision() == VoxelSceneRevision::new(151)
                    && failure.phase() == RasterConvergenceFailurePhase::Upload
                    && failure.region_identity().is_some()
                    && failure.source().contains("resource bookkeeping")
        )));
        assert_eq!(
            convergence.request_retry()?,
            RasterConvergenceRetry::Requested {
                revision: VoxelSceneRevision::new(151)
            }
        );
        wait_until_ready_without_draining(&mut convergence)?;
        convergence.upload_ready_with_test_resources(&render_path, &mut |region| {
            Ok(fake_resources(region.identity().clone(), 60))
        })?;
        let RasterConvergenceCommit::Committed { retirement } =
            convergence.commit_at_frame_boundary(&mut render_path)?
        else {
            return Err("retried complete view was not committed".into());
        };
        assert_eq!(retirement.resource_count(), 1);
        assert_eq!(
            render_path.installed_source_revision(),
            Some(VoxelSceneRevision::new(151))
        );
        Ok(())
    }

    #[test]
    fn installation_generation_overflow_uses_the_contextual_upload_failure_path()
    -> Result<(), Box<dyn std::error::Error>> {
        let frontend = frontend(155, 1)?;
        let mut render_path = render_path(&frontend)?;
        let installation = render_path
            .installed_regions
            .get_mut(0)
            .ok_or("missing visible installation")?;
        installation.installation_generation = RasterRegionInstallationGeneration::new(u64::MAX);
        let visible_installations = render_path.installed_regions().to_vec();
        let mut convergence = RasterConvergence::from_visible(&render_path)?;
        convergence.accept(changed(&frontend, 0)?)?;
        wait_until_ready_without_draining(&mut convergence)?;

        assert!(matches!(
            convergence.upload_ready_with_optional_device(None, &render_path)?,
            RasterConvergenceUpload::NoReadyPreparation
        ));
        assert_eq!(render_path.installed_regions(), visible_installations);
        assert!(convergence.active.is_none());
        assert!(convergence.paused.is_some());
        assert!(convergence.events.retained.iter().any(|event| matches!(
            event,
            RasterConvergenceEvent::Failure { failure }
                if failure.failed_revision() == VoxelSceneRevision::new(156)
                    && failure.phase() == RasterConvergenceFailurePhase::Upload
                    && failure.region_identity().is_some()
                    && failure.source().contains("generation overflow")
        )));
        Ok(())
    }

    #[test]
    fn missing_configured_region_uses_the_contextual_upload_failure_path()
    -> Result<(), Box<dyn std::error::Error>> {
        let frontend = frontend(157, 2)?;
        let mut render_path = render_path(&frontend)?;
        let retained_region = render_path
            .installed_regions()
            .first()
            .ok_or("missing first visible installation")?;
        render_path.region_resources = vec![fake_resources(retained_region.identity().clone(), 40)];
        let mut convergence = RasterConvergence::from_visible(&render_path)?;
        convergence.accept(changed(&frontend, 1)?)?;
        wait_until_ready_without_draining(&mut convergence)?;

        assert!(matches!(
            convergence.upload_ready_with_test_resources(&render_path, &mut |region| {
                Ok(fake_resources(region.identity().clone(), 50))
            })?,
            RasterConvergenceUpload::NoReadyPreparation
        ));
        assert!(convergence.active.is_none());
        assert!(convergence.paused.is_some());
        assert!(convergence.events.retained.iter().any(|event| matches!(
            event,
            RasterConvergenceEvent::Failure { failure }
                if failure.failed_revision() == VoxelSceneRevision::new(158)
                    && failure.phase() == RasterConvergenceFailurePhase::Upload
                    && failure.region_identity().is_some()
                    && failure.source().contains("configured GPU resources are missing")
        )));
        Ok(())
    }

    #[test]
    fn commit_failure_discards_the_hidden_candidate_and_pauses_the_required_target()
    -> Result<(), Box<dyn std::error::Error>> {
        let frontend = frontend(160, 1)?;
        let mut render_path = render_path(&frontend)?;
        render_path.region_resources = render_path
            .installed_regions()
            .iter()
            .map(|installation| fake_resources(installation.identity().clone(), 20))
            .collect();
        let visible_installations = render_path.installed_regions().to_vec();
        let mut convergence = RasterConvergence::from_visible(&render_path)?;
        convergence.accept(changed(&frontend, 0)?)?;
        wait_until_ready_without_draining(&mut convergence)?;
        convergence.upload_ready_with_test_resources(&render_path, &mut |region| {
            Ok(fake_resources(region.identity().clone(), 30))
        })?;
        convergence
            .hidden_candidate
            .as_mut()
            .ok_or("missing hidden candidate")?
            .visible_installations
            .clear();

        let RasterConvergenceCommit::Failed { retirement } =
            convergence.commit_at_frame_boundary(&mut render_path)?
        else {
            return Err("commit failure did not return candidate resources for retirement".into());
        };
        assert_eq!(retirement.resource_count(), 1);
        assert_eq!(render_path.installed_regions(), visible_installations);
        assert!(convergence.hidden_candidate.is_none());
        assert!(convergence.events.retained.iter().any(|event| matches!(
            event,
            RasterConvergenceEvent::Failure { failure }
                if failure.failed_revision() == VoxelSceneRevision::new(161)
                    && failure.phase() == RasterConvergenceFailurePhase::Commit
                    && failure.source().contains("changed before candidate commit")
        )));
        assert_eq!(
            convergence.request_retry()?,
            RasterConvergenceRetry::Requested {
                revision: VoxelSceneRevision::new(161)
            }
        );
        Ok(())
    }

    #[test]
    fn late_older_derivation_failure_is_observable_without_pausing_newer_convergence()
    -> Result<(), Box<dyn std::error::Error>> {
        let frontend = frontend(170, 2)?;
        let mut convergence = convergence(&frontend)?;
        convergence.accept(changed(&frontend, 0)?)?;
        wait_until_ready_without_draining(&mut convergence)?;

        let active = convergence
            .active
            .as_mut()
            .ok_or("missing older preparation")?;
        let (completion_sender, completion_receiver) = mpsc::sync_channel(1);
        completion_sender.send(RasterPreparationCompletion::Completed(Err(
            RasterDerivationFailure {
                region_identity: Some(RasterRegionIdentity {
                    volume_identity: VoxelVolumeId::new("terrain"),
                    core_origin: VoxelCoordinate::new(0, 0, 0),
                }),
                source: metadata_dimensions_error(VoxelSceneRevision::new(171)),
            },
        )))?;
        active.status = RasterActivePreparationStatus::Running;
        active.completion_receiver = completion_receiver;
        active.worker = Some(thread::spawn(|| {}));
        convergence.accept(changed(&frontend, 1)?)?;

        let events = convergence.drain_events()?;
        assert!(events.iter().any(|event| matches!(
            event,
            RasterConvergenceEvent::Failure { failure }
                if failure.failed_revision() == VoxelSceneRevision::new(171)
                    && failure.phase() == RasterConvergenceFailurePhase::Derivation
                    && failure.region_identity().is_some()
        )));
        wait_until_ready_without_draining(&mut convergence)?;
        assert_eq!(
            convergence.required_revision(),
            VoxelSceneRevision::new(172)
        );
        assert!(convergence.paused.is_none());
        Ok(())
    }

    #[test]
    fn newer_changed_outcome_resumes_after_the_older_requirement_was_paused()
    -> Result<(), Box<dyn std::error::Error>> {
        let frontend = frontend(180, 2)?;
        let mut convergence = convergence(&frontend)?;
        convergence.accept(changed(&frontend, 0)?)?;
        wait_until_ready_without_draining(&mut convergence)?;
        let active = convergence
            .active
            .as_mut()
            .ok_or("missing older preparation")?;
        let (completion_sender, completion_receiver) = mpsc::sync_channel(1);
        completion_sender.send(RasterPreparationCompletion::Completed(Err(
            RasterDerivationFailure {
                region_identity: None,
                source: metadata_dimensions_error(VoxelSceneRevision::new(181)),
            },
        )))?;
        active.status = RasterActivePreparationStatus::Running;
        active.completion_receiver = completion_receiver;
        active.worker = Some(thread::spawn(|| {}));
        convergence.drain_events()?;
        assert!(convergence.paused.is_some());

        convergence.accept(changed(&frontend, 1)?)?;
        wait_until_ready_without_draining(&mut convergence)?;
        assert_eq!(
            convergence.required_revision(),
            VoxelSceneRevision::new(182)
        );
        assert!(convergence.paused.is_none());
        Ok(())
    }

    #[test]
    fn superseded_candidate_is_rejected_and_event_retention_stays_bounded()
    -> Result<(), Box<dyn std::error::Error>> {
        let frontend = frontend(200, 24)?;
        let mut render_path = render_path(&frontend)?;
        let mut convergence = RasterConvergence::from_visible(&render_path)?;

        convergence.accept(changed(&frontend, 0)?)?;
        wait_until_ready_without_draining(&mut convergence)?;
        convergence.upload_ready_with_optional_device(None, &render_path)?;
        convergence.accept(changed(&frontend, 1)?)?;
        assert!(matches!(
            convergence.commit_at_frame_boundary(&mut render_path)?,
            RasterConvergenceCommit::Rejected { .. }
        ));
        assert!(convergence.events.retained.iter().any(|event| matches!(
            event,
            RasterConvergenceEvent::CandidateRejected { revision, .. }
                if *revision == VoxelSceneRevision::new(201)
        )));
        assert_eq!(convergence.visible_revision(), VoxelSceneRevision::new(200));

        for coordinate in 2..24 {
            convergence.accept(changed(&frontend, coordinate)?)?;
            wait_until_ready_without_draining(&mut convergence)?;
            convergence.upload_ready_with_optional_device(None, &render_path)?;
            let commit = convergence.commit_at_frame_boundary(&mut render_path)?;
            if !matches!(commit, RasterConvergenceCommit::Committed { .. }) {
                return Err("newest candidate was not committed".into());
            }
        }
        assert_eq!(
            convergence.events.retained.len(),
            RASTER_CONVERGENCE_EVENT_CAPACITY
        );
        let events = convergence.drain_events()?;
        assert!(matches!(
            events.first(),
            Some(RasterConvergenceEvent::EventsCompacted { discarded }) if *discarded > 0
        ));
        assert!(events.iter().any(|event| matches!(
            event,
            RasterConvergenceEvent::CandidateRejectionsCompacted {
                first_revision,
                last_revision,
                discarded: 1,
                disposition: RasterPreparationDisposition::SupersededAfterUpload,
            } if *first_revision == VoxelSceneRevision::new(201)
                && *last_revision == VoxelSceneRevision::new(201)
        )));
        assert!(
            events
                .iter()
                .any(|event| matches!(event, RasterConvergenceEvent::CandidateCommitted { .. }))
        );
        Ok(())
    }

    #[test]
    fn newer_generation_rejects_a_same_revision_candidate() -> Result<(), Box<dyn std::error::Error>>
    {
        let frontend = frontend(300, 1)?;
        let mut render_path = render_path(&frontend)?;
        let mut convergence = RasterConvergence::from_visible(&render_path)?;
        convergence.accept(changed(&frontend, 0)?)?;
        wait_until_ready_without_draining(&mut convergence)?;
        convergence.upload_ready_with_optional_device(None, &render_path)?;
        assert_eq!(
            convergence.request_retry()?,
            RasterConvergenceRetry::Requested {
                revision: VoxelSceneRevision::new(301),
            }
        );

        assert!(matches!(
            convergence.commit_at_frame_boundary(&mut render_path)?,
            RasterConvergenceCommit::Rejected { .. }
        ));
        assert_eq!(convergence.visible_revision(), VoxelSceneRevision::new(300));
        assert_eq!(
            render_path.installed_source_revision(),
            Some(VoxelSceneRevision::new(300))
        );
        Ok(())
    }
}

fn u32_bytes(values: &[u32]) -> &[u8] {
    let byte_length = std::mem::size_of_val(values);
    // Every u32 bit pattern is initialized data and valid to read as bytes.
    unsafe { std::slice::from_raw_parts(values.as_ptr().cast(), byte_length) }
}

fn f32_bytes(values: &[f32]) -> &[u8] {
    let byte_length = std::mem::size_of_val(values);
    // Every f32 bit pattern is initialized data and valid to read as bytes.
    unsafe { std::slice::from_raw_parts(values.as_ptr().cast(), byte_length) }
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

fn interpolate_vector(start: [f32; 3], end: [f32; 3], progress: f32) -> [f32; 3] {
    let [start_x, start_y, start_z] = start;
    let [end_x, end_y, end_z] = end;
    [
        interpolate_scalar(start_x, end_x, progress),
        interpolate_scalar(start_y, end_y, progress),
        interpolate_scalar(start_z, end_z, progress),
    ]
}

fn interpolate_scalar(start: f32, end: f32, progress: f32) -> f32 {
    start + (end - start) * progress
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

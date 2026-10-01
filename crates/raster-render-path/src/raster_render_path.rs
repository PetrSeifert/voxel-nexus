#![cfg_attr(
    not(feature = "qualification"),
    doc = r"Failure injection and deterministic holds require the `qualification` feature.

```compile_fail
use raster_render_path::RasterLifecycleController;
let hook = RasterLifecycleController::hold_next_cpu_generation_after_regions;
```

```compile_fail
use raster_render_path::RasterLifecycleController;
let hook = RasterLifecycleController::release_cpu_barrier;
```

```compile_fail
use raster_render_path::RasterArtifactInstaller;
let hook = RasterArtifactInstaller::inject_next_upload_failure;
```

```compile_fail
use raster_render_path::RasterPreparationBarrier;
let hook = RasterPreparationBarrier::held;
```

```compile_fail
use raster_render_path::RasterRenderPathAdapter;
let hook = RasterRenderPathAdapter::enable_lifecycle_control_with_hold;
```
```compile_fail
use raster_render_path::RasterPreparationBarrierRelease;
```
"
)]

#[cfg(any(test, feature = "qualification"))]
mod raster_preparation_barrier;
#[cfg(any(test, feature = "qualification"))]
pub use raster_preparation_barrier::{
    RasterPreparationBarrier, RasterPreparationBarrierError, RasterPreparationBarrierRelease,
};

use ash::vk;
pub use render_backend::{
    CameraConfigurationError, CameraState as CameraPose, DeterministicCameraMove,
};
use render_backend::{
    CameraStateRevision, PresentationConfigurationId, RenderPath, RenderPathAttachmentIdentity,
    RenderPathDeviceContext, RenderPathFrameContext, RenderPathReadiness, RenderPathResult,
    RenderPathRetirement, RenderPathStamp, RenderPathStrategy, RenderPathTarget,
    SwitchableRenderPath,
};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use voxel_frontend::{
    VoxelChangeSet, VoxelEditOutcome, VoxelResidencySelection, VoxelSceneId, VoxelSceneRevision,
    VoxelSceneView,
};

pub const RASTER_STRATEGY: RenderPathStrategy = RenderPathStrategy::new("voxel-nexus.raster");

fn raster_gpu_resource_usage<'resources>(
    resources: impl IntoIterator<Item = &'resources RasterRegionGpuResources>,
) -> Result<RasterGpuResourceUsage, RasterLifecycleControlError> {
    let mut usage = RasterGpuResourceUsage::default();
    for region_resources in resources {
        usage.bytes = usage
            .bytes
            .checked_add(region_resources.vertex_buffer_bytes)
            .and_then(|bytes| bytes.checked_add(region_resources.index_buffer_bytes))
            .and_then(|bytes| bytes.checked_add(region_resources.material_buffer_bytes))
            .ok_or(RasterLifecycleControlError)?;
        usage.resources = usage
            .resources
            .checked_add(usize::from(
                region_resources.vertex_buffer != vk::Buffer::null(),
            ))
            .and_then(|count| {
                count.checked_add(usize::from(
                    region_resources.index_buffer != vk::Buffer::null(),
                ))
            })
            .and_then(|count| {
                count.checked_add(usize::from(
                    region_resources.material_buffer != vk::Buffer::null(),
                ))
            })
            .ok_or(RasterLifecycleControlError)?;
    }
    Ok(usage)
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
    camera_eye_and_far_clip: [f32; 4],
    camera_presentation: [f32; 4],
    background: [f32; 4],
    acknowledged_camera_revision: CameraStateRevision,
    installed_regions: Vec<RasterRegionInstallation>,
    convergence: Option<RasterConvergence>,
    lifecycle_control: Option<RasterLifecycleController>,
}

pub struct RasterRenderPathAdapter {
    render_path: RasterRenderPath,
    scene_identity: VoxelSceneId,
    initial_revision: VoxelSceneRevision,
    semantic_face_controller: Option<RasterSemanticFaceController>,
}

impl RasterRenderPathAdapter {
    pub fn awaiting_artifact_with_camera_control(
        camera_pose: CameraPose,
        camera_state_revision: CameraStateRevision,
        scene_identity: VoxelSceneId,
        expected_source_revision: VoxelSceneRevision,
    ) -> (Self, RasterArtifactInstaller, RasterCameraController) {
        let (render_path, artifact_installer, camera_controller) =
            RasterRenderPath::awaiting_artifact_with_camera_revision(
                camera_pose,
                camera_state_revision,
                Some(scene_identity.clone()),
                expected_source_revision,
            );
        (
            Self {
                render_path,
                scene_identity,
                initial_revision: expected_source_revision,
                semantic_face_controller: None,
            },
            artifact_installer,
            camera_controller,
        )
    }

    pub fn from_residency_artifact(
        artifact: RasterArtifact,
        camera_pose: CameraPose,
        camera_state_revision: CameraStateRevision,
    ) -> Result<Self, RasterConvergenceError> {
        if artifact.residency_selection().is_none() {
            return Err(RasterConvergenceError::UnsupportedVisibleInstallation);
        }
        let scene_identity = artifact.scene_identity().clone();
        let initial_revision = artifact.source_revision();
        let mut render_path = RasterRenderPath::with_camera_pose(camera_pose);
        render_path.camera_control =
            RasterCameraController::new(camera_pose, camera_state_revision);
        render_path.acknowledged_camera_revision = camera_state_revision;
        render_path.install_artifact(artifact);
        render_path.begin_convergence()?;
        Ok(Self {
            render_path,
            scene_identity,
            initial_revision,
            semantic_face_controller: None,
        })
    }

    #[cfg(feature = "qualification")]
    pub fn qualification_drain_convergence_events(
        &mut self,
    ) -> Result<Vec<RasterConvergenceEvent>, RasterConvergenceError> {
        self.render_path.drain_convergence_events()
    }
    #[cfg(feature = "qualification")]
    pub fn qualification_fail_next_convergence(
        &mut self,
        phase: RasterConvergenceFailurePhase,
    ) -> Result<(), RasterConvergenceError> {
        self.render_path.qualification_fail_next_convergence(phase)
    }
    #[cfg(feature = "qualification")]
    pub fn qualification_request_retry(
        &mut self,
    ) -> Result<RasterConvergenceRetry, RasterConvergenceError> {
        self.render_path.request_convergence_retry()
    }
    #[cfg(any(test, feature = "qualification"))]
    pub fn qualification_advance_frame_boundary(&mut self) -> Result<(), RasterConvergenceError> {
        self.render_path.qualification_advance_frame_boundary()
    }

    pub fn enable_lifecycle_control(&mut self) -> RasterLifecycleController {
        self.enable_lifecycle_control_inner(false)
    }

    #[cfg(any(test, feature = "qualification"))]
    pub fn enable_lifecycle_control_with_hold(
        &mut self,
        hold_post_upload: bool,
    ) -> RasterLifecycleController {
        self.enable_lifecycle_control_inner(hold_post_upload)
    }

    fn enable_lifecycle_control_inner(
        &mut self,
        hold_post_upload: bool,
    ) -> RasterLifecycleController {
        self.render_path
            .enable_lifecycle_control_inner(hold_post_upload)
    }

    pub fn enable_semantic_face_observation(&mut self) -> RasterSemanticFaceController {
        let controller = RasterSemanticFaceController {
            state: Arc::new(Mutex::new(RasterSemanticFaceControlState::default())),
        };
        self.semantic_face_controller = Some(controller.clone());
        controller
    }

    fn required_revision(&self) -> VoxelSceneRevision {
        self.render_path
            .required_revision()
            .or(self.render_path.expected_source_revision)
            .unwrap_or(self.initial_revision)
    }

    fn visible_revision(&self) -> VoxelSceneRevision {
        self.render_path
            .visible_revision()
            .or(self.render_path.installed_source_revision)
            .unwrap_or(self.initial_revision)
    }
}

impl RenderPath for RasterRenderPathAdapter {
    fn installed_residency_coverage(&self) -> Option<&render_backend::RenderPathCoverage> {
        self.render_path.installed_residency_coverage()
    }

    fn submit_residency_selection(
        &mut self,
        selection: VoxelResidencySelection,
    ) -> RenderPathResult<()> {
        self.render_path.submit_residency_selection(selection)
    }

    fn submit_edit_outcome(&mut self, outcome: VoxelEditOutcome) -> RenderPathResult<()> {
        self.render_path.submit_edit_outcome(outcome)
    }

    fn publish_camera_state(
        &mut self,
        camera_state: CameraPose,
        camera_state_revision: CameraStateRevision,
    ) -> RenderPathResult<()> {
        self.render_path
            .camera_control
            .set_state(camera_state, camera_state_revision)?;
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

    fn advance_frame_boundary(
        &mut self,
        device: RenderPathDeviceContext<'_>,
        target: RenderPathTarget<'_>,
    ) -> RenderPathResult<()> {
        self.render_path.advance_frame_boundary(device, target)
    }

    fn shutdown(&mut self, device: RenderPathDeviceContext<'_>) -> RenderPathResult<()> {
        self.render_path.shutdown(device)
    }

    fn record(&mut self, frame: RenderPathFrameContext<'_>) -> RenderPathResult<()> {
        let frame_sequence = frame.target().frame_sequence();
        self.render_path.record(frame)?;
        if let Some(controller) = &self.semantic_face_controller {
            let artifact = self
                .render_path
                .installed_artifact()
                .ok_or(RasterResourceError::MissingArtifact)?;
            controller.observe_presented_artifact(artifact, frame_sequence)?;
        }
        Ok(())
    }
}

impl SwitchableRenderPath for RasterRenderPathAdapter {
    fn stamp(&self) -> RenderPathStamp {
        let readiness = if self.render_path.installed_source_revision.is_some()
            && self.render_path.configuration_id.is_some()
        {
            RenderPathReadiness::Recordable
        } else {
            RenderPathReadiness::Preparing
        };
        let stamp = RenderPathStamp::new(
            RASTER_STRATEGY,
            self.scene_identity.clone(),
            self.required_revision(),
            self.visible_revision(),
            self.render_path.acknowledged_camera_revision,
            self.render_path.configuration_id,
            readiness,
        );
        match (
            self.render_path.required_residency(),
            self.render_path.installed_residency(),
        ) {
            (Some(required), Some(installed)) => {
                stamp.with_residency(required.clone(), installed.clone())
            }
            _ => stamp,
        }
    }

    fn retire_at_frame_boundary(
        &mut self,
        device: RenderPathDeviceContext<'_>,
    ) -> RenderPathResult<RenderPathRetirement> {
        self.render_path.shutdown(device)?;
        Ok(RenderPathRetirement::Complete)
    }
}

impl Default for RasterRenderPath {
    fn default() -> Self {
        let camera_pose = CameraPose::default();
        let camera_state_revision = CameraStateRevision::new(1);
        Self {
            artifact: None,
            installation: None,
            expected_source_revision: None,
            installed_source_revision: None,
            camera_control: RasterCameraController::new(camera_pose, camera_state_revision),
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
            camera_eye_and_far_clip: [0.0; 4],
            camera_presentation: [0.0; 4],
            background: [0.0; 4],
            acknowledged_camera_revision: camera_state_revision,
            installed_regions: Vec::new(),
            convergence: None,
            lifecycle_control: None,
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
            camera_control: RasterCameraController::new(camera_pose, CameraStateRevision::new(1)),
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
        Self::awaiting_artifact_with_camera_revision(
            camera_pose,
            CameraStateRevision::new(1),
            None,
            expected_source_revision,
        )
    }

    fn awaiting_artifact_with_camera_revision(
        camera_pose: CameraPose,
        camera_state_revision: CameraStateRevision,
        expected_scene_identity: Option<VoxelSceneId>,
        expected_source_revision: VoxelSceneRevision,
    ) -> (Self, RasterArtifactInstaller, RasterCameraController) {
        let installer = RasterArtifactInstaller {
            state: Arc::new(Mutex::new(RasterArtifactInstallationState {
                expected_scene_identity,
                expected_revision: expected_source_revision,
                staged_artifact: None,
                artifact_was_published: false,
                installed_revision: None,
                #[cfg(any(test, feature = "qualification"))]
                inject_upload_failure: false,
            })),
        };
        let camera_control = RasterCameraController::new(camera_pose, camera_state_revision);
        let render_path = Self {
            camera_control: camera_control.clone(),
            acknowledged_camera_revision: camera_state_revision,
            installation: Some(installer.clone()),
            expected_source_revision: Some(expected_source_revision),
            ..Self::default()
        };
        (render_path, installer, camera_control)
    }

    pub fn camera_pose(&self) -> Result<CameraPose, RasterCameraControlError> {
        self.camera_control.pose()
    }

    pub fn enable_lifecycle_control(&mut self) -> RasterLifecycleController {
        self.enable_lifecycle_control_inner(false)
    }

    #[cfg(any(test, feature = "qualification"))]
    pub fn enable_lifecycle_control_with_hold(
        &mut self,
        hold_post_upload: bool,
    ) -> RasterLifecycleController {
        self.enable_lifecycle_control_inner(hold_post_upload)
    }

    fn enable_lifecycle_control_inner(
        &mut self,
        hold_post_upload: bool,
    ) -> RasterLifecycleController {
        let controller = RasterLifecycleController {
            state: Arc::new(Mutex::new(RasterLifecycleControlState {
                pending_outcomes: VecDeque::new(),
                post_upload_held: hold_post_upload,
                post_upload_revision: None,
                shutdown_owned_resource_count: None,
                cpu_barrier: None,
                status: None,
                rejected_candidate: None,
                peak_live_gpu_bytes: 0,
                peak_live_gpu_resources: 0,
                installed_gpu_resources: RasterGpuResourceUsage::default(),
                characterization: None,
                residency_status: None,
            })),
        };
        self.lifecycle_control = Some(controller.clone());
        controller
    }

    fn observe_live_gpu_resources<'resources>(
        &self,
        resources: impl IntoIterator<Item = &'resources RasterRegionGpuResources>,
    ) -> Result<(), RasterLifecycleControlError> {
        let Some(controller) = &self.lifecycle_control else {
            return Ok(());
        };
        let usage = raster_gpu_resource_usage(resources)?;
        let mut state = controller
            .state
            .lock()
            .map_err(|_| RasterLifecycleControlError)?;
        state.peak_live_gpu_bytes = state.peak_live_gpu_bytes.max(usage.bytes);
        state.peak_live_gpu_resources = state.peak_live_gpu_resources.max(usage.resources);
        if let Some(characterization) = &mut state.characterization {
            characterization.peak.bytes = characterization.peak.bytes.max(usage.bytes);
            characterization.peak.resources = characterization.peak.resources.max(usage.resources);
        }
        Ok(())
    }

    fn observe_installed_gpu_resources<'resources>(
        &self,
        resources: impl IntoIterator<Item = &'resources RasterRegionGpuResources>,
    ) -> Result<(), RasterLifecycleControlError> {
        let Some(controller) = &self.lifecycle_control else {
            return Ok(());
        };
        let usage = raster_gpu_resource_usage(resources)?;
        let mut state = controller
            .state
            .lock()
            .map_err(|_| RasterLifecycleControlError)?;
        state.installed_gpu_resources = usage;
        if let Some(characterization) = &mut state.characterization {
            characterization.installed = usage;
        }
        Ok(())
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

    pub fn accept_residency_selection(
        &mut self,
        selection: VoxelResidencySelection,
    ) -> Result<bool, RasterConvergenceError> {
        if self.installed_residency().is_none() {
            return Ok(false);
        }
        if self.convergence.is_none() {
            self.begin_convergence()?;
        }
        self.convergence
            .as_mut()
            .ok_or(RasterConvergenceError::NotStarted)?
            .accept_residency_selection(selection)
    }

    pub fn installed_residency(&self) -> Option<&VoxelResidencySelection> {
        self.artifact
            .as_ref()
            .and_then(RasterArtifact::residency_selection)
    }

    pub fn required_residency(&self) -> Option<&VoxelResidencySelection> {
        self.convergence
            .as_ref()
            .and_then(|convergence| convergence.residency_target.as_ref())
            .map(|target| &target.selection)
            .or_else(|| self.installed_residency())
    }

    pub fn residency_status(&self) -> Result<RasterResidencyStatus, RasterConvergenceError> {
        self.residency_status_with_convergence(self.convergence.as_ref())
    }

    fn residency_status_with_convergence(
        &self,
        convergence: Option<&RasterConvergence>,
    ) -> Result<RasterResidencyStatus, RasterConvergenceError> {
        let cache = convergence
            .and_then(|convergence| convergence.residency_target.as_ref())
            .map(|target| &target.cache)
            .or_else(|| {
                self.artifact
                    .as_ref()
                    .and_then(|artifact| artifact.residency.as_ref())
                    .map(|residency| &residency.cache)
            });
        let (representation_copies, derived_volumes) = match cache {
            Some(cache) => {
                let cache = cache
                    .lock()
                    .map_err(|_| RasterConvergenceError::LifecycleControlUnavailable)?;
                (cache.retained_copies(), cache.derived_volumes)
            }
            None => (0, 0),
        };
        Ok(RasterResidencyStatus {
            representation_copies,
            derived_volumes,
            workers: convergence
                .and_then(|convergence| convergence.worker_pool.as_ref())
                .map_or(0, |pool| pool.worker_count()),
            gpu_allocations: usize::from(self.depth_memory != vk::DeviceMemory::null())
                + self
                    .region_resources
                    .iter()
                    .chain(
                        convergence
                            .and_then(|convergence| convergence.hidden_candidate.as_ref())
                            .into_iter()
                            .flat_map(|candidate| candidate.successor_gpu_resources.iter()),
                    )
                    .map(|resources| {
                        usize::from(resources.vertex_memory != vk::DeviceMemory::null())
                            + usize::from(resources.index_memory != vk::DeviceMemory::null())
                            + usize::from(resources.material_memory != vk::DeviceMemory::null())
                    })
                    .sum::<usize>(),
        })
    }

    #[cfg(any(test, feature = "qualification"))]
    pub fn qualification_fail_next_convergence(
        &mut self,
        phase: RasterConvergenceFailurePhase,
    ) -> Result<(), RasterConvergenceError> {
        self.convergence
            .as_mut()
            .ok_or(RasterConvergenceError::NotStarted)?
            .injected_failure = Some(phase);
        Ok(())
    }

    #[cfg(any(test, feature = "qualification"))]
    pub fn qualification_retire(&mut self) -> Result<(), RasterConvergenceError> {
        if !self.region_resources.is_empty() {
            return Err(RasterConvergenceError::ConfiguredResourcesRequireDevice);
        }
        if let Some(convergence) = &mut self.convergence {
            let shutdown = convergence.shutdown();
            shutdown.retirement.release_with(drop);
            if let Some(error) = shutdown.worker_error {
                return Err(error);
            }
        }
        self.release_residency_artifact();
        if let Some(controller) = &self.lifecycle_control {
            let status = self.residency_status()?;
            controller
                .state
                .lock()
                .map_err(|_| RasterConvergenceError::LifecycleControlUnavailable)?
                .residency_status = Some(status);
        }
        Ok(())
    }

    fn release_residency_artifact(&mut self) {
        if self.installed_residency().is_some() {
            self.artifact = None;
            self.installed_regions.clear();
            self.installed_source_revision = None;
        }
    }

    #[cfg(any(test, feature = "qualification"))]
    pub fn qualification_advance_frame_boundary(&mut self) -> Result<(), RasterConvergenceError> {
        self.advance_convergence_at_frame_boundary(None)
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
        let mut queued_wait = Duration::ZERO;
        let mut submission_bookkeeping = Duration::ZERO;
        if let Some(controller) = self.lifecycle_control.clone() {
            let (outcomes, cpu_barrier) = {
                let mut state = controller
                    .state
                    .lock()
                    .map_err(|_| RasterConvergenceError::LifecycleControlUnavailable)?;
                (
                    state.pending_outcomes.drain(..).collect::<Vec<_>>(),
                    state.cpu_barrier.clone(),
                )
            };
            if let Some(convergence) = &mut self.convergence {
                convergence.cpu_barrier = cpu_barrier.clone();
            }
            if !outcomes.is_empty() && self.convergence.is_none() {
                self.begin_convergence()?;
                if let Some(convergence) = &mut self.convergence {
                    convergence.cpu_barrier = cpu_barrier;
                }
            }
            for queued in outcomes {
                let acceptance_started_at = Instant::now();
                queued_wait = queued_wait.saturating_add(
                    acceptance_started_at.saturating_duration_since(queued.queued_at),
                );
                self.accept_edit_outcome(queued.outcome)?;
                submission_bookkeeping =
                    submission_bookkeeping.saturating_add(acceptance_started_at.elapsed());
            }
            controller
                .update_characterization(|characterization| {
                    characterization.phases.queued_wait_milliseconds +=
                        queued_wait.as_secs_f64() * 1_000.0;
                    characterization.phases.submission_bookkeeping_milliseconds +=
                        submission_bookkeeping.as_secs_f64() * 1_000.0;
                })
                .map_err(|_| RasterConvergenceError::LifecycleControlUnavailable)?;
        }
        let Some(mut convergence) = self.convergence.take() else {
            return Ok(());
        };
        let mut rejected_candidate = None;
        let result = (|| {
            let upload_started_at = Instant::now();
            let upload = convergence.upload_ready_with_optional_device(device, self)?;
            if matches!(upload, RasterConvergenceUpload::Uploaded { .. })
                && let Some(controller) = &self.lifecycle_control
            {
                let upload_milliseconds = upload_started_at.elapsed().as_secs_f64() * 1_000.0;
                controller
                    .update_characterization(|characterization| {
                        characterization.phases.upload_milliseconds += upload_milliseconds;
                    })
                    .map_err(|_| RasterConvergenceError::LifecycleControlUnavailable)?;
            }
            if let Some(candidate) = &convergence.hidden_candidate {
                self.observe_live_gpu_resources(
                    self.region_resources
                        .iter()
                        .chain(candidate.successor_gpu_resources.iter()),
                )
                .map_err(|_| RasterConvergenceError::LifecycleControlUnavailable)?;
                let hidden = raster_gpu_resource_usage(candidate.successor_gpu_resources.iter())
                    .map_err(|_| RasterConvergenceError::LifecycleControlUnavailable)?;
                if let Some(controller) = &self.lifecycle_control {
                    controller
                        .update_characterization(|characterization| {
                            characterization.hidden.bytes =
                                characterization.hidden.bytes.max(hidden.bytes);
                            characterization.hidden.resources =
                                characterization.hidden.resources.max(hidden.resources);
                        })
                        .map_err(|_| RasterConvergenceError::LifecycleControlUnavailable)?;
                }
            }
            if matches!(
                upload,
                RasterConvergenceUpload::Uploaded { .. }
                    | RasterConvergenceUpload::CandidateAlreadyRetained { .. }
            ) && let Some(controller) = &self.lifecycle_control
            {
                let mut state = controller
                    .state
                    .lock()
                    .map_err(|_| RasterConvergenceError::LifecycleControlUnavailable)?;
                if state.post_upload_held {
                    state.post_upload_revision = convergence
                        .hidden_candidate
                        .as_ref()
                        .map(|candidate| candidate.target.view.revision());
                    return Ok(());
                }
            }
            let commit_started_at = Instant::now();
            let commit = convergence.commit_at_frame_boundary(self)?;
            let (retirement, retirement_revision, retirement_disposition) = match commit {
                RasterConvergenceCommit::NoCandidate => return Ok(()),
                RasterConvergenceCommit::Rejected {
                    revision,
                    retirement,
                } => {
                    let stale_regions = u64::try_from(retirement.resource_count())
                        .map_err(|_| RasterConvergenceError::ResourceBookkeepingAllocation)?;
                    convergence.work_disposition.completed = convergence
                        .work_disposition
                        .completed
                        .saturating_sub(stale_regions);
                    convergence.work_disposition.stale = convergence
                        .work_disposition
                        .stale
                        .saturating_add(stale_regions);
                    rejected_candidate = Some(RasterRejectedCandidate {
                        revision,
                        retired_resource_count: retirement.resource_count(),
                    });
                    (
                        retirement,
                        revision,
                        RasterSafeRetirementDisposition::StaleCandidate,
                    )
                }
                RasterConvergenceCommit::Failed { retirement, .. } => (
                    retirement,
                    convergence.required_revision(),
                    RasterSafeRetirementDisposition::StaleCandidate,
                ),
                RasterConvergenceCommit::Committed { retirement, .. } => (
                    retirement,
                    convergence.visible_revision(),
                    RasterSafeRetirementDisposition::ReplacedInstallation,
                ),
            };
            if let Some(controller) = &self.lifecycle_control {
                let commit_milliseconds = commit_started_at.elapsed().as_secs_f64() * 1_000.0;
                controller
                    .update_characterization(|characterization| {
                        characterization.phases.frame_boundary_commit_milliseconds +=
                            commit_milliseconds;
                    })
                    .map_err(|_| RasterConvergenceError::LifecycleControlUnavailable)?;
            }
            self.observe_live_gpu_resources(
                self.region_resources
                    .iter()
                    .chain(retirement.resources.iter()),
            )
            .map_err(|_| RasterConvergenceError::LifecycleControlUnavailable)?;
            if retirement.resource_count() == 0 {
                return Ok(());
            }
            let retired = raster_gpu_resource_usage(retirement.resources.iter())
                .map_err(|_| RasterConvergenceError::LifecycleControlUnavailable)?;
            let device = device.ok_or(RasterConvergenceError::ConfiguredResourcesRequireDevice)?;
            // SAFETY: The backend invokes this hook only after its sole in-flight frame fence has completed.
            unsafe { retirement.release_after_gpu_completion(device) };
            if let Some(controller) = &self.lifecycle_control {
                controller
                    .update_characterization(|characterization| {
                        characterization.retired.bytes =
                            characterization.retired.bytes.saturating_add(retired.bytes);
                        characterization.retired.resources = characterization
                            .retired
                            .resources
                            .saturating_add(retired.resources);
                        characterization
                            .safe_retirements
                            .push(RasterSafeRetirementEvent {
                                revision: retirement_revision,
                                disposition: retirement_disposition,
                                resources: retired,
                            });
                    })
                    .map_err(|_| RasterConvergenceError::LifecycleControlUnavailable)?;
            }
            Ok(())
        })();
        let residency_status = convergence
            .residency_target
            .as_ref()
            .map(|_| self.residency_status_with_convergence(Some(&convergence)))
            .transpose()?;
        if let Some(controller) = &self.lifecycle_control {
            let installed = raster_gpu_resource_usage(self.region_resources.iter())
                .map_err(|_| RasterConvergenceError::LifecycleControlUnavailable)?;
            let mut state = controller
                .state
                .lock()
                .map_err(|_| RasterConvergenceError::LifecycleControlUnavailable)?;
            state.installed_gpu_resources = installed;
            state.residency_status = residency_status;
            state.status = Some(convergence.status(self.installed_regions.len()));
            if let Some(characterization) = &mut state.characterization {
                characterization.phases.cpu_derivation_milliseconds =
                    convergence.cpu_derivation.as_secs_f64() * 1_000.0;
                characterization.work = convergence.work_disposition;
                characterization.installed = installed;
                characterization.cancellation_observations =
                    convergence.cancellation_observations.clone();
            }
            if let Some(rejected_candidate) = rejected_candidate {
                state.rejected_candidate = Some(rejected_candidate);
                state.post_upload_revision = None;
            }
        }
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
        if self.installed_residency().is_some() {
            return Err(RasterAdjacentChangeError::StreamedRequiresConvergence);
        }
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

#[cfg(test)]
mod installation_measurements;

mod semantic_faces;
use semantic_faces::*;
pub use semantic_faces::{
    AxisNormal, RasterSemanticFaceControlError, RasterSemanticFaceController,
    RasterSemanticFaceCorrespondence, RasterSemanticFaceObservation, SemanticFace,
    qualify_raster_semantic_faces,
};

mod meshing;
use meshing::*;
pub use meshing::{
    DecodedRasterVertex, RasterArtifact, RasterArtifactBuildCause, RasterArtifactBuildError,
    RasterArtifactBuildPhase, RasterGeometry, RasterInspectionGeometry, RasterRegionIdentity,
    RasterRegionResult, RasterVertex, derive_raster_artifact, derive_raster_regions,
};

mod installation;
use installation::*;
pub use installation::{
    RasterAdjacentChangeError, RasterAdjacentChangeMismatch, RasterAdjacentChangeOutcome,
    RasterArtifactInstallationError, RasterArtifactInstallationPhase, RasterArtifactInstaller,
    RasterArtifactInstallerError, RasterRegionActivity, RasterRegionGpuResourceIdentity,
    RasterRegionInstallation, RasterRegionInstallationGeneration, RasterRegionResourceOwnership,
};

mod lifecycle;
use lifecycle::*;
pub use lifecycle::{
    RasterCameraControlError, RasterCameraController, RasterCancellationObservation,
    RasterConvergenceCharacterization, RasterConvergenceCpuBarrierObservation,
    RasterConvergencePhaseTimings, RasterConvergenceStatus, RasterGpuResourcePeak,
    RasterGpuResourceUsage, RasterLifecycleControlError, RasterLifecycleController,
    RasterRegionWorkDisposition, RasterRejectedCandidate, RasterSafeRetirementDisposition,
    RasterSafeRetirementEvent,
};

mod convergence;
use convergence::*;
pub use convergence::{
    RasterConvergence, RasterConvergenceAcceptance, RasterConvergenceError, RasterConvergenceEvent,
    RasterConvergenceFailure, RasterConvergenceFailurePhase, RasterConvergenceRetry,
    RasterPreparationDisposition,
};

mod preparation;
mod worker_pool;
pub use preparation::{
    RasterArtifactPreparation, RasterArtifactPreparationError, RasterArtifactPreparationEvent,
};

mod gpu_resources;
use gpu_resources::*;

#[cfg(test)]
mod convergence_tests;

mod residency;
pub use residency::{RasterResidencyStatus, derive_raster_residency};

#![cfg_attr(
    not(feature = "qualification"),
    doc = r"Failure injection and deterministic holds require the `qualification` feature.

```compile_fail
use compute_ray_render_path::ComputeConvergenceController;
let hook = ComputeConvergenceController::inject_next_failure;
```

```compile_fail
use compute_ray_render_path::ComputeConvergenceController;
let hook = ComputeConvergenceController::hold_next_preparation_after_blocks;
```

```compile_fail
use compute_ray_render_path::ComputeConvergenceController;
let hook = ComputeConvergenceController::release_preparation_barrier;
```

```compile_fail
use compute_ray_render_path::ComputeRayRenderPathAdapter;
let hook = ComputeRayRenderPathAdapter::enable_convergence_control_with_hold;
```

```compile_fail
use compute_ray_render_path::ComputeSceneBuildError;
let failure = ComputeSceneBuildError::InjectedPreparationFailure;
```
```compile_fail
use compute_ray_render_path::ComputeSceneBuildError;
let failure = ComputeSceneBuildError::PreparationBarrier;
```
"
)]

use ash::vk;
use render_backend::{
    CameraState, CameraStateRevision, RenderPath, RenderPathDeviceContext, RenderPathFrameContext,
    RenderPathReadiness, RenderPathResult, RenderPathRetirement, RenderPathStamp,
    RenderPathStrategy, RenderPathTarget, SwitchableRenderPath,
};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use voxel_frontend::{VoxelEditOutcome, VoxelSceneView};

mod brickmap_validation;
pub use brickmap_validation::BrickmapValidationError;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ComputeRepresentation {
    #[default]
    Dense,
    Brickmap {
        budget_bytes: u64,
    },
}

mod brickmap_scene;
mod compute_convergence;
mod compute_scene;
pub use brickmap_scene::{BrickmapBuildError, BrickmapObservations, BrickmapSceneBundle};
mod scene_words;

pub use compute_convergence::{
    ComputeCandidateDisposition, ComputeConvergenceAcceptance, ComputeConvergenceControlError,
    ComputeConvergenceController, ComputeConvergenceError, ComputeConvergenceEvent,
    ComputeConvergenceFailure, ComputeConvergenceFailurePhase, ComputeConvergenceGeneration,
    ComputeConvergenceRetry, ComputeConvergenceStatus, ComputeConvergenceWorkStamp,
    ComputePreparationBarrierObservation,
};
pub use compute_scene::{
    BrickmapGrowthObservations, ComputeSceneBuildError, ComputeSceneBundle, ComputeVolumeHeader,
};

pub const COMPUTE_RAY_STRATEGY: RenderPathStrategy =
    RenderPathStrategy::new("voxel-nexus.compute-ray");

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

pub struct ComputeRayRenderPathAdapter {
    render_path: ComputeRayRenderPath,
    camera_state_revision: CameraStateRevision,
    published_camera_state_revision: CameraStateRevision,
}

impl ComputeRayRenderPathAdapter {
    #[cfg(feature = "qualification")]
    pub fn qualification_from_bundle(
        bundle: ComputeSceneBundle,
        camera: CameraState,
        revision: CameraStateRevision,
    ) -> Self {
        Self {
            render_path: ComputeRayRenderPath::new(bundle, camera, None),
            camera_state_revision: revision,
            published_camera_state_revision: revision,
        }
    }
    pub fn new(
        view: VoxelSceneView,
        camera_state: CameraState,
        camera_state_revision: CameraStateRevision,
    ) -> Result<Self, ComputeSceneBuildError> {
        Self::new_with_representation(
            view,
            camera_state,
            camera_state_revision,
            ComputeRepresentation::Dense,
        )
    }

    pub fn new_with_representation(
        view: VoxelSceneView,
        camera_state: CameraState,
        camera_state_revision: CameraStateRevision,
        representation: ComputeRepresentation,
    ) -> Result<Self, ComputeSceneBuildError> {
        let scene_bundle =
            ComputeSceneBundle::from_view_with_representation(&view, representation)?;
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
        Self::new_with_representation_and_measurement(
            view,
            camera_state,
            camera_state_revision,
            ComputeRepresentation::Dense,
        )
    }

    pub fn new_with_representation_and_measurement(
        view: VoxelSceneView,
        camera_state: CameraState,
        camera_state_revision: CameraStateRevision,
        representation: ComputeRepresentation,
    ) -> Result<(Self, ComputeMeasurementController), ComputeSceneBuildError> {
        let started_at = Instant::now();
        let (scene_bundle, timings) =
            ComputeSceneBundle::from_view_with_preparation_timings(&view, representation)?;
        let measurement = ComputeMeasurementController::with_initial_event(ComputeTimingEvent {
            phase: ComputeTimingPhase::Preparation,
            uploaded_bytes: 0,
            scene_identity: scene_bundle.scene_identity().clone(),
            revision: scene_bundle.revision(),
            generation: 0,
            elapsed_milliseconds: started_at.elapsed().as_secs_f64() * 1_000.0,
        });
        for (phase, duration) in timings {
            measurement
                .record(ComputeTimingEvent {
                    phase,
                    uploaded_bytes: 0,
                    scene_identity: scene_bundle.scene_identity().clone(),
                    revision: scene_bundle.revision(),
                    generation: 0,
                    elapsed_milliseconds: duration.as_secs_f64() * 1_000.0,
                })
                .map_err(|_| ComputeSceneBuildError::PreparationControl)?;
        }
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

    pub fn enable_convergence_control(&mut self) -> ComputeConvergenceController {
        self.enable_convergence_control_inner(false)
    }

    #[cfg(any(test, feature = "qualification"))]
    pub fn enable_convergence_control_with_hold(
        &mut self,
        hold_post_upload: bool,
    ) -> ComputeConvergenceController {
        self.enable_convergence_control_inner(hold_post_upload)
    }

    fn enable_convergence_control_inner(
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
    fn submit_edit_outcome(&mut self, outcome: VoxelEditOutcome) -> RenderPathResult<()> {
        self.render_path.submit_edit_outcome(outcome)
    }

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
            COMPUTE_RAY_STRATEGY,
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

impl RenderPath for ComputeRayRenderPath {
    fn submit_edit_outcome(&mut self, outcome: VoxelEditOutcome) -> RenderPathResult<()> {
        let controller = match self.convergence_control.clone() {
            Some(controller) => controller,
            None => {
                let controller = self.convergence.enable_control(false);
                self.convergence_control = Some(controller.clone());
                controller
            }
        };
        controller.submit(outcome)?;
        Ok(())
    }

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

mod semantic_rays;
use semantic_rays::*;
pub use semantic_rays::{
    ComputeCameraRayError, ComputeSemanticRayControlError, ComputeSemanticRayController,
    ComputeSemanticRayProbeObservation, camera_semantic_ray,
};

mod capabilities;
pub use capabilities::{
    ComputeCapabilityAssessment, ComputeCapabilityRecord, ComputeDescriptorRequirement,
    ComputeDispatchConfiguration, ComputeRenderPathRejection, qualify_compute_render_path,
};

mod resource_error;
use resource_error::*;

mod observation;
pub use observation::{
    ComputeLifecycleControlError, ComputeLifecycleController, ComputeMeasurementControlError,
    ComputeMeasurementController, ComputeOwnedResourceCounts, ComputeResourceObservation,
    ComputeResourceObservationPoint, ComputeResourceRole, ComputeTimingEvent, ComputeTimingPhase,
};

mod gpu_resources;
use gpu_resources::*;

mod installation;

#[cfg(test)]
mod tests;

pub use compute_scene::BrickmapPatchObservations;

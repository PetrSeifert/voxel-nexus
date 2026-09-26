use render_backend::CameraState as CameraPose;
use render_backend::CameraStateRevision;
use std::collections::VecDeque;
#[cfg(any(test, feature = "qualification"))]
use std::sync::Condvar;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use thiserror::Error;
use voxel_frontend::{VoxelEditOutcome, VoxelSceneRevision};

#[derive(Clone)]
pub struct RasterCameraController {
    state: Arc<Mutex<RasterCameraState>>,
}

#[derive(Clone, Copy)]
pub(super) struct RasterCameraState {
    pub(super) pose: CameraPose,
    pub(super) revision: CameraStateRevision,
}

pub(super) struct RasterLifecycleControlState {
    pub(super) pending_outcomes: VecDeque<RasterQueuedOutcome>,
    pub(super) post_upload_held: bool,
    pub(super) post_upload_revision: Option<VoxelSceneRevision>,
    pub(super) shutdown_owned_resource_count: Option<usize>,
    pub(super) cpu_barrier: Option<Arc<RasterConvergenceCpuBarrierShared>>,
    pub(super) status: Option<RasterConvergenceStatus>,
    pub(super) rejected_candidate: Option<RasterRejectedCandidate>,
    pub(super) peak_live_gpu_bytes: u64,
    pub(super) peak_live_gpu_resources: usize,
    pub(super) installed_gpu_resources: RasterGpuResourceUsage,
    pub(super) characterization: Option<RasterConvergenceCharacterization>,
}

pub(super) struct RasterQueuedOutcome {
    pub(super) queued_at: Instant,
    pub(super) outcome: VoxelEditOutcome,
}

#[derive(Clone)]
pub struct RasterLifecycleController {
    pub(super) state: Arc<Mutex<RasterLifecycleControlState>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RasterConvergenceStatus {
    pub required_revision: VoxelSceneRevision,
    pub visible_revision: VoxelSceneRevision,
    pub affected_region_count: usize,
    pub unaffected_region_count: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RasterConvergenceCpuBarrierObservation {
    pub reached_revision: Option<VoxelSceneRevision>,
    pub scheduled_region_count: usize,
    pub finished: bool,
    pub cancelled: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RasterRejectedCandidate {
    pub revision: VoxelSceneRevision,
    pub retired_resource_count: usize,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RasterGpuResourcePeak {
    pub bytes: u64,
    pub resources: usize,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RasterGpuResourceUsage {
    pub bytes: u64,
    pub resources: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RasterConvergencePhaseTimings {
    pub submission_bookkeeping_milliseconds: f64,
    pub queued_wait_milliseconds: f64,
    pub cpu_derivation_milliseconds: f64,
    pub upload_milliseconds: f64,
    pub frame_boundary_commit_milliseconds: f64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RasterRegionWorkDisposition {
    pub scheduled: u64,
    pub completed: u64,
    pub cancelled: u64,
    pub stale: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RasterCancellationObservation {
    pub revision: VoxelSceneRevision,
    pub scheduled_regions: u64,
    pub completed_regions: u64,
    pub cancelled_regions: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RasterSafeRetirementDisposition {
    StaleCandidate,
    ReplacedInstallation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RasterSafeRetirementEvent {
    pub revision: VoxelSceneRevision,
    pub disposition: RasterSafeRetirementDisposition,
    pub resources: RasterGpuResourceUsage,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RasterConvergenceCharacterization {
    pub phases: RasterConvergencePhaseTimings,
    pub work: RasterRegionWorkDisposition,
    pub installed: RasterGpuResourceUsage,
    pub hidden: RasterGpuResourceUsage,
    pub retired: RasterGpuResourceUsage,
    pub peak: RasterGpuResourceUsage,
    pub cancellation_observations: Vec<RasterCancellationObservation>,
    pub safe_retirements: Vec<RasterSafeRetirementEvent>,
}

#[cfg(any(test, feature = "qualification"))]
#[derive(Default)]
pub(super) struct RasterConvergenceCpuBarrierState {
    #[cfg(any(test, feature = "qualification"))]
    reached_revision: Option<VoxelSceneRevision>,
    #[cfg(any(test, feature = "qualification"))]
    scheduled_region_count: usize,
    #[cfg(any(test, feature = "qualification"))]
    released: bool,
    #[cfg(any(test, feature = "qualification"))]
    finished: bool,
    #[cfg(any(test, feature = "qualification"))]
    cancelled: bool,
}

pub(super) struct RasterConvergenceCpuBarrierShared {
    #[cfg(any(test, feature = "qualification"))]
    hold_after_scheduled_regions: usize,
    #[cfg(any(test, feature = "qualification"))]
    state: Mutex<RasterConvergenceCpuBarrierState>,
    #[cfg(any(test, feature = "qualification"))]
    released: Condvar,
}

#[cfg(any(test, feature = "qualification"))]
#[derive(Clone, Copy)]
pub(super) struct RasterConvergenceCpuBarrierError;

#[cfg(any(test, feature = "qualification"))]
impl RasterConvergenceCpuBarrierShared {
    #[cfg(any(test, feature = "qualification"))]
    fn observation(
        &self,
    ) -> Result<RasterConvergenceCpuBarrierObservation, RasterConvergenceCpuBarrierError> {
        self.state
            .lock()
            .map(|state| RasterConvergenceCpuBarrierObservation {
                reached_revision: state.reached_revision,
                scheduled_region_count: state.scheduled_region_count,
                finished: state.finished,
                cancelled: state.cancelled,
            })
            .map_err(|_| RasterConvergenceCpuBarrierError)
    }

    pub(super) fn schedule_and_wait(
        &self,
        revision: VoxelSceneRevision,
    ) -> Result<(), RasterConvergenceCpuBarrierError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| RasterConvergenceCpuBarrierError)?;
        if state.reached_revision.is_some() {
            return Ok(());
        }
        state.scheduled_region_count = state
            .scheduled_region_count
            .checked_add(1)
            .ok_or(RasterConvergenceCpuBarrierError)?;
        if state.scheduled_region_count != self.hold_after_scheduled_regions {
            return Ok(());
        }
        state.reached_revision = Some(revision);
        while !state.released {
            state = self
                .released
                .wait(state)
                .map_err(|_| RasterConvergenceCpuBarrierError)?;
        }
        Ok(())
    }

    pub(super) fn finish(
        &self,
        revision: VoxelSceneRevision,
        cancelled: bool,
    ) -> Result<(), RasterConvergenceCpuBarrierError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| RasterConvergenceCpuBarrierError)?;
        if state.reached_revision == Some(revision) {
            state.finished = true;
            state.cancelled = cancelled;
        }
        Ok(())
    }

    pub(super) fn release(&self) -> Result<(), RasterConvergenceCpuBarrierError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| RasterConvergenceCpuBarrierError)?;
        state.released = true;
        self.released.notify_all();
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
#[error("the raster lifecycle control state is unavailable")]
pub struct RasterLifecycleControlError;

impl RasterLifecycleController {
    pub fn submit(&self, outcome: VoxelEditOutcome) -> Result<(), RasterLifecycleControlError> {
        let submission_started_at = Instant::now();
        let mut state = self.state.lock().map_err(|_| RasterLifecycleControlError)?;
        state.pending_outcomes.push_back(RasterQueuedOutcome {
            queued_at: submission_started_at,
            outcome,
        });
        let phase_boundary = Instant::now();
        state
            .pending_outcomes
            .back_mut()
            .ok_or(RasterLifecycleControlError)?
            .queued_at = phase_boundary;
        let elapsed_milliseconds = phase_boundary
            .saturating_duration_since(submission_started_at)
            .as_secs_f64()
            * 1_000.0;
        if let Some(characterization) = &mut state.characterization {
            characterization.phases.submission_bookkeeping_milliseconds += elapsed_milliseconds;
        }
        Ok(())
    }

    pub fn begin_characterization(&self) -> Result<(), RasterLifecycleControlError> {
        let mut state = self.state.lock().map_err(|_| RasterLifecycleControlError)?;
        let installed = state.installed_gpu_resources;
        state.characterization = Some(RasterConvergenceCharacterization {
            installed,
            peak: installed,
            ..RasterConvergenceCharacterization::default()
        });
        Ok(())
    }

    pub fn characterization(
        &self,
    ) -> Result<Option<RasterConvergenceCharacterization>, RasterLifecycleControlError> {
        self.state
            .lock()
            .map(|state| state.characterization.clone())
            .map_err(|_| RasterLifecycleControlError)
    }

    pub(super) fn update_characterization(
        &self,
        update: impl FnOnce(&mut RasterConvergenceCharacterization),
    ) -> Result<(), RasterLifecycleControlError> {
        let mut state = self.state.lock().map_err(|_| RasterLifecycleControlError)?;
        if let Some(characterization) = &mut state.characterization {
            update(characterization);
        }
        Ok(())
    }

    pub fn post_upload_revision(
        &self,
    ) -> Result<Option<VoxelSceneRevision>, RasterLifecycleControlError> {
        self.state
            .lock()
            .map(|state| state.post_upload_revision)
            .map_err(|_| RasterLifecycleControlError)
    }

    #[cfg(any(test, feature = "qualification"))]
    pub fn release_post_upload(&self) -> Result<(), RasterLifecycleControlError> {
        let mut state = self.state.lock().map_err(|_| RasterLifecycleControlError)?;
        state.post_upload_held = false;
        Ok(())
    }

    #[cfg(any(test, feature = "qualification"))]
    pub fn hold_next_cpu_generation_after_regions(
        &self,
        scheduled_region_count: usize,
    ) -> Result<(), RasterLifecycleControlError> {
        let barrier = Arc::new(RasterConvergenceCpuBarrierShared {
            hold_after_scheduled_regions: scheduled_region_count,
            state: Mutex::new(RasterConvergenceCpuBarrierState::default()),
            released: Condvar::new(),
        });
        self.state
            .lock()
            .map_err(|_| RasterLifecycleControlError)?
            .cpu_barrier = Some(barrier);
        Ok(())
    }

    #[cfg(any(test, feature = "qualification"))]
    pub fn cpu_barrier_observation(
        &self,
    ) -> Result<Option<RasterConvergenceCpuBarrierObservation>, RasterLifecycleControlError> {
        let barrier = self
            .state
            .lock()
            .map_err(|_| RasterLifecycleControlError)?
            .cpu_barrier
            .clone();
        barrier
            .map(|barrier| {
                barrier
                    .observation()
                    .map_err(|_| RasterLifecycleControlError)
            })
            .transpose()
    }

    #[cfg(any(test, feature = "qualification"))]
    pub fn release_cpu_barrier(&self) -> Result<(), RasterLifecycleControlError> {
        let barrier = self
            .state
            .lock()
            .map_err(|_| RasterLifecycleControlError)?
            .cpu_barrier
            .clone()
            .ok_or(RasterLifecycleControlError)?;
        barrier.release().map_err(|_| RasterLifecycleControlError)
    }

    pub fn convergence_status(
        &self,
    ) -> Result<Option<RasterConvergenceStatus>, RasterLifecycleControlError> {
        self.state
            .lock()
            .map(|state| state.status)
            .map_err(|_| RasterLifecycleControlError)
    }

    pub fn rejected_candidate(
        &self,
    ) -> Result<Option<RasterRejectedCandidate>, RasterLifecycleControlError> {
        self.state
            .lock()
            .map(|state| state.rejected_candidate)
            .map_err(|_| RasterLifecycleControlError)
    }

    pub fn shutdown_owned_resource_count(
        &self,
    ) -> Result<Option<usize>, RasterLifecycleControlError> {
        self.state
            .lock()
            .map(|state| state.shutdown_owned_resource_count)
            .map_err(|_| RasterLifecycleControlError)
    }

    pub fn gpu_resource_peak(&self) -> Result<RasterGpuResourcePeak, RasterLifecycleControlError> {
        self.state
            .lock()
            .map(|state| RasterGpuResourcePeak {
                bytes: state.peak_live_gpu_bytes,
                resources: state.peak_live_gpu_resources,
            })
            .map_err(|_| RasterLifecycleControlError)
    }
}

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
#[error("the raster camera control state is unavailable")]
pub struct RasterCameraControlError;

impl RasterCameraController {
    pub(super) fn new(camera_pose: CameraPose, revision: CameraStateRevision) -> Self {
        Self {
            state: Arc::new(Mutex::new(RasterCameraState {
                pose: camera_pose,
                revision,
            })),
        }
    }

    pub fn set_state(
        &self,
        camera_pose: CameraPose,
        revision: CameraStateRevision,
    ) -> Result<(), RasterCameraControlError> {
        let mut state = self.state.lock().map_err(|_| RasterCameraControlError)?;
        state.pose = camera_pose;
        state.revision = revision;
        Ok(())
    }

    pub fn pose(&self) -> Result<CameraPose, RasterCameraControlError> {
        self.state
            .lock()
            .map(|state| state.pose)
            .map_err(|_| RasterCameraControlError)
    }

    pub(super) fn state(&self) -> Result<RasterCameraState, RasterCameraControlError> {
        self.state
            .lock()
            .map(|state| *state)
            .map_err(|_| RasterCameraControlError)
    }
}

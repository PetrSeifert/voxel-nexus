use crate::{ComputeSceneBuildError, ComputeSceneBundle};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use thiserror::Error;
use voxel_frontend::{
    VoxelChangeSet, VoxelEditOutcome, VoxelFrontend, VoxelFrontendError, VoxelResidencySelection,
    VoxelResidencySelectionId, VoxelSceneId, VoxelSceneRevision, VoxelSceneView,
};

const RETAINED_EVENT_CAPACITY: usize = 64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ComputePreparationBarrierObservation {
    reached_revision: Option<VoxelSceneRevision>,
    completed_block_count: usize,
    finished: bool,
    cancelled: bool,
}

impl ComputePreparationBarrierObservation {
    pub fn reached_revision(self) -> Option<VoxelSceneRevision> {
        self.reached_revision
    }

    pub fn completed_block_count(self) -> usize {
        self.completed_block_count
    }

    pub fn finished(self) -> bool {
        self.finished
    }

    pub fn cancelled(self) -> bool {
        self.cancelled
    }
}

#[derive(Default)]
struct ComputePreparationBarrierState {
    #[cfg(any(test, feature = "qualification"))]
    reached_revision: Option<VoxelSceneRevision>,
    #[cfg(any(test, feature = "qualification"))]
    completed_block_count: usize,
    released: bool,
    #[cfg(any(test, feature = "qualification"))]
    finished: bool,
    #[cfg(any(test, feature = "qualification"))]
    cancelled: bool,
}

struct ComputePreparationBarrierShared {
    #[cfg(any(test, feature = "qualification"))]
    hold_after_completed_blocks: usize,
    state: Mutex<ComputePreparationBarrierState>,
    released: Condvar,
}

impl ComputePreparationBarrierShared {
    #[cfg(any(test, feature = "qualification"))]
    fn observation(&self) -> Result<ComputePreparationBarrierObservation, ()> {
        self.state
            .lock()
            .map(|state| ComputePreparationBarrierObservation {
                reached_revision: state.reached_revision,
                completed_block_count: state.completed_block_count,
                finished: state.finished,
                cancelled: state.cancelled,
            })
            .map_err(|_| ())
    }

    #[cfg(any(test, feature = "qualification"))]
    fn complete_block_and_wait(&self, revision: VoxelSceneRevision) -> Result<(), ()> {
        let mut state = self.state.lock().map_err(|_| ())?;
        if state.reached_revision.is_some() {
            return Ok(());
        }
        state.completed_block_count = state.completed_block_count.checked_add(1).ok_or(())?;
        if state.completed_block_count != self.hold_after_completed_blocks {
            return Ok(());
        }
        state.reached_revision = Some(revision);
        while !state.released {
            state = self.released.wait(state).map_err(|_| ())?;
        }
        Ok(())
    }

    #[cfg(any(test, feature = "qualification"))]
    fn finish(&self, revision: VoxelSceneRevision, cancelled: bool) -> Result<(), ()> {
        let mut state = self.state.lock().map_err(|_| ())?;
        if state.reached_revision == Some(revision) {
            state.finished = true;
            state.cancelled = cancelled;
        }
        Ok(())
    }

    fn release(&self) -> Result<(), ()> {
        let mut state = self.state.lock().map_err(|_| ())?;
        state.released = true;
        self.released.notify_all();
        Ok(())
    }
}

struct ComputeConvergenceControlState {
    pending_outcome: Option<VoxelEditOutcome>,
    pending_selection: Option<VoxelResidencySelection>,
    retry_requested: bool,
    pending_failure: Option<ComputeConvergenceFailurePhase>,
    partial_write_failure: bool,
    status: ComputeConvergenceStatus,
    events: VecDeque<ComputeConvergenceEvent>,
    preparation_barrier: Option<Arc<ComputePreparationBarrierShared>>,
    post_upload_held: bool,
    post_upload_revision: Option<VoxelSceneRevision>,
}

#[derive(Clone)]
pub struct ComputeConvergenceController {
    state: Arc<Mutex<ComputeConvergenceControlState>>,
}

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum ComputeConvergenceControlError {
    #[error("a compute residency selection is already pending at the frame boundary")]
    PendingSelection,
    #[error("the compute convergence control state is unavailable")]
    Unavailable,
    #[error("a compute edit outcome is already pending at the frame boundary")]
    PendingOutcome,
    #[cfg(any(test, feature = "qualification"))]
    #[error("a compute convergence failure is already pending")]
    PendingFailure,
    #[error("a compute convergence retry is already pending")]
    PendingRetry,
    #[cfg(any(test, feature = "qualification"))]
    #[error("the compute preparation barrier needs a positive completed-block count")]
    InvalidCompletedBlockCount,
    #[cfg(any(test, feature = "qualification"))]
    #[error("the compute preparation barrier has not been configured")]
    PreparationBarrierUnavailable,
}

impl ComputeConvergenceController {
    #[cfg(any(test, feature = "qualification"))]
    pub fn inject_partial_write_failure(&self) -> Result<(), ComputeConvergenceControlError> {
        self.state
            .lock()
            .map_err(|_| ComputeConvergenceControlError::Unavailable)?
            .partial_write_failure = true;
        Ok(())
    }

    pub(super) fn take_partial_write_failure(
        &self,
    ) -> Result<bool, ComputeConvergenceControlError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| ComputeConvergenceControlError::Unavailable)?;
        Ok(std::mem::take(&mut state.partial_write_failure))
    }

    fn new(status: ComputeConvergenceStatus, hold_post_upload: bool) -> Self {
        Self {
            state: Arc::new(Mutex::new(ComputeConvergenceControlState {
                pending_outcome: None,
                pending_selection: None,
                retry_requested: false,
                pending_failure: None,
                partial_write_failure: false,
                status,
                events: VecDeque::new(),
                preparation_barrier: None,
                post_upload_held: hold_post_upload,
                post_upload_revision: None,
            })),
        }
    }

    pub fn submit(&self, outcome: VoxelEditOutcome) -> Result<(), ComputeConvergenceControlError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| ComputeConvergenceControlError::Unavailable)?;
        if state.retry_requested {
            return Err(ComputeConvergenceControlError::PendingRetry);
        }
        if state.pending_outcome.is_some() {
            return Err(ComputeConvergenceControlError::PendingOutcome);
        }
        state.pending_outcome = Some(outcome);
        Ok(())
    }

    pub fn status(&self) -> Result<ComputeConvergenceStatus, ComputeConvergenceControlError> {
        self.state
            .lock()
            .map(|state| state.status)
            .map_err(|_| ComputeConvergenceControlError::Unavailable)
    }

    pub fn submit_residency_selection(
        &self,
        selection: VoxelResidencySelection,
    ) -> Result<(), ComputeConvergenceControlError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| ComputeConvergenceControlError::Unavailable)?;
        if state.retry_requested {
            return Err(ComputeConvergenceControlError::PendingRetry);
        }
        if let Some(pending) = &state.pending_selection {
            if pending.identity() == selection.identity() && *pending != selection {
                return Err(ComputeConvergenceControlError::PendingSelection);
            }
            if selection.identity() <= pending.identity() {
                return Ok(());
            }
        }
        state.pending_selection = Some(selection);
        Ok(())
    }

    fn take_pending_selection(
        &self,
    ) -> Result<Option<VoxelResidencySelection>, ComputeConvergenceControlError> {
        self.state
            .lock()
            .map(|mut state| state.pending_selection.take())
            .map_err(|_| ComputeConvergenceControlError::Unavailable)
    }

    pub fn drain_events(
        &self,
    ) -> Result<Vec<ComputeConvergenceEvent>, ComputeConvergenceControlError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| ComputeConvergenceControlError::Unavailable)?;
        Ok(state.events.drain(..).collect())
    }

    pub fn request_retry(&self) -> Result<(), ComputeConvergenceControlError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| ComputeConvergenceControlError::Unavailable)?;
        if state.pending_outcome.is_some() {
            return Err(ComputeConvergenceControlError::PendingOutcome);
        }
        if state.pending_selection.is_some() {
            return Err(ComputeConvergenceControlError::PendingSelection);
        }
        if state.retry_requested {
            return Err(ComputeConvergenceControlError::PendingRetry);
        }
        state.retry_requested = true;
        Ok(())
    }

    #[cfg(any(test, feature = "qualification"))]
    pub fn inject_next_failure(
        &self,
        phase: ComputeConvergenceFailurePhase,
    ) -> Result<(), ComputeConvergenceControlError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| ComputeConvergenceControlError::Unavailable)?;
        if state.pending_failure.is_some() {
            return Err(ComputeConvergenceControlError::PendingFailure);
        }
        state.pending_failure = Some(phase);
        Ok(())
    }

    #[cfg(any(test, feature = "qualification"))]
    pub fn hold_next_preparation_after_blocks(
        &self,
        completed_block_count: usize,
    ) -> Result<(), ComputeConvergenceControlError> {
        if completed_block_count == 0 {
            return Err(ComputeConvergenceControlError::InvalidCompletedBlockCount);
        }
        let barrier = Arc::new(ComputePreparationBarrierShared {
            hold_after_completed_blocks: completed_block_count,
            state: Mutex::new(ComputePreparationBarrierState::default()),
            released: Condvar::new(),
        });
        self.state
            .lock()
            .map_err(|_| ComputeConvergenceControlError::Unavailable)?
            .preparation_barrier = Some(barrier);
        Ok(())
    }

    #[cfg(any(test, feature = "qualification"))]
    pub fn preparation_barrier_observation(
        &self,
    ) -> Result<Option<ComputePreparationBarrierObservation>, ComputeConvergenceControlError> {
        let barrier = self
            .state
            .lock()
            .map_err(|_| ComputeConvergenceControlError::Unavailable)?
            .preparation_barrier
            .clone();
        barrier
            .map(|barrier| {
                barrier
                    .observation()
                    .map_err(|_| ComputeConvergenceControlError::Unavailable)
            })
            .transpose()
    }

    #[cfg(any(test, feature = "qualification"))]
    pub fn release_preparation_barrier(&self) -> Result<(), ComputeConvergenceControlError> {
        let barrier = self
            .state
            .lock()
            .map_err(|_| ComputeConvergenceControlError::Unavailable)?
            .preparation_barrier
            .clone()
            .ok_or(ComputeConvergenceControlError::PreparationBarrierUnavailable)?;
        barrier
            .release()
            .map_err(|_| ComputeConvergenceControlError::Unavailable)
    }

    pub fn post_upload_revision(
        &self,
    ) -> Result<Option<VoxelSceneRevision>, ComputeConvergenceControlError> {
        self.state
            .lock()
            .map(|state| state.post_upload_revision)
            .map_err(|_| ComputeConvergenceControlError::Unavailable)
    }

    #[cfg(any(test, feature = "qualification"))]
    pub fn release_post_upload(&self) -> Result<(), ComputeConvergenceControlError> {
        self.state
            .lock()
            .map_err(|_| ComputeConvergenceControlError::Unavailable)?
            .post_upload_held = false;
        Ok(())
    }

    pub(crate) fn take_pending_outcome(
        &self,
    ) -> Result<Option<VoxelEditOutcome>, ComputeConvergenceControlError> {
        self.state
            .lock()
            .map(|mut state| state.pending_outcome.take())
            .map_err(|_| ComputeConvergenceControlError::Unavailable)
    }

    pub(crate) fn take_retry_request(&self) -> Result<bool, ComputeConvergenceControlError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| ComputeConvergenceControlError::Unavailable)?;
        let retry_requested = state.retry_requested;
        state.retry_requested = false;
        Ok(retry_requested)
    }

    fn take_injected_failure(
        &self,
        phase: ComputeConvergenceFailurePhase,
    ) -> Result<bool, ComputeConvergenceControlError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| ComputeConvergenceControlError::Unavailable)?;
        if state.pending_failure != Some(phase) {
            return Ok(false);
        }
        state.pending_failure = None;
        Ok(true)
    }

    fn preparation_barrier(
        &self,
    ) -> Result<Option<Arc<ComputePreparationBarrierShared>>, ComputeConvergenceControlError> {
        self.state
            .lock()
            .map(|state| state.preparation_barrier.clone())
            .map_err(|_| ComputeConvergenceControlError::Unavailable)
    }

    pub(crate) fn synchronize(
        &self,
        status: ComputeConvergenceStatus,
        events: Vec<ComputeConvergenceEvent>,
    ) -> Result<(), ComputeConvergenceControlError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| ComputeConvergenceControlError::Unavailable)?;
        state.status = status;
        for event in events {
            if state.events.len() == RETAINED_EVENT_CAPACITY {
                state.events.pop_front();
            }
            state.events.push_back(event);
        }
        Ok(())
    }

    pub(crate) fn hold_post_upload(
        &self,
        revision: VoxelSceneRevision,
    ) -> Result<bool, ComputeConvergenceControlError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| ComputeConvergenceControlError::Unavailable)?;
        if !state.post_upload_held {
            return Ok(false);
        }
        state.post_upload_revision = Some(revision);
        Ok(true)
    }

    pub(crate) fn clear_post_upload_revision(
        &self,
        revision: VoxelSceneRevision,
    ) -> Result<(), ComputeConvergenceControlError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| ComputeConvergenceControlError::Unavailable)?;
        if state.post_upload_revision == Some(revision) {
            state.post_upload_revision = None;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ComputeConvergenceGeneration(u64);

impl ComputeConvergenceGeneration {
    fn initial() -> Self {
        Self(0)
    }

    fn checked_successor(self) -> Option<Self> {
        self.0.checked_add(1).map(Self)
    }

    pub fn value(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ComputeConvergenceWorkStamp {
    revision: VoxelSceneRevision,
    selection: Option<VoxelResidencySelectionId>,
    generation: ComputeConvergenceGeneration,
}

impl ComputeConvergenceWorkStamp {
    fn new(revision: VoxelSceneRevision, generation: ComputeConvergenceGeneration) -> Self {
        Self {
            revision,
            selection: None,
            generation,
        }
    }

    pub fn revision(self) -> VoxelSceneRevision {
        self.revision
    }

    pub fn generation(self) -> ComputeConvergenceGeneration {
        self.generation
    }

    pub fn residency_selection(self) -> Option<VoxelResidencySelectionId> {
        self.selection
    }

    fn with_selection(mut self, selection: Option<&VoxelResidencySelection>) -> Self {
        self.selection = selection.map(VoxelResidencySelection::identity);
        self
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ComputeConvergenceAcceptance {
    Unchanged { revision: VoxelSceneRevision },
    NotNewer { revision: VoxelSceneRevision },
    Accepted { stamp: ComputeConvergenceWorkStamp },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ComputeConvergenceRetry {
    NoRequiredWork,
    Requested { stamp: ComputeConvergenceWorkStamp },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ComputeConvergenceFailurePhase {
    Preparation,
    Upload,
    Installation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ComputeCandidateDisposition {
    SupersededBeforeUpload,
    SupersededAfterUpload,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComputeConvergenceFailure {
    scene_identity: VoxelSceneId,
    failed: ComputeConvergenceWorkStamp,
    required_revision: VoxelSceneRevision,
    visible_revision: VoxelSceneRevision,
    phase: ComputeConvergenceFailurePhase,
    source: String,
}

impl ComputeConvergenceFailure {
    pub fn scene_identity(&self) -> &VoxelSceneId {
        &self.scene_identity
    }

    pub fn failed(&self) -> ComputeConvergenceWorkStamp {
        self.failed
    }

    pub fn required_revision(&self) -> VoxelSceneRevision {
        self.required_revision
    }

    pub fn visible_revision(&self) -> VoxelSceneRevision {
        self.visible_revision
    }

    pub fn phase(&self) -> ComputeConvergenceFailurePhase {
        self.phase
    }

    pub fn source(&self) -> &str {
        &self.source
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ComputeConvergenceEvent {
    PreparationStarted {
        stamp: ComputeConvergenceWorkStamp,
    },
    PreparationReady {
        stamp: ComputeConvergenceWorkStamp,
    },
    CandidateUploaded {
        stamp: ComputeConvergenceWorkStamp,
    },
    CandidateDiscarded {
        stamp: ComputeConvergenceWorkStamp,
        disposition: ComputeCandidateDisposition,
    },
    CandidateInstalled {
        stamp: ComputeConvergenceWorkStamp,
    },
    Failure(ComputeConvergenceFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ComputeConvergenceStatus {
    required_revision: VoxelSceneRevision,
    required_selection: Option<VoxelResidencySelectionId>,
    visible_revision: VoxelSceneRevision,
    required_generation: ComputeConvergenceGeneration,
    installed: ComputeConvergenceWorkStamp,
    preparing: Option<ComputeConvergenceWorkStamp>,
    pending: Option<ComputeConvergenceWorkStamp>,
    paused: Option<ComputeConvergenceWorkStamp>,
    hidden: Option<ComputeConvergenceWorkStamp>,
    cleanup_debt: Option<ComputeConvergenceWorkStamp>,
    retained_event_count: usize,
    dropped_event_count: u64,
    worker_count: usize,
    pub installed_growth: Option<crate::BrickmapGrowthObservations>,
    pub hidden_growth: Option<crate::BrickmapGrowthObservations>,
    pub installed_patch: crate::BrickmapPatchObservations,
    pub hidden_patch: Option<crate::BrickmapPatchObservations>,
}

impl ComputeConvergenceStatus {
    pub fn required_residency_selection(self) -> Option<VoxelResidencySelectionId> {
        self.required_selection
    }

    pub fn installed_residency_selection(self) -> Option<VoxelResidencySelectionId> {
        self.installed.selection
    }

    pub fn required_revision(self) -> VoxelSceneRevision {
        self.required_revision
    }

    pub fn visible_revision(self) -> VoxelSceneRevision {
        self.visible_revision
    }

    pub fn required_generation(self) -> ComputeConvergenceGeneration {
        self.required_generation
    }

    pub fn installed(self) -> ComputeConvergenceWorkStamp {
        self.installed
    }

    pub fn preparing(self) -> Option<ComputeConvergenceWorkStamp> {
        self.preparing
    }

    pub fn pending(self) -> Option<ComputeConvergenceWorkStamp> {
        self.pending
    }

    pub fn paused(self) -> Option<ComputeConvergenceWorkStamp> {
        self.paused
    }

    pub fn hidden(self) -> Option<ComputeConvergenceWorkStamp> {
        self.hidden
    }

    pub fn cleanup_debt(self) -> Option<ComputeConvergenceWorkStamp> {
        self.cleanup_debt
    }

    pub fn retained_event_count(self) -> usize {
        self.retained_event_count
    }

    pub fn dropped_event_count(self) -> u64 {
        self.dropped_event_count
    }

    pub fn worker_count(self) -> usize {
        self.worker_count
    }
}

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub(crate) enum ComputeConvergenceShutdownError {
    #[error("the compute convergence preparation barrier is unavailable")]
    PreparationBarrierUnavailable,
    #[error("compute convergence worker terminated for Voxel Scene Revision {revision}")]
    WorkerTerminated { revision: VoxelSceneRevision },
}

#[derive(Debug, Error)]
pub enum ComputeConvergenceError {
    #[error("this compute path has no streamed residency owner")]
    ResidencyUnavailable,
    #[error("streamed Brickmap construction supports at most nine selected volumes")]
    ResidencyVolumeLimit,
    #[error(transparent)]
    Residency(#[from] VoxelFrontendError),
    #[error("brickmap patch base revision or pool allocation changed before installation")]
    PatchBaseMismatch,
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
    #[error("compute convergence generation identity overflow")]
    GenerationOverflow,
    #[error("could not start preparation for Voxel Scene Revision {revision}: {source}")]
    PreparationStart {
        revision: VoxelSceneRevision,
        #[source]
        source: std::io::Error,
    },
    #[error(
        "visible compute installation revision mismatch: expected {expected}, received {actual}"
    )]
    VisibleRevisionMismatch {
        expected: VoxelSceneRevision,
        actual: VoxelSceneRevision,
    },
    #[error(transparent)]
    Control(#[from] ComputeConvergenceControlError),
}

#[derive(Clone)]
struct ComputePreparationTarget {
    view: VoxelSceneView,
    residency: Option<(Arc<VoxelFrontend>, VoxelResidencySelection)>,
    base: ComputeSceneBundle,
    changes: Vec<VoxelChangeSet>,
}

impl ComputePreparationTarget {
    fn stamp(&self, generation: ComputeConvergenceGeneration) -> ComputeConvergenceWorkStamp {
        ComputeConvergenceWorkStamp::new(self.view.revision(), generation)
            .with_selection(self.residency.as_ref().map(|(_, selection)| selection))
    }

    fn prepare(
        &self,
        cancellation: &AtomicBool,
        preparation_barrier: Option<&ComputePreparationBarrierShared>,
    ) -> Result<ComputeSceneBundle, ComputeSceneBuildError> {
        let block_completed = || {
            #[cfg(any(test, feature = "qualification"))]
            if let Some(barrier) = preparation_barrier {
                barrier
                    .complete_block_and_wait(self.view.revision())
                    .map_err(|_| ComputeSceneBuildError::PreparationBarrier)?;
            }
            #[cfg(not(any(test, feature = "qualification")))]
            let _ = preparation_barrier;
            Ok(())
        };
        if let Some((frontend, selection)) = &self.residency {
            let copies = frontend
                .materialize_residency_until_cancelled(selection, &self.view, || {
                    cancellation.load(Ordering::Acquire)
                })
                .map_err(|error| match error {
                    VoxelFrontendError::ResidencySuperseded
                        if cancellation.load(Ordering::Acquire) =>
                    {
                        ComputeSceneBuildError::Cancelled
                    }
                    error => ComputeSceneBuildError::VoxelFrontend(error),
                })?;
            let counter = self
                .base
                .residency_copy_counter()
                .ok_or(ComputeSceneBuildError::PatchBaseMismatch)?;
            return ComputeSceneBundle::from_residency_with_progress(
                copies,
                self.base.representation(),
                counter,
                || {
                    if cancellation.load(Ordering::Acquire) {
                        return Err(ComputeSceneBuildError::Cancelled);
                    }
                    block_completed()?;
                    if cancellation.load(Ordering::Acquire) {
                        return Err(ComputeSceneBuildError::Cancelled);
                    }
                    Ok(())
                },
            );
        }
        self.base.successor_with_block_completion(
            &self.view,
            &self.changes,
            || cancellation.load(Ordering::Acquire),
            block_completed,
        )
    }
}

enum ComputePreparationCompletion {
    Completed(Result<Box<ComputeSceneBundle>, ComputeSceneBuildError>),
    WorkerTerminated,
}

enum ComputeActivePreparationStatus {
    Running,
    Ready(Box<ComputeSceneBundle>),
}

struct ComputeActivePreparation {
    generation: ComputeConvergenceGeneration,
    target: ComputePreparationTarget,
    cancellation: Arc<AtomicBool>,
    completion_receiver: mpsc::Receiver<ComputePreparationCompletion>,
    worker: Option<JoinHandle<()>>,
    status: ComputeActivePreparationStatus,
    preparation_barrier: Option<Arc<ComputePreparationBarrierShared>>,
}

impl ComputeActivePreparation {
    fn stamp(&self) -> ComputeConvergenceWorkStamp {
        self.target.stamp(self.generation)
    }
}

struct ComputeHiddenCandidate {
    generation: ComputeConvergenceGeneration,
    target: ComputePreparationTarget,
    bundle: ComputeSceneBundle,
}

impl ComputeHiddenCandidate {
    fn stamp(&self) -> ComputeConvergenceWorkStamp {
        self.target.stamp(self.generation)
    }
}

struct ComputeConvergenceEvents {
    retained: VecDeque<ComputeConvergenceEvent>,
    dropped: u64,
}

impl ComputeConvergenceEvents {
    fn new() -> Self {
        Self {
            retained: VecDeque::with_capacity(RETAINED_EVENT_CAPACITY),
            dropped: 0,
        }
    }

    fn push(&mut self, event: ComputeConvergenceEvent) {
        if self.retained.len() == RETAINED_EVENT_CAPACITY {
            self.retained.pop_front();
            self.dropped = self.dropped.saturating_add(1);
        }
        self.retained.push_back(event);
    }

    fn drain(&mut self) -> Vec<ComputeConvergenceEvent> {
        self.retained.drain(..).collect()
    }
}

pub(crate) struct ComputeConvergence {
    scene_identity: VoxelSceneId,
    installed_bundle: ComputeSceneBundle,
    installed_generation: ComputeConvergenceGeneration,
    changes: Vec<VoxelChangeSet>,
    required_revision: VoxelSceneRevision,
    frontend: Option<Arc<VoxelFrontend>>,
    required_view: Option<VoxelSceneView>,
    required_selection: Option<VoxelResidencySelection>,
    residency_copy_counter: Arc<AtomicUsize>,
    required_generation: ComputeConvergenceGeneration,
    active: Option<ComputeActivePreparation>,
    pending: Option<(ComputeConvergenceGeneration, ComputePreparationTarget)>,
    paused: Option<(ComputeConvergenceGeneration, ComputePreparationTarget)>,
    hidden: Option<ComputeHiddenCandidate>,
    events: ComputeConvergenceEvents,
    control: Option<ComputeConvergenceController>,
}

impl ComputeConvergence {
    pub(crate) fn new(installed_bundle: ComputeSceneBundle) -> Self {
        let scene_identity = installed_bundle.scene_identity().clone();
        let required_revision = installed_bundle.revision();
        let required_selection = installed_bundle.residency_selection().cloned();
        let required_view = installed_bundle.residency_view().cloned();
        let residency_copy_counter = installed_bundle
            .residency_copy_counter()
            .unwrap_or_default();
        Self {
            scene_identity,
            installed_bundle,
            installed_generation: ComputeConvergenceGeneration::initial(),
            changes: Vec::new(),
            required_revision,
            frontend: None,
            required_view,
            required_selection,
            residency_copy_counter,
            required_generation: ComputeConvergenceGeneration::initial(),
            active: None,
            pending: None,
            paused: None,
            hidden: None,
            events: ComputeConvergenceEvents::new(),
            control: None,
        }
    }

    pub(crate) fn with_frontend(mut self, frontend: Arc<VoxelFrontend>) -> Self {
        self.frontend = Some(frontend);
        self
    }

    pub(crate) fn required_selection(&self) -> Option<&VoxelResidencySelection> {
        self.required_selection.as_ref()
    }

    pub(crate) fn residency_held_copy_count(&self) -> usize {
        self.residency_copy_counter.load(Ordering::Acquire)
    }

    pub(crate) fn accept_residency_selection(
        &mut self,
        selection: VoxelResidencySelection,
    ) -> Result<(), ComputeConvergenceError> {
        let frontend = self
            .frontend
            .as_ref()
            .ok_or(ComputeConvergenceError::ResidencyUnavailable)?;
        let view = self
            .required_view
            .as_ref()
            .ok_or(ComputeConvergenceError::ResidencyUnavailable)?;
        if selection.scene_id() != &self.scene_identity {
            return Err(VoxelFrontendError::ResidencySceneMismatch.into());
        }
        if selection.volumes().len() > crate::MAXIMUM_RESIDENCY_VOLUME_COUNT {
            return Err(ComputeConvergenceError::ResidencyVolumeLimit);
        }
        for identity in selection.volumes() {
            view.volume_content_version(identity)?;
        }
        if let Some(required) = &self.required_selection {
            if selection.identity() == required.identity() && selection != *required {
                return Err(VoxelFrontendError::ResidencyIdentityConflict.into());
            }
            if selection.identity() <= required.identity() {
                return Ok(());
            }
        }
        let generation = self
            .required_generation
            .checked_successor()
            .ok_or(ComputeConvergenceError::GenerationOverflow)?;
        let target = ComputePreparationTarget {
            view: view.clone(),
            residency: Some((frontend.clone(), selection.clone())),
            base: self.installed_bundle.clone(),
            changes: Vec::new(),
        };
        self.schedule_target(generation, target)?;
        self.required_selection = Some(selection);
        self.required_generation = generation;
        Ok(())
    }

    pub(crate) fn enable_control(
        &mut self,
        hold_post_upload: bool,
    ) -> ComputeConvergenceController {
        let controller = ComputeConvergenceController::new(self.status(), hold_post_upload);
        self.control = Some(controller.clone());
        controller
    }

    pub(crate) fn installed_bundle(&self) -> &ComputeSceneBundle {
        &self.installed_bundle
    }

    pub(crate) fn status(&self) -> ComputeConvergenceStatus {
        ComputeConvergenceStatus {
            required_revision: self.required_revision,
            required_selection: self
                .required_selection
                .as_ref()
                .map(VoxelResidencySelection::identity),
            visible_revision: self.installed_bundle.revision(),
            required_generation: self.required_generation,
            installed: ComputeConvergenceWorkStamp::new(
                self.installed_bundle.revision(),
                self.installed_generation,
            )
            .with_selection(self.installed_bundle.residency_selection()),
            preparing: self.active.as_ref().map(ComputeActivePreparation::stamp),
            pending: self
                .pending
                .as_ref()
                .map(|(generation, target)| target.stamp(*generation)),
            paused: self
                .paused
                .as_ref()
                .map(|(generation, target)| target.stamp(*generation)),
            hidden: self.hidden.as_ref().map(ComputeHiddenCandidate::stamp),
            cleanup_debt: None,
            retained_event_count: self.events.retained.len(),
            dropped_event_count: self.events.dropped,
            installed_growth: self.installed_bundle.brickmap_growth_observations(),
            hidden_growth: self
                .hidden_bundle()
                .and_then(ComputeSceneBundle::brickmap_growth_observations),
            installed_patch: self.installed_bundle.brickmap_patch_observations(),
            hidden_patch: self
                .hidden_bundle()
                .map(ComputeSceneBundle::brickmap_patch_observations),
            worker_count: usize::from(
                self.active
                    .as_ref()
                    .is_some_and(|active| active.worker.is_some()),
            ),
        }
    }

    pub(crate) fn owned_view_count(&self) -> usize {
        usize::from(self.required_view.is_some())
            + usize::from(self.active.is_some())
            + usize::from(
                self.active
                    .as_ref()
                    .is_some_and(|active| active.worker.is_some()),
            )
            + usize::from(self.pending.is_some())
            + usize::from(self.paused.is_some())
            + usize::from(self.hidden.is_some())
    }

    pub(crate) fn accept(
        &mut self,
        outcome: VoxelEditOutcome,
    ) -> Result<ComputeConvergenceAcceptance, ComputeConvergenceError> {
        let (view, change_set) = match outcome {
            VoxelEditOutcome::Unchanged(view) => {
                return Ok(ComputeConvergenceAcceptance::Unchanged {
                    revision: view.revision(),
                });
            }
            VoxelEditOutcome::Changed { view, change_set } => (view, change_set),
        };
        if change_set.scene_identity() != &self.scene_identity
            || view.scene_id() != &self.scene_identity
        {
            return Err(ComputeConvergenceError::SceneIdentityMismatch {
                expected: self.scene_identity.clone(),
                change_set: change_set.scene_identity().clone(),
                view: view.scene_id().clone(),
            });
        }
        if change_set.successor_revision() != view.revision() {
            return Err(ComputeConvergenceError::SuccessorRevisionMismatch {
                change_set: change_set.successor_revision(),
                view: view.revision(),
            });
        }
        if !view.revision().is_newer_than(self.required_revision) {
            return Ok(ComputeConvergenceAcceptance::NotNewer {
                revision: view.revision(),
            });
        }

        let generation = self
            .required_generation
            .checked_successor()
            .ok_or(ComputeConvergenceError::GenerationOverflow)?;
        // A stalled consumer must not retain an unbounded history. Dropping the
        // chain makes the next preparation use its authoritative full view.
        let mut changes = if self.changes.len() < RETAINED_EVENT_CAPACITY {
            self.changes.clone()
        } else {
            Vec::new()
        };
        changes.push(change_set.clone());
        let target = ComputePreparationTarget {
            view: view.clone(),
            residency: self
                .frontend
                .as_ref()
                .zip(self.required_selection.as_ref())
                .map(|(frontend, selection)| (frontend.clone(), selection.clone())),
            base: self.installed_bundle.clone(),
            changes: changes.clone(),
        };
        self.schedule_target(generation, target)?;
        self.changes = changes;
        self.required_generation = generation;
        self.required_revision = change_set.successor_revision();
        if self.frontend.is_some() {
            self.required_view = Some(view);
        }
        Ok(ComputeConvergenceAcceptance::Accepted {
            stamp: ComputeConvergenceWorkStamp::new(self.required_revision, generation)
                .with_selection(self.required_selection.as_ref()),
        })
    }

    pub(crate) fn request_retry(
        &mut self,
    ) -> Result<ComputeConvergenceRetry, ComputeConvergenceError> {
        if self.required_revision == self.installed_bundle.revision()
            && self.required_selection.as_ref() == self.installed_bundle.residency_selection()
        {
            return Ok(ComputeConvergenceRetry::NoRequiredWork);
        }
        let Some(target) = self.newest_target().cloned() else {
            return Ok(ComputeConvergenceRetry::NoRequiredWork);
        };
        let generation = self
            .required_generation
            .checked_successor()
            .ok_or(ComputeConvergenceError::GenerationOverflow)?;
        self.schedule_target(generation, target)?;
        self.required_generation = generation;
        Ok(ComputeConvergenceRetry::Requested {
            stamp: ComputeConvergenceWorkStamp::new(self.required_revision, generation)
                .with_selection(self.required_selection.as_ref()),
        })
    }

    pub(crate) fn drain_events(&mut self) -> Vec<ComputeConvergenceEvent> {
        self.poll_preparation();
        self.events.drain()
    }

    pub(crate) fn apply_controlled_request_at_frame_boundary(
        &mut self,
    ) -> Result<(), ComputeConvergenceError> {
        let Some(control) = self.control.as_ref() else {
            return Ok(());
        };
        let outcome = control.take_pending_outcome()?;
        let selection = control.take_pending_selection()?;
        let retry_requested = control.take_retry_request()?;
        if let Some(outcome) = outcome {
            self.accept(outcome)?;
        } else if retry_requested {
            self.request_retry()?;
        }
        if let Some(selection) = selection {
            self.accept_residency_selection(selection)?;
        }
        Ok(())
    }

    pub(crate) fn retain_ready_candidate(&mut self) {
        self.poll_preparation();
        self.reject_stale_hidden();
        if self.hidden.is_some() {
            return;
        }
        let Some(active) = self.active.take() else {
            return;
        };
        let stamp = active.stamp();
        let ComputeActivePreparationStatus::Ready(bundle) = active.status else {
            self.active = Some(active);
            return;
        };
        if stamp.generation != self.required_generation || stamp.revision != self.required_revision
        {
            self.events
                .push(ComputeConvergenceEvent::CandidateDiscarded {
                    stamp,
                    disposition: ComputeCandidateDisposition::SupersededBeforeUpload,
                });
            self.start_pending_preparation();
            return;
        }
        self.hidden = Some(ComputeHiddenCandidate {
            generation: active.generation,
            target: active.target,
            bundle: *bundle,
        });
    }

    pub(crate) fn hidden_bundle(&self) -> Option<&ComputeSceneBundle> {
        self.hidden.as_ref().map(|candidate| &candidate.bundle)
    }

    pub(crate) fn record_growth_allocation(&mut self, peak_bytes: u64) {
        if let Some(candidate) = &mut self.hidden {
            candidate.bundle.record_growth_allocation(peak_bytes);
        }
    }

    pub(crate) fn prepare_hidden_allocation(
        &mut self,
        old_allocation_bytes: u64,
        new_allocation_bytes: u64,
        staging_bytes: u64,
    ) -> Result<(), ComputeSceneBuildError> {
        let candidate = self
            .hidden
            .as_mut()
            .ok_or(ComputeSceneBuildError::PatchBaseMismatch)?;
        match candidate.bundle.predict_allocation_peak(
            old_allocation_bytes,
            new_allocation_bytes,
            staging_bytes,
        ) {
            Ok(predicted_peak_bytes) => {
                candidate
                    .bundle
                    .record_growth_prediction(predicted_peak_bytes);
                Ok(())
            }
            Err(error) => {
                self.fail_hidden(ComputeConvergenceFailurePhase::Upload, error.to_string());
                Err(error)
            }
        }
    }

    pub(crate) fn mark_hidden_uploaded(&mut self) {
        if let Some(candidate) = &self.hidden {
            self.events
                .push(ComputeConvergenceEvent::CandidateUploaded {
                    stamp: candidate.stamp(),
                });
        }
    }

    pub(crate) fn validate_hidden_base(&self) -> Result<(), ComputeConvergenceError> {
        if self
            .hidden
            .as_ref()
            .is_some_and(|candidate| !candidate.bundle.patch_base_matches(&self.installed_bundle))
        {
            return Err(ComputeConvergenceError::PatchBaseMismatch);
        }
        Ok(())
    }

    pub(crate) fn install_hidden(
        &mut self,
        actual_visible_revision: VoxelSceneRevision,
    ) -> Result<Option<ComputeSceneBundle>, ComputeConvergenceError> {
        if actual_visible_revision != self.installed_bundle.revision() {
            return Err(ComputeConvergenceError::VisibleRevisionMismatch {
                expected: self.installed_bundle.revision(),
                actual: actual_visible_revision,
            });
        }
        self.reject_stale_hidden();
        self.validate_hidden_base()?;
        let Some(candidate) = self.hidden.take() else {
            return Ok(None);
        };
        let stamp = candidate.stamp();
        let retired = std::mem::replace(&mut self.installed_bundle, candidate.bundle);
        self.installed_bundle.finish_installation();
        self.installed_generation = candidate.generation;
        self.changes.clear();
        self.events
            .push(ComputeConvergenceEvent::CandidateInstalled { stamp });
        Ok(Some(retired))
    }

    pub(crate) fn fail_hidden(&mut self, phase: ComputeConvergenceFailurePhase, source: String) {
        let Some(candidate) = self.hidden.take() else {
            return;
        };
        let stamp = candidate.stamp();
        let is_current = stamp.generation == self.required_generation
            && stamp.revision == self.required_revision;
        self.record_failure(stamp, phase, source);
        if is_current {
            self.pending = None;
            self.paused = Some((candidate.generation, candidate.target));
        } else {
            self.start_pending_preparation();
        }
    }

    #[cfg(any(test, feature = "qualification"))]
    pub(crate) fn fail_hidden_if_injected(
        &mut self,
        phase: ComputeConvergenceFailurePhase,
    ) -> Result<bool, ComputeConvergenceError> {
        let Some(control) = self.control.as_ref() else {
            return Ok(false);
        };
        if !control.take_injected_failure(phase)? {
            return Ok(false);
        }
        let source = match phase {
            ComputeConvergenceFailurePhase::Preparation => "injected preparation failure",
            ComputeConvergenceFailurePhase::Upload => "injected upload failure",
            ComputeConvergenceFailurePhase::Installation => "injected installation failure",
        };
        self.fail_hidden(phase, source.to_owned());
        Ok(true)
    }

    pub(crate) fn shutdown(&mut self) -> Result<(), ComputeConvergenceShutdownError> {
        if let Some(active) = &self.active {
            active.cancellation.store(true, Ordering::Release);
        }
        let barrier_error = self.active.as_ref().and_then(|active| {
            active
                .preparation_barrier
                .as_ref()
                .and_then(|barrier| barrier.release().err())
                .map(|_| ComputeConvergenceShutdownError::PreparationBarrierUnavailable)
        });
        let active = self.active.take();
        self.pending = None;
        self.paused = None;
        self.hidden = None;
        self.required_view = None;
        let worker_error = if let Some(mut active) = active
            && let Some(worker) = active.worker.take()
            && worker.join().is_err()
        {
            Some(ComputeConvergenceShutdownError::WorkerTerminated {
                revision: active.target.view.revision(),
            })
        } else {
            None
        };
        self.installed_bundle.release_residency();
        self.frontend = None;
        barrier_error.or(worker_error).map_or(Ok(()), Err)
    }

    fn schedule_target(
        &mut self,
        generation: ComputeConvergenceGeneration,
        target: ComputePreparationTarget,
    ) -> Result<(), ComputeConvergenceError> {
        if self
            .active
            .as_ref()
            .is_some_and(|active| matches!(active.status, ComputeActivePreparationStatus::Ready(_)))
        {
            let prior = self.active.take();
            let prior_stamp = prior.as_ref().map(ComputeActivePreparation::stamp);
            let prior_target = prior
                .as_ref()
                .map(|active| (active.generation, active.target.clone()));
            drop(prior);
            let preparation_barrier = self.preparation_barrier()?;
            let preparation = Self::start_preparation(
                generation,
                target,
                &mut self.events,
                self.control.clone(),
                preparation_barrier,
            );
            match preparation {
                Ok(preparation) => {
                    if let Some(stamp) = prior_stamp {
                        self.events
                            .push(ComputeConvergenceEvent::CandidateDiscarded {
                                stamp,
                                disposition: ComputeCandidateDisposition::SupersededBeforeUpload,
                            });
                    }
                    self.pending = None;
                    self.paused = None;
                    self.active = Some(preparation);
                    return Ok(());
                }
                Err(error) => {
                    if let Some(prior_target) = prior_target {
                        self.paused = Some(prior_target);
                    }
                    return Err(error);
                }
            }
        }
        if let Some(active) = &self.active {
            active.cancellation.store(true, Ordering::Release);
            self.pending = Some((generation, target));
            self.paused = None;
            return Ok(());
        }
        if let Some(candidate) = self.hidden.take() {
            self.events
                .push(ComputeConvergenceEvent::CandidateDiscarded {
                    stamp: candidate.stamp(),
                    disposition: ComputeCandidateDisposition::SupersededAfterUpload,
                });
        }
        let preparation_barrier = self.preparation_barrier()?;
        let preparation = Self::start_preparation(
            generation,
            target,
            &mut self.events,
            self.control.clone(),
            preparation_barrier,
        )?;
        self.pending = None;
        self.paused = None;
        self.active = Some(preparation);
        Ok(())
    }

    fn start_preparation(
        generation: ComputeConvergenceGeneration,
        target: ComputePreparationTarget,
        events: &mut ComputeConvergenceEvents,
        control: Option<ComputeConvergenceController>,
        preparation_barrier: Option<Arc<ComputePreparationBarrierShared>>,
    ) -> Result<ComputeActivePreparation, ComputeConvergenceError> {
        let stamp = target.stamp(generation);
        let cancellation = Arc::new(AtomicBool::new(false));
        let (completion_sender, completion_receiver) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name(format!("compute-convergence-{}", stamp.revision))
            .spawn({
                let view = target.view.clone();
                let target = target.clone();
                let cancellation = Arc::clone(&cancellation);
                let preparation_barrier = preparation_barrier.clone();
                move || {
                    let injected_failure = control
                        .as_ref()
                        .map(|control| {
                            control.take_injected_failure(
                                ComputeConvergenceFailurePhase::Preparation,
                            )
                        })
                        .transpose();
                    let result = match injected_failure {
                        #[cfg(any(test, feature = "qualification"))]
                        Ok(Some(true)) => Err(ComputeSceneBuildError::InjectedPreparationFailure),
                        Ok(_) => target.prepare(&cancellation, preparation_barrier.as_deref()),
                        Err(_) => Err(ComputeSceneBuildError::PreparationControl),
                    };
                    #[cfg(any(test, feature = "qualification"))]
                    let result = {
                    let mut result = result;
                    if let Some(barrier) = &preparation_barrier {
                        let cancelled = matches!(result, Err(ComputeSceneBuildError::Cancelled));
                        if barrier.finish(view.revision(), cancelled).is_err() {
                            result = Err(ComputeSceneBuildError::PreparationBarrier);
                        }
                    }
                    result
                    };
                    if completion_sender
                        .send(ComputePreparationCompletion::Completed(result.map(Box::new)))
                        .is_err()
                    {
                        eprintln!(
                            "compute convergence result receiver closed for Voxel Scene Revision {}",
                            view.revision()
                        );
                    }
                }
            })
            .map_err(|source| ComputeConvergenceError::PreparationStart {
                revision: stamp.revision,
                source,
            })?;
        events.push(ComputeConvergenceEvent::PreparationStarted { stamp });
        Ok(ComputeActivePreparation {
            generation,
            target,
            cancellation,
            completion_receiver,
            worker: Some(worker),
            status: ComputeActivePreparationStatus::Running,
            preparation_barrier,
        })
    }

    fn poll_preparation(&mut self) {
        let completion = {
            let Some(active) = self.active.as_mut() else {
                return;
            };
            if matches!(active.status, ComputeActivePreparationStatus::Ready(_)) {
                return;
            }
            match active.completion_receiver.try_recv() {
                Ok(completion) => completion,
                Err(mpsc::TryRecvError::Empty) => return,
                Err(mpsc::TryRecvError::Disconnected) => {
                    ComputePreparationCompletion::WorkerTerminated
                }
            }
        };
        let Some(mut active) = self.active.take() else {
            return;
        };
        let stamp = active.stamp();
        let worker_terminated = active
            .worker
            .take()
            .is_none_or(|worker| worker.join().is_err());
        let completion = if worker_terminated {
            ComputePreparationCompletion::WorkerTerminated
        } else {
            completion
        };
        let is_current = stamp.generation == self.required_generation
            && stamp.revision == self.required_revision;
        match completion {
            ComputePreparationCompletion::Completed(Ok(bundle)) if is_current => {
                active.status = ComputeActivePreparationStatus::Ready(bundle);
                self.active = Some(active);
                self.events
                    .push(ComputeConvergenceEvent::PreparationReady { stamp });
            }
            ComputePreparationCompletion::Completed(Err(ComputeSceneBuildError::Cancelled)) => {
                self.events
                    .push(ComputeConvergenceEvent::CandidateDiscarded {
                        stamp,
                        disposition: ComputeCandidateDisposition::SupersededBeforeUpload,
                    });
                self.start_pending_preparation();
            }
            ComputePreparationCompletion::Completed(Ok(bundle)) => {
                drop(bundle);
                self.events
                    .push(ComputeConvergenceEvent::CandidateDiscarded {
                        stamp,
                        disposition: ComputeCandidateDisposition::SupersededBeforeUpload,
                    });
                self.start_pending_preparation();
            }
            ComputePreparationCompletion::Completed(Err(error)) => {
                self.record_failure(
                    stamp,
                    ComputeConvergenceFailurePhase::Preparation,
                    error.to_string(),
                );
                if is_current {
                    self.pending = None;
                    self.paused = Some((active.generation, active.target));
                } else {
                    self.start_pending_preparation();
                }
            }
            ComputePreparationCompletion::WorkerTerminated => {
                self.record_failure(
                    stamp,
                    ComputeConvergenceFailurePhase::Preparation,
                    "compute convergence worker terminated".to_owned(),
                );
                if is_current {
                    self.pending = None;
                    self.paused = Some((active.generation, active.target));
                } else {
                    self.start_pending_preparation();
                }
            }
        }
    }

    fn reject_stale_hidden(&mut self) {
        let is_stale = self.hidden.as_ref().is_some_and(|candidate| {
            candidate.generation != self.required_generation
                || candidate.target.view.revision() != self.required_revision
        });
        if !is_stale {
            return;
        }
        if let Some(candidate) = self.hidden.take() {
            self.events
                .push(ComputeConvergenceEvent::CandidateDiscarded {
                    stamp: candidate.stamp(),
                    disposition: ComputeCandidateDisposition::SupersededAfterUpload,
                });
        }
        self.start_pending_preparation();
    }

    fn start_pending_preparation(&mut self) {
        if self.active.is_some() {
            return;
        }
        let Some((generation, target)) = self.pending.take() else {
            return;
        };
        let preparation_barrier = match self.preparation_barrier() {
            Ok(barrier) => barrier,
            Err(error) => {
                let stamp = target.stamp(generation);
                self.pending = Some((generation, target));
                self.record_failure(
                    stamp,
                    ComputeConvergenceFailurePhase::Preparation,
                    error.to_string(),
                );
                return;
            }
        };
        match Self::start_preparation(
            generation,
            target.clone(),
            &mut self.events,
            self.control.clone(),
            preparation_barrier,
        ) {
            Ok(preparation) => self.active = Some(preparation),
            Err(error) => {
                let stamp = target.stamp(generation);
                self.pending = Some((generation, target));
                self.record_failure(
                    stamp,
                    ComputeConvergenceFailurePhase::Preparation,
                    error.to_string(),
                );
            }
        }
    }

    fn newest_target(&self) -> Option<&ComputePreparationTarget> {
        self.pending
            .as_ref()
            .map(|(_, target)| target)
            .or_else(|| self.active.as_ref().map(|active| &active.target))
            .or_else(|| self.hidden.as_ref().map(|candidate| &candidate.target))
            .or_else(|| self.paused.as_ref().map(|(_, target)| target))
    }

    fn preparation_barrier(
        &self,
    ) -> Result<Option<Arc<ComputePreparationBarrierShared>>, ComputeConvergenceControlError> {
        self.control
            .as_ref()
            .map(ComputeConvergenceController::preparation_barrier)
            .transpose()
            .map(Option::flatten)
    }

    fn record_failure(
        &mut self,
        failed: ComputeConvergenceWorkStamp,
        phase: ComputeConvergenceFailurePhase,
        source: String,
    ) {
        self.events.push(ComputeConvergenceEvent::Failure(
            ComputeConvergenceFailure {
                scene_identity: self.scene_identity.clone(),
                failed,
                required_revision: self.required_revision,
                visible_revision: self.installed_bundle.revision(),
                phase,
                source,
            },
        ));
    }
}

impl Drop for ComputeConvergence {
    fn drop(&mut self) {
        if let Err(error) = self.shutdown() {
            eprintln!("{error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::thread;
    use std::time::{Duration, Instant};
    use voxel_frontend::{
        DenseVoxelBatch, DenseVoxelScene, DenseVoxelVolume, VoxelCoordinate, VoxelEditCommand,
        VoxelExtent, VoxelFrontend, VoxelMaterial, VoxelMaterialId, VoxelRegion, VoxelValue,
        VoxelVolumeId, VoxelVolumeMetadata,
    };

    fn frontend(
        scene_identity: &str,
        revision: u64,
        extent: VoxelExtent,
    ) -> Result<VoxelFrontend, Box<dyn std::error::Error>> {
        let [width, height, depth] = extent.dimensions();
        let value_count = usize::try_from(width)?
            .checked_mul(usize::try_from(height)?)
            .and_then(|count| count.checked_mul(usize::try_from(depth).ok()?))
            .ok_or("test volume size overflow")?;
        let frontend = VoxelFrontend::new();
        frontend.publish(DenseVoxelScene::new(
            VoxelSceneId::new(scene_identity),
            VoxelSceneRevision::new(revision),
            vec![VoxelMaterial::new(
                VoxelMaterialId::new("stone"),
                [0.2, 0.3, 0.4, 1.0],
            )],
            vec![DenseVoxelVolume::new(
                VoxelVolumeMetadata::new(VoxelVolumeId::new("terrain"), extent, [0.0; 3], 1.0),
                vec![DenseVoxelBatch::new(
                    VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), extent),
                    vec![VoxelValue::Empty; value_count],
                )],
            )],
        ))?;
        Ok(frontend)
    }

    fn changed_edit(
        frontend: &VoxelFrontend,
        coordinate: VoxelCoordinate,
    ) -> Result<VoxelEditOutcome, voxel_frontend::VoxelFrontendError> {
        frontend.edit(VoxelEditCommand::new(
            VoxelVolumeId::new("terrain"),
            coordinate,
            VoxelValue::Occupied(VoxelMaterialId::new("stone")),
        ))
    }

    fn convergence(
        frontend: &VoxelFrontend,
    ) -> Result<ComputeConvergence, Box<dyn std::error::Error>> {
        Ok(ComputeConvergence::new(ComputeSceneBundle::from_view(
            &frontend.scene_view()?,
        )?))
    }

    fn drain_until_ready(
        convergence: &mut ComputeConvergence,
        revision: VoxelSceneRevision,
    ) -> Result<Vec<ComputeConvergenceEvent>, Box<dyn std::error::Error>> {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut observed = Vec::new();
        while Instant::now() < deadline {
            observed.extend(convergence.drain_events());
            if observed.iter().any(|event| {
                matches!(
                    event,
                    ComputeConvergenceEvent::PreparationReady { stamp }
                        if stamp.revision() == revision
                )
            }) {
                return Ok(observed);
            }
            thread::yield_now();
        }
        Err(format!("revision {revision} did not become ready").into())
    }

    fn drain_until_failure(
        convergence: &mut ComputeConvergence,
        phase: ComputeConvergenceFailurePhase,
    ) -> Result<Vec<ComputeConvergenceEvent>, Box<dyn std::error::Error>> {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut observed = Vec::new();
        while Instant::now() < deadline {
            observed.extend(convergence.drain_events());
            if observed.iter().any(|event| {
                matches!(
                    event,
                    ComputeConvergenceEvent::Failure(failure) if failure.phase() == phase
                )
            }) {
                return Ok(observed);
            }
            thread::yield_now();
        }
        Err(format!("{phase:?} failure was not observed").into())
    }

    fn brickmap_convergence(
        frontend: &VoxelFrontend,
        budget_bytes: u64,
    ) -> Result<ComputeConvergence, Box<dyn std::error::Error>> {
        Ok(ComputeConvergence::new(
            ComputeSceneBundle::from_view_with_representation(
                &frontend.scene_view()?,
                crate::ComputeRepresentation::Brickmap { budget_bytes },
            )?,
        ))
    }

    fn fill_cell(
        frontend: &VoxelFrontend,
        value: VoxelValue,
    ) -> Result<VoxelEditOutcome, voxel_frontend::VoxelFrontendError> {
        frontend.edit(VoxelEditCommand::from_edits(
            (0..512)
                .map(|index| {
                    voxel_frontend::VoxelEdit::new(
                        VoxelVolumeId::new("terrain"),
                        VoxelCoordinate::new(index % 8, index / 8 % 8, index / 64),
                        value.clone(),
                    )
                })
                .collect(),
        ))
    }

    fn install_brickmap_edit(
        convergence: &mut ComputeConvergence,
        outcome: VoxelEditOutcome,
    ) -> Result<crate::BrickmapPatchObservations, Box<dyn std::error::Error>> {
        convergence.accept(outcome)?;
        drain_until_ready(convergence, convergence.required_revision)?;
        convergence.retain_ready_candidate();
        let observation = convergence
            .hidden_bundle()
            .ok_or("missing candidate")?
            .brickmap_patch_observations();
        convergence
            .install_hidden(convergence.status().visible_revision())?
            .ok_or("missing installation")?;
        Ok(observation)
    }

    #[test]
    fn brickmap_growth_plans_headroom_and_complete_peak() -> Result<(), Box<dyn std::error::Error>>
    {
        let frontend = frontend("growth", 0, VoxelExtent::new(32, 8, 8))?;
        changed_edit(&frontend, VoxelCoordinate::new(0, 0, 0))?;
        let mut convergence = brickmap_convergence(&frontend, 8444)?;
        // One required brick plus rounded-up 25% headroom is two slots.
        assert_eq!(
            convergence.installed_bundle().storage_word_count() * 4,
            2132
        );
        install_brickmap_edit(
            &mut convergence,
            changed_edit(&frontend, VoxelCoordinate::new(8, 0, 0))?,
        )?;
        install_brickmap_edit(
            &mut convergence,
            changed_edit(&frontend, VoxelCoordinate::new(16, 0, 0))?,
        )?;
        let growth = convergence
            .status()
            .installed_growth
            .ok_or("missing growth")?;
        assert_eq!((growth.old_capacity, growth.new_capacity), (2, 3));
        assert_eq!(growth.trigger_revision, VoxelSceneRevision::new(3));
        // 2,132 old bytes + 3,156 new bytes + 3,156 upload staging bytes.
        assert_eq!(growth.predicted_peak_bytes, 8444);
        let ray =
            semantic_ray_oracle::SemanticRay::new([15.0, 0.5, 0.5], [1.0, 0.0, 0.0], 0.0, 20.0)?;
        assert_eq!(
            convergence.installed_bundle().observe(&ray),
            semantic_ray_oracle::observe(&frontend.scene_view()?, &ray)?
        );
        Ok(())
    }

    #[test]
    fn brickmap_growth_budget_rejection_is_repeatable() -> Result<(), Box<dyn std::error::Error>> {
        let frontend = frontend("growth-budget", 0, VoxelExtent::new(32, 8, 8))?;
        changed_edit(&frontend, VoxelCoordinate::new(0, 0, 0))?;
        let mut convergence = brickmap_convergence(&frontend, 8443)?;
        install_brickmap_edit(
            &mut convergence,
            changed_edit(&frontend, VoxelCoordinate::new(8, 0, 0))?,
        )?;
        let visible = convergence.installed_bundle().storage_words();
        convergence.accept(changed_edit(&frontend, VoxelCoordinate::new(16, 0, 0))?)?;
        for retry in [false, true] {
            if retry {
                convergence.request_retry()?;
            }
            let events = drain_until_failure(
                &mut convergence,
                ComputeConvergenceFailurePhase::Preparation,
            )?;
            assert!(events.iter().any(|event| matches!(event, ComputeConvergenceEvent::Failure(failure)
                if failure.source() == "brickmap growth peak 8444 bytes exceeds configured budget 8443 bytes")));
            assert_eq!(
                convergence.status().visible_revision(),
                VoxelSceneRevision::new(2)
            );
            assert_eq!(convergence.installed_bundle().storage_words(), visible);
            assert!(convergence.status().hidden().is_none());
        }
        Ok(())
    }

    #[test]
    fn rounded_growth_allocation_rejection_preserves_visible_revision()
    -> Result<(), Box<dyn std::error::Error>> {
        let frontend = frontend("rounded-growth-budget", 0, VoxelExtent::new(32, 8, 8))?;
        changed_edit(&frontend, VoxelCoordinate::new(0, 0, 0))?;
        let mut convergence = brickmap_convergence(&frontend, 8444)?;
        install_brickmap_edit(
            &mut convergence,
            changed_edit(&frontend, VoxelCoordinate::new(8, 0, 0))?,
        )?;
        let visible = convergence.installed_bundle().storage_words();
        convergence.accept(changed_edit(&frontend, VoxelCoordinate::new(16, 0, 0))?)?;
        for retry in [false, true] {
            if retry {
                convergence.request_retry()?;
            }
            drain_until_ready(&mut convergence, VoxelSceneRevision::new(3))?;
            convergence.retain_ready_candidate();
            // Packed data fits exactly, but two rounded Vulkan allocations add 24 bytes.
            let error = convergence
                .prepare_hidden_allocation(2144, 3168, 3156)
                .unwrap_err();
            let ComputeSceneBuildError::GrowthBudgetExceeded {
                predicted_peak_bytes,
                budget_bytes,
            } = error
            else {
                return Err("expected a typed growth-budget rejection".into());
            };
            assert_eq!((predicted_peak_bytes, budget_bytes), (8468, 8444));
            let events = convergence.drain_events();
            assert!(events.iter().any(|event| matches!(event, ComputeConvergenceEvent::Failure(failure)
                if failure.phase() == ComputeConvergenceFailurePhase::Upload
                    && failure.visible_revision() == VoxelSceneRevision::new(2)
                    && failure.source() == "brickmap growth peak 8468 bytes exceeds configured budget 8444 bytes")));
            assert_eq!(
                convergence.status().visible_revision(),
                VoxelSceneRevision::new(2)
            );
            assert_eq!(convergence.installed_bundle().storage_words(), visible);
            assert!(convergence.status().hidden().is_none());
            if !retry {
                println!(
                    "{{\"event\":\"predicted_rejection\",\"predicted_bytes\":{predicted_peak_bytes},\"budget_bytes\":{budget_bytes},\"rejected\":true}}"
                );
            }
        }
        Ok(())
    }

    #[test]
    fn superseded_growth_releases_allocation_and_bounds_candidates()
    -> Result<(), Box<dyn std::error::Error>> {
        let frontend = frontend("growth-supersession", 0, VoxelExtent::new(32, 8, 8))?;
        let mut convergence = brickmap_convergence(&frontend, 16384)?;
        convergence.accept(changed_edit(&frontend, VoxelCoordinate::new(0, 0, 0))?)?;
        for coordinate in 1..8 {
            drain_until_ready(&mut convergence, VoxelSceneRevision::new(coordinate as u64))?;
            convergence.retain_ready_candidate();
            let witness = convergence
                .hidden_bundle()
                .ok_or("missing candidate")?
                .allocation_witness();
            assert!(witness.upgrade().is_some());
            assert!(convergence.status().preparing().is_none());
            assert_eq!(convergence.status().worker_count(), 0);
            convergence.accept(changed_edit(
                &frontend,
                VoxelCoordinate::new(coordinate, 0, 0),
            )?)?;
            assert!(witness.upgrade().is_none());
            assert!(convergence.status().hidden().is_none());
            assert_eq!(convergence.status().worker_count(), 1);
        }
        drain_until_ready(&mut convergence, VoxelSceneRevision::new(8))?;
        convergence.retain_ready_candidate();
        let witness = convergence
            .hidden_bundle()
            .ok_or("missing candidate")?
            .allocation_witness();
        convergence.shutdown()?;
        assert!(witness.upgrade().is_none());
        Ok(())
    }

    #[test]
    fn brickmap_structural_transitions_and_retirement() -> Result<(), Box<dyn std::error::Error>> {
        let frontend = frontend("transitions", 0, VoxelExtent::new(16, 8, 8))?;
        let mut convergence = brickmap_convergence(&frontend, 8192)?;
        let mixed = install_brickmap_edit(
            &mut convergence,
            changed_edit(&frontend, VoxelCoordinate::new(0, 0, 0))?,
        )?;
        assert_eq!(
            (
                mixed.dirty_cells,
                mixed.slots_reserved,
                mixed.slots_retired,
                mixed.uploaded_bytes
            ),
            (0, 0, 0, 0)
        );
        assert!(convergence.status().installed_growth.is_some());
        let empty =
            install_brickmap_edit(&mut convergence, fill_cell(&frontend, VoxelValue::Empty)?)?;
        assert_eq!(
            (
                empty.slots_reserved,
                empty.slots_retired,
                empty.uploaded_bytes
            ),
            (0, 1, 4)
        );
        install_brickmap_edit(
            &mut convergence,
            changed_edit(&frontend, VoxelCoordinate::new(0, 0, 0))?,
        )?;
        let uniform = install_brickmap_edit(
            &mut convergence,
            fill_cell(
                &frontend,
                VoxelValue::Occupied(VoxelMaterialId::new("stone")),
            )?,
        )?;
        assert_eq!(
            (
                uniform.dirty_cells,
                uniform.slots_reserved,
                uniform.slots_retired
            ),
            (1, 0, 1)
        );
        let mixed = install_brickmap_edit(
            &mut convergence,
            frontend.edit(VoxelEditCommand::new(
                VoxelVolumeId::new("terrain"),
                VoxelCoordinate::new(0, 0, 0),
                VoxelValue::Empty,
            ))?,
        )?;
        assert_eq!((mixed.slots_reserved, mixed.slots_retired), (1, 0));
        let ray =
            semantic_ray_oracle::SemanticRay::new([-1.0, 0.5, 0.5], [1.0, 0.0, 0.0], 0.0, 20.0)?;
        assert_eq!(
            convergence.installed_bundle().observe(&ray),
            semantic_ray_oracle::observe(&frontend.scene_view()?, &ray)?
        );
        Ok(())
    }

    #[test]
    fn brickmap_growth_rejection_preserves_visible_and_retirement_waits_for_installation()
    -> Result<(), Box<dyn std::error::Error>> {
        let frontend = frontend("capacity", 0, VoxelExtent::new(16, 8, 8))?;
        let mut convergence = brickmap_convergence(&frontend, 3000)?;
        install_brickmap_edit(
            &mut convergence,
            changed_edit(&frontend, VoxelCoordinate::new(0, 0, 0))?,
        )?;
        let visible = convergence.installed_bundle().storage_words();
        let outcome = frontend.edit(VoxelEditCommand::from_edits(vec![
            voxel_frontend::VoxelEdit::new(
                VoxelVolumeId::new("terrain"),
                VoxelCoordinate::new(0, 0, 0),
                VoxelValue::Empty,
            ),
            voxel_frontend::VoxelEdit::new(
                VoxelVolumeId::new("terrain"),
                VoxelCoordinate::new(8, 0, 0),
                VoxelValue::Occupied(VoxelMaterialId::new("stone")),
            ),
        ]))?;
        convergence.accept(outcome)?;
        let events = drain_until_failure(
            &mut convergence,
            ComputeConvergenceFailurePhase::Preparation,
        )?;
        assert!(events.iter().any(|event| matches!(event, ComputeConvergenceEvent::Failure(failure) if failure.source().contains("exceeds configured budget"))));
        assert_eq!(
            convergence.status().visible_revision(),
            VoxelSceneRevision::new(1)
        );
        assert_eq!(convergence.installed_bundle().storage_words(), visible);
        let empty = frontend.edit(VoxelEditCommand::new(
            VoxelVolumeId::new("terrain"),
            VoxelCoordinate::new(8, 0, 0),
            VoxelValue::Empty,
        ))?;
        install_brickmap_edit(&mut convergence, empty)?;
        let reused = install_brickmap_edit(
            &mut convergence,
            changed_edit(&frontend, VoxelCoordinate::new(8, 0, 0))?,
        )?;
        assert_eq!(reused.slots_reserved, 1);
        Ok(())
    }

    #[test]
    fn brickmap_supersession_and_skipped_revisions_release_private_reservations()
    -> Result<(), Box<dyn std::error::Error>> {
        let frontend = frontend("supersession", 0, VoxelExtent::new(16, 8, 8))?;
        let mut convergence = brickmap_convergence(&frontend, 3000)?;
        convergence.accept(changed_edit(&frontend, VoxelCoordinate::new(0, 0, 0))?)?;
        drain_until_ready(&mut convergence, VoxelSceneRevision::new(1))?;
        convergence.retain_ready_candidate();
        let original = convergence.installed_bundle().storage_words();
        frontend.edit(VoxelEditCommand::new(
            VoxelVolumeId::new("terrain"),
            VoxelCoordinate::new(0, 0, 0),
            VoxelValue::Empty,
        ))?;
        let newest = changed_edit(&frontend, VoxelCoordinate::new(8, 0, 0))?;
        let observation = install_brickmap_edit(&mut convergence, newest)?;
        assert_eq!(observation.slots_reserved, 0);
        assert!(convergence.status().installed_growth.is_some());
        assert_eq!(
            convergence.status().visible_revision(),
            VoxelSceneRevision::new(3)
        );
        assert_ne!(convergence.installed_bundle().storage_words(), original);
        Ok(())
    }

    #[test]
    fn brickmap_cancellation_releases_only_private_slots() -> Result<(), Box<dyn std::error::Error>>
    {
        let frontend = frontend("cancel-slots", 0, VoxelExtent::new(16, 8, 8))?;
        changed_edit(&frontend, VoxelCoordinate::new(1, 0, 0))?;
        let mut convergence = brickmap_convergence(&frontend, 8192)?;
        install_brickmap_edit(
            &mut convergence,
            changed_edit(&frontend, VoxelCoordinate::new(0, 0, 0))?,
        )?;
        let control = convergence.enable_control(false);
        control.hold_next_preparation_after_blocks(1)?;
        convergence.accept(changed_edit(&frontend, VoxelCoordinate::new(8, 0, 0))?)?;
        let deadline = Instant::now() + Duration::from_secs(10);
        while control
            .preparation_barrier_observation()?
            .and_then(|observation| observation.reached_revision())
            .is_none()
        {
            assert!(Instant::now() < deadline);
            thread::yield_now();
        }
        let visible = convergence.installed_bundle().storage_words();
        convergence.accept(changed_edit(&frontend, VoxelCoordinate::new(9, 0, 0))?)?;
        control.release_preparation_barrier()?;
        drain_until_ready(&mut convergence, VoxelSceneRevision::new(4))?;
        convergence.retain_ready_candidate();
        assert_eq!(convergence.installed_bundle().storage_words(), visible);
        let patch = convergence
            .hidden_bundle()
            .ok_or("missing candidate")?
            .brickmap_patch_observations();
        assert_eq!((patch.slots_reserved, patch.slots_retired), (1, 0));
        convergence.install_hidden(VoxelSceneRevision::new(2))?;
        Ok(())
    }

    #[test]
    fn brickmap_rejects_changed_base_revision_or_pool_allocation()
    -> Result<(), Box<dyn std::error::Error>> {
        for replace_revision in [false, true] {
            let frontend = frontend("stale", 0, VoxelExtent::new(16, 8, 8))?;
            let initial = frontend.scene_view()?;
            let mut convergence = brickmap_convergence(&frontend, 8192)?;
            convergence.accept(changed_edit(&frontend, VoxelCoordinate::new(0, 0, 0))?)?;
            drain_until_ready(&mut convergence, VoxelSceneRevision::new(1))?;
            convergence.retain_ready_candidate();
            let view = if replace_revision {
                frontend.scene_view()?
            } else {
                initial
            };
            convergence.installed_bundle = if replace_revision {
                // Preserve allocation identity to isolate the revision-base check.
                convergence
                    .hidden_bundle()
                    .ok_or("missing candidate")?
                    .clone()
            } else {
                ComputeSceneBundle::from_view_with_representation(
                    &view,
                    crate::ComputeRepresentation::Brickmap { budget_bytes: 8192 },
                )?
            };
            assert!(matches!(
                convergence.install_hidden(view.revision()),
                Err(ComputeConvergenceError::PatchBaseMismatch)
            ));
            assert_eq!(convergence.installed_bundle().revision(), view.revision());
        }
        Ok(())
    }

    #[test]
    fn brickmap_edit_reserves_a_private_slot_without_rebuilding()
    -> Result<(), Box<dyn std::error::Error>> {
        let frontend = frontend("brickmap", 0, VoxelExtent::new(16, 8, 8))?;
        changed_edit(&frontend, VoxelCoordinate::new(8, 0, 0))?;
        let bundle = ComputeSceneBundle::from_view_with_representation(
            &frontend.scene_view()?,
            crate::ComputeRepresentation::Brickmap { budget_bytes: 8192 },
        )?;
        let original = bundle.storage_words();
        let mut convergence = ComputeConvergence::new(bundle);
        convergence.accept(changed_edit(&frontend, VoxelCoordinate::new(0, 0, 0))?)?;
        drain_until_ready(&mut convergence, VoxelSceneRevision::new(2))?;
        convergence.retain_ready_candidate();
        let candidate = convergence.hidden_bundle().ok_or("missing candidate")?;
        assert_eq!(candidate.predecessor(), Some(VoxelSceneRevision::new(1)));
        assert_eq!(candidate.patches().len(), 257);
        assert_eq!(convergence.installed_bundle().storage_words(), original);
        convergence
            .install_hidden(VoxelSceneRevision::new(1))?
            .ok_or("missing installation")?;
        assert_eq!(
            convergence.status().visible_revision(),
            VoxelSceneRevision::new(2)
        );
        Ok(())
    }

    #[test]
    fn installing_a_broad_edit_releases_upload_patches() -> Result<(), Box<dyn std::error::Error>> {
        let frontend = frontend("broad", 0, VoxelExtent::new(4096, 1, 1))?;
        let mut convergence = convergence(&frontend)?;
        let outcome = frontend.edit(VoxelEditCommand::from_edits(
            (0..4096)
                .map(|coordinate| {
                    voxel_frontend::VoxelEdit::new(
                        VoxelVolumeId::new("terrain"),
                        VoxelCoordinate::new(coordinate, 0, 0),
                        VoxelValue::Occupied(VoxelMaterialId::new("stone")),
                    )
                })
                .collect(),
        ))?;
        convergence.accept(outcome)?;
        drain_until_ready(&mut convergence, VoxelSceneRevision::new(1))?;
        convergence.retain_ready_candidate();
        assert_eq!(
            convergence
                .hidden_bundle()
                .ok_or("missing candidate")?
                .patches()
                .len(),
            4096
        );
        convergence
            .install_hidden(VoxelSceneRevision::new(0))?
            .ok_or("missing installation")?;
        assert!(convergence.installed_bundle().patches().is_empty());
        assert!(
            convergence
                .installed_bundle()
                .voxel_words()
                .iter()
                .all(|word| *word == 1)
        );
        Ok(())
    }

    #[test]
    fn incremental_chain_preserves_visible_words_and_coalesces_repeated_edits()
    -> Result<(), Box<dyn std::error::Error>> {
        let frontend = frontend("incremental", 0, VoxelExtent::new(65, 33, 2))?;
        let visible = ComputeSceneBundle::from_view(&frontend.scene_view()?)?;
        let original_words = visible.storage_words();
        let mut changes = Vec::new();
        for (coordinate, value) in [
            (
                VoxelCoordinate::new(64, 32, 1),
                VoxelValue::Occupied(VoxelMaterialId::new("stone")),
            ),
            (
                VoxelCoordinate::new(0, 0, 0),
                VoxelValue::Occupied(VoxelMaterialId::new("stone")),
            ),
            (VoxelCoordinate::new(64, 32, 1), VoxelValue::Empty),
        ] {
            let VoxelEditOutcome::Changed { change_set, .. } = frontend.edit(
                VoxelEditCommand::new(VoxelVolumeId::new("terrain"), coordinate, value),
            )?
            else {
                return Err("edit did not change the scene".into());
            };
            changes.push(change_set);
        }
        let view = frontend.scene_view()?;
        let successor =
            visible.successor_with_block_completion(&view, &changes, || false, || Ok(()))?;
        assert_eq!(
            successor.storage_words(),
            ComputeSceneBundle::from_view(&view)?.storage_words()
        );
        assert_eq!(visible.storage_words(), original_words);
        assert_eq!(successor.predecessor(), Some(visible.revision()));
        assert_eq!(successor.patches().len(), 2);
        for chain in [&changes[1..], &changes[..2], &[]] {
            let rebuilt =
                visible.successor_with_block_completion(&view, chain, || false, || Ok(()))?;
            assert_eq!(rebuilt.predecessor(), None);
            assert_eq!(rebuilt.storage_words(), successor.storage_words());
        }
        Ok(())
    }

    #[test]
    #[ignore = "preparation benchmark; run with --release --ignored --nocapture"]
    fn measure_incremental_preparation() -> Result<(), Box<dyn std::error::Error>> {
        for edge in [32, 64, 128] {
            let frontend = frontend("measurement", 0, VoxelExtent::new(edge, edge, edge))?;
            let mut visible = ComputeSceneBundle::from_view(&frontend.scene_view()?)?;
            let mut full_milliseconds = 0.0;
            let mut incremental_milliseconds = 0.0;
            let mut uploaded_bytes = 0;
            for index in 0..20 {
                let value = if index % 2 == 0 {
                    VoxelValue::Occupied(VoxelMaterialId::new("stone"))
                } else {
                    VoxelValue::Empty
                };
                let VoxelEditOutcome::Changed { view, change_set } =
                    frontend.edit(VoxelEditCommand::new(
                        VoxelVolumeId::new("terrain"),
                        VoxelCoordinate::new(1, 1, 1),
                        value,
                    ))?
                else {
                    return Err("benchmark edit did not change the scene".into());
                };
                let started = Instant::now();
                let full = ComputeSceneBundle::from_view(&view)?;
                full_milliseconds += started.elapsed().as_secs_f64() * 1000.0;
                let started = Instant::now();
                let incremental = visible.successor_with_block_completion(
                    &view,
                    &[change_set],
                    || false,
                    || Ok(()),
                )?;
                incremental_milliseconds += started.elapsed().as_secs_f64() * 1000.0;
                uploaded_bytes += incremental.patches().len() * 4;
                assert_eq!(incremental.storage_words(), full.storage_words());
                visible = incremental;
            }
            println!(
                "edge={edge} full_prepare_ms={:.6} incremental_prepare_ms={:.6} full_upload_bytes={} incremental_upload_bytes={}",
                full_milliseconds / 20.0,
                incremental_milliseconds / 20.0,
                visible.storage_word_count() * 4,
                uploaded_bytes / 20
            );
        }
        Ok(())
    }

    #[test]
    fn atomic_command_installs_one_complete_bundle() -> Result<(), Box<dyn std::error::Error>> {
        let frontend = frontend("atomic", 0, VoxelExtent::new(513, 1, 1))?;
        let mut convergence = convergence(&frontend)?;
        let outcome = frontend.edit(VoxelEditCommand::from_edits(
            [0, 255, 256, 512]
                .map(|coordinate| {
                    voxel_frontend::VoxelEdit::new(
                        VoxelVolumeId::new("terrain"),
                        VoxelCoordinate::new(coordinate, 0, 0),
                        VoxelValue::Occupied(VoxelMaterialId::new("stone")),
                    )
                })
                .to_vec(),
        ))?;
        let expected = ComputeSceneBundle::from_view(outcome.view())?;
        convergence.accept(outcome)?;
        let events = drain_until_ready(&mut convergence, VoxelSceneRevision::new(1))?;
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, ComputeConvergenceEvent::PreparationStarted { .. }))
                .count(),
            1
        );
        convergence.retain_ready_candidate();
        convergence.mark_hidden_uploaded();
        convergence
            .install_hidden(VoxelSceneRevision::new(0))?
            .ok_or("candidate not installed")?;
        assert_eq!(
            convergence.installed_bundle().revision(),
            VoxelSceneRevision::new(1)
        );
        assert_eq!(
            convergence.installed_bundle().voxel_words(),
            expected.voxel_words()
        );
        Ok(())
    }

    #[test]
    fn rapid_requirements_install_only_the_newest_authoritative_bundle()
    -> Result<(), Box<dyn std::error::Error>> {
        let frontend = frontend("newest", 10, VoxelExtent::new(64, 1, 1))?;
        let mut convergence = convergence(&frontend)?;

        convergence.accept(changed_edit(&frontend, VoxelCoordinate::new(0, 0, 0))?)?;
        convergence.accept(changed_edit(&frontend, VoxelCoordinate::new(63, 0, 0))?)?;

        let scheduled = convergence.status();
        assert_eq!(scheduled.worker_count(), 1);
        assert_eq!(
            scheduled
                .pending()
                .map(ComputeConvergenceWorkStamp::revision),
            Some(VoxelSceneRevision::new(12))
        );
        let mut observed = drain_until_ready(&mut convergence, VoxelSceneRevision::new(12))?;
        convergence.retain_ready_candidate();
        convergence.mark_hidden_uploaded();
        assert_eq!(
            convergence
                .status()
                .hidden()
                .map(ComputeConvergenceWorkStamp::revision),
            Some(VoxelSceneRevision::new(12))
        );

        convergence.accept(changed_edit(&frontend, VoxelCoordinate::new(62, 0, 0))?)?;
        assert!(
            convergence
                .install_hidden(VoxelSceneRevision::new(10))?
                .is_none()
        );
        observed.extend(drain_until_ready(
            &mut convergence,
            VoxelSceneRevision::new(13),
        )?);
        convergence.retain_ready_candidate();
        convergence.mark_hidden_uploaded();
        let retired = convergence
            .install_hidden(VoxelSceneRevision::new(10))?
            .ok_or("newest candidate was not installed")?;
        observed.extend(convergence.drain_events());

        assert_eq!(retired.revision(), VoxelSceneRevision::new(10));
        assert_eq!(
            convergence.installed_bundle().revision(),
            VoxelSceneRevision::new(13)
        );
        let installed = observed
            .iter()
            .filter_map(|event| match event {
                ComputeConvergenceEvent::CandidateInstalled { stamp } => Some(stamp.revision()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(installed, vec![VoxelSceneRevision::new(13)]);
        assert!(observed.iter().any(|event| matches!(
            event,
            ComputeConvergenceEvent::CandidateDiscarded {
                stamp,
                disposition: ComputeCandidateDisposition::SupersededAfterUpload,
            } if stamp.revision() == VoxelSceneRevision::new(12)
        )));
        Ok(())
    }

    #[test]
    fn deterministic_burst_bounds_revision_two_and_installs_only_revision_four()
    -> Result<(), Box<dyn std::error::Error>> {
        let frontend = frontend("burst", 1, VoxelExtent::new(65, 1, 1))?;
        let mut convergence = convergence(&frontend)?;
        let controller = convergence.enable_control(true);
        controller.hold_next_preparation_after_blocks(1)?;
        controller.submit(changed_edit(&frontend, VoxelCoordinate::new(0, 0, 0))?)?;
        let revision_two = controller
            .take_pending_outcome()?
            .ok_or("revision 2 was not queued")?;
        convergence.accept(revision_two)?;

        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline
            && !controller
                .preparation_barrier_observation()?
                .is_some_and(|observation| {
                    observation.reached_revision() == Some(VoxelSceneRevision::new(2))
                })
        {
            thread::yield_now();
        }
        let held = controller
            .preparation_barrier_observation()?
            .ok_or("revision 2 did not reach the preparation barrier")?;
        assert_eq!(held.completed_block_count(), 1);

        convergence.accept(changed_edit(&frontend, VoxelCoordinate::new(64, 0, 0))?)?;
        controller.release_preparation_barrier()?;
        let mut observed = drain_until_ready(&mut convergence, VoxelSceneRevision::new(3))?;
        let cancelled = controller
            .preparation_barrier_observation()?
            .ok_or("revision 2 preparation observation disappeared")?;
        assert_eq!(cancelled.completed_block_count(), 1);
        assert!(cancelled.finished());
        assert!(cancelled.cancelled());

        convergence.retain_ready_candidate();
        convergence.mark_hidden_uploaded();
        assert!(controller.hold_post_upload(VoxelSceneRevision::new(3))?);
        convergence.accept(changed_edit(&frontend, VoxelCoordinate::new(63, 0, 0))?)?;
        convergence.retain_ready_candidate();
        observed.extend(drain_until_ready(
            &mut convergence,
            VoxelSceneRevision::new(4),
        )?);
        convergence.retain_ready_candidate();
        convergence.mark_hidden_uploaded();
        let retired = convergence
            .install_hidden(VoxelSceneRevision::new(1))?
            .ok_or("revision 4 was not installed")?;
        observed.extend(convergence.drain_events());

        assert_eq!(retired.revision(), VoxelSceneRevision::new(1));
        assert_eq!(
            convergence.installed_bundle().revision(),
            VoxelSceneRevision::new(4)
        );
        assert!(observed.iter().any(|event| matches!(
            event,
            ComputeConvergenceEvent::CandidateDiscarded {
                stamp,
                disposition: ComputeCandidateDisposition::SupersededBeforeUpload,
            } if stamp.revision() == VoxelSceneRevision::new(2)
        )));
        assert!(observed.iter().any(|event| matches!(
            event,
            ComputeConvergenceEvent::CandidateDiscarded {
                stamp,
                disposition: ComputeCandidateDisposition::SupersededAfterUpload,
            } if stamp.revision() == VoxelSceneRevision::new(3)
        )));
        let installed = observed
            .iter()
            .filter_map(|event| match event {
                ComputeConvergenceEvent::CandidateInstalled { stamp } => Some(stamp.revision()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(installed, vec![VoxelSceneRevision::new(4)]);
        Ok(())
    }

    #[test]
    fn discontinuous_newer_outcomes_use_the_same_complete_rebuild_path()
    -> Result<(), Box<dyn std::error::Error>> {
        let initial = frontend("resync", 3, VoxelExtent::new(2, 1, 1))?;
        let resynchronized = frontend("resync", 20, VoxelExtent::new(2, 1, 1))?;
        let mut convergence = convergence(&initial)?;

        convergence.accept(changed_edit(&initial, VoxelCoordinate::new(0, 0, 0))?)?;
        convergence.accept(changed_edit(
            &resynchronized,
            VoxelCoordinate::new(1, 0, 0),
        )?)?;

        drain_until_ready(&mut convergence, VoxelSceneRevision::new(21))?;
        convergence.retain_ready_candidate();
        convergence.mark_hidden_uploaded();
        let retired = convergence
            .install_hidden(VoxelSceneRevision::new(3))?
            .ok_or("resynchronized candidate was not installed")?;
        assert_eq!(retired.revision(), VoxelSceneRevision::new(3));
        assert_eq!(
            convergence.installed_bundle().revision(),
            VoxelSceneRevision::new(21)
        );
        Ok(())
    }

    #[test]
    fn a_newer_requirement_replaces_a_ready_bundle_without_starting_a_second_worker()
    -> Result<(), Box<dyn std::error::Error>> {
        let frontend = frontend("ready", 30, VoxelExtent::new(2, 1, 1))?;
        let mut convergence = convergence(&frontend)?;
        convergence.accept(changed_edit(&frontend, VoxelCoordinate::new(0, 0, 0))?)?;
        drain_until_ready(&mut convergence, VoxelSceneRevision::new(31))?;

        convergence.accept(changed_edit(&frontend, VoxelCoordinate::new(1, 0, 0))?)?;

        let status = convergence.status();
        assert_eq!(status.worker_count(), 1);
        assert_eq!(status.pending(), None);
        assert_eq!(
            status
                .preparing()
                .map(ComputeConvergenceWorkStamp::revision),
            Some(VoxelSceneRevision::new(32))
        );
        let events = drain_until_ready(&mut convergence, VoxelSceneRevision::new(32))?;
        assert!(events.iter().any(|event| matches!(
            event,
            ComputeConvergenceEvent::CandidateDiscarded {
                stamp,
                disposition: ComputeCandidateDisposition::SupersededBeforeUpload,
            } if stamp.revision() == VoxelSceneRevision::new(31)
        )));
        Ok(())
    }

    #[test]
    fn explicit_retry_reuses_the_required_view_with_a_new_generation()
    -> Result<(), Box<dyn std::error::Error>> {
        let frontend = frontend("retry", 80, VoxelExtent::new(1, 1, 1))?;
        let mut convergence = convergence(&frontend)?;
        let controller = convergence.enable_control(false);
        assert_eq!(
            convergence.request_retry()?,
            ComputeConvergenceRetry::NoRequiredWork
        );
        let accepted =
            convergence.accept(changed_edit(&frontend, VoxelCoordinate::new(0, 0, 0))?)?;
        let ComputeConvergenceAcceptance::Accepted { stamp: first } = accepted else {
            return Err("changed outcome was not accepted".into());
        };
        drain_until_ready(&mut convergence, VoxelSceneRevision::new(81))?;
        convergence.retain_ready_candidate();
        controller.inject_next_failure(ComputeConvergenceFailurePhase::Upload)?;
        assert!(convergence.fail_hidden_if_injected(ComputeConvergenceFailurePhase::Upload)?);
        let observed =
            drain_until_failure(&mut convergence, ComputeConvergenceFailurePhase::Upload)?;
        assert_eq!(
            convergence.installed_bundle().revision(),
            VoxelSceneRevision::new(80)
        );
        assert!(observed.iter().any(|event| matches!(
            event,
            ComputeConvergenceEvent::Failure(failure)
                if failure.failed() == first
                    && failure.required_revision() == VoxelSceneRevision::new(81)
                    && failure.visible_revision() == VoxelSceneRevision::new(80)
                    && failure.source().contains("injected upload failure")
        )));
        assert_eq!(
            convergence
                .status()
                .paused()
                .map(ComputeConvergenceWorkStamp::generation),
            Some(first.generation())
        );

        controller.request_retry()?;
        convergence.apply_controlled_request_at_frame_boundary()?;
        let retry = convergence
            .status()
            .preparing()
            .ok_or("retry did not start at the frame boundary")?;
        assert_eq!(retry.revision(), first.revision());
        assert!(retry.generation().value() > first.generation().value());
        drain_until_ready(&mut convergence, VoxelSceneRevision::new(81))?;
        convergence.retain_ready_candidate();
        convergence.mark_hidden_uploaded();
        convergence
            .install_hidden(VoxelSceneRevision::new(80))?
            .ok_or("retried candidate was not installed")?;
        assert_eq!(
            convergence.installed_bundle().revision(),
            VoxelSceneRevision::new(81)
        );
        assert_eq!(
            convergence.status().installed().generation(),
            retry.generation()
        );
        assert_eq!(convergence.status().cleanup_debt(), None);
        Ok(())
    }

    #[test]
    fn preparation_failure_preserves_visible_state_and_retries_the_same_revision()
    -> Result<(), Box<dyn std::error::Error>> {
        let frontend = frontend("preparation-failure", 100, VoxelExtent::new(1, 1, 1))?;
        let mut convergence = convergence(&frontend)?;
        let controller = convergence.enable_control(false);
        controller.inject_next_failure(ComputeConvergenceFailurePhase::Preparation)?;
        let ComputeConvergenceAcceptance::Accepted { stamp: failed } =
            convergence.accept(changed_edit(&frontend, VoxelCoordinate::new(0, 0, 0))?)?
        else {
            return Err("changed outcome was not accepted".into());
        };

        let observed = drain_until_failure(
            &mut convergence,
            ComputeConvergenceFailurePhase::Preparation,
        )?;

        assert_eq!(
            convergence.installed_bundle().revision(),
            VoxelSceneRevision::new(100)
        );
        assert_eq!(convergence.status().paused(), Some(failed));
        assert!(observed.iter().any(|event| matches!(
            event,
            ComputeConvergenceEvent::Failure(failure)
                if failure.failed() == failed
                    && failure.required_revision() == VoxelSceneRevision::new(101)
                    && failure.visible_revision() == VoxelSceneRevision::new(100)
                    && failure.source().contains("injected preparation failure")
        )));

        controller.request_retry()?;
        convergence.apply_controlled_request_at_frame_boundary()?;
        let retry = convergence
            .status()
            .preparing()
            .ok_or("retry did not start at the frame boundary")?;
        assert_eq!(retry.revision(), failed.revision());
        assert!(retry.generation().value() > failed.generation().value());
        drain_until_ready(&mut convergence, VoxelSceneRevision::new(101))?;
        convergence.retain_ready_candidate();
        convergence.mark_hidden_uploaded();
        convergence
            .install_hidden(VoxelSceneRevision::new(100))?
            .ok_or("retried preparation did not install")?;
        assert_eq!(
            convergence.installed_bundle().revision(),
            VoxelSceneRevision::new(101)
        );
        Ok(())
    }

    #[test]
    fn pre_swap_installation_failure_preserves_visible_state_and_retries_the_same_revision()
    -> Result<(), Box<dyn std::error::Error>> {
        let frontend = frontend("installation-failure", 110, VoxelExtent::new(1, 1, 1))?;
        let mut convergence = convergence(&frontend)?;
        let controller = convergence.enable_control(false);
        let ComputeConvergenceAcceptance::Accepted { stamp: failed } =
            convergence.accept(changed_edit(&frontend, VoxelCoordinate::new(0, 0, 0))?)?
        else {
            return Err("changed outcome was not accepted".into());
        };
        drain_until_ready(&mut convergence, VoxelSceneRevision::new(111))?;
        convergence.retain_ready_candidate();
        convergence.mark_hidden_uploaded();
        controller.inject_next_failure(ComputeConvergenceFailurePhase::Installation)?;

        assert!(convergence.fail_hidden_if_injected(ComputeConvergenceFailurePhase::Installation)?);
        let observed = drain_until_failure(
            &mut convergence,
            ComputeConvergenceFailurePhase::Installation,
        )?;

        assert_eq!(
            convergence.installed_bundle().revision(),
            VoxelSceneRevision::new(110)
        );
        assert_eq!(convergence.status().paused(), Some(failed));
        assert!(observed.iter().any(|event| matches!(
            event,
            ComputeConvergenceEvent::Failure(failure)
                if failure.failed() == failed
                    && failure.required_revision() == VoxelSceneRevision::new(111)
                    && failure.visible_revision() == VoxelSceneRevision::new(110)
                    && failure.source().contains("injected installation failure")
        )));

        controller.request_retry()?;
        convergence.apply_controlled_request_at_frame_boundary()?;
        let retry = convergence
            .status()
            .preparing()
            .ok_or("retry did not start at the frame boundary")?;
        assert_eq!(retry.revision(), failed.revision());
        assert!(retry.generation().value() > failed.generation().value());
        drain_until_ready(&mut convergence, VoxelSceneRevision::new(111))?;
        convergence.retain_ready_candidate();
        convergence.mark_hidden_uploaded();
        convergence
            .install_hidden(VoxelSceneRevision::new(110))?
            .ok_or("retried installation did not install")?;
        assert_eq!(
            convergence.installed_bundle().revision(),
            VoxelSceneRevision::new(111)
        );
        Ok(())
    }

    #[test]
    fn newer_work_supersedes_a_paused_failure() -> Result<(), Box<dyn std::error::Error>> {
        let frontend = frontend("paused-supersession", 120, VoxelExtent::new(2, 1, 1))?;
        let mut convergence = convergence(&frontend)?;
        let controller = convergence.enable_control(false);
        let ComputeConvergenceAcceptance::Accepted { stamp: failed } =
            convergence.accept(changed_edit(&frontend, VoxelCoordinate::new(0, 0, 0))?)?
        else {
            return Err("first changed outcome was not accepted".into());
        };
        drain_until_ready(&mut convergence, VoxelSceneRevision::new(121))?;
        convergence.retain_ready_candidate();
        controller.inject_next_failure(ComputeConvergenceFailurePhase::Upload)?;
        assert!(convergence.fail_hidden_if_injected(ComputeConvergenceFailurePhase::Upload)?);
        assert_eq!(convergence.status().paused(), Some(failed));

        let ComputeConvergenceAcceptance::Accepted { stamp: newer } =
            convergence.accept(changed_edit(&frontend, VoxelCoordinate::new(1, 0, 0))?)?
        else {
            return Err("newer changed outcome was not accepted".into());
        };

        assert_eq!(convergence.status().paused(), None);
        let mut observed = drain_until_ready(&mut convergence, VoxelSceneRevision::new(122))?;
        convergence.retain_ready_candidate();
        convergence.mark_hidden_uploaded();
        convergence
            .install_hidden(VoxelSceneRevision::new(120))?
            .ok_or("newer candidate was not installed")?;
        observed.extend(convergence.drain_events());
        assert_eq!(
            convergence.installed_bundle().revision(),
            VoxelSceneRevision::new(122)
        );
        let installed = observed
            .iter()
            .filter_map(|event| match event {
                ComputeConvergenceEvent::CandidateInstalled { stamp } => Some(*stamp),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(installed, vec![newer]);
        assert!(!installed.contains(&failed));
        Ok(())
    }

    #[test]
    fn shutdown_releases_an_active_preparation_worker_and_every_retained_view()
    -> Result<(), Box<dyn std::error::Error>> {
        let frontend = frontend("active-shutdown", 130, VoxelExtent::new(64, 1, 1))?;
        let mut convergence = convergence(&frontend)?;
        let controller = convergence.enable_control(false);
        controller.hold_next_preparation_after_blocks(1)?;
        convergence.accept(changed_edit(&frontend, VoxelCoordinate::new(0, 0, 0))?)?;
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut barrier_reached = false;
        while Instant::now() < deadline {
            if controller
                .preparation_barrier_observation()?
                .is_some_and(|observation| observation.completed_block_count() == 1)
            {
                barrier_reached = true;
                break;
            }
            thread::yield_now();
        }
        assert!(barrier_reached);
        assert_eq!(convergence.status().worker_count(), 1);
        assert!(convergence.owned_view_count() > 0);

        convergence.shutdown()?;

        assert_eq!(convergence.status().worker_count(), 0);
        assert_eq!(convergence.owned_view_count(), 0);
        Ok(())
    }

    #[test]
    fn shutdown_releases_a_hidden_candidate_view() -> Result<(), Box<dyn std::error::Error>> {
        let frontend = frontend("hidden-shutdown", 140, VoxelExtent::new(1, 1, 1))?;
        let mut convergence = convergence(&frontend)?;
        convergence.accept(changed_edit(&frontend, VoxelCoordinate::new(0, 0, 0))?)?;
        drain_until_ready(&mut convergence, VoxelSceneRevision::new(141))?;
        convergence.retain_ready_candidate();
        convergence.mark_hidden_uploaded();
        assert_eq!(convergence.status().worker_count(), 0);
        assert_eq!(convergence.owned_view_count(), 1);

        convergence.shutdown()?;

        assert_eq!(convergence.status().hidden(), None);
        assert_eq!(convergence.owned_view_count(), 0);
        Ok(())
    }

    #[test]
    fn invalid_and_non_newer_outcomes_do_not_mutate_convergence_state()
    -> Result<(), Box<dyn std::error::Error>> {
        let current = frontend("admission", 90, VoxelExtent::new(1, 1, 1))?;
        let older = frontend("admission", 10, VoxelExtent::new(1, 1, 1))?;
        let foreign = frontend("foreign", 90, VoxelExtent::new(1, 1, 1))?;
        let mut convergence = convergence(&current)?;
        let initial = convergence.status();

        assert_eq!(
            convergence.accept(changed_edit(&older, VoxelCoordinate::new(0, 0, 0))?)?,
            ComputeConvergenceAcceptance::NotNewer {
                revision: VoxelSceneRevision::new(11),
            }
        );
        assert_eq!(convergence.status(), initial);
        assert!(matches!(
            convergence.accept(changed_edit(&foreign, VoxelCoordinate::new(0, 0, 0))?),
            Err(ComputeConvergenceError::SceneIdentityMismatch { .. })
        ));
        assert_eq!(convergence.status(), initial);
        Ok(())
    }

    #[test]
    fn retained_events_are_bounded_and_count_compacted_history() {
        let mut events = ComputeConvergenceEvents::new();
        for revision in 1..=100 {
            events.push(ComputeConvergenceEvent::PreparationStarted {
                stamp: ComputeConvergenceWorkStamp::new(
                    VoxelSceneRevision::new(revision),
                    ComputeConvergenceGeneration::initial(),
                ),
            });
        }

        assert_eq!(events.retained.len(), RETAINED_EVENT_CAPACITY);
        assert_eq!(events.dropped, 36);
    }

    #[test]
    fn scene_preparation_checks_cancellation_between_bounded_region_reads()
    -> Result<(), Box<dyn std::error::Error>> {
        let frontend = frontend("cancel", 1, VoxelExtent::new(33, 1, 1))?;
        let checks = Cell::new(0_u32);

        let result = ComputeSceneBundle::from_view_until_cancelled(&frontend.scene_view()?, || {
            let next = checks.get().saturating_add(1);
            checks.set(next);
            next == 2
        });

        assert!(matches!(result, Err(ComputeSceneBuildError::Cancelled)));
        assert_eq!(checks.get(), 2);
        Ok(())
    }
}

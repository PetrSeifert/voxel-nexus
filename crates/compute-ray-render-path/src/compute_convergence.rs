use crate::{ComputeSceneBuildError, ComputeSceneBundle};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use thiserror::Error;
use voxel_frontend::{VoxelEditOutcome, VoxelSceneId, VoxelSceneRevision, VoxelSceneView};

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
    reached_revision: Option<VoxelSceneRevision>,
    completed_block_count: usize,
    released: bool,
    finished: bool,
    cancelled: bool,
}

struct ComputePreparationBarrierShared {
    hold_after_completed_blocks: usize,
    state: Mutex<ComputePreparationBarrierState>,
    released: Condvar,
}

impl ComputePreparationBarrierShared {
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
    #[error("the compute convergence control state is unavailable")]
    Unavailable,
    #[error("a compute edit outcome is already pending at the frame boundary")]
    PendingOutcome,
    #[error("the compute preparation barrier needs a positive completed-block count")]
    InvalidCompletedBlockCount,
    #[error("the compute preparation barrier has not been configured")]
    PreparationBarrierUnavailable,
}

impl ComputeConvergenceController {
    fn new(status: ComputeConvergenceStatus, hold_post_upload: bool) -> Self {
        Self {
            state: Arc::new(Mutex::new(ComputeConvergenceControlState {
                pending_outcome: None,
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

    pub fn drain_events(
        &self,
    ) -> Result<Vec<ComputeConvergenceEvent>, ComputeConvergenceControlError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| ComputeConvergenceControlError::Unavailable)?;
        Ok(state.events.drain(..).collect())
    }

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
    generation: ComputeConvergenceGeneration,
}

impl ComputeConvergenceWorkStamp {
    fn new(revision: VoxelSceneRevision, generation: ComputeConvergenceGeneration) -> Self {
        Self {
            revision,
            generation,
        }
    }

    pub fn revision(self) -> VoxelSceneRevision {
        self.revision
    }

    pub fn generation(self) -> ComputeConvergenceGeneration {
        self.generation
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
}

impl ComputeConvergenceStatus {
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

#[derive(Debug, Error)]
pub enum ComputeConvergenceError {
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
}

impl ComputePreparationTarget {
    fn stamp(&self, generation: ComputeConvergenceGeneration) -> ComputeConvergenceWorkStamp {
        ComputeConvergenceWorkStamp::new(self.view.revision(), generation)
    }
}

enum ComputePreparationCompletion {
    Completed(Result<ComputeSceneBundle, ComputeSceneBuildError>),
    WorkerTerminated,
}

enum ComputeActivePreparationStatus {
    Running,
    Ready(ComputeSceneBundle),
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
    required_revision: VoxelSceneRevision,
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
        Self {
            scene_identity,
            installed_bundle,
            installed_generation: ComputeConvergenceGeneration::initial(),
            required_revision,
            required_generation: ComputeConvergenceGeneration::initial(),
            active: None,
            pending: None,
            paused: None,
            hidden: None,
            events: ComputeConvergenceEvents::new(),
            control: None,
        }
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
            visible_revision: self.installed_bundle.revision(),
            required_generation: self.required_generation,
            installed: ComputeConvergenceWorkStamp::new(
                self.installed_bundle.revision(),
                self.installed_generation,
            ),
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
            worker_count: usize::from(
                self.active
                    .as_ref()
                    .is_some_and(|active| active.worker.is_some()),
            ),
        }
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
        let target = ComputePreparationTarget { view };
        self.schedule_target(generation, target)?;
        self.required_generation = generation;
        self.required_revision = change_set.successor_revision();
        Ok(ComputeConvergenceAcceptance::Accepted {
            stamp: ComputeConvergenceWorkStamp::new(self.required_revision, generation),
        })
    }

    pub(crate) fn request_retry(
        &mut self,
    ) -> Result<ComputeConvergenceRetry, ComputeConvergenceError> {
        if self.required_revision == self.installed_bundle.revision() {
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
            stamp: ComputeConvergenceWorkStamp::new(self.required_revision, generation),
        })
    }

    pub(crate) fn drain_events(&mut self) -> Vec<ComputeConvergenceEvent> {
        self.poll_preparation();
        self.events.drain()
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
            bundle,
        });
    }

    pub(crate) fn hidden_bundle(&self) -> Option<&ComputeSceneBundle> {
        self.hidden.as_ref().map(|candidate| &candidate.bundle)
    }

    pub(crate) fn mark_hidden_uploaded(&mut self) {
        if let Some(candidate) = &self.hidden {
            self.events
                .push(ComputeConvergenceEvent::CandidateUploaded {
                    stamp: candidate.stamp(),
                });
        }
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
        let Some(candidate) = self.hidden.take() else {
            return Ok(None);
        };
        let stamp = candidate.stamp();
        let retired = std::mem::replace(&mut self.installed_bundle, candidate.bundle);
        self.installed_generation = candidate.generation;
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

    pub(crate) fn shutdown(&mut self) -> Result<(), String> {
        if let Some(active) = &self.active {
            active.cancellation.store(true, Ordering::Release);
        }
        let barrier_error = self.active.as_ref().and_then(|active| {
            active
                .preparation_barrier
                .as_ref()
                .and_then(|barrier| barrier.release().err())
                .map(|_| "the compute convergence preparation barrier is unavailable".to_owned())
        });
        let active = self.active.take();
        self.pending = None;
        self.paused = None;
        self.hidden = None;
        let worker_error = if let Some(mut active) = active
            && let Some(worker) = active.worker.take()
            && worker.join().is_err()
        {
            Some(format!(
                "compute convergence worker terminated for Voxel Scene Revision {}",
                active.target.view.revision()
            ))
        } else {
            None
        };
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
            let preparation =
                Self::start_preparation(generation, target, &mut self.events, preparation_barrier);
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
        let preparation_barrier = self.preparation_barrier()?;
        let preparation =
            Self::start_preparation(generation, target, &mut self.events, preparation_barrier)?;
        self.pending = None;
        self.paused = None;
        self.active = Some(preparation);
        Ok(())
    }

    fn start_preparation(
        generation: ComputeConvergenceGeneration,
        target: ComputePreparationTarget,
        events: &mut ComputeConvergenceEvents,
        preparation_barrier: Option<Arc<ComputePreparationBarrierShared>>,
    ) -> Result<ComputeActivePreparation, ComputeConvergenceError> {
        let stamp = target.stamp(generation);
        let cancellation = Arc::new(AtomicBool::new(false));
        let (completion_sender, completion_receiver) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name(format!("compute-convergence-{}", stamp.revision))
            .spawn({
                let view = target.view.clone();
                let cancellation = Arc::clone(&cancellation);
                let preparation_barrier = preparation_barrier.clone();
                move || {
                    let mut result = ComputeSceneBundle::from_view_with_block_completion(
                        &view,
                        || cancellation.load(Ordering::Acquire),
                        || {
                            preparation_barrier
                                .as_ref()
                                .map(|barrier| {
                                    barrier.complete_block_and_wait(view.revision()).map_err(|_| {
                                        ComputeSceneBuildError::PreparationBarrier
                                    })
                                })
                                .unwrap_or(Ok(()))
                        },
                    );
                    if let Some(barrier) = &preparation_barrier {
                        let cancelled = matches!(result, Err(ComputeSceneBuildError::Cancelled));
                        if barrier.finish(view.revision(), cancelled).is_err() {
                            result = Err(ComputeSceneBuildError::PreparationBarrier);
                        }
                    }
                    if completion_sender
                        .send(ComputePreparationCompletion::Completed(result))
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
            ComputePreparationCompletion::Completed(Ok(_)) => {
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
        convergence.fail_hidden(
            ComputeConvergenceFailurePhase::Upload,
            "injected upload failure".to_owned(),
        );
        assert_eq!(
            convergence
                .status()
                .paused()
                .map(ComputeConvergenceWorkStamp::generation),
            Some(first.generation())
        );

        let ComputeConvergenceRetry::Requested { stamp: retry } = convergence.request_retry()?
        else {
            return Err("retry was not requested".into());
        };
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

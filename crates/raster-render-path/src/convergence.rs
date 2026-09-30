use super::RasterRenderPath;
use super::gpu_resources::{
    RasterRegionGpuResources, release_raster_region_resources, upload_raster_region_resources,
};
use super::installation::{
    RasterRegionActivity, RasterRegionInstallation, RasterRegionInstallationGeneration,
};
use super::lifecycle::{
    RasterCancellationObservation, RasterConvergenceCpuBarrierShared, RasterConvergenceStatus,
    RasterRegionWorkDisposition,
};
use super::meshing::{
    RasterArtifact, RasterArtifactBuildError, RasterRegionIdentity, RasterRegionResult,
    affected_raster_region_identities, assemble_raster_artifact, derive_raster_region,
    visit_raster_region_cores,
};
use super::residency::{RasterResidencyTarget, derive_selection};
use super::worker_pool::{RasterTask, RasterWorkerPool};
use render_backend::RenderPathDeviceContext;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};
use thiserror::Error;
use voxel_frontend::{
    VoxelEditOutcome, VoxelExtent, VoxelFrontendError, VoxelResidencySelection, VoxelSceneId,
    VoxelSceneRevision, VoxelSceneView,
};

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

pub(super) const RASTER_CONVERGENCE_EVENT_CAPACITY: usize = 16;

pub(super) struct RasterConvergenceEvents {
    pub(super) retained: VecDeque<RasterConvergenceEvent>,
    discarded: u64,
    compacted_candidate_rejections: Option<RasterCompactedCandidateRejections>,
}

pub(super) struct RasterCompactedCandidateRejections {
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
pub(super) enum RasterConvergenceUpload {
    NoReadyPreparation,
    CandidateAlreadyRetained { revision: VoxelSceneRevision },
    Uploaded { revision: VoxelSceneRevision },
}

#[must_use = "retired GPU resources must be handed to safe retirement"]
pub(super) struct RasterCandidateRetirement {
    pub(super) resources: Vec<RasterRegionGpuResources>,
}

pub(super) struct RasterConvergenceShutdown {
    pub(super) retirement: RasterCandidateRetirement,
    pub(super) worker_error: Option<RasterConvergenceError>,
}

pub(super) struct RasterHiddenRelease {
    pub(super) retirement: RasterCandidateRetirement,
    pub(super) restart_error: Option<RasterConvergenceError>,
}

impl RasterCandidateRetirement {
    pub(super) fn resource_count(&self) -> usize {
        self.resources.len()
    }

    pub(super) fn release_with(mut self, mut release: impl FnMut(RasterRegionGpuResources)) {
        for resources in self.resources.drain(..) {
            release(resources);
        }
    }

    /// # Safety
    ///
    /// No submitted GPU work may still reference any resource in this handoff.
    pub(super) unsafe fn release_after_gpu_completion(self, device: &RenderPathDeviceContext<'_>) {
        self.release_with(|resources| release_raster_region_resources(device, resources));
    }
}

#[must_use = "the commit outcome contains resources that must be handed to safe retirement"]
pub(super) enum RasterConvergenceCommit {
    NoCandidate,
    Failed {
        retirement: RasterCandidateRetirement,
    },
    Rejected {
        revision: VoxelSceneRevision,
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
    #[error(transparent)]
    Residency(#[from] VoxelFrontendError),
    #[error("Raster residency selection exceeds nine volumes")]
    ResidencySelectionTooLarge,
    #[error("Raster Convergence has already been started")]
    AlreadyStarted,
    #[error("Raster Convergence has not been started")]
    NotStarted,
    #[cfg(any(test, feature = "qualification"))]
    #[error("Raster Convergence CPU barrier state is unavailable")]
    CpuBarrierSynchronization,
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
    #[error("the Raster Convergence lifecycle control state is unavailable")]
    LifecycleControlUnavailable,
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
pub(super) enum RasterPreparationTargetScope {
    Localized(HashSet<RasterRegionIdentity>),
    FullRebuild,
    Residency(RasterResidencyTarget),
}

#[derive(Clone)]
pub(super) struct RasterPreparationTarget {
    pub(super) view: VoxelSceneView,
    region_extent: VoxelExtent,
    pub(super) scope: RasterPreparationTargetScope,
}

pub(super) enum RasterPreparationCompletion {
    Selection(Result<Option<RasterArtifact>, RasterArtifactBuildError>),
    Completed(Result<Vec<RasterRegionResult>, RasterDerivationFailure>),
    Cancelled,
    #[cfg(any(test, feature = "qualification"))]
    SynchronizationFailed,
}

pub(super) struct RasterDerivationFailure {
    pub(super) region_identity: Option<RasterRegionIdentity>,
    pub(super) source: RasterArtifactBuildError,
}

pub(super) enum RasterActivePreparationStatus {
    Running,
    Ready { regions: Vec<RasterRegionResult> },
}

pub(super) struct RasterActivePreparation {
    generation: RasterConvergenceGeneration,
    target: RasterPreparationTarget,
    cancellation: Arc<AtomicBool>,
    pub(super) completion_receiver: mpsc::Receiver<RasterPreparationCompletion>,
    pub(super) worker: Option<RasterTask<()>>,
    pub(super) status: RasterActivePreparationStatus,
    counters: Arc<RasterPreparationCounters>,
    prepared_artifact: Option<RasterArtifact>,
}

#[derive(Default)]
pub(super) struct RasterPreparationCounters {
    scheduled_regions: AtomicU64,
    completed_regions: AtomicU64,
    cpu_derivation_nanoseconds: AtomicU64,
}

pub(super) struct RasterHiddenCandidate {
    generation: RasterConvergenceGeneration,
    pub(super) target: RasterPreparationTarget,
    artifact: RasterArtifact,
    installations: Vec<RasterRegionInstallation>,
    pub(super) visible_installations: Vec<RasterRegionInstallation>,
    affected_regions: HashSet<RasterRegionIdentity>,
    pub(super) successor_gpu_resources: Vec<RasterRegionGpuResources>,
    retired_gpu_resources: Vec<RasterRegionGpuResources>,
    configured_resources: bool,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) struct RasterConvergenceGeneration(u64);

impl RasterConvergenceGeneration {
    fn initial() -> Self {
        Self(0)
    }

    fn checked_successor(self) -> Option<Self> {
        self.0.checked_add(1).map(Self)
    }
}

pub struct RasterConvergence {
    pub(super) worker_pool: Option<Arc<RasterWorkerPool>>,
    scene_identity: VoxelSceneId,
    visible_revision: VoxelSceneRevision,
    required_revision: VoxelSceneRevision,
    region_extent: VoxelExtent,
    required_generation: RasterConvergenceGeneration,
    pub(super) residency_target: Option<RasterResidencyTarget>,
    required_view: Option<VoxelSceneView>,
    #[cfg(any(test, feature = "qualification"))]
    pub(super) injected_failure: Option<RasterConvergenceFailurePhase>,
    pub(super) active: Option<RasterActivePreparation>,
    pub(super) pending: Option<RasterPreparationTarget>,
    pub(super) paused: Option<RasterPreparationTarget>,
    pub(super) hidden_candidate: Option<RasterHiddenCandidate>,
    pub(super) events: RasterConvergenceEvents,
    pub(super) cpu_barrier: Option<Arc<RasterConvergenceCpuBarrierShared>>,
    last_affected_region_count: usize,
    pub(super) cpu_derivation: Duration,
    pub(super) work_disposition: RasterRegionWorkDisposition,
    pub(super) cancellation_observations: Vec<RasterCancellationObservation>,
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
            worker_pool: None,
            scene_identity,
            visible_revision,
            required_revision: visible_revision,
            region_extent,
            required_generation: RasterConvergenceGeneration::initial(),
            residency_target: artifact
                .residency
                .as_ref()
                .map(|residency| RasterResidencyTarget {
                    selection: residency.coverage.installed_selection().clone(),
                    frontend: residency.frontend.clone(),
                    cache: residency.cache.clone(),
                }),
            required_view: artifact
                .residency
                .as_ref()
                .map(|residency| residency.view.clone()),
            #[cfg(any(test, feature = "qualification"))]
            injected_failure: None,
            active: None,
            pending: None,
            paused: None,
            hidden_candidate: None,
            events: RasterConvergenceEvents::new(),
            cpu_barrier: None,
            last_affected_region_count: 0,
            cpu_derivation: Duration::ZERO,
            work_disposition: RasterRegionWorkDisposition::default(),
            cancellation_observations: Vec::new(),
        })
    }

    pub fn visible_revision(&self) -> VoxelSceneRevision {
        self.visible_revision
    }

    pub fn required_revision(&self) -> VoxelSceneRevision {
        self.required_revision
    }

    pub(super) fn status(&self, installed_region_count: usize) -> RasterConvergenceStatus {
        let affected_region_count = match self.newest_target().map(|target| &target.scope) {
            Some(RasterPreparationTargetScope::Localized(affected)) => affected.len(),
            Some(
                RasterPreparationTargetScope::FullRebuild
                | RasterPreparationTargetScope::Residency(_),
            ) => installed_region_count,
            None => self.last_affected_region_count,
        }
        .min(installed_region_count);
        RasterConvergenceStatus {
            required_revision: self.required_revision,
            visible_revision: self.visible_revision,
            affected_region_count,
            unaffected_region_count: installed_region_count - affected_region_count,
        }
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

        let target_scope = if let Some(residency) = &self.residency_target {
            RasterPreparationTargetScope::Residency(residency.clone())
        } else if change_set.predecessor_revision() == self.required_revision {
            let mut affected = match self.newest_target().map(|target| &target.scope) {
                Some(RasterPreparationTargetScope::Localized(affected)) => affected.clone(),
                Some(
                    RasterPreparationTargetScope::FullRebuild
                    | RasterPreparationTargetScope::Residency(_),
                ) => HashSet::new(),
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
        let required_view = view.clone();
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
        if self.residency_target.is_some() {
            self.required_view = Some(required_view);
        }
        Ok(RasterConvergenceAcceptance::Accepted {
            revision: self.required_revision,
        })
    }

    pub fn accept_residency_selection(
        &mut self,
        selection: VoxelResidencySelection,
    ) -> Result<bool, RasterConvergenceError> {
        let Some(current) = self.residency_target.as_ref() else {
            return Ok(false);
        };
        let view = self
            .required_view
            .as_ref()
            .ok_or(RasterConvergenceError::MissingVisibleInstallation)?;
        render_backend::RenderPathCoverage::new(view, selection.clone())?;
        if selection.volumes().len() > 9 {
            return Err(RasterConvergenceError::ResidencySelectionTooLarge);
        }
        if selection.identity() == current.selection.identity() && selection != current.selection {
            return Err(VoxelFrontendError::ResidencyIdentityConflict.into());
        }
        if selection.identity() <= current.selection.identity() {
            return Ok(false);
        }
        let residency = RasterResidencyTarget {
            selection,
            frontend: current.frontend.clone(),
            cache: current.cache.clone(),
        };
        let target = RasterPreparationTarget {
            view: view.clone(),
            region_extent: self.region_extent,
            scope: RasterPreparationTargetScope::Residency(residency.clone()),
        };
        let generation = self
            .required_generation
            .checked_successor()
            .ok_or(RasterConvergenceError::GenerationOverflow)?;
        self.schedule_target(generation, target)?;
        self.required_generation = generation;
        self.residency_target = Some(residency);
        Ok(true)
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

    pub(super) fn upload_ready_with_optional_device(
        &mut self,
        device: Option<&RenderPathDeviceContext<'_>>,
        render_path: &RasterRenderPath,
    ) -> Result<RasterConvergenceUpload, RasterConvergenceError> {
        self.upload_ready_with_resource_adapter(device, render_path, None)
    }

    #[cfg(test)]
    pub(super) fn upload_ready_with_test_resources(
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
        if self.active.is_none() && self.hidden_candidate.is_none() {
            self.start_pending_preparation()?;
        }
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
        #[cfg(any(test, feature = "qualification"))]
        if self.injected_failure == Some(RasterConvergenceFailurePhase::Upload) {
            self.injected_failure = None;
            self.pause_after_upload_failure(
                target,
                revision,
                "injected upload failure".into(),
                None,
            );
            return Ok(RasterConvergenceUpload::NoReadyPreparation);
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
            .filter(|region| {
                !matches!(
                    active.target.scope,
                    RasterPreparationTargetScope::Residency(_)
                ) || !installed_artifact.regions().iter().any(|prior| {
                    prior.identity() == region.identity()
                        && Arc::ptr_eq(&prior.geometry, &region.geometry)
                })
            })
            .map(|region| region.identity().clone())
            .collect::<HashSet<_>>();
        let successor_regions = match &active.target.scope {
            RasterPreparationTargetScope::FullRebuild
            | RasterPreparationTargetScope::Residency(_) => regions.clone(),
            RasterPreparationTargetScope::Localized(_) => {
                let replacements = regions
                    .iter()
                    .map(|region| (region.identity(), region))
                    .collect::<HashMap<_, _>>();
                installed_artifact
                    .regions()
                    .iter()
                    .map(|region| {
                        replacements
                            .get(region.identity())
                            .copied()
                            .unwrap_or(region)
                            .clone()
                    })
                    .collect()
            }
        };
        let artifact = if let Some(artifact) = &active.prepared_artifact {
            artifact.clone()
        } else {
            retain_after_upload_failure!(
                assemble_raster_artifact(
                    self.scene_identity.clone(),
                    revision,
                    self.region_extent,
                    successor_regions,
                ),
                None
            )
        };
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
                if !installed_artifact
                    .regions()
                    .iter()
                    .any(|region| region.identity() == identity)
                {
                    continue;
                }
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
                    .try_reserve_exact(artifact.regions().len())
                    .map_err(|_| RasterConvergenceError::ResourceBookkeepingAllocation),
                None
            );
            retain_after_upload_failure!(
                retired_gpu_resources
                    .try_reserve_exact(render_path.region_resources.len())
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

    pub(super) fn commit_at_frame_boundary(
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
                revision,
                retirement: RasterCandidateRetirement {
                    resources: candidate.successor_gpu_resources,
                },
            });
        }
        #[cfg(any(test, feature = "qualification"))]
        if self.injected_failure == Some(RasterConvergenceFailurePhase::Commit) {
            self.injected_failure = None;
            return self.fail_hidden_candidate("injected pre-swap installation failure".into());
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
        self.last_affected_region_count = candidate.affected_regions.len();
        if candidate.configured_resources {
            for resources in std::mem::take(&mut render_path.region_resources) {
                if candidate.affected_regions.contains(&resources.identity)
                    || !candidate
                        .artifact
                        .regions()
                        .iter()
                        .any(|region| region.identity() == &resources.identity)
                {
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

    pub(super) fn take_hidden_resources_for_release(&mut self) -> RasterHiddenRelease {
        self.take_hidden_resources_for_release_with_restart(|convergence| {
            convergence.start_pending_preparation()
        })
    }

    pub(super) fn take_hidden_resources_for_release_with_restart(
        &mut self,
        restart: impl FnOnce(&mut Self) -> Result<(), RasterConvergenceError>,
    ) -> RasterHiddenRelease {
        let Some(candidate) = self.hidden_candidate.take() else {
            return RasterHiddenRelease {
                retirement: RasterCandidateRetirement {
                    resources: Vec::new(),
                },
                restart_error: None,
            };
        };
        let candidate_is_current = candidate.generation == self.required_generation
            && candidate.target.view.revision() == self.required_revision;
        let mut restart_error = None;
        if candidate_is_current && self.active.is_none() && self.pending.is_none() {
            self.pending = Some(candidate.target);
            restart_error = restart(self).err();
        }
        RasterHiddenRelease {
            retirement: RasterCandidateRetirement {
                resources: candidate.successor_gpu_resources,
            },
            restart_error,
        }
    }

    pub(super) fn shutdown(&mut self) -> RasterConvergenceShutdown {
        if let Some(active) = &self.active {
            active.cancellation.store(true, Ordering::Release);
        }
        #[cfg(any(test, feature = "qualification"))]
        let barrier_error = self.cpu_barrier.as_ref().and_then(|barrier| {
            barrier
                .release()
                .err()
                .map(|_| RasterConvergenceError::CpuBarrierSynchronization)
        });
        let active = self.active.take();
        self.pending = None;
        self.paused = None;
        let resources = self
            .hidden_candidate
            .take()
            .map(|candidate| candidate.successor_gpu_resources)
            .unwrap_or_default();
        #[cfg(not(any(test, feature = "qualification")))]
        let barrier_error = None;
        let mut worker_error = barrier_error;
        if let Some(mut active) = active {
            let revision = active.target.view.revision();
            if let Some(worker) = active.worker.take()
                && worker.join().is_err()
                && worker_error.is_none()
            {
                worker_error = Some(RasterConvergenceError::PreparationTerminated { revision });
            }
        }
        self.worker_pool.take();
        RasterConvergenceShutdown {
            retirement: RasterCandidateRetirement { resources },
            worker_error,
        }
    }

    pub(super) fn active_is_ready(&self) -> bool {
        self.active.as_ref().is_some_and(|active| {
            matches!(active.status, RasterActivePreparationStatus::Ready { .. })
        })
    }

    fn schedule_target(
        &mut self,
        generation: RasterConvergenceGeneration,
        target: RasterPreparationTarget,
    ) -> Result<(), RasterConvergenceError> {
        let result = if self.residency_target.is_some() && self.hidden_candidate.is_some() {
            self.pending = Some(target);
            Ok(())
        } else if self.active_is_ready() {
            self.replace_ready_preparation(generation, target)
        } else if let Some(active) = &self.active {
            active.cancellation.store(true, Ordering::Release);
            self.pending = Some(target);
            Ok(())
        } else {
            let preparation = Self::start_preparation(
                &mut self.worker_pool,
                generation,
                target,
                self.cpu_barrier.clone(),
                &mut self.events,
            )?;
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
        if self.residency_target.is_some() {
            self.discard_ready_preparation();
            self.pending = Some(target);
            return self.start_pending_preparation_with_generation(generation);
        }
        let mut started_events = RasterConvergenceEvents::new();
        let replacement = Self::start_preparation(
            &mut self.worker_pool,
            generation,
            target,
            self.cpu_barrier.clone(),
            &mut started_events,
        )?;
        self.discard_ready_preparation();
        self.active = Some(replacement);
        self.events.append(&mut started_events);
        Ok(())
    }

    fn start_preparation(
        worker_pool: &mut Option<Arc<RasterWorkerPool>>,
        generation: RasterConvergenceGeneration,
        target: RasterPreparationTarget,
        cpu_barrier: Option<Arc<RasterConvergenceCpuBarrierShared>>,
        events: &mut RasterConvergenceEvents,
    ) -> Result<RasterActivePreparation, RasterConvergenceError> {
        let revision = target.view.revision();
        let cancellation = Arc::new(AtomicBool::new(false));
        let counters = Arc::new(RasterPreparationCounters::default());
        let (completion_sender, completion_receiver) = mpsc::sync_channel(1);
        if worker_pool.is_none() {
            *worker_pool = Some(Arc::new(
                (if matches!(target.scope, RasterPreparationTargetScope::Residency(_)) {
                    RasterWorkerPool::with_region_workers(0)
                } else {
                    RasterWorkerPool::new()
                })
                .map_err(|source| RasterConvergenceError::PreparationStart { revision, source })?,
            ));
        }
        let pool = worker_pool
            .as_ref()
            .ok_or(RasterConvergenceError::PreparationTerminated { revision })?;
        let worker = pool.execute({
                let pool = pool.clone();
                let target = target.clone();
                let cancellation = cancellation.clone();
                let counters = counters.clone();
                move || {
                    let completion = derive_convergence_target(
                        &target,
                        cancellation,
                        cpu_barrier.as_deref(),
                        counters,
                        &pool,
                    );
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
            counters,
            prepared_artifact: None,
        })
    }

    pub(super) fn poll_preparation(&mut self) -> Result<(), RasterConvergenceError> {
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
        let completion = match completion {
            RasterPreparationCompletion::Selection(Ok(Some(artifact))) => {
                let regions = artifact.regions().to_vec();
                active.prepared_artifact = Some(artifact);
                RasterPreparationCompletion::Completed(Ok(regions))
            }
            RasterPreparationCompletion::Selection(Ok(None)) => {
                RasterPreparationCompletion::Cancelled
            }
            RasterPreparationCompletion::Selection(Err(source)) => {
                RasterPreparationCompletion::Completed(Err(RasterDerivationFailure {
                    region_identity: None,
                    source,
                }))
            }
            completion => completion,
        };
        if active
            .worker
            .take()
            .is_none_or(|worker| worker.join().is_err())
        {
            self.active = Some(active);
            return self.retain_target_after_termination();
        }
        let scheduled_regions = active.counters.scheduled_regions.load(Ordering::Relaxed);
        let completed_regions = active.counters.completed_regions.load(Ordering::Relaxed);
        let cpu_derivation_nanoseconds = active
            .counters
            .cpu_derivation_nanoseconds
            .load(Ordering::Relaxed);
        let cancelled = matches!(completion, RasterPreparationCompletion::Cancelled);
        let cancelled_regions = u64::from(cancelled).saturating_mul(scheduled_regions);
        self.cpu_derivation = self
            .cpu_derivation
            .saturating_add(Duration::from_nanos(cpu_derivation_nanoseconds));
        self.work_disposition.scheduled = self
            .work_disposition
            .scheduled
            .saturating_add(scheduled_regions);
        if cancelled {
            self.work_disposition.cancelled = self
                .work_disposition
                .cancelled
                .saturating_add(cancelled_regions);
            self.cancellation_observations
                .push(RasterCancellationObservation {
                    revision,
                    scheduled_regions,
                    completed_regions,
                    cancelled_regions,
                });
        }
        let is_current =
            active.generation == self.required_generation && revision == self.required_revision;
        #[cfg(any(test, feature = "qualification"))]
        if matches!(
            completion,
            RasterPreparationCompletion::SynchronizationFailed
        ) {
            return Err(RasterConvergenceError::CpuBarrierSynchronization);
        }
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
        if !is_current || cancelled {
            if !is_current && !cancelled {
                self.work_disposition.stale = self
                    .work_disposition
                    .stale
                    .saturating_add(completed_regions);
            }
            self.events
                .push(RasterConvergenceEvent::PreparationDiscarded {
                    revision,
                    disposition: RasterPreparationDisposition::SupersededBeforeUpload,
                });
            // The completion also holds region geometry; release both holds before the next admission.
            drop(completion);
            drop(active);
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
        self.work_disposition.completed = self
            .work_disposition
            .completed
            .saturating_add(completed_regions);
        #[cfg(any(test, feature = "qualification"))]
        if self.injected_failure == Some(RasterConvergenceFailurePhase::Derivation) {
            self.injected_failure = None;
            self.record_failure(
                revision,
                RasterConvergenceFailurePhase::Derivation,
                "injected preparation failure".into(),
                None,
            );
            self.paused = Some(active.target);
            return Ok(());
        }
        active.status = RasterActivePreparationStatus::Ready { regions };
        self.active = Some(active);
        self.events
            .push(RasterConvergenceEvent::PreparationReady { revision });
        Ok(())
    }

    fn start_pending_preparation(&mut self) -> Result<(), RasterConvergenceError> {
        self.start_pending_preparation_with_generation(self.required_generation)
    }

    fn start_pending_preparation_with_generation(
        &mut self,
        generation: RasterConvergenceGeneration,
    ) -> Result<(), RasterConvergenceError> {
        let Some(target) = self.pending.take() else {
            return Ok(());
        };
        let preparation = Self::start_preparation(
            &mut self.worker_pool,
            generation,
            target.clone(),
            self.cpu_barrier.clone(),
            &mut self.events,
        );
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
        #[cfg(any(test, feature = "qualification"))]
        if let Some(barrier) = &self.cpu_barrier
            && barrier.release().is_err()
        {
            eprintln!("Raster Convergence CPU barrier state was unavailable during drop");
        }
        if let Some(mut active) = self.active.take()
            && let Some(worker) = active.worker.take()
            && worker.join().is_err()
        {
            eprintln!(
                "Raster Convergence worker panicked for Voxel Scene Revision {}",
                active.target.view.revision()
            );
        }
    }
}

fn derive_convergence_target(
    target: &RasterPreparationTarget,
    cancellation: Arc<AtomicBool>,
    cpu_barrier: Option<&RasterConvergenceCpuBarrierShared>,
    counters: Arc<RasterPreparationCounters>,
    pool: &RasterWorkerPool,
) -> RasterPreparationCompletion {
    if let RasterPreparationTargetScope::Residency(residency) = &target.scope {
        let result = derive_selection(
            &target.view,
            target.region_extent,
            residency,
            &cancellation,
            cpu_barrier,
        );
        #[cfg(any(test, feature = "qualification"))]
        if cpu_barrier.is_some_and(|barrier| {
            barrier
                .finish(target.view.revision(), matches!(result, Ok(None)))
                .is_err()
        }) {
            return RasterPreparationCompletion::SynchronizationFailed;
        }
        return RasterPreparationCompletion::Selection(result);
    }
    let mut cores = Vec::new();
    let traversal = visit_raster_region_cores(
        &target.view,
        target.region_extent,
        |metadata, core| {
            if cancellation.load(Ordering::Acquire) {
                return Ok(false);
            }
            let identity = RasterRegionIdentity {
                volume_identity: metadata.identity().clone(),
                core_origin: core.origin(),
            };
            if matches!(&target.scope, RasterPreparationTargetScope::FullRebuild)
                || matches!(&target.scope, RasterPreparationTargetScope::Localized(affected) if affected.contains(&identity))
            {
                cores.push((metadata.clone(), core, identity));
            }
            Ok(true)
        },
    );
    let mut regions = Vec::new();
    let result = (|| {
        if let Err(source) = traversal {
            return RasterPreparationCompletion::Completed(Err(RasterDerivationFailure {
                region_identity: None,
                source,
            }));
        }
        let batch_size = pool.region_worker_count();
        #[cfg(any(test, feature = "qualification"))]
        let batch_size = match cpu_barrier.map(|barrier| barrier.is_armed()).transpose() {
            // An armed qualification hold stops dispatch at the exact requested region count.
            Ok(Some(true)) => 1,
            Ok(_) => batch_size,
            Err(_) => return RasterPreparationCompletion::SynchronizationFailed,
        };
        #[cfg(not(any(test, feature = "qualification")))]
        let _cpu_barrier = cpu_barrier;
        for batch in cores.chunks(batch_size) {
            if cancellation.load(Ordering::Acquire) {
                return RasterPreparationCompletion::Cancelled;
            }
            let mut tasks = Vec::new();
            for (metadata, core, identity) in batch {
                let view = target.view.clone();
                let metadata = metadata.clone();
                let core = *core;
                let cancellation = cancellation.clone();
                let counters = counters.clone();
                tasks.push((
                    identity.clone(),
                    pool.execute(move || {
                        if cancellation.load(Ordering::Acquire) {
                            return None;
                        }
                        saturating_add_atomic(&counters.scheduled_regions, 1);
                        let started = Instant::now();
                        let result = derive_raster_region(&view, &metadata, core);
                        saturating_add_atomic(
                            &counters.cpu_derivation_nanoseconds,
                            started.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64,
                        );
                        if result.is_ok() {
                            saturating_add_atomic(&counters.completed_regions, 1);
                        }
                        Some(result)
                    }),
                ));
            }
            let mut failure = None;
            for (identity, task) in tasks {
                let result = task.map_err(|_| ()).and_then(|task| task.join());
                match result {
                    Ok(Some(Ok(region))) => regions.push(region),
                    Ok(Some(Err(source))) => {
                        if failure.is_none() {
                            failure = Some(RasterDerivationFailure {
                                region_identity: Some(identity),
                                source,
                            });
                        }
                    }
                    Ok(None) => {}
                    Err(()) => {
                        if failure.is_none() {
                            failure = Some(RasterDerivationFailure {
                                region_identity: Some(identity),
                                source: super::meshing::worker_terminated_error(
                                    target.view.revision(),
                                ),
                            });
                        }
                    }
                }
            }
            if let Some(failure) = failure {
                return RasterPreparationCompletion::Completed(Err(failure));
            }
            #[cfg(any(test, feature = "qualification"))]
            if cpu_barrier
                .is_some_and(|barrier| barrier.schedule_and_wait(target.view.revision()).is_err())
            {
                return RasterPreparationCompletion::SynchronizationFailed;
            }
        }
        if cancellation.load(Ordering::Acquire) {
            RasterPreparationCompletion::Cancelled
        } else {
            RasterPreparationCompletion::Completed(Ok(regions))
        }
    })();
    #[cfg(any(test, feature = "qualification"))]
    if cpu_barrier.is_some_and(|barrier| {
        barrier
            .finish(
                target.view.revision(),
                matches!(result, RasterPreparationCompletion::Cancelled),
            )
            .is_err()
    }) {
        return RasterPreparationCompletion::SynchronizationFailed;
    }
    result
}

fn saturating_add_atomic(value: &AtomicU64, addend: u64) {
    let mut current = value.load(Ordering::Relaxed);
    loop {
        let next = current.saturating_add(addend);
        match value.compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return,
            Err(observed) => current = observed,
        }
    }
}

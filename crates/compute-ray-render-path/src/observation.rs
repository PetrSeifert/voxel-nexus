use super::compute_convergence::ComputeConvergenceStatus;
use std::sync::{Arc, Mutex};
use thiserror::Error;
use voxel_frontend::{VoxelSceneId, VoxelSceneRevision};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ComputeTimingPhase {
    Preparation,
    Enumeration,
    Construction,
    Serialization,
    Upload,
    Installation,
    Dispatch,
    Composite,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ComputeTimingEvent {
    pub(super) phase: ComputeTimingPhase,
    pub(super) uploaded_bytes: u64,
    pub(super) scene_identity: VoxelSceneId,
    pub(super) revision: VoxelSceneRevision,
    pub(super) generation: u64,
    pub(super) elapsed_milliseconds: f64,
}

impl ComputeTimingEvent {
    pub fn uploaded_bytes(&self) -> u64 {
        self.uploaded_bytes
    }

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
pub(super) struct ComputeMeasurementState {
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
    pub(super) fn with_initial_event(event: ComputeTimingEvent) -> Self {
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

    pub(super) fn record(
        &self,
        event: ComputeTimingEvent,
    ) -> Result<(), ComputeMeasurementControlError> {
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
    pub(super) bytes: u64,
    pub(super) objects: usize,
    pub(super) allocations: usize,
    pub(super) workers: usize,
    pub(super) views: usize,
    pub(super) residency_copies: usize,
}

impl ComputeOwnedResourceCounts {
    pub fn residency_copies(self) -> usize {
        self.residency_copies
    }

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

pub(super) struct ComputeLifecycleControlState {
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
    pub(super) fn new() -> Self {
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

    pub(super) fn record_resources(
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

    pub(super) fn set_role(
        &self,
        role: ComputeResourceRole,
    ) -> Result<(), ComputeLifecycleControlError> {
        self.state
            .lock()
            .map_err(|_| ComputeLifecycleControlError)?
            .role = role;
        Ok(())
    }

    pub(super) fn record_shutdown(
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

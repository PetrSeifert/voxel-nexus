use crate::{
    CameraState, PresentationConfigurationId, RenderPath, RenderPathDeviceContext,
    RenderPathFrameContext, RenderPathResult, RenderPathTarget,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::{error::Error as StdError, fmt};
use thiserror::Error;
use voxel_frontend::{VoxelSceneId, VoxelSceneRevision};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CameraStateRevision(u64);

impl CameraStateRevision {
    pub fn new(revision: u64) -> Self {
        Self(revision)
    }

    pub fn checked_successor(self) -> Option<Self> {
        self.0.checked_add(1).map(Self)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RenderPathStrategy {
    Raster,
    ComputeRay,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RenderPathReadiness {
    Preparing,
    Recordable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenderPathStamp {
    strategy: RenderPathStrategy,
    scene_identity: VoxelSceneId,
    required_revision: VoxelSceneRevision,
    visible_revision: VoxelSceneRevision,
    camera_state_revision: CameraStateRevision,
    presentation_configuration: Option<PresentationConfigurationId>,
    readiness: RenderPathReadiness,
}

impl RenderPathStamp {
    pub fn new(
        strategy: RenderPathStrategy,
        scene_identity: VoxelSceneId,
        required_revision: VoxelSceneRevision,
        visible_revision: VoxelSceneRevision,
        camera_state_revision: CameraStateRevision,
        presentation_configuration: Option<PresentationConfigurationId>,
        readiness: RenderPathReadiness,
    ) -> Self {
        Self {
            strategy,
            scene_identity,
            required_revision,
            visible_revision,
            camera_state_revision,
            presentation_configuration,
            readiness,
        }
    }

    pub fn strategy(&self) -> RenderPathStrategy {
        self.strategy
    }

    pub fn scene_identity(&self) -> &VoxelSceneId {
        &self.scene_identity
    }

    pub fn required_revision(&self) -> VoxelSceneRevision {
        self.required_revision
    }

    pub fn visible_revision(&self) -> VoxelSceneRevision {
        self.visible_revision
    }

    pub fn camera_state_revision(&self) -> CameraStateRevision {
        self.camera_state_revision
    }

    pub fn presentation_configuration(&self) -> Option<PresentationConfigurationId> {
        self.presentation_configuration
    }

    pub fn readiness(&self) -> RenderPathReadiness {
        self.readiness
    }

    fn is_fully_converged(&self) -> bool {
        self.required_revision == self.visible_revision
            && self.readiness == RenderPathReadiness::Recordable
    }

    fn handoff_mismatch(&self, replacement: &Self) -> Option<RenderPathHandoffMismatch> {
        if self.scene_identity != replacement.scene_identity {
            Some(RenderPathHandoffMismatch::SceneIdentity)
        } else if !self.is_fully_converged()
            || self.visible_revision != replacement.visible_revision
            || replacement.required_revision != replacement.visible_revision
        {
            Some(RenderPathHandoffMismatch::VoxelSceneRevision)
        } else if self.camera_state_revision != replacement.camera_state_revision {
            Some(RenderPathHandoffMismatch::CameraStateRevision)
        } else if self.presentation_configuration.is_none()
            || self.presentation_configuration != replacement.presentation_configuration
        {
            Some(RenderPathHandoffMismatch::PresentationConfiguration)
        } else if replacement.readiness != RenderPathReadiness::Recordable {
            Some(RenderPathHandoffMismatch::Readiness)
        } else {
            None
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RenderPathRetirement {
    Pending,
    Complete,
}

pub trait SwitchableRenderPath: RenderPath {
    fn stamp(&self) -> RenderPathStamp;

    fn retire_at_frame_boundary(
        &mut self,
        device: RenderPathDeviceContext<'_>,
    ) -> RenderPathResult<RenderPathRetirement>;
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum RenderPathSwitchRequestError {
    #[error("the active Render Path does not support runtime switching")]
    SwitchingUnavailable,
    #[error("a Render Path switch is already in progress")]
    SwitchInProgress,
    #[error(
        "the Presenting Render Path is not fully converged: Required revision {required_revision}, Visible revision {visible_revision}, readiness {readiness:?}"
    )]
    PresentingPathNotConverged {
        required_revision: VoxelSceneRevision,
        visible_revision: VoxelSceneRevision,
        readiness: RenderPathReadiness,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RenderPathSwitchEvent {
    Requested {
        presenting: RenderPathStrategy,
        replacement: RenderPathStrategy,
    },
    ReplacementFailed {
        replacement: RenderPathStrategy,
        message: String,
    },
    ReplacementCleaned {
        replacement: RenderPathStrategy,
    },
    ReplacementCleanupFailed {
        replacement: RenderPathStrategy,
        message: String,
    },
    Rejected {
        presenting: RenderPathStrategy,
        reason: RenderPathSwitchRequestError,
    },
    HandoffDeferred {
        presenting: RenderPathStrategy,
        replacement: RenderPathStrategy,
        mismatch: RenderPathHandoffMismatch,
    },
    HandoffHeld {
        presenting: RenderPathStrategy,
        replacement: RenderPathStrategy,
    },
    HandedOff {
        presenting: RenderPathStrategy,
        retiring: RenderPathStrategy,
    },
    Retired {
        retired: RenderPathStrategy,
    },
    RetirementFailed {
        retiring: RenderPathStrategy,
        message: String,
    },
}

#[derive(Clone, Debug)]
pub struct RenderPathHandoffControl {
    held: Arc<AtomicBool>,
}

impl RenderPathHandoffControl {
    pub fn hold(&self) {
        self.held.store(true, Ordering::SeqCst);
    }

    pub fn release(&self) {
        self.held.store(false, Ordering::SeqCst);
    }

    pub fn is_held(&self) -> bool {
        self.held.load(Ordering::SeqCst)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RenderPathHandoffMismatch {
    SceneIdentity,
    VoxelSceneRevision,
    CameraStateRevision,
    PresentationConfiguration,
    Readiness,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RenderPathRoleStatus {
    presenting: RenderPathStrategy,
    replacement: Option<RenderPathStrategy>,
    retiring: Option<RenderPathStrategy>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenderPathSwitchDiagnostics {
    roles: RenderPathRoleStatus,
    presenting: RenderPathStamp,
    replacement: Option<RenderPathStamp>,
    retiring: Option<RenderPathStamp>,
    events: Vec<RenderPathSwitchEvent>,
}

impl RenderPathSwitchDiagnostics {
    pub fn roles(&self) -> RenderPathRoleStatus {
        self.roles
    }

    pub fn presenting(&self) -> &RenderPathStamp {
        &self.presenting
    }

    pub fn replacement(&self) -> Option<&RenderPathStamp> {
        self.replacement.as_ref()
    }

    pub fn retiring(&self) -> Option<&RenderPathStamp> {
        self.retiring.as_ref()
    }

    pub fn events(&self) -> &[RenderPathSwitchEvent] {
        &self.events
    }
}

impl RenderPathRoleStatus {
    pub fn presenting(&self) -> RenderPathStrategy {
        self.presenting
    }

    pub fn replacement(&self) -> Option<RenderPathStrategy> {
        self.replacement
    }

    pub fn retiring(&self) -> Option<RenderPathStrategy> {
        self.retiring
    }
}

pub struct RenderPathSwitchOwner {
    presenting: Box<dyn SwitchableRenderPath>,
    replacement: Option<Box<dyn SwitchableRenderPath>>,
    replacement_needs_configuration: bool,
    replacement_cleanup_pending: bool,
    retiring: Option<Box<dyn SwitchableRenderPath>>,
    handoff_control: RenderPathHandoffControl,
    events: Vec<RenderPathSwitchEvent>,
}

#[derive(Debug, Error)]
#[error("Replacement Render Path failed: {failure}; replacement cleanup failed: {cleanup}")]
struct RenderPathReplacementCleanupError {
    #[source]
    failure: Box<dyn std::error::Error + Send + Sync>,
    cleanup: Box<dyn std::error::Error + Send + Sync>,
}

#[derive(Clone, Copy)]
enum RenderPathOwnedRole {
    Presenting,
    Replacement,
    Retiring,
}

impl fmt::Display for RenderPathOwnedRole {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Presenting => "Presenting",
            Self::Replacement => "Replacement",
            Self::Retiring => "Retiring",
        })
    }
}

struct RenderPathLifecycleFailure {
    role: RenderPathOwnedRole,
    strategy: RenderPathStrategy,
    source: Box<dyn StdError + Send + Sync>,
}

struct RenderPathLifecycleError {
    operation: RenderPathLifecycleOperation,
    failures: Vec<RenderPathLifecycleFailure>,
}

#[derive(Clone, Copy)]
enum RenderPathLifecycleOperation {
    Release,
    Configure,
    Shutdown,
}

impl fmt::Display for RenderPathLifecycleOperation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Release => "release",
            Self::Configure => "configure",
            Self::Shutdown => "shutdown",
        })
    }
}

impl fmt::Debug for RenderPathLifecycleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl fmt::Display for RenderPathLifecycleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "owned Render Path {} failures: ", self.operation)?;
        for (index, failure) in self.failures.iter().enumerate() {
            if index != 0 {
                formatter.write_str("; ")?;
            }
            write!(
                formatter,
                "{} {:?}: {}",
                failure.role, failure.strategy, failure.source
            )?;
        }
        Ok(())
    }
}

impl StdError for RenderPathLifecycleError {}

fn finish_owned_lifecycle_operation(
    operation: RenderPathLifecycleOperation,
    failures: Vec<RenderPathLifecycleFailure>,
) -> RenderPathResult<()> {
    if failures.is_empty() {
        Ok(())
    } else {
        Err(Box::new(RenderPathLifecycleError {
            operation,
            failures,
        }))
    }
}

fn retain_lifecycle_result(
    failures: &mut Vec<RenderPathLifecycleFailure>,
    role: RenderPathOwnedRole,
    strategy: RenderPathStrategy,
    result: RenderPathResult<()>,
) {
    if let Err(source) = result {
        failures.push(RenderPathLifecycleFailure {
            role,
            strategy,
            source,
        });
    }
}

impl RenderPathSwitchOwner {
    pub fn new(presenting: Box<dyn SwitchableRenderPath>) -> Self {
        Self {
            presenting,
            replacement: None,
            replacement_needs_configuration: false,
            replacement_cleanup_pending: false,
            retiring: None,
            handoff_control: RenderPathHandoffControl {
                held: Arc::new(AtomicBool::new(false)),
            },
            events: Vec::new(),
        }
    }

    pub fn handoff_control(&self) -> RenderPathHandoffControl {
        self.handoff_control.clone()
    }

    pub fn request_switch(
        &mut self,
        replacement: Box<dyn SwitchableRenderPath>,
    ) -> Result<(), RenderPathSwitchRequestError> {
        let presenting_stamp = self.presenting.stamp();
        let presenting = presenting_stamp.strategy();
        if self.replacement.is_some() || self.retiring.is_some() {
            let reason = RenderPathSwitchRequestError::SwitchInProgress;
            self.events.push(RenderPathSwitchEvent::Rejected {
                presenting,
                reason: reason.clone(),
            });
            return Err(reason);
        }
        if !presenting_stamp.is_fully_converged() {
            let reason = RenderPathSwitchRequestError::PresentingPathNotConverged {
                required_revision: presenting_stamp.required_revision(),
                visible_revision: presenting_stamp.visible_revision(),
                readiness: presenting_stamp.readiness(),
            };
            self.events.push(RenderPathSwitchEvent::Rejected {
                presenting,
                reason: reason.clone(),
            });
            return Err(reason);
        }
        let replacement_strategy = replacement.stamp().strategy();
        self.events.push(RenderPathSwitchEvent::Requested {
            presenting,
            replacement: replacement_strategy,
        });
        self.replacement = Some(replacement);
        self.replacement_needs_configuration = true;
        self.replacement_cleanup_pending = false;
        Ok(())
    }

    pub fn role_status(&self) -> RenderPathRoleStatus {
        RenderPathRoleStatus {
            presenting: self.presenting.stamp().strategy(),
            replacement: self
                .replacement
                .as_ref()
                .map(|replacement| replacement.stamp().strategy()),
            retiring: self
                .retiring
                .as_ref()
                .map(|retiring| retiring.stamp().strategy()),
        }
    }

    pub fn events(&self) -> &[RenderPathSwitchEvent] {
        &self.events
    }

    pub fn diagnostics(&self) -> RenderPathSwitchDiagnostics {
        RenderPathSwitchDiagnostics {
            roles: self.role_status(),
            presenting: self.presenting.stamp(),
            replacement: self
                .replacement
                .as_ref()
                .map(|replacement| replacement.stamp()),
            retiring: self.retiring.as_ref().map(|retiring| retiring.stamp()),
            events: self.events.clone(),
        }
    }

    fn handoff_mismatch(&self) -> Option<RenderPathHandoffMismatch> {
        let presenting = self.presenting.stamp();
        let replacement = self
            .replacement
            .as_ref()
            .map(|replacement| replacement.stamp())?;
        presenting.handoff_mismatch(&replacement)
    }

    fn hand_off_matching_replacement(&mut self) {
        let Some(replacement_strategy) = self
            .replacement
            .as_ref()
            .map(|replacement| replacement.stamp().strategy())
        else {
            return;
        };
        if let Some(mismatch) = self.handoff_mismatch() {
            let event = RenderPathSwitchEvent::HandoffDeferred {
                presenting: self.presenting.stamp().strategy(),
                replacement: replacement_strategy,
                mismatch,
            };
            if self.events.last() != Some(&event) {
                self.events.push(event);
            }
            return;
        }
        if self.handoff_control.is_held() {
            let event = RenderPathSwitchEvent::HandoffHeld {
                presenting: self.presenting.stamp().strategy(),
                replacement: replacement_strategy,
            };
            if self.events.last() != Some(&event) {
                self.events.push(event);
            }
            return;
        }
        let Some(replacement) = self.replacement.take() else {
            return;
        };
        self.replacement_needs_configuration = false;
        self.replacement_cleanup_pending = false;
        let retiring = std::mem::replace(&mut self.presenting, replacement);
        self.events.push(RenderPathSwitchEvent::HandedOff {
            presenting: self.presenting.stamp().strategy(),
            retiring: retiring.stamp().strategy(),
        });
        self.retiring = Some(retiring);
    }

    fn record_replacement_failure(&mut self, failure: &(dyn std::error::Error + Send + Sync)) {
        let Some(replacement) = self.replacement.as_ref() else {
            return;
        };
        self.events.push(RenderPathSwitchEvent::ReplacementFailed {
            replacement: replacement.stamp().strategy(),
            message: failure.to_string(),
        });
        self.replacement_cleanup_pending = true;
    }

    fn clean_pending_replacement(
        &mut self,
        device: RenderPathDeviceContext<'_>,
    ) -> RenderPathResult<()> {
        let Some(mut replacement) = self.replacement.take() else {
            self.replacement_cleanup_pending = false;
            return Ok(());
        };
        let replacement_strategy = replacement.stamp().strategy();
        self.replacement_needs_configuration = false;
        match replacement.shutdown(device) {
            Ok(()) => {
                self.replacement_cleanup_pending = false;
                self.events.push(RenderPathSwitchEvent::ReplacementCleaned {
                    replacement: replacement_strategy,
                });
                Ok(())
            }
            Err(cleanup) => {
                let cleanup_message = cleanup.to_string();
                self.events
                    .push(RenderPathSwitchEvent::ReplacementCleanupFailed {
                        replacement: replacement_strategy,
                        message: cleanup_message.clone(),
                    });
                self.replacement = Some(replacement);
                Err(cleanup)
            }
        }
    }

    fn fail_and_clean_replacement(
        &mut self,
        device: RenderPathDeviceContext<'_>,
        failure: Box<dyn std::error::Error + Send + Sync>,
    ) -> Box<dyn std::error::Error + Send + Sync> {
        self.record_replacement_failure(failure.as_ref());
        match self.clean_pending_replacement(device) {
            Ok(()) => failure,
            Err(cleanup) => Box::new(RenderPathReplacementCleanupError { failure, cleanup }),
        }
    }

    fn configure_replacement(
        &mut self,
        device: RenderPathDeviceContext<'_>,
        target: RenderPathTarget<'_>,
    ) -> RenderPathResult<()> {
        let Some(replacement) = self.replacement.as_mut() else {
            return Ok(());
        };
        if let Err(error) = replacement.configure(device, target) {
            return Err(self.fail_and_clean_replacement(device, error));
        }
        self.replacement_needs_configuration = false;
        Ok(())
    }
}

impl RenderPath for RenderPathSwitchOwner {
    fn publish_camera_state(
        &mut self,
        camera_state: CameraState,
        camera_state_revision: CameraStateRevision,
    ) -> RenderPathResult<()> {
        self.presenting
            .publish_camera_state(camera_state, camera_state_revision)?;
        if !self.replacement_cleanup_pending
            && let Some(replacement) = self.replacement.as_mut()
            && let Err(error) =
                replacement.publish_camera_state(camera_state, camera_state_revision)
        {
            self.record_replacement_failure(error.as_ref());
            return Err(error);
        }
        Ok(())
    }

    fn request_switch(
        &mut self,
        replacement: Box<dyn SwitchableRenderPath>,
    ) -> Result<(), RenderPathSwitchRequestError> {
        RenderPathSwitchOwner::request_switch(self, replacement)
    }

    fn switch_diagnostics(&self) -> Option<RenderPathSwitchDiagnostics> {
        Some(self.diagnostics())
    }

    fn release(&mut self, device: RenderPathDeviceContext<'_>) -> RenderPathResult<()> {
        let mut failures = Vec::new();
        let presenting_strategy = self.presenting.stamp().strategy();
        retain_lifecycle_result(
            &mut failures,
            RenderPathOwnedRole::Presenting,
            presenting_strategy,
            self.presenting.release(device),
        );
        if self.replacement_cleanup_pending {
            let replacement_strategy = self
                .replacement
                .as_ref()
                .map(|replacement| replacement.stamp().strategy());
            if let Some(strategy) = replacement_strategy {
                retain_lifecycle_result(
                    &mut failures,
                    RenderPathOwnedRole::Replacement,
                    strategy,
                    self.clean_pending_replacement(device),
                );
            } else {
                self.replacement_cleanup_pending = false;
            }
        } else if let Some(replacement) = self.replacement.as_mut() {
            let replacement_strategy = replacement.stamp().strategy();
            if let Err(error) = replacement.release(device) {
                let source = self.fail_and_clean_replacement(device, error);
                retain_lifecycle_result(
                    &mut failures,
                    RenderPathOwnedRole::Replacement,
                    replacement_strategy,
                    Err(source),
                );
            } else {
                self.replacement_needs_configuration = true;
            }
        }
        if let Some(retiring) = self.retiring.as_mut() {
            let retiring_strategy = retiring.stamp().strategy();
            retain_lifecycle_result(
                &mut failures,
                RenderPathOwnedRole::Retiring,
                retiring_strategy,
                retiring.release(device),
            );
        }
        finish_owned_lifecycle_operation(RenderPathLifecycleOperation::Release, failures)
    }

    fn configure(
        &mut self,
        device: RenderPathDeviceContext<'_>,
        target: RenderPathTarget<'_>,
    ) -> RenderPathResult<()> {
        let mut failures = Vec::new();
        let presenting_strategy = self.presenting.stamp().strategy();
        retain_lifecycle_result(
            &mut failures,
            RenderPathOwnedRole::Presenting,
            presenting_strategy,
            self.presenting.configure(device, target),
        );
        if self.replacement_cleanup_pending {
            let replacement_strategy = self
                .replacement
                .as_ref()
                .map(|replacement| replacement.stamp().strategy());
            if let Some(strategy) = replacement_strategy {
                retain_lifecycle_result(
                    &mut failures,
                    RenderPathOwnedRole::Replacement,
                    strategy,
                    self.clean_pending_replacement(device),
                );
            } else {
                self.replacement_cleanup_pending = false;
            }
        } else if let Some(replacement_strategy) = self
            .replacement
            .as_ref()
            .map(|replacement| replacement.stamp().strategy())
            && let Err(source) = self.configure_replacement(device, target)
        {
            retain_lifecycle_result(
                &mut failures,
                RenderPathOwnedRole::Replacement,
                replacement_strategy,
                Err(source),
            );
        }
        if let Some(retiring) = self.retiring.as_mut() {
            let retiring_strategy = retiring.stamp().strategy();
            retain_lifecycle_result(
                &mut failures,
                RenderPathOwnedRole::Retiring,
                retiring_strategy,
                retiring.configure(device, target),
            );
        }
        finish_owned_lifecycle_operation(RenderPathLifecycleOperation::Configure, failures)
    }

    fn advance_frame_boundary(
        &mut self,
        device: RenderPathDeviceContext<'_>,
        target: RenderPathTarget<'_>,
    ) -> RenderPathResult<()> {
        self.presenting.advance_frame_boundary(device, target)?;
        if self.replacement_cleanup_pending {
            self.clean_pending_replacement(device)?;
        } else if self.replacement.is_some() {
            if self.replacement_needs_configuration {
                self.configure_replacement(device, target)?;
            }
            let replacement_result = self
                .replacement
                .as_mut()
                .map(|replacement| replacement.advance_frame_boundary(device, target));
            if let Some(Err(error)) = replacement_result {
                return Err(self.fail_and_clean_replacement(device, error));
            }
        }
        if let Some(retiring) = self.retiring.as_mut() {
            let retiring_strategy = retiring.stamp().strategy();
            match retiring.retire_at_frame_boundary(device) {
                Ok(RenderPathRetirement::Pending) => {}
                Ok(RenderPathRetirement::Complete) => {
                    self.retiring = None;
                    self.events.push(RenderPathSwitchEvent::Retired {
                        retired: retiring_strategy,
                    });
                }
                Err(error) => {
                    self.events.push(RenderPathSwitchEvent::RetirementFailed {
                        retiring: retiring_strategy,
                        message: error.to_string(),
                    });
                    return Err(error);
                }
            }
        } else {
            self.hand_off_matching_replacement();
        }
        Ok(())
    }

    fn shutdown(&mut self, device: RenderPathDeviceContext<'_>) -> RenderPathResult<()> {
        let mut failures = Vec::new();
        let presenting_strategy = self.presenting.stamp().strategy();
        retain_lifecycle_result(
            &mut failures,
            RenderPathOwnedRole::Presenting,
            presenting_strategy,
            self.presenting.shutdown(device),
        );
        if let Some(mut replacement) = self.replacement.take() {
            let replacement_strategy = replacement.stamp().strategy();
            retain_lifecycle_result(
                &mut failures,
                RenderPathOwnedRole::Replacement,
                replacement_strategy,
                replacement.shutdown(device),
            );
        }
        self.replacement_needs_configuration = false;
        self.replacement_cleanup_pending = false;
        if let Some(mut retiring) = self.retiring.take() {
            let retiring_strategy = retiring.stamp().strategy();
            retain_lifecycle_result(
                &mut failures,
                RenderPathOwnedRole::Retiring,
                retiring_strategy,
                retiring.shutdown(device),
            );
        }
        finish_owned_lifecycle_operation(RenderPathLifecycleOperation::Shutdown, failures)
    }

    fn record(&mut self, frame: RenderPathFrameContext<'_>) -> RenderPathResult<()> {
        self.presenting.record(frame)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        RenderPathAttachment, RenderPathAttachmentIdentity, RenderPathFrameContext,
        RenderPathFrameTarget, RenderPathTarget,
    };
    use ash::vk;
    use std::marker::PhantomData;
    use std::ptr;
    use std::sync::atomic::AtomicUsize;

    #[derive(Clone, Copy)]
    enum ProofFailurePoint {
        Publication,
        Release,
        Configure,
        AdvanceFrameBoundary,
    }

    struct ProofRenderPath {
        stamp: RenderPathStamp,
        pending_camera_state_revision: Option<CameraStateRevision>,
        configuration_tracks_target: bool,
        become_recordable: Option<Arc<AtomicBool>>,
        failure_point: Option<ProofFailurePoint>,
        retirement_fails: bool,
        configure_count: Option<Arc<AtomicUsize>>,
        record_count: Option<Arc<AtomicUsize>>,
        shutdown_count: Option<Arc<AtomicUsize>>,
    }

    impl RenderPath for ProofRenderPath {
        fn publish_camera_state(
            &mut self,
            _camera_state: CameraState,
            camera_state_revision: CameraStateRevision,
        ) -> RenderPathResult<()> {
            if matches!(self.failure_point, Some(ProofFailurePoint::Publication)) {
                return Err(std::io::Error::other("proof replacement failure").into());
            }
            self.pending_camera_state_revision = Some(camera_state_revision);
            Ok(())
        }

        fn release(&mut self, _device: RenderPathDeviceContext<'_>) -> RenderPathResult<()> {
            if matches!(self.failure_point, Some(ProofFailurePoint::Release)) {
                return Err(std::io::Error::other("proof replacement failure").into());
            }
            if self.configuration_tracks_target {
                self.stamp.presentation_configuration = None;
            }
            Ok(())
        }

        fn configure(
            &mut self,
            _device: RenderPathDeviceContext<'_>,
            target: RenderPathTarget<'_>,
        ) -> RenderPathResult<()> {
            if matches!(self.failure_point, Some(ProofFailurePoint::Configure)) {
                return Err(std::io::Error::other("proof replacement failure").into());
            }
            if self.configuration_tracks_target {
                self.stamp.presentation_configuration = Some(target.configuration_id());
            }
            if let Some(configure_count) = self.configure_count.as_ref() {
                configure_count.fetch_add(1, Ordering::SeqCst);
            }
            Ok(())
        }

        fn advance_frame_boundary(
            &mut self,
            _device: RenderPathDeviceContext<'_>,
            _target: RenderPathTarget<'_>,
        ) -> RenderPathResult<()> {
            if matches!(
                self.failure_point,
                Some(ProofFailurePoint::AdvanceFrameBoundary)
            ) {
                return Err(std::io::Error::other("proof replacement failure").into());
            }
            if let Some(camera_state_revision) = self.pending_camera_state_revision.take() {
                self.stamp.camera_state_revision = camera_state_revision;
            }
            if self
                .become_recordable
                .as_ref()
                .is_some_and(|signal| signal.load(Ordering::SeqCst))
            {
                self.stamp.readiness = RenderPathReadiness::Recordable;
            }
            Ok(())
        }

        fn shutdown(&mut self, _device: RenderPathDeviceContext<'_>) -> RenderPathResult<()> {
            if let Some(shutdown_count) = self.shutdown_count.as_ref() {
                shutdown_count.fetch_add(1, Ordering::SeqCst);
            }
            Ok(())
        }

        fn record(&mut self, _frame: RenderPathFrameContext<'_>) -> RenderPathResult<()> {
            if let Some(record_count) = self.record_count.as_ref() {
                record_count.fetch_add(1, Ordering::SeqCst);
            }
            Ok(())
        }
    }

    impl SwitchableRenderPath for ProofRenderPath {
        fn stamp(&self) -> RenderPathStamp {
            self.stamp.clone()
        }

        fn retire_at_frame_boundary(
            &mut self,
            _device: RenderPathDeviceContext<'_>,
        ) -> RenderPathResult<RenderPathRetirement> {
            if self.retirement_fails {
                Err(std::io::Error::other("proof retirement failure").into())
            } else {
                Ok(RenderPathRetirement::Complete)
            }
        }
    }

    #[derive(Clone, Copy, Eq, PartialEq)]
    enum LifecycleFailurePoint {
        Release,
        Configure,
        Shutdown,
    }

    struct FailingLifecycleRenderPath {
        stamp: RenderPathStamp,
        message: &'static str,
        failure_point: LifecycleFailurePoint,
        failure_count: Arc<AtomicUsize>,
        failure_enabled: Arc<AtomicBool>,
    }

    impl FailingLifecycleRenderPath {
        fn fail_at(&self, point: LifecycleFailurePoint) -> RenderPathResult<()> {
            if self.failure_point != point {
                return Ok(());
            }
            self.failure_count.fetch_add(1, Ordering::SeqCst);
            if self.failure_enabled.load(Ordering::SeqCst) {
                Err(std::io::Error::other(self.message).into())
            } else {
                Ok(())
            }
        }
    }

    impl RenderPath for FailingLifecycleRenderPath {
        fn release(&mut self, _device: RenderPathDeviceContext<'_>) -> RenderPathResult<()> {
            self.fail_at(LifecycleFailurePoint::Release)
        }

        fn configure(
            &mut self,
            _device: RenderPathDeviceContext<'_>,
            _target: RenderPathTarget<'_>,
        ) -> RenderPathResult<()> {
            self.fail_at(LifecycleFailurePoint::Configure)
        }

        fn shutdown(&mut self, _device: RenderPathDeviceContext<'_>) -> RenderPathResult<()> {
            self.fail_at(LifecycleFailurePoint::Shutdown)
        }

        fn record(&mut self, _frame: RenderPathFrameContext<'_>) -> RenderPathResult<()> {
            Ok(())
        }
    }

    impl SwitchableRenderPath for FailingLifecycleRenderPath {
        fn stamp(&self) -> RenderPathStamp {
            self.stamp.clone()
        }

        fn retire_at_frame_boundary(
            &mut self,
            _device: RenderPathDeviceContext<'_>,
        ) -> RenderPathResult<RenderPathRetirement> {
            Ok(RenderPathRetirement::Complete)
        }
    }

    fn lifecycle_failing_path(
        stamp: RenderPathStamp,
        message: &'static str,
        failure_point: LifecycleFailurePoint,
        failure_enabled: bool,
    ) -> (
        Box<dyn SwitchableRenderPath>,
        Arc<AtomicUsize>,
        Arc<AtomicBool>,
    ) {
        let failure_count = Arc::new(AtomicUsize::new(0));
        let failure_enabled = Arc::new(AtomicBool::new(failure_enabled));
        (
            Box::new(FailingLifecycleRenderPath {
                stamp,
                message,
                failure_point,
                failure_count: Arc::clone(&failure_count),
                failure_enabled: Arc::clone(&failure_enabled),
            }),
            failure_count,
            failure_enabled,
        )
    }

    fn require_operation_error<OperationError>(
        result: Result<(), OperationError>,
        unexpected_success: &'static str,
    ) -> RenderPathResult<OperationError> {
        match result {
            Err(error) => Ok(error),
            Ok(()) => Err(std::io::Error::other(unexpected_success).into()),
        }
    }

    fn stamp(
        strategy: RenderPathStrategy,
        required_revision: u64,
        visible_revision: u64,
    ) -> RenderPathStamp {
        RenderPathStamp::new(
            strategy,
            VoxelSceneId::new("canonical"),
            VoxelSceneRevision::new(required_revision),
            VoxelSceneRevision::new(visible_revision),
            CameraStateRevision::new(1),
            Some(PresentationConfigurationId(1)),
            RenderPathReadiness::Recordable,
        )
    }

    fn proof_path(stamp: RenderPathStamp) -> Box<dyn SwitchableRenderPath> {
        Box::new(ProofRenderPath {
            stamp,
            pending_camera_state_revision: None,
            configuration_tracks_target: false,
            become_recordable: None,
            failure_point: None,
            retirement_fails: false,
            configure_count: None,
            record_count: None,
            shutdown_count: None,
        })
    }

    fn failing_path(
        stamp: RenderPathStamp,
        failure_point: ProofFailurePoint,
    ) -> (Box<dyn SwitchableRenderPath>, Arc<AtomicUsize>) {
        let shutdown_count = Arc::new(AtomicUsize::new(0));
        (
            Box::new(ProofRenderPath {
                stamp,
                pending_camera_state_revision: None,
                configuration_tracks_target: false,
                become_recordable: None,
                failure_point: Some(failure_point),
                retirement_fails: false,
                configure_count: None,
                record_count: None,
                shutdown_count: Some(Arc::clone(&shutdown_count)),
            }),
            shutdown_count,
        )
    }

    fn retirement_failing_path(stamp: RenderPathStamp) -> Box<dyn SwitchableRenderPath> {
        Box::new(ProofRenderPath {
            stamp,
            pending_camera_state_revision: None,
            configuration_tracks_target: false,
            become_recordable: None,
            failure_point: None,
            retirement_fails: true,
            configure_count: None,
            record_count: None,
            shutdown_count: None,
        })
    }

    fn controllable_path(
        mut stamp: RenderPathStamp,
    ) -> (
        Box<dyn SwitchableRenderPath>,
        Arc<AtomicBool>,
        Arc<AtomicUsize>,
    ) {
        stamp.readiness = RenderPathReadiness::Preparing;
        let become_recordable = Arc::new(AtomicBool::new(false));
        let configure_count = Arc::new(AtomicUsize::new(0));
        (
            Box::new(ProofRenderPath {
                stamp,
                pending_camera_state_revision: None,
                configuration_tracks_target: false,
                become_recordable: Some(Arc::clone(&become_recordable)),
                failure_point: None,
                retirement_fails: false,
                configure_count: Some(Arc::clone(&configure_count)),
                record_count: None,
                shutdown_count: None,
            }),
            become_recordable,
            configure_count,
        )
    }

    fn observable_path(
        mut stamp: RenderPathStamp,
        starts_preparing: bool,
    ) -> (
        Box<dyn SwitchableRenderPath>,
        Arc<AtomicBool>,
        Arc<AtomicUsize>,
    ) {
        if starts_preparing {
            stamp.readiness = RenderPathReadiness::Preparing;
        }
        let become_recordable = Arc::new(AtomicBool::new(false));
        let record_count = Arc::new(AtomicUsize::new(0));
        (
            Box::new(ProofRenderPath {
                stamp,
                pending_camera_state_revision: None,
                configuration_tracks_target: false,
                become_recordable: starts_preparing.then(|| Arc::clone(&become_recordable)),
                failure_point: None,
                retirement_fails: false,
                configure_count: None,
                record_count: Some(Arc::clone(&record_count)),
                shutdown_count: None,
            }),
            become_recordable,
            record_count,
        )
    }

    fn lifecycle_path(
        stamp: RenderPathStamp,
    ) -> (
        Box<dyn SwitchableRenderPath>,
        Arc<AtomicUsize>,
        Arc<AtomicUsize>,
    ) {
        let configure_count = Arc::new(AtomicUsize::new(0));
        let record_count = Arc::new(AtomicUsize::new(0));
        (
            Box::new(ProofRenderPath {
                stamp,
                pending_camera_state_revision: None,
                configuration_tracks_target: true,
                become_recordable: None,
                failure_point: None,
                retirement_fails: false,
                configure_count: Some(Arc::clone(&configure_count)),
                record_count: Some(Arc::clone(&record_count)),
                shutdown_count: None,
            }),
            configure_count,
            record_count,
        )
    }

    fn proof_device() -> ash::Device {
        unsafe { ash::Device::load_with(|_| ptr::null(), vk::Device::null()) }
    }

    fn proof_target(configuration_id: u64, width: u32, height: u32) -> RenderPathTarget<'static> {
        RenderPathTarget {
            configuration_id: PresentationConfigurationId(configuration_id),
            format: vk::Format::B8G8R8A8_SRGB,
            extent: vk::Extent2D { width, height },
            images: &[],
        }
    }

    fn proof_device_context(device: &ash::Device) -> RenderPathDeviceContext<'_> {
        RenderPathDeviceContext {
            device,
            memory_properties: vk::PhysicalDeviceMemoryProperties::default(),
            capabilities: crate::RenderPathDeviceCapabilities::default(),
        }
    }

    fn advance_owner(
        owner: &mut RenderPathSwitchOwner,
        device: &ash::Device,
    ) -> RenderPathResult<()> {
        owner.advance_frame_boundary(proof_device_context(device), proof_target(1, 800, 600))
    }

    fn record_owner(
        owner: &mut RenderPathSwitchOwner,
        device: &ash::Device,
    ) -> RenderPathResult<()> {
        owner.record(RenderPathFrameContext {
            device,
            command_buffer: vk::CommandBuffer::null(),
            target: RenderPathFrameTarget {
                frame_sequence: 1,
                configuration_id: PresentationConfigurationId(1),
                attachment: RenderPathAttachment {
                    identity: RenderPathAttachmentIdentity(0),
                    _image: vk::Image::null(),
                    view: vk::ImageView::null(),
                    lifetime: PhantomData,
                },
                format: vk::Format::B8G8R8A8_SRGB,
                extent: vk::Extent2D {
                    width: 800,
                    height: 600,
                },
            },
        })
    }

    fn changed_camera_state() -> CameraState {
        CameraState::new(
            [7.0, 6.0, 5.0],
            [0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            50.0,
            0.1,
            100.0,
        )
    }

    #[test]
    fn rejected_switch_is_not_queued_when_the_presenting_path_is_not_converged() {
        let mut owner =
            RenderPathSwitchOwner::new(proof_path(stamp(RenderPathStrategy::Raster, 2, 1)));

        let rejection = owner
            .request_switch(proof_path(stamp(RenderPathStrategy::ComputeRay, 1, 1)))
            .expect_err("a non-converged Presenting Render Path must reject switching");

        assert_eq!(
            rejection,
            RenderPathSwitchRequestError::PresentingPathNotConverged {
                required_revision: VoxelSceneRevision::new(2),
                visible_revision: VoxelSceneRevision::new(1),
                readiness: RenderPathReadiness::Recordable,
            }
        );
        assert_eq!(owner.role_status().presenting(), RenderPathStrategy::Raster);
        assert_eq!(owner.role_status().replacement(), None);
        assert_eq!(owner.role_status().retiring(), None);
        assert_eq!(
            owner.events(),
            &[RenderPathSwitchEvent::Rejected {
                presenting: RenderPathStrategy::Raster,
                reason: rejection,
            }]
        );
    }

    #[test]
    fn rejected_switch_is_not_queued_when_the_presenting_path_is_not_recordable() {
        let mut presenting_stamp = stamp(RenderPathStrategy::Raster, 1, 1);
        presenting_stamp.readiness = RenderPathReadiness::Preparing;
        let mut owner = RenderPathSwitchOwner::new(proof_path(presenting_stamp));

        let rejection = owner
            .request_switch(proof_path(stamp(RenderPathStrategy::ComputeRay, 1, 1)))
            .expect_err("a Preparing Presenting Render Path must reject switching");

        assert_eq!(
            rejection,
            RenderPathSwitchRequestError::PresentingPathNotConverged {
                required_revision: VoxelSceneRevision::new(1),
                visible_revision: VoxelSceneRevision::new(1),
                readiness: RenderPathReadiness::Preparing,
            }
        );
        assert_eq!(owner.role_status().presenting(), RenderPathStrategy::Raster);
        assert_eq!(owner.role_status().replacement(), None);
        assert_eq!(owner.role_status().retiring(), None);
        assert_eq!(
            owner.events(),
            &[RenderPathSwitchEvent::Rejected {
                presenting: RenderPathStrategy::Raster,
                reason: rejection,
            }]
        );
    }

    #[test]
    fn rejected_switch_is_not_queued_while_another_switch_is_in_progress() {
        let mut owner =
            RenderPathSwitchOwner::new(proof_path(stamp(RenderPathStrategy::Raster, 1, 1)));
        owner
            .request_switch(proof_path(stamp(RenderPathStrategy::ComputeRay, 1, 1)))
            .expect("the first request should be admitted");

        let rejection = owner
            .request_switch(proof_path(stamp(RenderPathStrategy::Raster, 1, 1)))
            .expect_err("a request during switching must be rejected");

        assert_eq!(rejection, RenderPathSwitchRequestError::SwitchInProgress);
        assert_eq!(
            owner.role_status().replacement(),
            Some(RenderPathStrategy::ComputeRay)
        );
        assert_eq!(
            owner.events(),
            &[
                RenderPathSwitchEvent::Requested {
                    presenting: RenderPathStrategy::Raster,
                    replacement: RenderPathStrategy::ComputeRay,
                },
                RenderPathSwitchEvent::Rejected {
                    presenting: RenderPathStrategy::Raster,
                    reason: rejection,
                },
            ]
        );
    }

    #[test]
    fn replacement_prepares_without_presenting_then_hands_off_matching_stamps_atomically()
    -> RenderPathResult<()> {
        let mut owner =
            RenderPathSwitchOwner::new(proof_path(stamp(RenderPathStrategy::Raster, 1, 1)));
        let (replacement, become_recordable, configure_count) =
            controllable_path(stamp(RenderPathStrategy::ComputeRay, 1, 1));
        owner
            .request_switch(replacement)
            .expect("the converged idle Presenting Render Path should admit switching");
        let device = proof_device();

        advance_owner(&mut owner, &device)?;
        assert_eq!(configure_count.load(Ordering::SeqCst), 1);
        assert_eq!(owner.role_status().presenting(), RenderPathStrategy::Raster);
        assert_eq!(
            owner.role_status().replacement(),
            Some(RenderPathStrategy::ComputeRay)
        );
        assert_eq!(owner.role_status().retiring(), None);

        become_recordable.store(true, Ordering::SeqCst);
        advance_owner(&mut owner, &device)?;
        assert_eq!(configure_count.load(Ordering::SeqCst), 1);

        assert_eq!(
            owner.role_status().presenting(),
            RenderPathStrategy::ComputeRay
        );
        assert_eq!(owner.role_status().replacement(), None);
        assert_eq!(
            owner.role_status().retiring(),
            Some(RenderPathStrategy::Raster)
        );
        assert_eq!(
            owner.events(),
            &[
                RenderPathSwitchEvent::Requested {
                    presenting: RenderPathStrategy::Raster,
                    replacement: RenderPathStrategy::ComputeRay,
                },
                RenderPathSwitchEvent::HandoffDeferred {
                    presenting: RenderPathStrategy::Raster,
                    replacement: RenderPathStrategy::ComputeRay,
                    mismatch: RenderPathHandoffMismatch::Readiness,
                },
                RenderPathSwitchEvent::HandedOff {
                    presenting: RenderPathStrategy::ComputeRay,
                    retiring: RenderPathStrategy::Raster,
                },
            ]
        );
        Ok(())
    }

    #[test]
    fn held_replacement_tracks_camera_and_presentation_recreation_before_handoff()
    -> RenderPathResult<()> {
        let (presenting, presenting_configures, presenting_records) =
            lifecycle_path(stamp(RenderPathStrategy::Raster, 1, 1));
        let (replacement, replacement_configures, replacement_records) =
            lifecycle_path(stamp(RenderPathStrategy::ComputeRay, 1, 1));
        let mut owner = RenderPathSwitchOwner::new(presenting);
        let handoff_control = owner.handoff_control();
        handoff_control.hold();
        owner
            .request_switch(replacement)
            .expect("the held replacement should be admitted");
        owner.publish_camera_state(changed_camera_state(), CameraStateRevision::new(2))?;
        let device = proof_device();

        owner.advance_frame_boundary(proof_device_context(&device), proof_target(1, 800, 600))?;
        record_owner(&mut owner, &device)?;
        assert_eq!(
            owner.diagnostics().presenting().camera_state_revision(),
            CameraStateRevision::new(2)
        );
        assert_eq!(
            owner
                .diagnostics()
                .replacement()
                .map(RenderPathStamp::camera_state_revision),
            Some(CameraStateRevision::new(2))
        );
        assert_eq!(presenting_records.load(Ordering::SeqCst), 1);
        assert_eq!(replacement_records.load(Ordering::SeqCst), 0);

        for (configuration_id, width, height) in [
            (2, 1200, 700),
            (3, 700, 1200),
            (4, 1000, 700),
            (5, 1000, 700),
        ] {
            owner.release(proof_device_context(&device))?;
            assert_eq!(
                owner
                    .diagnostics()
                    .presenting()
                    .presentation_configuration(),
                None
            );
            assert_eq!(
                owner
                    .diagnostics()
                    .replacement()
                    .and_then(RenderPathStamp::presentation_configuration),
                None
            );
            owner.configure(
                proof_device_context(&device),
                proof_target(configuration_id, width, height),
            )?;
            owner.advance_frame_boundary(
                proof_device_context(&device),
                proof_target(configuration_id, width, height),
            )?;
            assert_eq!(owner.role_status().presenting(), RenderPathStrategy::Raster);
            assert_eq!(
                owner.role_status().replacement(),
                Some(RenderPathStrategy::ComputeRay)
            );
            assert_eq!(owner.role_status().retiring(), None);
        }

        assert_eq!(presenting_configures.load(Ordering::SeqCst), 4);
        assert_eq!(replacement_configures.load(Ordering::SeqCst), 5);
        assert_eq!(
            owner.events().last(),
            Some(&RenderPathSwitchEvent::HandoffHeld {
                presenting: RenderPathStrategy::Raster,
                replacement: RenderPathStrategy::ComputeRay,
            })
        );
        handoff_control.release();
        owner.advance_frame_boundary(proof_device_context(&device), proof_target(5, 1000, 700))?;
        record_owner(&mut owner, &device)?;

        assert_eq!(
            owner.role_status().presenting(),
            RenderPathStrategy::ComputeRay
        );
        assert_eq!(owner.role_status().replacement(), None);
        assert_eq!(presenting_records.load(Ordering::SeqCst), 1);
        assert_eq!(replacement_records.load(Ordering::SeqCst), 1);
        Ok(())
    }

    #[test]
    fn handoff_defers_for_each_mismatched_path_neutral_stamp() -> RenderPathResult<()> {
        let device = proof_device();
        let mut mismatched_replacements = [
            (
                RenderPathHandoffMismatch::SceneIdentity,
                stamp(RenderPathStrategy::ComputeRay, 1, 1),
            ),
            (
                RenderPathHandoffMismatch::VoxelSceneRevision,
                stamp(RenderPathStrategy::ComputeRay, 2, 2),
            ),
            (
                RenderPathHandoffMismatch::CameraStateRevision,
                stamp(RenderPathStrategy::ComputeRay, 1, 1),
            ),
            (
                RenderPathHandoffMismatch::PresentationConfiguration,
                stamp(RenderPathStrategy::ComputeRay, 1, 1),
            ),
            (
                RenderPathHandoffMismatch::Readiness,
                stamp(RenderPathStrategy::ComputeRay, 1, 1),
            ),
        ];
        mismatched_replacements[0].1.scene_identity = VoxelSceneId::new("other");
        mismatched_replacements[2].1.camera_state_revision = CameraStateRevision::new(2);
        mismatched_replacements[3].1.presentation_configuration =
            Some(PresentationConfigurationId(2));
        mismatched_replacements[4].1.readiness = RenderPathReadiness::Preparing;

        for (mismatch, replacement_stamp) in mismatched_replacements {
            let mut owner =
                RenderPathSwitchOwner::new(proof_path(stamp(RenderPathStrategy::Raster, 1, 1)));
            owner
                .request_switch(proof_path(replacement_stamp))
                .expect("the converged idle Presenting Render Path should admit switching");

            advance_owner(&mut owner, &device)?;

            assert_eq!(owner.role_status().presenting(), RenderPathStrategy::Raster);
            assert_eq!(
                owner.role_status().replacement(),
                Some(RenderPathStrategy::ComputeRay)
            );
            assert_eq!(owner.role_status().retiring(), None);
            assert_eq!(
                owner.events().last(),
                Some(&RenderPathSwitchEvent::HandoffDeferred {
                    presenting: RenderPathStrategy::Raster,
                    replacement: RenderPathStrategy::ComputeRay,
                    mismatch,
                })
            );
        }
        Ok(())
    }

    #[test]
    fn failed_replacement_is_cleaned_before_a_fresh_cold_switch() -> RenderPathResult<()> {
        let device = proof_device();
        for failure_point in [
            ProofFailurePoint::Publication,
            ProofFailurePoint::Release,
            ProofFailurePoint::Configure,
            ProofFailurePoint::AdvanceFrameBoundary,
        ] {
            let mut owner =
                RenderPathSwitchOwner::new(proof_path(stamp(RenderPathStrategy::Raster, 1, 1)));
            let (failed_replacement, shutdown_count) =
                failing_path(stamp(RenderPathStrategy::ComputeRay, 1, 1), failure_point);
            owner.request_switch(failed_replacement)?;

            let failure = match failure_point {
                ProofFailurePoint::Publication => {
                    owner.publish_camera_state(changed_camera_state(), CameraStateRevision::new(2))
                }
                ProofFailurePoint::Release => owner.release(proof_device_context(&device)),
                ProofFailurePoint::Configure => {
                    owner.configure(proof_device_context(&device), proof_target(1, 800, 600))
                }
                ProofFailurePoint::AdvanceFrameBoundary => advance_owner(&mut owner, &device),
            };
            let Err(error) = failure else {
                return Err("the injected replacement failure did not reach the caller".into());
            };
            let expected_error = match failure_point {
                ProofFailurePoint::Release => {
                    "owned Render Path release failures: Replacement ComputeRay: proof replacement failure"
                }
                ProofFailurePoint::Configure => {
                    "owned Render Path configure failures: Replacement ComputeRay: proof replacement failure"
                }
                ProofFailurePoint::Publication | ProofFailurePoint::AdvanceFrameBoundary => {
                    "proof replacement failure"
                }
            };
            assert_eq!(error.to_string(), expected_error);
            if matches!(failure_point, ProofFailurePoint::Publication) {
                assert_eq!(shutdown_count.load(Ordering::SeqCst), 0);
                assert_eq!(
                    owner.role_status().replacement(),
                    Some(RenderPathStrategy::ComputeRay)
                );
                advance_owner(&mut owner, &device)?;
            }

            assert_eq!(shutdown_count.load(Ordering::SeqCst), 1);
            assert_eq!(owner.role_status().presenting(), RenderPathStrategy::Raster);
            assert_eq!(owner.role_status().replacement(), None);
            assert_eq!(owner.role_status().retiring(), None);
            assert!(owner.events().ends_with(&[
                RenderPathSwitchEvent::ReplacementFailed {
                    replacement: RenderPathStrategy::ComputeRay,
                    message: "proof replacement failure".to_owned(),
                },
                RenderPathSwitchEvent::ReplacementCleaned {
                    replacement: RenderPathStrategy::ComputeRay,
                },
            ]));

            let mut fresh_stamp = stamp(RenderPathStrategy::ComputeRay, 1, 1);
            if matches!(failure_point, ProofFailurePoint::Publication) {
                fresh_stamp.camera_state_revision = CameraStateRevision::new(2);
            }
            owner.request_switch(proof_path(fresh_stamp))?;
            advance_owner(&mut owner, &device)?;
            assert_eq!(
                owner.role_status().presenting(),
                RenderPathStrategy::ComputeRay
            );
        }
        Ok(())
    }

    #[test]
    fn release_aggregates_presenting_and_replacement_failures() -> RenderPathResult<()> {
        let device = proof_device();
        let (presenting, presenting_release_count, _) = lifecycle_failing_path(
            stamp(RenderPathStrategy::Raster, 1, 1),
            "presenting release failure",
            LifecycleFailurePoint::Release,
            true,
        );
        let (replacement, replacement_release_count, _) = lifecycle_failing_path(
            stamp(RenderPathStrategy::ComputeRay, 1, 1),
            "replacement release failure",
            LifecycleFailurePoint::Release,
            true,
        );
        let mut owner = RenderPathSwitchOwner::new(presenting);
        owner.request_switch(replacement)?;

        let error = require_operation_error(
            crate::run_render_path_phase(crate::RenderPathPhase::Release, || {
                owner.release(proof_device_context(&device))
            }),
            "both owned Render Paths unexpectedly completed release",
        )?;

        assert_eq!(presenting_release_count.load(Ordering::SeqCst), 1);
        assert_eq!(replacement_release_count.load(Ordering::SeqCst), 1);
        assert_eq!(
            error.to_string(),
            "Render Path release failed: owned Render Path release failures: Presenting Raster: presenting release failure; Replacement ComputeRay: replacement release failure"
        );
        Ok(())
    }

    #[test]
    fn configure_aggregates_presenting_and_replacement_failures() -> RenderPathResult<()> {
        let device = proof_device();
        let (presenting, presenting_configure_count, _) = lifecycle_failing_path(
            stamp(RenderPathStrategy::Raster, 1, 1),
            "presenting configure failure",
            LifecycleFailurePoint::Configure,
            true,
        );
        let (replacement, replacement_configure_count, _) = lifecycle_failing_path(
            stamp(RenderPathStrategy::ComputeRay, 1, 1),
            "replacement configure failure",
            LifecycleFailurePoint::Configure,
            true,
        );
        let mut owner = RenderPathSwitchOwner::new(presenting);
        owner.request_switch(replacement)?;

        let error = require_operation_error(
            crate::run_render_path_phase(crate::RenderPathPhase::Configure, || {
                owner.configure(proof_device_context(&device), proof_target(1, 800, 600))
            }),
            "both owned Render Paths unexpectedly completed configuration",
        )?;

        assert_eq!(presenting_configure_count.load(Ordering::SeqCst), 1);
        assert_eq!(replacement_configure_count.load(Ordering::SeqCst), 1);
        assert_eq!(
            error.to_string(),
            "Render Path configure failed: owned Render Path configure failures: Presenting Raster: presenting configure failure; Replacement ComputeRay: replacement configure failure"
        );
        Ok(())
    }

    #[test]
    fn release_aggregates_presenting_and_retiring_failures() -> RenderPathResult<()> {
        let device = proof_device();
        let (retiring, retiring_release_count, _) = lifecycle_failing_path(
            stamp(RenderPathStrategy::Raster, 1, 1),
            "retiring release failure",
            LifecycleFailurePoint::Release,
            true,
        );
        let (presenting, presenting_release_count, _) = lifecycle_failing_path(
            stamp(RenderPathStrategy::ComputeRay, 1, 1),
            "presenting release failure",
            LifecycleFailurePoint::Release,
            true,
        );
        let mut owner = RenderPathSwitchOwner::new(retiring);
        owner.request_switch(presenting)?;
        advance_owner(&mut owner, &device)?;

        let error = require_operation_error(
            crate::run_render_path_phase(crate::RenderPathPhase::Release, || {
                owner.release(proof_device_context(&device))
            }),
            "both owned Render Paths unexpectedly completed release",
        )?;

        assert_eq!(presenting_release_count.load(Ordering::SeqCst), 1);
        assert_eq!(retiring_release_count.load(Ordering::SeqCst), 1);
        assert_eq!(
            error.to_string(),
            "Render Path release failed: owned Render Path release failures: Presenting ComputeRay: presenting release failure; Retiring Raster: retiring release failure"
        );
        Ok(())
    }

    #[test]
    fn configure_aggregates_presenting_and_retiring_failures() -> RenderPathResult<()> {
        let device = proof_device();
        let (retiring, retiring_configure_count, retiring_failure_enabled) = lifecycle_failing_path(
            stamp(RenderPathStrategy::Raster, 1, 1),
            "retiring configure failure",
            LifecycleFailurePoint::Configure,
            false,
        );
        let (presenting, presenting_configure_count, presenting_failure_enabled) =
            lifecycle_failing_path(
                stamp(RenderPathStrategy::ComputeRay, 1, 1),
                "presenting configure failure",
                LifecycleFailurePoint::Configure,
                false,
            );
        let mut owner = RenderPathSwitchOwner::new(retiring);
        owner.request_switch(presenting)?;
        advance_owner(&mut owner, &device)?;
        retiring_failure_enabled.store(true, Ordering::SeqCst);
        presenting_failure_enabled.store(true, Ordering::SeqCst);

        let error = require_operation_error(
            crate::run_render_path_phase(crate::RenderPathPhase::Configure, || {
                owner.configure(proof_device_context(&device), proof_target(2, 650, 900))
            }),
            "both owned Render Paths unexpectedly completed configuration",
        )?;

        assert_eq!(presenting_configure_count.load(Ordering::SeqCst), 2);
        assert_eq!(retiring_configure_count.load(Ordering::SeqCst), 1);
        assert_eq!(
            error.to_string(),
            "Render Path configure failed: owned Render Path configure failures: Presenting ComputeRay: presenting configure failure; Retiring Raster: retiring configure failure"
        );
        Ok(())
    }

    #[test]
    fn shutdown_aggregates_presenting_and_replacement_failures() -> RenderPathResult<()> {
        let device = proof_device();
        let (presenting, presenting_shutdown_count, _) = lifecycle_failing_path(
            stamp(RenderPathStrategy::Raster, 1, 1),
            "presenting shutdown failure",
            LifecycleFailurePoint::Shutdown,
            true,
        );
        let (replacement, replacement_shutdown_count, _) = lifecycle_failing_path(
            stamp(RenderPathStrategy::ComputeRay, 1, 1),
            "replacement shutdown failure",
            LifecycleFailurePoint::Shutdown,
            true,
        );
        let mut owner = RenderPathSwitchOwner::new(presenting);
        owner.request_switch(replacement)?;

        let error = require_operation_error(
            owner.shutdown(proof_device_context(&device)),
            "both owned Render Paths unexpectedly completed shutdown",
        )?;

        assert_eq!(presenting_shutdown_count.load(Ordering::SeqCst), 1);
        assert_eq!(replacement_shutdown_count.load(Ordering::SeqCst), 1);
        assert_eq!(owner.role_status().replacement(), None);
        assert_eq!(owner.role_status().retiring(), None);
        assert_eq!(
            error.to_string(),
            "owned Render Path shutdown failures: Presenting Raster: presenting shutdown failure; Replacement ComputeRay: replacement shutdown failure"
        );
        Ok(())
    }

    #[test]
    fn post_handoff_retirement_failure_retains_the_new_presenting_path_without_rollback()
    -> RenderPathResult<()> {
        let mut owner = RenderPathSwitchOwner::new(retirement_failing_path(stamp(
            RenderPathStrategy::Raster,
            1,
            1,
        )));
        owner
            .request_switch(proof_path(stamp(RenderPathStrategy::ComputeRay, 1, 1)))
            .expect("the converged idle Presenting Render Path should admit switching");
        let device = proof_device();
        advance_owner(&mut owner, &device)?;

        let error = advance_owner(&mut owner, &device)
            .expect_err("the injected retirement failure should reach the caller");

        assert_eq!(error.to_string(), "proof retirement failure");
        assert_eq!(
            owner.role_status().presenting(),
            RenderPathStrategy::ComputeRay
        );
        assert_eq!(owner.role_status().replacement(), None);
        assert_eq!(
            owner.role_status().retiring(),
            Some(RenderPathStrategy::Raster)
        );
        assert_eq!(
            owner.events().last(),
            Some(&RenderPathSwitchEvent::RetirementFailed {
                retiring: RenderPathStrategy::Raster,
                message: "proof retirement failure".to_owned(),
            })
        );
        let diagnostics = owner.diagnostics();
        assert_eq!(
            diagnostics.presenting().strategy(),
            RenderPathStrategy::ComputeRay
        );
        assert_eq!(diagnostics.replacement(), None);
        assert_eq!(
            diagnostics.retiring().map(RenderPathStamp::strategy),
            Some(RenderPathStrategy::Raster)
        );
        assert!(diagnostics.events().iter().any(|event| matches!(
            event,
            RenderPathSwitchEvent::HandedOff {
                presenting: RenderPathStrategy::ComputeRay,
                retiring: RenderPathStrategy::Raster,
            }
        )));
        Ok(())
    }

    #[test]
    fn exactly_one_presenting_path_records_and_retirement_completion_returns_switching_to_idle()
    -> RenderPathResult<()> {
        let (presenting, _, presenting_record_count) =
            observable_path(stamp(RenderPathStrategy::Raster, 1, 1), false);
        let (replacement, become_recordable, replacement_record_count) =
            observable_path(stamp(RenderPathStrategy::ComputeRay, 1, 1), true);
        let mut owner = RenderPathSwitchOwner::new(presenting);
        owner
            .request_switch(replacement)
            .expect("the converged idle Presenting Render Path should admit switching");
        let device = proof_device();

        record_owner(&mut owner, &device)?;
        advance_owner(&mut owner, &device)?;
        record_owner(&mut owner, &device)?;
        assert_eq!(presenting_record_count.load(Ordering::SeqCst), 2);
        assert_eq!(replacement_record_count.load(Ordering::SeqCst), 0);

        become_recordable.store(true, Ordering::SeqCst);
        advance_owner(&mut owner, &device)?;
        record_owner(&mut owner, &device)?;
        advance_owner(&mut owner, &device)?;
        record_owner(&mut owner, &device)?;

        assert_eq!(presenting_record_count.load(Ordering::SeqCst), 2);
        assert_eq!(replacement_record_count.load(Ordering::SeqCst), 2);
        assert_eq!(
            owner.role_status().presenting(),
            RenderPathStrategy::ComputeRay
        );
        assert_eq!(owner.role_status().replacement(), None);
        assert_eq!(owner.role_status().retiring(), None);
        assert_eq!(
            owner.events().last(),
            Some(&RenderPathSwitchEvent::Retired {
                retired: RenderPathStrategy::Raster,
            })
        );
        Ok(())
    }

    #[test]
    fn revision_four_paths_complete_the_raster_compute_raster_compute_round_trip()
    -> RenderPathResult<()> {
        let mut owner =
            RenderPathSwitchOwner::new(proof_path(stamp(RenderPathStrategy::Raster, 4, 4)));
        let device = proof_device();

        for (replacement, retired) in [
            (RenderPathStrategy::ComputeRay, RenderPathStrategy::Raster),
            (RenderPathStrategy::Raster, RenderPathStrategy::ComputeRay),
            (RenderPathStrategy::ComputeRay, RenderPathStrategy::Raster),
        ] {
            owner
                .request_switch(proof_path(stamp(replacement, 4, 4)))
                .expect("a converged idle presenter should admit the next cold replacement");
            advance_owner(&mut owner, &device)?;
            assert_eq!(owner.role_status().presenting(), replacement);
            assert_eq!(owner.role_status().replacement(), None);
            assert_eq!(owner.role_status().retiring(), Some(retired));

            advance_owner(&mut owner, &device)?;
            assert_eq!(owner.role_status().presenting(), replacement);
            assert_eq!(owner.role_status().replacement(), None);
            assert_eq!(owner.role_status().retiring(), None);
            assert_eq!(
                owner.events().last(),
                Some(&RenderPathSwitchEvent::Retired { retired })
            );
        }

        assert_eq!(
            owner.role_status().presenting(),
            RenderPathStrategy::ComputeRay
        );
        Ok(())
    }
}

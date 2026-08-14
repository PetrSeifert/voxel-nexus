use crate::{
    CameraState, PresentationConfigurationId, RenderPath, RenderPathDeviceContext,
    RenderPathFrameContext, RenderPathResult, RenderPathTarget,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
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
    retiring: Option<Box<dyn SwitchableRenderPath>>,
    handoff_control: RenderPathHandoffControl,
    events: Vec<RenderPathSwitchEvent>,
}

impl RenderPathSwitchOwner {
    pub fn new(presenting: Box<dyn SwitchableRenderPath>) -> Self {
        Self {
            presenting,
            replacement: None,
            replacement_needs_configuration: false,
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
        let retiring = std::mem::replace(&mut self.presenting, replacement);
        self.events.push(RenderPathSwitchEvent::HandedOff {
            presenting: self.presenting.stamp().strategy(),
            retiring: retiring.stamp().strategy(),
        });
        self.retiring = Some(retiring);
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
        if let Some(replacement) = self.replacement.as_mut() {
            replacement.publish_camera_state(camera_state, camera_state_revision)?;
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
        self.presenting.release(device)?;
        if let Some(replacement) = self.replacement.as_mut() {
            replacement.release(device)?;
            self.replacement_needs_configuration = true;
        }
        if let Some(retiring) = self.retiring.as_mut() {
            retiring.release(device)?;
        }
        Ok(())
    }

    fn configure(
        &mut self,
        device: RenderPathDeviceContext<'_>,
        target: RenderPathTarget<'_>,
    ) -> RenderPathResult<()> {
        self.presenting.configure(device, target)?;
        if let Some(replacement) = self.replacement.as_mut() {
            replacement.configure(device, target)?;
            self.replacement_needs_configuration = false;
        }
        if let Some(retiring) = self.retiring.as_mut() {
            retiring.configure(device, target)?;
        }
        Ok(())
    }

    fn advance_frame_boundary(
        &mut self,
        device: RenderPathDeviceContext<'_>,
        target: RenderPathTarget<'_>,
    ) -> RenderPathResult<()> {
        self.presenting.advance_frame_boundary(device, target)?;
        if let Some(replacement) = self.replacement.as_mut() {
            if self.replacement_needs_configuration {
                replacement.configure(device, target)?;
                self.replacement_needs_configuration = false;
            }
            replacement.advance_frame_boundary(device, target)?;
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
        self.presenting.shutdown(device)?;
        if let Some(replacement) = self.replacement.as_mut() {
            replacement.shutdown(device)?;
        }
        if let Some(retiring) = self.retiring.as_mut() {
            retiring.shutdown(device)?;
        }
        Ok(())
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

    struct ProofRenderPath {
        stamp: RenderPathStamp,
        pending_camera_state_revision: Option<CameraStateRevision>,
        configuration_tracks_target: bool,
        become_recordable: Option<Arc<AtomicBool>>,
        retirement_fails: bool,
        configure_count: Option<Arc<AtomicUsize>>,
        record_count: Option<Arc<AtomicUsize>>,
    }

    impl RenderPath for ProofRenderPath {
        fn publish_camera_state(
            &mut self,
            _camera_state: CameraState,
            camera_state_revision: CameraStateRevision,
        ) -> RenderPathResult<()> {
            self.pending_camera_state_revision = Some(camera_state_revision);
            Ok(())
        }

        fn release(&mut self, _device: RenderPathDeviceContext<'_>) -> RenderPathResult<()> {
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
            retirement_fails: false,
            configure_count: None,
            record_count: None,
        })
    }

    fn retirement_failing_path(stamp: RenderPathStamp) -> Box<dyn SwitchableRenderPath> {
        Box::new(ProofRenderPath {
            stamp,
            pending_camera_state_revision: None,
            configuration_tracks_target: false,
            become_recordable: None,
            retirement_fails: true,
            configure_count: None,
            record_count: None,
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
                retirement_fails: false,
                configure_count: Some(Arc::clone(&configure_count)),
                record_count: None,
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
                retirement_fails: false,
                configure_count: None,
                record_count: Some(Arc::clone(&record_count)),
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
                retirement_fails: false,
                configure_count: Some(Arc::clone(&configure_count)),
                record_count: Some(Arc::clone(&record_count)),
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
}

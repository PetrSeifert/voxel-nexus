use super::{
    BackendError, BackendFrameSequences, FrameBoundaryOperations, QueueFamilyCapabilities,
    run_frame_boundary_operations, select_graphics_queue_family,
};
use ash::vk;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TargetSnapshot {
    configuration_id: u64,
    format: vk::Format,
    extent: vk::Extent2D,
    attachment_count: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FrameBoundaryEvent {
    Wait,
    Advance(TargetSnapshot),
    Acquire,
}

struct ProofFrameBoundaryOperations {
    current_target: TargetSnapshot,
    events: Vec<FrameBoundaryEvent>,
    advance_error: Option<BackendError>,
}

impl FrameBoundaryOperations for ProofFrameBoundaryOperations {
    type Acquired = u32;

    fn wait_for_preceding_frame(&mut self) -> Result<(), BackendError> {
        self.events.push(FrameBoundaryEvent::Wait);
        Ok(())
    }

    fn advance_render_path(&mut self) -> Result<(), BackendError> {
        self.events
            .push(FrameBoundaryEvent::Advance(self.current_target));
        match self.advance_error.take() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn acquire_image(&mut self) -> Result<Self::Acquired, BackendError> {
        self.events.push(FrameBoundaryEvent::Acquire);
        Ok(2)
    }
}

fn target_snapshot(configuration_id: u64, width: u32, height: u32) -> TargetSnapshot {
    TargetSnapshot {
        configuration_id,
        format: vk::Format::B8G8R8A8_SRGB,
        extent: vk::Extent2D { width, height },
        attachment_count: 3,
    }
}

#[test]
fn frame_boundary_waits_then_advances_the_current_complete_target_before_acquisition()
-> Result<(), BackendError> {
    let mut operations = ProofFrameBoundaryOperations {
        current_target: target_snapshot(1, 800, 600),
        events: Vec::new(),
        advance_error: None,
    };
    let recreated_target = target_snapshot(2, 1200, 700);
    operations.current_target = recreated_target;

    assert_eq!(run_frame_boundary_operations(&mut operations)?, 2);
    assert_eq!(
        operations.events,
        vec![
            FrameBoundaryEvent::Wait,
            FrameBoundaryEvent::Advance(recreated_target),
            FrameBoundaryEvent::Acquire,
        ]
    );
    Ok(())
}

#[test]
fn frame_boundary_failure_prevents_image_acquisition() {
    let mut operations = ProofFrameBoundaryOperations {
        current_target: target_snapshot(2, 1200, 700),
        events: Vec::new(),
        advance_error: Some(BackendError::FrameSequenceIdentityExhausted),
    };

    assert!(run_frame_boundary_operations(&mut operations).is_err());
    assert_eq!(
        operations.events,
        vec![
            FrameBoundaryEvent::Wait,
            FrameBoundaryEvent::Advance(target_snapshot(2, 1200, 700)),
        ]
    );
}

#[test]
fn backend_frame_sequences_remain_monotonic_across_presentation_generations()
-> Result<(), Box<dyn std::error::Error>> {
    let mut sequences = BackendFrameSequences::new();
    assert_eq!(sequences.pending(), 1);
    sequences.record_submission()?;

    assert_eq!(sequences.pending(), 2);
    sequences.record_submission()?;
    assert_eq!(sequences.pending(), 3);
    Ok(())
}

#[test]
fn graphics_queue_selection_prefers_a_compute_capable_graphics_family() {
    let queue_families = [
        QueueFamilyCapabilities {
            supports_graphics: true,
            supports_compute: false,
            supports_presentation: true,
        },
        QueueFamilyCapabilities {
            supports_graphics: true,
            supports_compute: true,
            supports_presentation: false,
        },
    ];

    assert_eq!(select_graphics_queue_family(&queue_families), Some(1));
}

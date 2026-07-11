use render_backend::{FrameObservation, FrameObservationBuffer, FrameObservationError};

#[test]
fn frame_observation_buffer_keeps_only_the_latest_completed_frame()
-> Result<(), Box<dyn std::error::Error>> {
    let mut observations = FrameObservationBuffer::default();
    observations.publish(FrameObservation::from_gpu_timestamps(10, 30, 0.5)?)?;
    observations.publish(FrameObservation::from_gpu_timestamps(40, 100, 0.5)?)?;

    assert_eq!(
        observations.take(),
        Some(FrameObservation {
            gpu_frame_milliseconds: 0.00003,
        })
    );
    assert_eq!(observations.take(), None);
    Ok(())
}

#[test]
fn frame_observation_rejects_invalid_or_reversed_timestamps() {
    assert_eq!(
        FrameObservation::from_gpu_timestamps(30, 10, 1.0),
        Err(FrameObservationError::ReversedTimestamps)
    );
    assert_eq!(
        FrameObservation::from_gpu_timestamps(10, 30, f64::NAN),
        Err(FrameObservationError::InvalidTimestampPeriod)
    );
}

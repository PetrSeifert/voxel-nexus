use render_backend::{FrameObservation, FrameObservationBuffer, FrameObservationError};

#[test]
fn frame_observation_buffer_keeps_only_the_latest_completed_frame()
-> Result<(), Box<dyn std::error::Error>> {
    let mut observations = FrameObservationBuffer::default();
    observations.publish(FrameObservation::from_gpu_timestamps(1, 10, 30, 64, 0.5)?)?;
    observations.publish(FrameObservation::from_gpu_timestamps(2, 40, 100, 64, 0.5)?)?;

    assert_eq!(
        observations.take(),
        Some(FrameObservation {
            sequence: 2,
            gpu_frame_milliseconds: 0.00003,
        })
    );
    assert_eq!(observations.take(), None);
    Ok(())
}

#[test]
fn frame_observation_uses_valid_bits_for_wrapped_timestamps()
-> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        FrameObservation::from_gpu_timestamps(7, 250, 10, 8, 1_000.0)?,
        FrameObservation {
            sequence: 7,
            gpu_frame_milliseconds: 0.016,
        }
    );
    assert_eq!(
        FrameObservation::from_gpu_timestamps(1, 10, 30, 64, f64::NAN),
        Err(FrameObservationError::InvalidTimestampPeriod)
    );
    assert_eq!(
        FrameObservation::from_gpu_timestamps(1, 10, 30, 0, 1.0),
        Err(FrameObservationError::InvalidTimestampValidBits)
    );
    Ok(())
}

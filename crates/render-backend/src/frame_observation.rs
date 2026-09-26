use thiserror::Error;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrameOutcome {
    Presented,
    Recreate,
    RetryLater,
    Suspended,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameObservation {
    pub sequence: u64,
    pub gpu_frame_milliseconds: f64,
}

impl FrameObservation {
    pub fn from_gpu_timestamps(
        sequence: u64,
        start: u64,
        end: u64,
        timestamp_valid_bits: u32,
        timestamp_period_nanoseconds: f64,
    ) -> Result<Self, FrameObservationError> {
        if !timestamp_period_nanoseconds.is_finite() || timestamp_period_nanoseconds <= 0.0 {
            return Err(FrameObservationError::InvalidTimestampPeriod);
        }
        if timestamp_valid_bits == 0 || timestamp_valid_bits > 64 {
            return Err(FrameObservationError::InvalidTimestampValidBits);
        }
        let timestamp_mask = if timestamp_valid_bits == 64 {
            u64::MAX
        } else {
            (1_u64 << timestamp_valid_bits) - 1
        };
        let ticks = (end & timestamp_mask).wrapping_sub(start & timestamp_mask) & timestamp_mask;
        let gpu_frame_milliseconds = ticks as f64 * timestamp_period_nanoseconds / 1_000_000.0;
        if !gpu_frame_milliseconds.is_finite() {
            return Err(FrameObservationError::InvalidDuration);
        }
        Ok(Self {
            sequence,
            gpu_frame_milliseconds,
        })
    }
}

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum FrameObservationError {
    #[error("the graphics queue timestamp valid-bit count must be between 1 and 64")]
    InvalidTimestampValidBits,
    #[error("the Vulkan timestamp period must be finite and positive")]
    InvalidTimestampPeriod,
    #[error("the GPU timestamp duration is not finite")]
    InvalidDuration,
}

#[derive(Default)]
pub struct FrameObservationBuffer {
    latest: Option<FrameObservation>,
}

impl FrameObservationBuffer {
    pub fn publish(&mut self, observation: FrameObservation) -> Result<(), FrameObservationError> {
        if !observation.gpu_frame_milliseconds.is_finite()
            || observation.gpu_frame_milliseconds.is_sign_negative()
        {
            return Err(FrameObservationError::InvalidDuration);
        }
        self.latest = Some(observation);
        Ok(())
    }

    pub fn take(&mut self) -> Option<FrameObservation> {
        self.latest.take()
    }
}

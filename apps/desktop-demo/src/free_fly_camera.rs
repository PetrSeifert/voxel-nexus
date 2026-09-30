use render_backend::{CameraConfigurationError, CameraState};
use std::time::Duration;

/// Keeps `CameraState::new` away from an up-parallel view direction.
pub(super) const MAXIMUM_PITCH_RADIANS: f32 = 89.0 * std::f32::consts::PI / 180.0;
/// Keeps `eye + forward` distinguishable from `eye` in `f32` at every reachable position.
pub(super) const MAXIMUM_EYE_COORDINATE: f32 = 4096.0;
const CAMERA_UP: [f32; 3] = [0.0, 1.0, 0.0];
const MOVEMENT_SPEED: f32 = 4.0;
const FAST_MOVEMENT_MULTIPLIER: f32 = 4.0;
const LOOK_RADIANS_PER_COUNT: f32 = 0.0025;
/// A stalled frame must not teleport the camera.
const MAXIMUM_MOVEMENT_STEP: Duration = Duration::from_millis(100);

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct MovementInput {
    pub(super) forward: bool,
    pub(super) backward: bool,
    pub(super) left: bool,
    pub(super) right: bool,
    pub(super) up: bool,
    pub(super) down: bool,
    pub(super) fast: bool,
}

impl MovementInput {
    pub(super) fn is_moving(self) -> bool {
        axis(self.forward, self.backward) != 0.0
            || axis(self.right, self.left) != 0.0
            || axis(self.up, self.down) != 0.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct FreeFlyCamera {
    eye: [f32; 3],
    yaw: f32,
    pitch: f32,
    field_of_view_degrees: f32,
    near_plane: f32,
    far_plane: f32,
    radial_far_clip: bool,
}

impl FreeFlyCamera {
    pub(super) fn from_camera_state(camera_state: CameraState) -> Self {
        let [eye_x, eye_y, eye_z] = camera_state.eye();
        let [target_x, target_y, target_z] = camera_state.target();
        let direction = [target_x - eye_x, target_y - eye_y, target_z - eye_z];
        let [direction_x, direction_y, direction_z] = direction;
        let horizontal_length = direction_x.hypot(direction_z);
        let mut camera = Self {
            eye: camera_state.eye(),
            yaw: direction_z.atan2(direction_x),
            pitch: direction_y.atan2(horizontal_length),
            field_of_view_degrees: camera_state.field_of_view_degrees(),
            near_plane: camera_state.near_plane(),
            far_plane: camera_state.far_plane(),
            radial_far_clip: camera_state.radial_far_clip(),
        };
        camera.normalize_orientation();
        camera.clamp_eye();
        camera
    }

    pub(super) fn eye(&self) -> [f32; 3] {
        self.eye
    }

    #[cfg(test)]
    pub(super) fn pitch(&self) -> f32 {
        self.pitch
    }

    pub(super) fn far_plane(&self) -> f32 {
        self.far_plane
    }

    pub(super) fn forward(&self) -> [f32; 3] {
        [
            self.pitch.cos() * self.yaw.cos(),
            self.pitch.sin(),
            self.pitch.cos() * self.yaw.sin(),
        ]
    }

    pub(super) fn look(&mut self, delta_x: f64, delta_y: f64) {
        if !delta_x.is_finite() || !delta_y.is_finite() {
            return;
        }
        self.yaw += delta_x as f32 * LOOK_RADIANS_PER_COUNT;
        self.pitch -= delta_y as f32 * LOOK_RADIANS_PER_COUNT;
        self.normalize_orientation();
    }

    pub(super) fn advance(&mut self, input: MovementInput, elapsed: Duration) {
        let forward_amount = axis(input.forward, input.backward);
        let right_amount = axis(input.right, input.left);
        let up_amount = axis(input.up, input.down);
        let (yaw_sine, yaw_cosine) = self.yaw.sin_cos();
        // Horizontal forward is (cos, 0, sin) and right is forward x world up = (-sin, 0, cos).
        let direction = [
            yaw_cosine * forward_amount - yaw_sine * right_amount,
            up_amount,
            yaw_sine * forward_amount + yaw_cosine * right_amount,
        ];
        let length = direction
            .iter()
            .map(|component| component * component)
            .sum::<f32>()
            .sqrt();
        if length == 0.0 {
            return;
        }
        let speed = if input.fast {
            MOVEMENT_SPEED * FAST_MOVEMENT_MULTIPLIER
        } else {
            MOVEMENT_SPEED
        };
        let distance = speed * elapsed.min(MAXIMUM_MOVEMENT_STEP).as_secs_f32();
        for (eye, component) in self.eye.iter_mut().zip(direction) {
            *eye += component / length * distance;
        }
        self.clamp_eye();
    }

    pub(super) fn camera_state(&self) -> Result<CameraState, CameraConfigurationError> {
        let [eye_x, eye_y, eye_z] = self.eye;
        let [forward_x, forward_y, forward_z] = self.forward();
        let camera = CameraState::new(
            self.eye,
            [eye_x + forward_x, eye_y + forward_y, eye_z + forward_z],
            CAMERA_UP,
            self.field_of_view_degrees,
            self.near_plane,
            self.far_plane,
        )?;
        Ok(if self.radial_far_clip {
            camera.with_radial_far_clip()
        } else {
            camera
        })
    }

    fn normalize_orientation(&mut self) {
        self.yaw = if self.yaw.is_finite() {
            self.yaw.rem_euclid(std::f32::consts::TAU)
        } else {
            0.0
        };
        self.pitch = if self.pitch.is_finite() {
            self.pitch
                .clamp(-MAXIMUM_PITCH_RADIANS, MAXIMUM_PITCH_RADIANS)
        } else {
            0.0
        };
    }

    fn clamp_eye(&mut self) {
        for component in &mut self.eye {
            *component = component.clamp(-MAXIMUM_EYE_COORDINATE, MAXIMUM_EYE_COORDINATE);
        }
    }
}

fn axis(positive: bool, negative: bool) -> f32 {
    f32::from(u8::from(positive)) - f32::from(u8::from(negative))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn overview() -> Result<FreeFlyCamera, CameraConfigurationError> {
        Ok(FreeFlyCamera::from_camera_state(CameraState::new(
            [20.0, 14.0, 22.0],
            [0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            50.0,
            0.1,
            100.0,
        )?))
    }

    fn distance(left: [f32; 3], right: [f32; 3]) -> f32 {
        left.iter()
            .zip(right)
            .map(|(left, right)| (left - right) * (left - right))
            .sum::<f32>()
            .sqrt()
    }

    #[test]
    fn starting_pose_keeps_its_eye_and_view_direction() -> Result<(), CameraConfigurationError> {
        let camera = overview()?;
        let camera_state = camera.camera_state()?;
        assert_eq!(camera_state.eye(), [20.0, 14.0, 22.0]);
        let [forward_x, forward_y, forward_z] = camera.forward();
        let expected_length = (20.0_f32 * 20.0 + 14.0 * 14.0 + 22.0 * 22.0).sqrt();
        for (actual, expected) in [forward_x, forward_y, forward_z]
            .into_iter()
            .zip([-20.0, -14.0, -22.0])
        {
            assert!((actual - expected / expected_length).abs() < 1.0e-5);
        }
        Ok(())
    }

    #[test]
    fn pitch_is_clamped_short_of_vertical_and_stays_valid() -> Result<(), CameraConfigurationError>
    {
        let mut camera = overview()?;
        camera.look(0.0, -1.0e6);
        assert_eq!(camera.pitch(), MAXIMUM_PITCH_RADIANS);
        camera.camera_state()?;
        camera.look(0.0, 2.0e6);
        assert_eq!(camera.pitch(), -MAXIMUM_PITCH_RADIANS);
        camera.camera_state()?;
        camera.look(f64::NAN, f64::INFINITY);
        assert_eq!(camera.pitch(), -MAXIMUM_PITCH_RADIANS);
        Ok(())
    }

    #[test]
    fn straight_up_starting_pose_is_clamped() -> Result<(), CameraConfigurationError> {
        let camera = FreeFlyCamera::from_camera_state(CameraState::new(
            [0.0, 0.0, 0.0],
            [0.0, 5.0, 0.001],
            [0.0, 1.0, 0.0],
            50.0,
            0.1,
            100.0,
        )?);
        assert!(camera.pitch() <= MAXIMUM_PITCH_RADIANS);
        camera.camera_state()?;
        Ok(())
    }

    #[test]
    fn movement_scales_with_frame_time_not_with_event_count() -> Result<(), CameraConfigurationError>
    {
        let input = MovementInput {
            forward: true,
            ..MovementInput::default()
        };
        let mut one_frame = overview()?;
        one_frame.advance(input, Duration::from_millis(50));
        let mut two_frames = overview()?;
        two_frames.advance(input, Duration::from_millis(25));
        two_frames.advance(input, Duration::from_millis(25));
        let start = overview()?.eye();
        assert!((distance(start, one_frame.eye()) - MOVEMENT_SPEED * 0.05).abs() < 1.0e-4);
        assert!(distance(one_frame.eye(), two_frames.eye()) < 1.0e-4);

        let mut fast = overview()?;
        fast.advance(
            MovementInput {
                fast: true,
                ..input
            },
            Duration::from_millis(50),
        );
        assert!(
            (distance(start, fast.eye()) - MOVEMENT_SPEED * FAST_MOVEMENT_MULTIPLIER * 0.05).abs()
                < 1.0e-4
        );
        Ok(())
    }

    #[test]
    fn horizontal_movement_ignores_pitch_and_vertical_movement_is_world_up()
    -> Result<(), CameraConfigurationError> {
        let mut camera = overview()?;
        let start = camera.eye();
        camera.advance(
            MovementInput {
                forward: true,
                right: true,
                ..MovementInput::default()
            },
            Duration::from_millis(100),
        );
        let [_, start_y, _] = start;
        let [_, moved_y, _] = camera.eye();
        assert_eq!(start_y, moved_y);
        assert!((distance(start, camera.eye()) - MOVEMENT_SPEED * 0.1).abs() < 1.0e-4);

        let before_rising = camera.eye();
        camera.advance(
            MovementInput {
                up: true,
                ..MovementInput::default()
            },
            Duration::from_millis(100),
        );
        let [before_x, before_y, before_z] = before_rising;
        let [after_x, after_y, after_z] = camera.eye();
        assert_eq!((before_x, before_z), (after_x, after_z));
        assert!((after_y - before_y - MOVEMENT_SPEED * 0.1).abs() < 1.0e-5);
        Ok(())
    }

    #[test]
    fn opposing_keys_and_stalled_frames_are_bounded() -> Result<(), CameraConfigurationError> {
        let mut camera = overview()?;
        let start = camera.eye();
        let opposing = MovementInput {
            forward: true,
            backward: true,
            ..MovementInput::default()
        };
        assert!(!opposing.is_moving());
        camera.advance(opposing, Duration::from_secs(1));
        assert_eq!(camera.eye(), start);

        camera.advance(
            MovementInput {
                forward: true,
                ..MovementInput::default()
            },
            Duration::from_secs(10),
        );
        assert!(
            distance(start, camera.eye())
                <= MOVEMENT_SPEED * MAXIMUM_MOVEMENT_STEP.as_secs_f32() + 1.0e-4
        );
        Ok(())
    }

    #[test]
    fn every_reachable_pose_is_a_valid_camera_state() -> Result<(), CameraConfigurationError> {
        let mut camera = overview()?;
        let fast_climb = MovementInput {
            forward: true,
            up: true,
            fast: true,
            ..MovementInput::default()
        };
        for step in 0..20_000 {
            camera.advance(fast_climb, MAXIMUM_MOVEMENT_STEP);
            camera.look(
                f64::from(step % 97) * 13.0,
                f64::from(step % 53) * -29.0 + 700.0,
            );
            camera.camera_state()?;
        }
        assert!(
            camera
                .eye()
                .iter()
                .all(|component| component.abs() <= MAXIMUM_EYE_COORDINATE)
        );
        for pitch_delta in [-1.0e9, 1.0e9] {
            camera.look(0.0, pitch_delta);
            camera.camera_state()?;
        }
        Ok(())
    }
}

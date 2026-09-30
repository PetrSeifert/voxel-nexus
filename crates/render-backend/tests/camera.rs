use render_backend::{CameraConfigurationError, CameraState};

#[test]
fn coincident_eye_and_target_are_rejected() {
    assert_eq!(
        CameraState::new([0.0; 3], [0.0; 3], [0.0, 1.0, 0.0], 60.0, 0.1, 100.0),
        Err(CameraConfigurationError::CoincidentEyeAndTarget)
    );
}

#[test]
fn every_nonfinite_camera_parameter_is_rejected() {
    for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        for component in 0..12 {
            let mut parameters = [
                0.0, 0.0, 5.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 60.0, 0.1, 100.0,
            ];
            if let Some(parameter) = parameters.get_mut(component) {
                *parameter = value;
            }
            let [
                eye_x,
                eye_y,
                eye_z,
                target_x,
                target_y,
                target_z,
                up_x,
                up_y,
                up_z,
                field_of_view,
                near,
                far,
            ] = parameters;
            let parameter = match component {
                0..=2 => "eye",
                3..=5 => "target",
                6..=8 => "up",
                9 => "field of view",
                10 => "near plane",
                _ => "far plane",
            };
            assert_eq!(
                CameraState::new(
                    [eye_x, eye_y, eye_z],
                    [target_x, target_y, target_z],
                    [up_x, up_y, up_z],
                    field_of_view,
                    near,
                    far,
                ),
                Err(CameraConfigurationError::NonfiniteParameter { parameter })
            );
        }
    }
}

#[test]
fn zero_and_parallel_up_directions_are_rejected() {
    for up in [
        [0.0; 3],
        [1.0, 2.0, 3.0],
        [-2.0, -4.0, -6.0],
        [1073741824.0, 2147483648.0, 3221225472.0],
    ] {
        assert_eq!(
            CameraState::new([0.0; 3], [1.0, 2.0, 3.0], up, 60.0, 0.1, 100.0),
            Err(CameraConfigurationError::DegenerateUpDirection)
        );
    }
}

#[test]
fn projection_parameters_must_be_in_the_supported_range() {
    for field_of_view in [-1.0, 0.0, 180.0, 200.0] {
        assert_eq!(
            CameraState::new(
                [0.0, 0.0, 5.0],
                [0.0; 3],
                [0.0, 1.0, 0.0],
                field_of_view,
                0.1,
                100.0
            ),
            Err(CameraConfigurationError::InvalidFieldOfView)
        );
    }
    for [near, far] in [
        [0.0, 100.0],
        [-1.0, 100.0],
        [1.0, 1.0],
        [100.0, 1.0],
        [1.0, 0.0],
        [1.0, -1.0],
    ] {
        assert_eq!(
            CameraState::new([0.0, 0.0, 5.0], [0.0; 3], [0.0, 1.0, 0.0], 60.0, near, far),
            Err(CameraConfigurationError::InvalidClipPlanes)
        );
    }
}

#[test]
fn finite_inputs_that_exceed_transform_precision_or_range_are_rejected() {
    let cases = [
        (
            [f32::MAX, 0.0, 0.0],
            [-f32::MAX, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            60.0,
            0.1,
            100.0,
        ),
        (
            [0.0; 3],
            [f32::MIN_POSITIVE, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            60.0,
            0.1,
            100.0,
        ),
        (
            [0.0, 0.0, 5.0],
            [0.0; 3],
            [0.0, f32::MAX, 0.0],
            60.0,
            0.1,
            100.0,
        ),
        (
            [0.0, 0.0, 5.0],
            [0.0; 3],
            [0.0, 1.0, 0.0],
            f32::MIN_POSITIVE,
            0.1,
            100.0,
        ),
        (
            [0.0, 0.0, 5.0],
            [0.0; 3],
            [0.0, 1.0, 0.0],
            60.0,
            f32::MAX / 2.0,
            f32::MAX,
        ),
        (
            [1e30, 0.0, 5.0],
            [1e30, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            60.0,
            0.1,
            100.0,
        ),
    ];
    for (eye, target, up, field_of_view, near, far) in cases {
        assert_eq!(
            CameraState::new(eye, target, up, field_of_view, near, far),
            Err(CameraConfigurationError::UnrepresentableTransform)
        );
    }
}

#[test]
fn accepted_cameras_produce_finite_matrices_at_extreme_extents()
-> Result<(), CameraConfigurationError> {
    for field_of_view in [0.001, 45.0, 60.0, 179.99998] {
        for up in [[0.0, 1.0, 0.0], [0.0, 2.0, 1.0], [0.0, 1e-10, 0.0]] {
            let camera =
                CameraState::new([0.0, 0.0, 5.0], [0.0; 3], up, field_of_view, 0.1, 100.0)?;
            for extent in [
                [1, 1],
                [800, 600],
                [1, u32::MAX],
                [u32::MAX, 1],
                [u32::MAX, u32::MAX],
            ] {
                assert!(
                    camera
                        .view_projection(extent)?
                        .iter()
                        .all(|value| value.is_finite())
                );
            }
        }
    }
    Ok(())
}

#[test]
fn default_camera_remains_valid_and_zero_extents_are_rejected()
-> Result<(), CameraConfigurationError> {
    let camera = CameraState::default();
    assert_eq!(
        camera,
        CameraState::new(
            camera.eye(),
            camera.target(),
            camera.up(),
            camera.field_of_view_degrees(),
            camera.near_plane(),
            camera.far_plane()
        )?
    );
    for extent in [[0, 0], [0, 600], [800, 0]] {
        assert_eq!(
            camera.view_projection(extent),
            Err(CameraConfigurationError::ZeroDrawableExtent)
        );
    }
    Ok(())
}

#[test]
fn camera_move_rejects_a_degenerate_intermediate_pose() -> Result<(), Box<dyn std::error::Error>> {
    use render_backend::DeterministicCameraMove;

    let start = CameraState::new([0.0, 0.0, 5.0], [0.0; 3], [0.0, 1.0, 0.0], 60.0, 0.1, 100.0)?;
    let end = CameraState::new(
        [0.0, 0.0, -5.0],
        [0.0; 3],
        [0.0, 1.0, 0.0],
        60.0,
        0.1,
        100.0,
    )?;
    let movement = DeterministicCameraMove::new(start, end, 2)?;
    assert_eq!(movement.pose_at_step(0)?, start);
    assert_eq!(
        movement.pose_at_step(1),
        Err(CameraConfigurationError::CoincidentEyeAndTarget)
    );
    assert_eq!(movement.pose_at_step(2)?, end);
    Ok(())
}

#[test]
fn camera_moves_preserve_radial_clipping_and_reject_mixed_modes()
-> Result<(), Box<dyn std::error::Error>> {
    use render_backend::DeterministicCameraMove;
    let planar = CameraState::default();
    let radial = planar.with_radial_far_clip();
    let movement = DeterministicCameraMove::new(radial, radial, 2)?;
    for step in 0..=2 {
        assert!(movement.pose_at_step(step)?.radial_far_clip());
    }
    for (start, end) in [(planar, radial), (radial, planar)] {
        assert_eq!(
            DeterministicCameraMove::new(start, end, 2),
            Err(CameraConfigurationError::DifferentFarClipModes)
        );
    }
    Ok(())
}

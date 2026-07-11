use canonical_scene::{CanonicalCameraPose, overview_to_cavity_camera_move};
use raster_render_path::RasterRenderPath;

#[test]
fn canonical_camera_poses_fix_every_scene_coordinate_parameter()
-> Result<(), Box<dyn std::error::Error>> {
    let cases = [
        (
            CanonicalCameraPose::Overview,
            [20.0, 14.0, 22.0],
            [0.0, 0.0, 0.0],
            50.0,
        ),
        (
            CanonicalCameraPose::CavityMaterialCloseUp,
            [0.0, 1.0, 17.0],
            [-0.75, -0.5, 0.0],
            45.0,
        ),
        (
            CanonicalCameraPose::BoundaryCutaway,
            [-14.0, 2.0, 8.0],
            [-7.5, -1.0, 0.0],
            45.0,
        ),
    ];

    for (identity, eye, target, field_of_view_degrees) in cases {
        let pose = identity.pose();
        assert_eq!(pose.eye(), eye);
        assert_eq!(pose.target(), target);
        assert_eq!(pose.up(), [0.0, 1.0, 0.0]);
        assert_eq!(pose.field_of_view_degrees(), field_of_view_degrees);
        assert_eq!(pose.near_plane(), 0.1);
        assert_eq!(pose.far_plane(), 100.0);
        let first = pose.view_projection([1600, 900])?;
        let second = pose.view_projection([1600, 900])?;
        assert_eq!(first, second);
        assert!(first.iter().all(|component| component.is_finite()));
    }
    Ok(())
}

#[test]
fn overview_to_cavity_move_has_fixed_step_outputs() -> Result<(), Box<dyn std::error::Error>> {
    let movement = overview_to_cavity_camera_move()?;
    assert_eq!(movement.total_steps(), 120);
    assert_eq!(
        movement.pose_at_step(0)?,
        CanonicalCameraPose::Overview.pose()
    );
    let midpoint = movement.pose_at_step(60)?;
    assert_eq!(midpoint.eye(), [10.0, 7.5, 19.5]);
    assert_eq!(midpoint.target(), [-0.375, -0.25, 0.0]);
    assert_eq!(midpoint.up(), [0.0, 1.0, 0.0]);
    assert_eq!(midpoint.field_of_view_degrees(), 47.5);
    assert_eq!(midpoint.near_plane(), 0.1);
    assert_eq!(midpoint.far_plane(), 100.0);
    assert_eq!(
        movement.pose_at_step(120)?,
        CanonicalCameraPose::CavityMaterialCloseUp.pose()
    );
    assert!(movement.pose_at_step(121).is_err());
    Ok(())
}

#[test]
fn raster_render_path_retains_the_selected_logical_camera_pose() {
    let pose = CanonicalCameraPose::BoundaryCutaway.pose();
    let render_path = RasterRenderPath::with_camera_pose(pose);

    assert_eq!(render_path.camera_pose(), pose);
}

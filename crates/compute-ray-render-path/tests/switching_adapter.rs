use compute_ray_render_path::ComputeRayRenderPathAdapter;
use render_backend::{
    CameraState, CameraStateRevision, RenderPathReadiness, RenderPathStrategy, SwitchableRenderPath,
};
use voxel_frontend::{DenseVoxelScene, VoxelFrontend, VoxelSceneId, VoxelSceneRevision};

#[test]
fn an_unconfigured_compute_adapter_reports_only_path_neutral_preparation_state()
-> Result<(), Box<dyn std::error::Error>> {
    let view = VoxelFrontend::new().publish(DenseVoxelScene::new(
        VoxelSceneId::new("compute-proof"),
        VoxelSceneRevision::new(7),
        Vec::new(),
        Vec::new(),
    ))?;
    let camera = CameraState::new(
        [2.0, 2.0, 2.0],
        [0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        50.0,
        0.1,
        100.0,
    );
    let adapter = ComputeRayRenderPathAdapter::new(view, camera, CameraStateRevision::new(3))?;

    let stamp = adapter.stamp();

    assert_eq!(stamp.strategy(), RenderPathStrategy::ComputeRay);
    assert_eq!(stamp.scene_identity(), &VoxelSceneId::new("compute-proof"));
    assert_eq!(stamp.required_revision(), VoxelSceneRevision::new(7));
    assert_eq!(stamp.visible_revision(), VoxelSceneRevision::new(7));
    assert_eq!(stamp.camera_state_revision(), CameraStateRevision::new(3));
    assert_eq!(stamp.presentation_configuration(), None);
    assert_eq!(stamp.readiness(), RenderPathReadiness::Preparing);
    assert_eq!(adapter.capability_assessment(), None);
    assert_eq!(adapter.camera_state(), camera);
    Ok(())
}

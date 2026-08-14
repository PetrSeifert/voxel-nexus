use compute_ray_render_path::ComputeRayRenderPathAdapter;
use render_backend::{
    CameraStateRevision, RenderPathReadiness, RenderPathStrategy, SwitchableRenderPath,
};
use voxel_frontend::{VoxelSceneId, VoxelSceneRevision};

#[test]
fn an_unconfigured_compute_adapter_reports_only_path_neutral_preparation_state() {
    let adapter = ComputeRayRenderPathAdapter::new(
        VoxelSceneId::new("compute-proof"),
        VoxelSceneRevision::new(7),
        CameraStateRevision::new(3),
    );

    let stamp = adapter.stamp();

    assert_eq!(stamp.strategy(), RenderPathStrategy::ComputeRay);
    assert_eq!(stamp.scene_identity(), &VoxelSceneId::new("compute-proof"));
    assert_eq!(stamp.required_revision(), VoxelSceneRevision::new(7));
    assert_eq!(stamp.visible_revision(), VoxelSceneRevision::new(7));
    assert_eq!(stamp.camera_state_revision(), CameraStateRevision::new(3));
    assert_eq!(stamp.presentation_configuration(), None);
    assert_eq!(stamp.readiness(), RenderPathReadiness::Preparing);
    assert_eq!(adapter.capability_assessment(), None);
}

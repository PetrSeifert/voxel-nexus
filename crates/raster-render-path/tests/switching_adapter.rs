use canonical_scene::{CanonicalSceneScale, generate_canonical_scene};
use raster_render_path::{
    CameraPose, RasterArtifactInstallerError, RasterRenderPathAdapter, derive_raster_artifact,
};
use render_backend::{
    CameraStateRevision, RenderPathReadiness, RenderPathStrategy, RenderPathSwitchOwner,
    SwitchableRenderPath,
};
use voxel_frontend::{VoxelFrontend, VoxelSceneId, VoxelSceneRevision};

fn camera_pose() -> CameraPose {
    CameraPose::default()
}

#[test]
fn awaiting_raster_adapter_is_the_initial_presenting_path_with_path_neutral_stamps() {
    let scene_identity = VoxelSceneId::new("canonical");
    let revision = VoxelSceneRevision::new(1);
    let camera_state_revision = CameraStateRevision::new(4);
    let (adapter, _, _) = RasterRenderPathAdapter::awaiting_artifact_with_camera_control(
        camera_pose(),
        camera_state_revision,
        scene_identity.clone(),
        revision,
    );

    let stamp = adapter.stamp();
    assert_eq!(stamp.strategy(), RenderPathStrategy::Raster);
    assert_eq!(stamp.scene_identity(), &scene_identity);
    assert_eq!(stamp.required_revision(), revision);
    assert_eq!(stamp.visible_revision(), revision);
    assert_eq!(stamp.camera_state_revision(), camera_state_revision);
    assert_eq!(stamp.presentation_configuration(), None);
    assert_eq!(stamp.readiness(), RenderPathReadiness::Preparing);

    let owner = RenderPathSwitchOwner::new(Box::new(adapter));
    assert_eq!(owner.role_status().presenting(), RenderPathStrategy::Raster);
    let diagnostics = owner.diagnostics();
    assert_eq!(diagnostics.roles(), owner.role_status());
    assert_eq!(diagnostics.presenting(), &stamp);
    assert_eq!(diagnostics.replacement(), None);
    assert_eq!(diagnostics.retiring(), None);
    assert!(diagnostics.events().is_empty());
}

#[test]
fn publishing_camera_state_does_not_claim_render_path_acknowledgement() -> Result<(), String> {
    let initial_camera_revision = CameraStateRevision::new(4);
    let (adapter, _, camera_controller) =
        RasterRenderPathAdapter::awaiting_artifact_with_camera_control(
            camera_pose(),
            initial_camera_revision,
            VoxelSceneId::new("canonical"),
            VoxelSceneRevision::new(1),
        );

    camera_controller
        .set_state(
            CameraPose::new(
                [7.0, 6.0, 5.0],
                [0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                50.0,
                0.1,
                100.0,
            )
            .map_err(|error| error.to_string())?,
            CameraStateRevision::new(5),
        )
        .map_err(|error| error.to_string())?;

    assert_eq!(
        adapter.stamp().camera_state_revision(),
        initial_camera_revision
    );
    Ok(())
}

#[test]
fn raster_adapter_rejects_an_artifact_from_another_voxel_scene() -> Result<(), String> {
    let canonical =
        generate_canonical_scene(CanonicalSceneScale::Small).map_err(|error| error.to_string())?;
    let volume_identity = canonical.metadata().volume_identity().clone();
    let view = VoxelFrontend::new()
        .publish(canonical.into_scene())
        .map_err(|error| error.to_string())?;
    let artifact =
        derive_raster_artifact(&view, &volume_identity).map_err(|error| error.to_string())?;
    let expected_scene_identity = VoxelSceneId::new("another-scene");
    let (_, artifact_installer, _) = RasterRenderPathAdapter::awaiting_artifact_with_camera_control(
        camera_pose(),
        CameraStateRevision::new(1),
        expected_scene_identity.clone(),
        view.revision(),
    );

    assert_eq!(
        artifact_installer.publish_complete(artifact),
        Err(RasterArtifactInstallerError::SceneIdentityMismatch {
            expected: expected_scene_identity,
            actual: view.scene_id().clone(),
        })
    );
    Ok(())
}

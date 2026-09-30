use canonical_scene::{CanonicalSceneScale, generate_canonical_scene};
use raster_render_path::RASTER_STRATEGY;
use raster_render_path::{
    CameraPose, RasterArtifactInstallerError, RasterRenderPathAdapter, derive_raster_artifact,
};
use render_backend::{
    CameraStateRevision, RenderPathReadiness, RenderPathSwitchOwner, SwitchableRenderPath,
};
use voxel_frontend::{VoxelFrontend, VoxelSceneId, VoxelSceneRevision};

#[test]
#[cfg(feature = "qualification")]
fn streamed_adapter_stamps_install_revision_and_selection_together()
-> Result<(), Box<dyn std::error::Error>> {
    use render_backend::RenderPath;
    use std::sync::Arc;
    use voxel_frontend::*;
    let frontend = Arc::new(VoxelFrontend::new());
    let view = frontend.publish(DenseVoxelScene::new(
        VoxelSceneId::new("streamed-stamp"),
        VoxelSceneRevision::new(1),
        vec![VoxelMaterial::new(VoxelMaterialId::new("stone"), [1.0; 4])],
        (0..2)
            .map(|index| {
                DenseVoxelVolume::new(
                    VoxelVolumeMetadata::new(
                        VoxelVolumeId::new(index.to_string()),
                        VoxelExtent::new(1, 1, 1),
                        [index as f32, 0.0, 0.0],
                        1.0,
                    ),
                    vec![DenseVoxelBatch::new(
                        VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), VoxelExtent::new(1, 1, 1)),
                        vec![VoxelValue::Occupied(VoxelMaterialId::new("stone"))],
                    )],
                )
            })
            .collect(),
    ))?;
    let old =
        view.residency_selection(VoxelResidencySelectionId::new(1), [VoxelVolumeId::new("0")])?;
    let newest =
        view.residency_selection(VoxelResidencySelectionId::new(2), [VoxelVolumeId::new("1")])?;
    let artifact = raster_render_path::derive_raster_residency(
        frontend.clone(),
        &view,
        old.clone(),
        VoxelExtent::new(1, 1, 1),
    )?;
    let mut adapter = RasterRenderPathAdapter::from_residency_artifact(
        artifact,
        camera_pose(),
        CameraStateRevision::new(4),
    )?;
    assert_eq!(adapter.stamp().installed_selection(), Some(old.identity()));
    adapter
        .submit_residency_selection(newest.clone())
        .map_err(|error| error.to_string())?;
    adapter
        .submit_edit_outcome(frontend.edit(VoxelEditCommand::new(
            VoxelVolumeId::new("1"),
            VoxelCoordinate::new(0, 0, 0),
            VoxelValue::Empty,
        ))?)
        .map_err(|error| error.to_string())?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if std::time::Instant::now() >= deadline {
            return Err("combined adapter stamp did not converge".into());
        }
        adapter.qualification_advance_frame_boundary()?;
        let stamp = adapter.stamp();
        assert_eq!(stamp.required_selection(), Some(newest.identity()));
        assert_eq!(stamp.required_revision(), VoxelSceneRevision::new(2));
        if stamp.installed_selection() == Some(newest.identity()) {
            assert_eq!(stamp.visible_revision(), VoxelSceneRevision::new(2));
            assert_eq!(
                adapter
                    .installed_residency_coverage()
                    .ok_or("missing coverage")?
                    .installed_selection(),
                &newest
            );
            break;
        }
        assert_eq!(stamp.installed_selection(), Some(old.identity()));
        assert_eq!(stamp.visible_revision(), VoxelSceneRevision::new(1));
        std::thread::yield_now();
    }
    Ok(())
}

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
    assert_eq!(stamp.strategy(), RASTER_STRATEGY);
    assert_eq!(stamp.scene_identity(), &scene_identity);
    assert_eq!(stamp.required_revision(), revision);
    assert_eq!(stamp.visible_revision(), revision);
    assert_eq!(stamp.camera_state_revision(), camera_state_revision);
    assert_eq!(stamp.presentation_configuration(), None);
    assert_eq!(stamp.readiness(), RenderPathReadiness::Preparing);

    let owner = RenderPathSwitchOwner::new(Box::new(adapter));
    assert_eq!(owner.role_status().presenting(), RASTER_STRATEGY);
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
        .publish_sparse(canonical.into_scene())
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

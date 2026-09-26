use compute_ray_render_path::{
    ComputeConvergenceAcceptance, ComputeRayRenderPathAdapter, ComputeTimingPhase,
};
use render_backend::{
    CameraState, CameraStateRevision, RenderPath, RenderPathReadiness, RenderPathStrategy,
    SwitchableRenderPath,
};
use voxel_frontend::{
    DenseVoxelBatch, DenseVoxelScene, DenseVoxelVolume, VoxelCoordinate, VoxelEditCommand,
    VoxelExtent, VoxelFrontend, VoxelMaterial, VoxelMaterialId, VoxelRegion, VoxelSceneId,
    VoxelSceneRevision, VoxelValue, VoxelVolumeId, VoxelVolumeMetadata,
};

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
    )?;
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

#[test]
fn cold_compute_construction_records_revision_attributed_preparation_time()
-> Result<(), Box<dyn std::error::Error>> {
    let view = VoxelFrontend::new().publish(DenseVoxelScene::new(
        VoxelSceneId::new("compute-measurement"),
        VoxelSceneRevision::new(9),
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
    )?;

    let (_adapter, measurement) = ComputeRayRenderPathAdapter::new_with_measurement(
        view,
        camera,
        CameraStateRevision::new(3),
    )?;
    let events = measurement.drain()?;
    let event = events
        .first()
        .ok_or_else(|| std::io::Error::other("missing preparation timing event"))?;

    assert_eq!(events.len(), 1);
    assert_eq!(event.phase(), ComputeTimingPhase::Preparation);
    assert_eq!(
        event.scene_identity(),
        &VoxelSceneId::new("compute-measurement")
    );
    assert_eq!(event.revision(), VoxelSceneRevision::new(9));
    assert!(event.elapsed_milliseconds().is_finite());
    assert!(event.elapsed_milliseconds() >= 0.0);
    Ok(())
}

#[test]
fn published_camera_state_waits_for_frame_boundary_acknowledgement()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let view = VoxelFrontend::new().publish(DenseVoxelScene::new(
        VoxelSceneId::new("compute-camera"),
        VoxelSceneRevision::new(1),
        Vec::new(),
        Vec::new(),
    ))?;
    let initial_camera = CameraState::new(
        [2.0, 2.0, 2.0],
        [0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        50.0,
        0.1,
        100.0,
    )?;
    let changed_camera = CameraState::new(
        [7.0, 6.0, 5.0],
        [0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        55.0,
        0.1,
        100.0,
    )?;
    let mut adapter =
        ComputeRayRenderPathAdapter::new(view, initial_camera, CameraStateRevision::new(3))?;

    adapter.publish_camera_state(changed_camera, CameraStateRevision::new(4))?;

    assert_eq!(adapter.camera_state(), changed_camera);
    assert_eq!(
        adapter.stamp().camera_state_revision(),
        CameraStateRevision::new(3)
    );
    Ok(())
}

#[test]
fn accepted_edits_advance_required_without_changing_the_visible_installation()
-> Result<(), Box<dyn std::error::Error>> {
    let frontend = VoxelFrontend::new();
    let extent = VoxelExtent::new(1, 1, 1);
    let view = frontend.publish(DenseVoxelScene::new(
        VoxelSceneId::new("compute-convergence"),
        VoxelSceneRevision::new(7),
        vec![VoxelMaterial::new(
            VoxelMaterialId::new("stone"),
            [0.2, 0.3, 0.4, 1.0],
        )],
        vec![DenseVoxelVolume::new(
            VoxelVolumeMetadata::new(VoxelVolumeId::new("terrain"), extent, [0.0; 3], 1.0),
            vec![DenseVoxelBatch::new(
                VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), extent),
                vec![VoxelValue::Empty],
            )],
        )],
    ))?;
    let camera = CameraState::new(
        [2.0, 2.0, 2.0],
        [0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        50.0,
        0.1,
        100.0,
    )?;
    let mut adapter = ComputeRayRenderPathAdapter::new(view, camera, CameraStateRevision::new(3))?;
    let outcome = frontend.edit(VoxelEditCommand::new(
        VoxelVolumeId::new("terrain"),
        VoxelCoordinate::new(0, 0, 0),
        VoxelValue::Occupied(VoxelMaterialId::new("stone")),
    ))?;

    let ComputeConvergenceAcceptance::Accepted { stamp } = adapter.accept_edit_outcome(outcome)?
    else {
        return Err("changed outcome was not accepted".into());
    };
    let path_stamp = adapter.stamp();

    assert_eq!(stamp.revision(), VoxelSceneRevision::new(8));
    assert_eq!(path_stamp.required_revision(), VoxelSceneRevision::new(8));
    assert_eq!(path_stamp.visible_revision(), VoxelSceneRevision::new(7));
    assert_eq!(
        adapter.scene_bundle().revision(),
        VoxelSceneRevision::new(7)
    );
    assert_eq!(adapter.convergence_status().worker_count(), 1);
    Ok(())
}

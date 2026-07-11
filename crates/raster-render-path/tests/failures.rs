use raster_render_path::{
    RasterArtifactBuildError, RasterArtifactBuildPhase, derive_raster_artifact,
};
use voxel_frontend::{
    DenseVoxelBatch, DenseVoxelScene, DenseVoxelVolume, VoxelCoordinate, VoxelExtent,
    VoxelFrontend, VoxelMaterial, VoxelMaterialId, VoxelRegion, VoxelSceneId, VoxelSceneRevision,
    VoxelValue, VoxelVolumeId, VoxelVolumeMetadata,
};

fn expected_error<T>(
    result: Result<T, RasterArtifactBuildError>,
    expected_failure: &'static str,
) -> Result<RasterArtifactBuildError, Box<dyn std::error::Error>> {
    match result {
        Ok(_) => Err(expected_failure.into()),
        Err(error) => Ok(error),
    }
}

fn one_voxel_view(
    scene_origin: [f32; 3],
    voxel_size: f32,
) -> Result<voxel_frontend::VoxelSceneView, voxel_frontend::VoxelFrontendError> {
    let material_identity = VoxelMaterialId::new("stone");
    let extent = VoxelExtent::new(1, 1, 1);
    VoxelFrontend::new().publish(DenseVoxelScene::new(
        VoxelSceneId::new("failure-scene"),
        VoxelSceneRevision::new(77),
        vec![VoxelMaterial::new(
            material_identity.clone(),
            [0.25, 0.5, 0.75, 1.0],
        )],
        vec![DenseVoxelVolume::new(
            VoxelVolumeMetadata::new(
                VoxelVolumeId::new("diagnostic"),
                extent,
                scene_origin,
                voxel_size,
            ),
            vec![DenseVoxelBatch::new(
                VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), extent),
                vec![VoxelValue::Occupied(material_identity)],
            )],
        )],
    ))
}

#[test]
fn unknown_volume_error_carries_metadata_phase_and_source_revision()
-> Result<(), Box<dyn std::error::Error>> {
    let view = one_voxel_view([0.0, 0.0, 0.0], 1.0)?;

    let error = expected_error(
        derive_raster_artifact(&view, &VoxelVolumeId::new("missing")),
        "an unknown volume should not produce an artifact",
    )?;

    assert_eq!(error.phase(), RasterArtifactBuildPhase::Metadata);
    assert_eq!(error.source_revision(), VoxelSceneRevision::new(77));
    assert!(error.to_string().contains("missing"));
    assert!(error.to_string().contains("metadata"));
    assert!(error.to_string().contains("VoxelSceneRevision(77)"));

    Ok(())
}

#[test]
fn non_finite_scene_transform_returns_only_a_contextual_geometry_error()
-> Result<(), Box<dyn std::error::Error>> {
    let view = one_voxel_view([f32::MAX, 0.0, 0.0], f32::MAX)?;

    let error = expected_error(
        derive_raster_artifact(&view, &VoxelVolumeId::new("diagnostic")),
        "a non-finite coordinate transform should not produce a partial artifact",
    )?;

    assert_eq!(error.phase(), RasterArtifactBuildPhase::Geometry);
    assert_eq!(error.source_revision(), VoxelSceneRevision::new(77));
    assert!(error.to_string().contains("non-finite"));
    assert!(error.to_string().contains("VoxelSceneRevision(77)"));

    Ok(())
}

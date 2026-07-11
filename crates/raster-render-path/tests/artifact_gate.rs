use raster_render_path::{
    CameraPose, RasterArtifactInstallerError, RasterRenderPath, derive_raster_artifact,
};
use voxel_frontend::{
    DenseVoxelBatch, DenseVoxelScene, DenseVoxelVolume, VoxelCoordinate, VoxelExtent,
    VoxelFrontend, VoxelMaterial, VoxelMaterialId, VoxelRegion, VoxelSceneId, VoxelSceneRevision,
    VoxelValue, VoxelVolumeId, VoxelVolumeMetadata,
};

fn artifact(
    revision: u64,
) -> Result<raster_render_path::RasterArtifact, Box<dyn std::error::Error>> {
    let extent = VoxelExtent::new(1, 1, 1);
    let material_identity = VoxelMaterialId::new("stone");
    let volume_identity = VoxelVolumeId::new("diagnostic");
    let view = VoxelFrontend::new().publish(DenseVoxelScene::new(
        VoxelSceneId::new("installation"),
        VoxelSceneRevision::new(revision),
        vec![VoxelMaterial::new(
            material_identity.clone(),
            [0.25, 0.5, 0.75, 1.0],
        )],
        vec![DenseVoxelVolume::new(
            VoxelVolumeMetadata::new(volume_identity.clone(), extent, [0.0; 3], 1.0),
            vec![DenseVoxelBatch::new(
                VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), extent),
                vec![VoxelValue::Occupied(material_identity)],
            )],
        )],
    ))?;
    Ok(derive_raster_artifact(&view, &volume_identity)?)
}

fn camera_pose() -> CameraPose {
    CameraPose::new(
        [5.0, 4.0, 6.0],
        [0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        55.0,
        0.1,
        100.0,
    )
}

#[test]
fn render_path_gate_accepts_exactly_one_complete_matching_revision()
-> Result<(), Box<dyn std::error::Error>> {
    let (_render_path, installer) =
        RasterRenderPath::awaiting_artifact(camera_pose(), VoxelSceneRevision::new(27));

    installer.publish_complete(artifact(27)?)?;

    assert_eq!(
        installer.staged_source_revision()?,
        Some(VoxelSceneRevision::new(27))
    );
    assert_eq!(installer.installed_source_revision()?, None);
    let duplicate = installer
        .publish_complete(artifact(27)?)
        .expect_err("a second complete artifact must be rejected");
    assert!(matches!(
        duplicate,
        RasterArtifactInstallerError::AlreadyPublished { .. }
    ));
    Ok(())
}

#[test]
fn render_path_gate_rejects_a_stale_complete_artifact() -> Result<(), Box<dyn std::error::Error>> {
    let (_render_path, installer) =
        RasterRenderPath::awaiting_artifact(camera_pose(), VoxelSceneRevision::new(27));

    let error = installer
        .publish_complete(artifact(26)?)
        .expect_err("a stale artifact must not enter the installation slot");

    assert_eq!(
        error,
        RasterArtifactInstallerError::RevisionMismatch {
            expected: VoxelSceneRevision::new(27),
            actual: VoxelSceneRevision::new(26),
        }
    );
    assert_eq!(installer.staged_source_revision()?, None);
    assert_eq!(installer.installed_source_revision()?, None);
    Ok(())
}

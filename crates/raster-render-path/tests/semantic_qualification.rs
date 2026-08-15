use raster_render_path::{
    AxisNormal, RasterSemanticFaceCorrespondence, SemanticFace, derive_raster_artifact,
    qualify_raster_semantic_faces,
};
use semantic_ray_oracle::{SemanticRay, SemanticRayProbe, observe_probe};
use voxel_frontend::{
    DenseVoxelBatch, DenseVoxelScene, DenseVoxelVolume, VoxelCoordinate, VoxelExtent,
    VoxelFrontend, VoxelMaterial, VoxelMaterialId, VoxelRegion, VoxelSceneId, VoxelSceneRevision,
    VoxelValue, VoxelVolumeId, VoxelVolumeMetadata,
};

#[test]
fn installed_faces_match_entered_contacts_without_inventing_started_inside_normals()
-> Result<(), Box<dyn std::error::Error>> {
    let volume_identity = VoxelVolumeId::new("volume");
    let material_identity = VoxelMaterialId::new("stone");
    let view = VoxelFrontend::new().publish(DenseVoxelScene::new(
        VoxelSceneId::new("semantic-qualification"),
        VoxelSceneRevision::new(7),
        vec![VoxelMaterial::new(material_identity.clone(), [1.0; 4])],
        vec![DenseVoxelVolume::new(
            VoxelVolumeMetadata::new(
                volume_identity.clone(),
                VoxelExtent::new(1, 1, 1),
                [0.0; 3],
                1.0,
            ),
            vec![DenseVoxelBatch::new(
                VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), VoxelExtent::new(1, 1, 1)),
                vec![VoxelValue::Occupied(material_identity.clone())],
            )],
        )],
    ))?;
    let artifact = derive_raster_artifact(&view, &volume_identity)?;
    let probes = [
        SemanticRayProbe::new(
            "entered",
            SemanticRay::new([-1.0, 0.5, 0.5], [1.0, 0.0, 0.0], 0.0, 3.0)?,
        )?,
        SemanticRayProbe::new(
            "started-inside",
            SemanticRay::new([0.5; 3], [1.0, 0.0, 0.0], 0.0, 1.0)?,
        )?,
        SemanticRayProbe::new(
            "miss",
            SemanticRay::new([-1.0, 2.0, 0.5], [1.0, 0.0, 0.0], 0.0, 3.0)?,
        )?,
    ];
    let oracle = probes
        .iter()
        .map(|probe| observe_probe(&view, probe))
        .collect::<Result<Vec<_>, _>>()?;

    let observations = qualify_raster_semantic_faces(&artifact, &oracle, 23)?;

    assert_eq!(observations.len(), 3);
    assert_eq!(observations[0].frame_sequence(), 23);
    assert_eq!(
        observations[0].correspondence(),
        &RasterSemanticFaceCorrespondence::Matched(SemanticFace::new(
            volume_identity,
            VoxelCoordinate::new(0, 0, 0),
            AxisNormal::NegativeX,
            material_identity,
        ))
    );
    assert_eq!(
        observations[1].correspondence(),
        &RasterSemanticFaceCorrespondence::NotApplicableStartedInside
    );
    assert_eq!(
        observations[2].correspondence(),
        &RasterSemanticFaceCorrespondence::NotApplicableMiss
    );
    assert!(observations.iter().all(|observation| observation.passed()));
    Ok(())
}

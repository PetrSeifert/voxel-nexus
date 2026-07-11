use canonical_scene::{CanonicalSceneScale, generate_canonical_scene};
use raster_render_path::derive_raster_artifact;
use voxel_frontend::{
    VoxelCoordinate, VoxelExtent, VoxelFrontend, VoxelMaterialId, VoxelRegion, VoxelValue,
};

#[test]
fn canonical_scales_report_fixed_identity_geometry_and_complexity()
-> Result<(), Box<dyn std::error::Error>> {
    let cases = [
        (
            CanonicalSceneScale::Small,
            [64, 32, 64],
            52_608,
            13_616,
            14_000,
        ),
        (
            CanonicalSceneScale::Medium,
            [128, 64, 128],
            420_864,
            54_464,
            56_000,
        ),
        (
            CanonicalSceneScale::Large,
            [256, 128, 256],
            3_366_912,
            217_856,
            224_000,
        ),
    ];

    for (scale, dimensions, occupied_count, exposed_face_count, exposed_face_limit) in cases {
        let canonical = generate_canonical_scene(scale)?;
        let metadata = canonical.metadata();
        assert_eq!(metadata.generator_identity(), "voxel-nexus-canonical-dense");
        assert_eq!(metadata.generator_version(), 1);
        assert_eq!(metadata.seed(), 0x564f_5845_4c4e_5853);
        assert_eq!(metadata.dimensions(), dimensions);
        assert_eq!(metadata.scene_origin(), [-8.0, -4.0, -8.0]);
        assert_eq!(metadata.voxel_size(), 0.25 / scale.factor() as f32);
        assert_eq!(metadata.occupied_count(), occupied_count);
        assert_eq!(metadata.exposed_face_count(), exposed_face_count);
        assert_eq!(metadata.exposed_face_limit(), exposed_face_limit);
        assert!(metadata.exposed_face_count() <= metadata.exposed_face_limit());
        assert_eq!(
            metadata
                .material_catalogue()
                .iter()
                .map(|material| (material.identity(), material.linear_base_color()))
                .collect::<Vec<_>>(),
            [
                ("canonical-warm", [0.95, 0.22, 0.1, 1.0]),
                ("canonical-green", [0.12, 0.75, 0.28, 1.0]),
                ("canonical-blue", [0.1, 0.32, 0.95, 1.0]),
            ]
        );
    }
    Ok(())
}

#[test]
fn canonical_scene_exposes_material_regions_cavity_overhang_and_boundary_through_public_reads()
-> Result<(), Box<dyn std::error::Error>> {
    let canonical = generate_canonical_scene(CanonicalSceneScale::Small)?;
    let volume_identity = canonical.metadata().volume_identity().clone();
    let view = VoxelFrontend::new().publish(canonical.into_scene())?;
    let samples = view.read_region(
        &volume_identity,
        VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), VoxelExtent::new(64, 32, 64)),
    )?;
    let value_at = |coordinate: [usize; 3]| -> Result<&VoxelValue, Box<dyn std::error::Error>> {
        let [coordinate_x, coordinate_y, coordinate_z] = coordinate;
        let index = coordinate_z
            .checked_mul(32)
            .and_then(|value| value.checked_add(coordinate_y))
            .and_then(|value| value.checked_mul(64))
            .and_then(|value| value.checked_add(coordinate_x))
            .ok_or("canonical sample index overflow")?;
        samples
            .get(index)
            .map(|sample| sample.value())
            .ok_or_else(|| "canonical sample missing".into())
    };

    assert_eq!(
        value_at([4, 4, 32])?,
        &VoxelValue::Occupied(VoxelMaterialId::new("canonical-warm"))
    );
    assert_eq!(
        value_at([32, 4, 32])?,
        &VoxelValue::Occupied(VoxelMaterialId::new("canonical-green"))
    );
    assert_eq!(
        value_at([52, 4, 32])?,
        &VoxelValue::Occupied(VoxelMaterialId::new("canonical-blue"))
    );
    assert_eq!(value_at([28, 14, 10])?, &VoxelValue::Empty);
    assert_eq!(value_at([28, 14, 53])?, &VoxelValue::Empty);
    assert_eq!(
        value_at([55, 22, 30])?,
        &VoxelValue::Occupied(VoxelMaterialId::new("canonical-blue"))
    );
    assert_eq!(
        value_at([0, 4, 32])?,
        &VoxelValue::Occupied(VoxelMaterialId::new("canonical-warm"))
    );
    assert_eq!(value_at([49, 22, 30])?, &VoxelValue::Empty);
    Ok(())
}

#[test]
fn every_canonical_scale_derives_the_recorded_bounded_surface()
-> Result<(), Box<dyn std::error::Error>> {
    for scale in [
        CanonicalSceneScale::Small,
        CanonicalSceneScale::Medium,
        CanonicalSceneScale::Large,
    ] {
        let canonical = generate_canonical_scene(scale)?;
        let volume_identity = canonical.metadata().volume_identity().clone();
        let expected_exposed_faces = canonical.metadata().exposed_face_count();
        let view = VoxelFrontend::new().publish(canonical.into_scene())?;
        let artifact = derive_raster_artifact(&view, &volume_identity)?;

        assert_eq!(
            u64::try_from(artifact.semantic_faces().len())?,
            expected_exposed_faces
        );
        assert_eq!(
            artifact.vertices().len(),
            artifact.semantic_faces().len() * 4
        );
        assert_eq!(
            artifact.indices().len(),
            artifact.semantic_faces().len() * 6
        );
    }
    Ok(())
}

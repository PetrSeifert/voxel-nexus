use voxel_frontend::{
    DenseVoxelBatch, DenseVoxelScene, DenseVoxelVolume, VoxelCoordinate, VoxelExtent,
    VoxelFrontend, VoxelFrontendError, VoxelMaterial, VoxelMaterialId, VoxelRegion, VoxelSceneId,
    VoxelSceneRevision, VoxelValue, VoxelVolumeId, VoxelVolumeMetadata,
};

fn expected_error<T>(
    result: Result<T, VoxelFrontendError>,
    expected_failure: &'static str,
) -> Result<VoxelFrontendError, Box<dyn std::error::Error>> {
    match result {
        Ok(_) => Err(expected_failure.into()),
        Err(error) => Ok(error),
    }
}

fn material(identity: &str) -> VoxelMaterial {
    VoxelMaterial::new(VoxelMaterialId::new(identity), [0.1, 0.2, 0.3, 1.0])
}

fn one_voxel_volume(identity: &str, value: VoxelValue) -> DenseVoxelVolume {
    DenseVoxelVolume::new(
        VoxelVolumeMetadata::new(
            VoxelVolumeId::new(identity),
            VoxelExtent::new(1, 1, 1),
            [0.0, 0.0, 0.0],
            1.0,
        ),
        vec![DenseVoxelBatch::new(
            VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), VoxelExtent::new(1, 1, 1)),
            vec![value],
        )],
    )
}

fn scene(materials: Vec<VoxelMaterial>, volumes: Vec<DenseVoxelVolume>) -> DenseVoxelScene {
    DenseVoxelScene::new(
        VoxelSceneId::new("validation-scene"),
        VoxelSceneRevision::new(12),
        materials,
        volumes,
    )
}

#[test]
fn publication_rejects_duplicate_catalogue_identities() -> Result<(), Box<dyn std::error::Error>> {
    let frontend = VoxelFrontend::new();
    let duplicate_material_error = expected_error(
        frontend.publish(scene(
            vec![material("stone"), material("stone")],
            Vec::new(),
        )),
        "duplicate material identities should fail publication",
    )?;
    assert!(duplicate_material_error.to_string().contains("stone"));
    assert!(
        duplicate_material_error
            .to_string()
            .contains("duplicate Voxel Material")
    );

    let duplicate_volume_error = expected_error(
        frontend.publish(scene(
            Vec::new(),
            vec![
                one_voxel_volume("terrain", VoxelValue::Empty),
                one_voxel_volume("terrain", VoxelValue::Empty),
            ],
        )),
        "duplicate volume identities should fail publication",
    )?;
    assert!(duplicate_volume_error.to_string().contains("terrain"));
    assert!(
        duplicate_volume_error
            .to_string()
            .contains("duplicate Voxel Volume")
    );

    Ok(())
}

#[test]
fn publication_rejects_invalid_volume_metadata_and_extents()
-> Result<(), Box<dyn std::error::Error>> {
    for (metadata, expected_context) in [
        (
            VoxelVolumeMetadata::new(
                VoxelVolumeId::new("empty-extent"),
                VoxelExtent::new(0, 1, 1),
                [0.0, 0.0, 0.0],
                1.0,
            ),
            "empty extent",
        ),
        (
            VoxelVolumeMetadata::new(
                VoxelVolumeId::new("invalid-origin"),
                VoxelExtent::new(1, 1, 1),
                [f32::NAN, 0.0, 0.0],
                1.0,
            ),
            "invalid scene origin or voxel size",
        ),
        (
            VoxelVolumeMetadata::new(
                VoxelVolumeId::new("invalid-size"),
                VoxelExtent::new(1, 1, 1),
                [0.0, 0.0, 0.0],
                0.0,
            ),
            "invalid scene origin or voxel size",
        ),
    ] {
        let error = expected_error(
            VoxelFrontend::new().publish(scene(
                Vec::new(),
                vec![DenseVoxelVolume::new(metadata, Vec::new())],
            )),
            "invalid volume metadata should fail publication",
        )?;
        assert!(error.to_string().contains(expected_context));
    }

    Ok(())
}

#[test]
fn publication_reports_unknown_material_with_volume_and_coordinate_context()
-> Result<(), Box<dyn std::error::Error>> {
    let error = expected_error(
        VoxelFrontend::new().publish(scene(
            vec![material("stone")],
            vec![one_voxel_volume(
                "terrain",
                VoxelValue::Occupied(VoxelMaterialId::new("missing")),
            )],
        )),
        "unknown occupied material should fail publication",
    )?;
    let message = error.to_string();
    assert!(message.contains("terrain"));
    assert!(message.contains("missing"));
    assert!(message.contains("VoxelCoordinate"));

    Ok(())
}

#[test]
fn publication_rejects_incomplete_overlapping_and_malformed_dense_batches()
-> Result<(), Box<dyn std::error::Error>> {
    let metadata = || {
        VoxelVolumeMetadata::new(
            VoxelVolumeId::new("terrain"),
            VoxelExtent::new(2, 1, 1),
            [0.0, 0.0, 0.0],
            1.0,
        )
    };
    let cases = [
        (
            vec![DenseVoxelBatch::new(
                VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), VoxelExtent::new(1, 1, 1)),
                vec![VoxelValue::Empty],
            )],
            "does not provide every coordinate",
        ),
        (
            vec![
                DenseVoxelBatch::new(
                    VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), VoxelExtent::new(2, 1, 1)),
                    vec![VoxelValue::Empty, VoxelValue::Empty],
                ),
                DenseVoxelBatch::new(
                    VoxelRegion::new(VoxelCoordinate::new(1, 0, 0), VoxelExtent::new(1, 1, 1)),
                    vec![VoxelValue::Empty],
                ),
            ],
            "more than once",
        ),
        (
            vec![DenseVoxelBatch::new(
                VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), VoxelExtent::new(2, 1, 1)),
                vec![VoxelValue::Empty],
            )],
            "contains 1 values",
        ),
    ];

    for (batches, expected_context) in cases {
        let error = expected_error(
            VoxelFrontend::new().publish(scene(
                Vec::new(),
                vec![DenseVoxelVolume::new(metadata(), batches)],
            )),
            "malformed dense batches should fail publication",
        )?;
        assert!(error.to_string().contains(expected_context));
        assert!(error.to_string().contains("terrain"));
    }

    Ok(())
}

#[test]
fn read_errors_identify_unknown_volumes_and_malformed_regions()
-> Result<(), Box<dyn std::error::Error>> {
    let view = VoxelFrontend::new().publish(scene(
        Vec::new(),
        vec![one_voxel_volume("terrain", VoxelValue::Empty)],
    ))?;

    let unknown_error = expected_error(
        view.read_region(
            &VoxelVolumeId::new("missing-volume"),
            VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), VoxelExtent::new(1, 1, 1)),
        ),
        "unknown volume should fail",
    )?;
    assert!(unknown_error.to_string().contains("missing-volume"));

    let empty_error = expected_error(
        view.read_region(
            &VoxelVolumeId::new("terrain"),
            VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), VoxelExtent::new(0, 1, 1)),
        ),
        "empty region should fail",
    )?;
    assert!(empty_error.to_string().contains("terrain"));
    assert!(empty_error.to_string().contains("empty extent"));

    let overflow_error = expected_error(
        view.read_region(
            &VoxelVolumeId::new("terrain"),
            VoxelRegion::new(
                VoxelCoordinate::new(i32::MAX, 0, 0),
                VoxelExtent::new(2, 1, 1),
            ),
        ),
        "overflowing region should fail",
    )?;
    assert!(
        overflow_error
            .to_string()
            .contains("invalid coordinate bounds")
    );

    Ok(())
}

#[test]
fn frontend_rejects_a_second_publication_and_retains_the_first()
-> Result<(), Box<dyn std::error::Error>> {
    let frontend = VoxelFrontend::new();
    let retained = frontend.publish(scene(
        Vec::new(),
        vec![one_voxel_volume("terrain", VoxelValue::Empty)],
    ))?;
    let second_publication_error = expected_error(
        frontend.publish(DenseVoxelScene::new(
            VoxelSceneId::new("replacement"),
            VoxelSceneRevision::new(13),
            vec![material("stone")],
            vec![one_voxel_volume(
                "terrain",
                VoxelValue::Occupied(VoxelMaterialId::new("stone")),
            )],
        )),
        "a second publication should be rejected",
    )?;

    assert!(
        second_publication_error
            .to_string()
            .contains("already been published")
    );
    assert_eq!(retained.scene_id(), &VoxelSceneId::new("validation-scene"));
    assert_eq!(retained.revision(), VoxelSceneRevision::new(12));
    let samples = retained.read_region(
        &VoxelVolumeId::new("terrain"),
        VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), VoxelExtent::new(1, 1, 1)),
    )?;
    assert_eq!(
        samples.first().map(|sample| sample.value()),
        Some(&VoxelValue::Empty)
    );

    Ok(())
}

#[test]
fn maximum_coordinate_is_a_valid_out_of_bounds_region() -> Result<(), Box<dyn std::error::Error>> {
    let view = VoxelFrontend::new().publish(scene(
        Vec::new(),
        vec![one_voxel_volume("terrain", VoxelValue::Empty)],
    ))?;

    let samples = view.read_region(
        &VoxelVolumeId::new("terrain"),
        VoxelRegion::new(
            VoxelCoordinate::new(i32::MAX, 0, 0),
            VoxelExtent::new(1, 1, 1),
        ),
    )?;

    assert_eq!(samples.len(), 1);
    assert_eq!(
        samples.first().map(|sample| sample.coordinate()),
        Some(VoxelCoordinate::new(i32::MAX, 0, 0))
    );
    assert_eq!(
        samples.first().map(|sample| sample.value()),
        Some(&VoxelValue::Empty)
    );

    Ok(())
}

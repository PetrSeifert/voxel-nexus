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
fn oversized_publication_returns_an_error_without_changing_state()
-> Result<(), Box<dyn std::error::Error>> {
    for storage_tier in [
        voxel_frontend::StorageTier::Dense,
        voxel_frontend::StorageTier::SparsePages,
    ] {
        let frontend = VoxelFrontend::new();
        let oversized_scene = || {
            scene(
                Vec::new(),
                vec![DenseVoxelVolume::new(
                    VoxelVolumeMetadata::new(
                        VoxelVolumeId::new("oversized"),
                        VoxelExtent::new(u32::MAX, u32::MAX, 1),
                        [0.0; 3],
                        1.0,
                    ),
                    Vec::new(),
                )],
            )
        };
        let error = expected_error(
            frontend.publish(oversized_scene().with_storage_tier(storage_tier)),
            "oversized publication succeeded",
        )?;
        assert!(
            matches!(error, VoxelFrontendError::VolumeTooLarge { identity } if identity == VoxelVolumeId::new("oversized"))
        );
        assert!(matches!(
            frontend.scene_view(),
            Err(VoxelFrontendError::SceneNotPublished)
        ));

        let retained = frontend.publish(
            scene(
                Vec::new(),
                vec![one_voxel_volume("terrain", VoxelValue::Empty)],
            )
            .with_storage_tier(storage_tier),
        )?;
        expected_error(
            frontend.publish(oversized_scene().with_storage_tier(storage_tier)),
            "oversized replacement succeeded",
        )?;
        let current = frontend.scene_view()?;
        assert_eq!(current.scene_id(), retained.scene_id());
        assert_eq!(current.revision(), retained.revision());
        assert_eq!(current.materials(), retained.materials());
        assert_eq!(current.volumes(), retained.volumes());
        let region = VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), VoxelExtent::new(1, 1, 1));
        assert_eq!(
            current.read_region(&VoxelVolumeId::new("terrain"), region)?,
            retained.read_region(&VoxelVolumeId::new("terrain"), region)?
        );
    }
    Ok(())
}

#[test]
fn publication_rejects_duplicate_catalogue_identities() -> Result<(), Box<dyn std::error::Error>> {
    for storage_tier in [
        voxel_frontend::StorageTier::Dense,
        voxel_frontend::StorageTier::SparsePages,
    ] {
        let frontend = VoxelFrontend::new();
        let duplicate_material_error = expected_error(
            frontend.publish(
                scene(vec![material("stone"), material("stone")], Vec::new())
                    .with_storage_tier(storage_tier),
            ),
            "duplicate material identities should fail publication",
        )?;
        assert!(duplicate_material_error.to_string().contains("stone"));
        assert!(
            duplicate_material_error
                .to_string()
                .contains("duplicate Voxel Material")
        );

        let duplicate_volume_error = expected_error(
            frontend.publish(
                scene(
                    Vec::new(),
                    vec![
                        one_voxel_volume("terrain", VoxelValue::Empty),
                        one_voxel_volume("terrain", VoxelValue::Empty),
                    ],
                )
                .with_storage_tier(storage_tier),
            ),
            "duplicate volume identities should fail publication",
        )?;
        assert!(duplicate_volume_error.to_string().contains("terrain"));
        assert!(
            duplicate_volume_error
                .to_string()
                .contains("duplicate Voxel Volume")
        );
    }
    Ok(())
}

#[test]
fn publication_rejects_invalid_volume_metadata_and_extents()
-> Result<(), Box<dyn std::error::Error>> {
    for storage_tier in [
        voxel_frontend::StorageTier::Dense,
        voxel_frontend::StorageTier::SparsePages,
    ] {
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
                VoxelFrontend::new().publish(
                    scene(
                        Vec::new(),
                        vec![DenseVoxelVolume::new(metadata, Vec::new())],
                    )
                    .with_storage_tier(storage_tier),
                ),
                "invalid volume metadata should fail publication",
            )?;
            assert!(error.to_string().contains(expected_context));
        }
    }
    Ok(())
}

#[test]
fn publication_reports_unknown_material_with_volume_and_coordinate_context()
-> Result<(), Box<dyn std::error::Error>> {
    for storage_tier in [
        voxel_frontend::StorageTier::Dense,
        voxel_frontend::StorageTier::SparsePages,
    ] {
        let error = expected_error(
            VoxelFrontend::new().publish(
                scene(
                    vec![material("stone")],
                    vec![one_voxel_volume(
                        "terrain",
                        VoxelValue::Occupied(VoxelMaterialId::new("missing")),
                    )],
                )
                .with_storage_tier(storage_tier),
            ),
            "unknown occupied material should fail publication",
        )?;
        let message = error.to_string();
        assert!(message.contains("terrain"));
        assert!(message.contains("missing"));
        assert!(message.contains("VoxelCoordinate"));
    }
    Ok(())
}

#[test]
fn publication_rejects_incomplete_overlapping_and_malformed_dense_batches()
-> Result<(), Box<dyn std::error::Error>> {
    for storage_tier in [
        voxel_frontend::StorageTier::Dense,
        voxel_frontend::StorageTier::SparsePages,
    ] {
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
                VoxelFrontend::new().publish(
                    scene(Vec::new(), vec![DenseVoxelVolume::new(metadata(), batches)])
                        .with_storage_tier(storage_tier),
                ),
                "malformed dense batches should fail publication",
            )?;
            assert!(error.to_string().contains(expected_context));
            assert!(error.to_string().contains("terrain"));
        }
    }
    Ok(())
}

#[test]
fn read_errors_identify_unknown_volumes_and_malformed_regions()
-> Result<(), Box<dyn std::error::Error>> {
    for storage_tier in [
        voxel_frontend::StorageTier::Dense,
        voxel_frontend::StorageTier::SparsePages,
    ] {
        let view = VoxelFrontend::new().publish(
            scene(
                Vec::new(),
                vec![one_voxel_volume("terrain", VoxelValue::Empty)],
            )
            .with_storage_tier(storage_tier),
        )?;

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
    }
    Ok(())
}

#[test]
fn frontend_rejects_a_second_publication_and_retains_the_first()
-> Result<(), Box<dyn std::error::Error>> {
    for storage_tier in [
        voxel_frontend::StorageTier::Dense,
        voxel_frontend::StorageTier::SparsePages,
    ] {
        let frontend = VoxelFrontend::new();
        let retained = frontend.publish(
            scene(
                Vec::new(),
                vec![one_voxel_volume("terrain", VoxelValue::Empty)],
            )
            .with_storage_tier(storage_tier),
        )?;
        let second_publication_error = expected_error(
            frontend.publish(
                DenseVoxelScene::new(
                    VoxelSceneId::new("replacement"),
                    VoxelSceneRevision::new(13),
                    vec![material("stone")],
                    vec![one_voxel_volume(
                        "terrain",
                        VoxelValue::Occupied(VoxelMaterialId::new("stone")),
                    )],
                )
                .with_storage_tier(storage_tier),
            ),
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
    }
    Ok(())
}

#[test]
fn maximum_coordinate_is_a_valid_out_of_bounds_region() -> Result<(), Box<dyn std::error::Error>> {
    for storage_tier in [
        voxel_frontend::StorageTier::Dense,
        voxel_frontend::StorageTier::SparsePages,
    ] {
        let view = VoxelFrontend::new().publish(
            scene(
                Vec::new(),
                vec![one_voxel_volume("terrain", VoxelValue::Empty)],
            )
            .with_storage_tier(storage_tier),
        )?;

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
    }
    Ok(())
}

#[test]
fn publication_rejects_unaddressable_dimensions_counts_and_byte_capacities()
-> Result<(), Box<dyn std::error::Error>> {
    for storage_tier in [
        voxel_frontend::StorageTier::Dense,
        voxel_frontend::StorageTier::SparsePages,
    ] {
        for extent in [
            VoxelExtent::new((1 << 31) + 1, 1, 1),
            VoxelExtent::new(1, (1 << 31) + 1, 1),
            VoxelExtent::new(1, 1, (1 << 31) + 1),
            VoxelExtent::new(1 << 31, 1 << 31, 1 << 31),
            VoxelExtent::new(1 << 31, 1 << 31, 1),
            VoxelExtent::new(1 << 29, 1 << 30, 1),
        ] {
            let frontend = VoxelFrontend::new();
            let error = expected_error(
                frontend.publish(
                    scene(
                        Vec::new(),
                        vec![DenseVoxelVolume::new(
                            VoxelVolumeMetadata::new(
                                VoxelVolumeId::new("too-large"),
                                extent,
                                [0.0; 3],
                                1.0,
                            ),
                            Vec::new(),
                        )],
                    )
                    .with_storage_tier(storage_tier),
                ),
                "unaddressable volume should fail",
            )?;
            assert!(
                matches!(error, VoxelFrontendError::VolumeTooLarge { identity } if identity == VoxelVolumeId::new("too-large"))
            );
            assert!(matches!(
                frontend.scene_view(),
                Err(VoxelFrontendError::SceneNotPublished)
            ));
        }
    }
    Ok(())
}

#[test]
fn large_addressable_volume_validates_batches_before_allocating()
-> Result<(), Box<dyn std::error::Error>> {
    for storage_tier in [
        voxel_frontend::StorageTier::Dense,
        voxel_frontend::StorageTier::SparsePages,
    ] {
        let extent = VoxelExtent::new(1 << 26, 1, 1);
        for (batches, expected_context) in [
            (Vec::new(), "does not provide every coordinate"),
            (
                vec![DenseVoxelBatch::new(
                    VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), extent),
                    Vec::new(),
                )],
                "contains 0 values",
            ),
            (
                vec![DenseVoxelBatch::new(
                    VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), VoxelExtent::new(0, 1, 1)),
                    Vec::new(),
                )],
                "empty region",
            ),
            (
                vec![DenseVoxelBatch::new(
                    VoxelRegion::new(
                        VoxelCoordinate::new(i32::MAX, 0, 0),
                        VoxelExtent::new(2, 1, 1),
                    ),
                    Vec::new(),
                )],
                "invalid coordinate bounds",
            ),
            (
                vec![DenseVoxelBatch::new(
                    VoxelRegion::new(VoxelCoordinate::new(-1, 0, 0), VoxelExtent::new(1, 1, 1)),
                    vec![VoxelValue::Empty],
                )],
                "outside the volume extent",
            ),
        ] {
            let frontend = VoxelFrontend::new();
            let error = expected_error(
                frontend.publish(
                    scene(
                        Vec::new(),
                        vec![DenseVoxelVolume::new(
                            VoxelVolumeMetadata::new(
                                VoxelVolumeId::new("large"),
                                extent,
                                [0.0; 3],
                                1.0,
                            ),
                            batches,
                        )],
                    )
                    .with_storage_tier(storage_tier),
                ),
                "malformed large volume should fail",
            )?;
            assert!(error.to_string().contains(expected_context), "{error}");
            assert!(error.to_string().contains("large"));
            assert!(matches!(
                frontend.scene_view(),
                Err(VoxelFrontendError::SceneNotPublished)
            ));
        }
    }
    Ok(())
}

#[test]
#[cfg(target_pointer_width = "64")]
fn maximum_addressable_dimension_reaches_batch_validation() -> Result<(), Box<dyn std::error::Error>>
{
    for storage_tier in [
        voxel_frontend::StorageTier::Dense,
        voxel_frontend::StorageTier::SparsePages,
    ] {
        let error = expected_error(
            VoxelFrontend::new().publish(
                scene(
                    Vec::new(),
                    vec![DenseVoxelVolume::new(
                        VoxelVolumeMetadata::new(
                            VoxelVolumeId::new("boundary"),
                            VoxelExtent::new(1 << 31, 1, 1),
                            [0.0; 3],
                            1.0,
                        ),
                        Vec::new(),
                    )],
                )
                .with_storage_tier(storage_tier),
            ),
            "missing batches should fail",
        )?;
        assert!(matches!(error, VoxelFrontendError::IncompleteVolume { .. }));
    }
    Ok(())
}

#[test]
fn dense_region_reads_preserve_order_overlap_and_reused_buffer_contents()
-> Result<(), Box<dyn std::error::Error>> {
    for storage_tier in [
        voxel_frontend::StorageTier::Dense,
        voxel_frontend::StorageTier::SparsePages,
    ] {
        let extent = VoxelExtent::new(3, 2, 2);
        let materials = (0..12)
            .map(|index| material(&format!("material-{index}")))
            .collect();
        let expected: Vec<_> = (0..12)
            .map(|index| VoxelValue::Occupied(VoxelMaterialId::new(format!("material-{index}"))))
            .collect();
        let identity = VoxelVolumeId::new("terrain");
        let view = VoxelFrontend::new().publish(
            scene(
                materials,
                vec![DenseVoxelVolume::new(
                    VoxelVolumeMetadata::new(identity.clone(), extent, [0.0; 3], 1.0),
                    vec![DenseVoxelBatch::new(
                        VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), extent),
                        expected.clone(),
                    )],
                )],
            )
            .with_storage_tier(storage_tier),
        )?;
        let mut values = vec![VoxelValue::Empty; 12];
        view.read_region_into(
            &identity,
            VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), extent),
            &mut values,
        )?;
        assert_eq!(values, expected);

        for origin in [
            VoxelCoordinate::new(-1, -1, -1),
            VoxelCoordinate::new(1, 1, 1),
        ] {
            let region = VoxelRegion::new(origin, extent);
            view.read_region_into(&identity, region, &mut values)?;
            let [origin_x, origin_y, origin_z] = origin.components();
            let mut expected_overlap = Vec::new();
            for z in origin_z..origin_z + 2 {
                for y in origin_y..origin_y + 2 {
                    for x in origin_x..origin_x + 3 {
                        expected_overlap.push(
                            if (0..3).contains(&x) && (0..2).contains(&y) && (0..2).contains(&z) {
                                VoxelValue::Occupied(VoxelMaterialId::new(format!(
                                    "material-{}",
                                    x + 3 * y + 6 * z
                                )))
                            } else {
                                VoxelValue::Empty
                            },
                        );
                    }
                }
            }
            assert_eq!(values, expected_overlap);
        }
        for origin in [
            VoxelCoordinate::new(i32::MIN, 0, 0),
            VoxelCoordinate::new(i32::MAX - 2, 0, 0),
        ] {
            view.read_region_into(&identity, VoxelRegion::new(origin, extent), &mut values)?;
            assert_eq!(values, vec![VoxelValue::Empty; 12]);
        }
    }
    Ok(())
}

#[test]
fn dense_region_read_errors_leave_buffers_unchanged() -> Result<(), Box<dyn std::error::Error>> {
    for storage_tier in [
        voxel_frontend::StorageTier::Dense,
        voxel_frontend::StorageTier::SparsePages,
    ] {
        let view = VoxelFrontend::new().publish(
            scene(
                Vec::new(),
                vec![one_voxel_volume("terrain", VoxelValue::Empty)],
            )
            .with_storage_tier(storage_tier),
        )?;
        let identity = VoxelVolumeId::new("terrain");
        let region = VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), VoxelExtent::new(2, 2, 2));
        for actual in [0, 7, 9] {
            let mut values = vec![VoxelValue::Occupied(VoxelMaterialId::new("sentinel")); actual];
            let unchanged = values.clone();
            let error = expected_error(
                view.read_region_into(&identity, region, &mut values),
                "buffer mismatch accepted",
            )?;
            assert!(
                matches!(error, VoxelFrontendError::RegionBufferSize { expected: 8, actual: count, .. } if count == actual)
            );
            assert_eq!(values, unchanged);
        }
        let mut values = vec![VoxelValue::Occupied(VoxelMaterialId::new("sentinel"))];
        let unchanged = values.clone();
        assert!(matches!(
            view.read_region_into(&VoxelVolumeId::new("missing"), region, &mut values),
            Err(VoxelFrontendError::UnknownVolumeIdentity { .. })
        ));
        assert_eq!(values, unchanged);
        assert!(matches!(
            view.read_region_into(
                &identity,
                VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), VoxelExtent::new(0, 1, 1)),
                &mut values
            ),
            Err(VoxelFrontendError::EmptyRegionRequest { .. })
        ));
        assert_eq!(values, unchanged);
        assert!(matches!(
            view.read_region_into(
                &identity,
                VoxelRegion::new(
                    VoxelCoordinate::new(i32::MAX, 0, 0),
                    VoxelExtent::new(2, 1, 1)
                ),
                &mut values
            ),
            Err(VoxelFrontendError::InvalidRegionBounds { .. })
        ));
        assert_eq!(values, unchanged);
    }
    Ok(())
}

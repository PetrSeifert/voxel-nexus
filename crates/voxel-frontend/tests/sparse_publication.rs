use voxel_frontend::*;

fn stone() -> VoxelValue {
    VoxelValue::Occupied(VoxelMaterialId::new("stone"))
}

fn grass() -> VoxelValue {
    VoxelValue::Occupied(VoxelMaterialId::new("grass"))
}

fn region(origin: (i32, i32, i32), size: (u32, u32, u32)) -> VoxelRegion {
    VoxelRegion::new(
        VoxelCoordinate::new(origin.0, origin.1, origin.2),
        VoxelExtent::new(size.0, size.1, size.2),
    )
}

fn fill(origin: (i32, i32, i32), size: (u32, u32, u32), value: VoxelValue) -> SparseVoxelBatch {
    SparseVoxelBatch::Fill(VoxelRegionFill::new(region(origin, size), value))
}

fn detail(
    origin: (i32, i32, i32),
    size: (u32, u32, u32),
    value: impl Fn(i32, i32, i32) -> VoxelValue,
) -> SparseVoxelBatch {
    let values = (0..size.2 as i32)
        .flat_map(|z| {
            (0..size.1 as i32).flat_map(move |y| (0..size.0 as i32).map(move |x| (x, y, z)))
        })
        .map(|(x, y, z)| value(origin.0 + x, origin.1 + y, origin.2 + z))
        .collect();
    SparseVoxelBatch::Detail(DenseVoxelBatch::new(region(origin, size), values))
}

fn materials() -> Vec<VoxelMaterial> {
    ["stone", "grass"]
        .map(|identity| VoxelMaterial::new(VoxelMaterialId::new(identity), [1.0; 4]))
        .into()
}

fn metadata(extent: VoxelExtent) -> VoxelVolumeMetadata {
    VoxelVolumeMetadata::new(VoxelVolumeId::new("terrain"), extent, [0.0; 3], 1.0)
}

fn sparse_scene(extent: VoxelExtent, batches: Vec<SparseVoxelBatch>) -> SparseVoxelScene {
    SparseVoxelScene::new(
        VoxelSceneId::new("sparse"),
        VoxelSceneRevision::new(3),
        materials(),
        vec![SparseVoxelVolume::new(
            metadata(extent),
            SparseVoxelBackground::Empty,
            batches,
        )],
    )
}

fn expected_error<T>(
    result: Result<T, VoxelFrontendError>,
    expected_failure: &str,
) -> Result<VoxelFrontendError, Box<dyn std::error::Error>> {
    match result {
        Ok(_) => Err(expected_failure.into()),
        Err(error) => Ok(error),
    }
}

// A 40 x 35 x 20 volume spans partial edge bricks. The batches cover whole bricks, partial
// bricks, brick-straddling details, and an explicit Empty fill.
fn terrain_value(x: i32, y: i32, z: i32) -> VoxelValue {
    if y < 18 && (0..32).contains(&x) && (0..16).contains(&z) {
        stone()
    } else if (18..21).contains(&y) && (5..37).contains(&x) && (3..19).contains(&z) {
        if (x + y + z) % 3 == 0 {
            VoxelValue::Empty
        } else if (x + z) % 2 == 0 {
            grass()
        } else {
            stone()
        }
    } else if (32..35).contains(&y) && (32..40).contains(&x) && (16..20).contains(&z) {
        grass()
    } else {
        VoxelValue::Empty
    }
}

fn terrain_batches() -> Vec<SparseVoxelBatch> {
    vec![
        fill((0, 0, 0), (32, 18, 16), stone()),
        detail((5, 18, 3), (32, 3, 16), terrain_value),
        fill((32, 32, 16), (8, 3, 4), grass()),
        fill((0, 21, 0), (40, 9, 20), VoxelValue::Empty),
    ]
}

#[test]
fn either_input_form_publishes_identical_values_into_either_tier()
-> Result<(), Box<dyn std::error::Error>> {
    let extent = VoxelExtent::new(40, 35, 20);
    let [width, height, depth] = extent.dimensions();
    let dense_values = (0..depth as i32)
        .flat_map(|z| {
            (0..height as i32).flat_map(move |y| (0..width as i32).map(move |x| (x, y, z)))
        })
        .map(|(x, y, z)| terrain_value(x, y, z))
        .collect();
    let dense_scene = DenseVoxelScene::new(
        VoxelSceneId::new("sparse"),
        VoxelSceneRevision::new(3),
        materials(),
        vec![DenseVoxelVolume::new(
            metadata(extent),
            vec![DenseVoxelBatch::new(
                region((0, 0, 0), (width, height, depth)),
                dense_values,
            )],
        )],
    );
    let mut views = Vec::new();
    for tier in [StorageTier::Dense, StorageTier::SparsePages] {
        views.push(
            VoxelFrontend::new()
                .publish_sparse(sparse_scene(extent, terrain_batches()).with_storage_tier(tier))?,
        );
        views.push(VoxelFrontend::new().publish(dense_scene.clone().with_storage_tier(tier))?);
    }
    let volume = VoxelVolumeId::new("terrain");
    let regions = [
        region((0, 0, 0), (40, 35, 20)),
        region((0, 0, 0), (16, 16, 16)),
        region((3, 2, 1), (20, 14, 12)),
        region((0, 18, 0), (40, 3, 20)),
        region((0, 21, 0), (40, 9, 20)),
        region((32, 32, 16), (8, 3, 4)),
        region((35, 29, 15), (6, 6, 6)),
        region((-3, -3, -3), (50, 50, 30)),
        region((16, 16, 0), (16, 16, 16)),
        region((39, 34, 19), (1, 1, 1)),
    ];
    for view in &views {
        assert_eq!(view.scene_id(), &VoxelSceneId::new("sparse"));
        assert_eq!(view.revision(), VoxelSceneRevision::new(3));
        assert_eq!(view.volumes(), &[metadata(extent)]);
        for region in regions {
            let samples = view.read_region(&volume, region)?;
            for sample in &samples {
                let [x, y, z] = sample.coordinate().components();
                let inside = (0..40).contains(&x) && (0..35).contains(&y) && (0..20).contains(&z);
                let expected = if inside {
                    terrain_value(x, y, z)
                } else {
                    VoxelValue::Empty
                };
                assert_eq!(sample.value(), &expected, "{:?}", sample.coordinate());
            }
            let first = samples.first().ok_or("empty read")?.value();
            let expected_content = if samples.iter().all(|sample| sample.value() == first) {
                VoxelRegionContent::Uniform(first.clone())
            } else {
                VoxelRegionContent::Mixed
            };
            assert_eq!(view.region_content(&volume, region)?, expected_content);
        }
    }
    Ok(())
}

#[test]
fn omitted_coordinates_are_empty_and_no_batches_are_required()
-> Result<(), Box<dyn std::error::Error>> {
    for tier in [StorageTier::Dense, StorageTier::SparsePages] {
        let view = VoxelFrontend::new().publish_sparse(
            sparse_scene(VoxelExtent::new(20, 20, 20), Vec::new()).with_storage_tier(tier),
        )?;
        assert_eq!(
            view.region_content(
                &VoxelVolumeId::new("terrain"),
                region((0, 0, 0), (20, 20, 20))
            )?,
            VoxelRegionContent::Uniform(VoxelValue::Empty)
        );
    }
    Ok(())
}

#[test]
fn overlapping_batches_are_rejected_even_with_equal_or_empty_values()
-> Result<(), Box<dyn std::error::Error>> {
    let cases = [
        (
            "fill with fill",
            vec![
                fill((0, 0, 0), (8, 8, 8), stone()),
                fill((7, 7, 7), (4, 4, 4), grass()),
            ],
        ),
        (
            "fill with detail",
            vec![
                fill((0, 0, 0), (8, 8, 8), stone()),
                detail((4, 0, 4), (2, 2, 2), |_, _, _| grass()),
            ],
        ),
        (
            "detail with detail",
            vec![
                detail((0, 0, 0), (3, 3, 3), |_, _, _| stone()),
                detail((2, 2, 2), (3, 3, 3), |_, _, _| grass()),
            ],
        ),
        (
            "equal values",
            vec![
                fill((0, 0, 0), (8, 8, 8), stone()),
                fill((0, 0, 0), (8, 8, 8), stone()),
            ],
        ),
        (
            "explicit Empty",
            vec![
                fill((0, 0, 0), (8, 8, 8), VoxelValue::Empty),
                detail((3, 3, 3), (1, 1, 1), |_, _, _| VoxelValue::Empty),
            ],
        ),
    ];
    for tier in [StorageTier::Dense, StorageTier::SparsePages] {
        for (case, overlapping) in &cases {
            // Unrelated batches around the overlapping pair keep the reported indices contextual.
            let mut batches = vec![fill((20, 0, 0), (4, 4, 4), stone())];
            batches.extend(overlapping.iter().cloned());
            batches.push(fill((0, 20, 0), (4, 4, 4), grass()));
            let frontend = VoxelFrontend::new();
            let error = expected_error(
                frontend.publish_sparse(
                    sparse_scene(VoxelExtent::new(32, 32, 32), batches).with_storage_tier(tier),
                ),
                case,
            )?;
            assert!(
                matches!(
                    &error,
                    VoxelFrontendError::OverlappingBatches {
                        identity,
                        first_batch_index: 1,
                        second_batch_index: 2,
                    } if identity == &VoxelVolumeId::new("terrain")
                ),
                "{case}: {error}"
            );
            assert!(error.to_string().contains("terrain"), "{case}");
            assert!(matches!(
                frontend.scene_view(),
                Err(VoxelFrontendError::SceneNotPublished)
            ));
        }
    }
    Ok(())
}

#[test]
fn batches_that_touch_without_overlapping_are_accepted() -> Result<(), Box<dyn std::error::Error>> {
    let batches = vec![
        fill((0, 0, 0), (4, 4, 4), stone()),
        fill((4, 0, 0), (4, 4, 4), grass()),
        detail((0, 4, 0), (4, 4, 4), |_, _, _| grass()),
        detail((0, 0, 4), (4, 4, 4), |_, _, _| stone()),
        fill((4, 4, 4), (4, 4, 4), VoxelValue::Empty),
    ];
    for tier in [StorageTier::Dense, StorageTier::SparsePages] {
        let view = VoxelFrontend::new().publish_sparse(
            sparse_scene(VoxelExtent::new(8, 8, 8), batches.clone()).with_storage_tier(tier),
        )?;
        let volume = VoxelVolumeId::new("terrain");
        for (origin, expected) in [
            ((0, 0, 0), stone()),
            ((4, 0, 0), grass()),
            ((0, 4, 0), grass()),
            ((0, 0, 4), stone()),
            ((4, 4, 4), VoxelValue::Empty),
            ((4, 4, 0), VoxelValue::Empty),
        ] {
            assert_eq!(
                view.region_content(&volume, region(origin, (4, 4, 4)))?,
                VoxelRegionContent::Uniform(expected),
                "{origin:?}"
            );
        }
    }
    Ok(())
}

#[test]
fn invalid_sparse_batches_are_rejected_like_dense_ones() -> Result<(), Box<dyn std::error::Error>> {
    let cases = [
        (
            fill((-1, 0, 0), (2, 2, 2), stone()),
            "outside the volume extent",
        ),
        (
            fill((7, 0, 0), (2, 2, 2), stone()),
            "outside the volume extent",
        ),
        (
            detail((0, 0, 7), (1, 1, 2), |_, _, _| stone()),
            "outside the volume extent",
        ),
        (fill((0, 0, 0), (0, 2, 2), stone()), "empty region"),
        (
            fill((i32::MAX, 0, 0), (2, 1, 1), stone()),
            "invalid coordinate bounds",
        ),
        (
            SparseVoxelBatch::Detail(DenseVoxelBatch::new(
                region((0, 0, 0), (2, 1, 1)),
                vec![stone()],
            )),
            "contains 1 values",
        ),
        (
            fill(
                (0, 0, 0),
                (1, 1, 1),
                VoxelValue::Occupied(VoxelMaterialId::new("missing")),
            ),
            "missing",
        ),
        (
            detail((2, 2, 2), (2, 2, 2), |x, _, _| {
                if x == 3 {
                    VoxelValue::Occupied(VoxelMaterialId::new("missing"))
                } else {
                    stone()
                }
            }),
            "VoxelCoordinate { x: 3, y: 2, z: 2 }",
        ),
    ];
    for tier in [StorageTier::Dense, StorageTier::SparsePages] {
        for (batch, expected_context) in &cases {
            let error = expected_error(
                VoxelFrontend::new().publish_sparse(
                    sparse_scene(
                        VoxelExtent::new(8, 8, 8),
                        vec![fill((4, 4, 4), (4, 4, 4), grass()), batch.clone()],
                    )
                    .with_storage_tier(tier),
                ),
                expected_context,
            )?;
            let message = error.to_string();
            assert!(message.contains(expected_context), "{message}");
            assert!(message.contains("terrain"), "{message}");
            if !expected_context.contains("missing") && !expected_context.contains("Coordinate") {
                assert!(message.contains("batch 1"), "{message}");
            }
        }
    }
    Ok(())
}

#[test]
fn sparse_publication_validates_the_scene_catalogue() -> Result<(), Box<dyn std::error::Error>> {
    let volume = |identity: &str| {
        SparseVoxelVolume::new(
            VoxelVolumeMetadata::new(
                VoxelVolumeId::new(identity),
                VoxelExtent::new(1, 1, 1),
                [0.0; 3],
                1.0,
            ),
            SparseVoxelBackground::Empty,
            Vec::new(),
        )
    };
    let error = expected_error(
        VoxelFrontend::new().publish_sparse(SparseVoxelScene::new(
            VoxelSceneId::new("sparse"),
            VoxelSceneRevision::new(0),
            materials(),
            vec![volume("terrain"), volume("terrain")],
        )),
        "duplicate volumes",
    )?;
    assert!(error.to_string().contains("duplicate Voxel Volume"));
    let frontend = VoxelFrontend::new();
    frontend.publish_sparse(SparseVoxelScene::new(
        VoxelSceneId::new("sparse"),
        VoxelSceneRevision::new(0),
        materials(),
        vec![volume("terrain")],
    ))?;
    let error = expected_error(
        frontend.publish_sparse(SparseVoxelScene::new(
            VoxelSceneId::new("sparse"),
            VoxelSceneRevision::new(0),
            materials(),
            vec![volume("terrain")],
        )),
        "second publication",
    )?;
    assert!(matches!(error, VoxelFrontendError::SceneAlreadyPublished));
    Ok(())
}

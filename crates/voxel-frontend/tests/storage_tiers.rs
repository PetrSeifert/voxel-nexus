use voxel_frontend::*;

#[test]
fn region_queries_and_mixed_tier_volumes_preserve_logical_values()
-> Result<(), Box<dyn std::error::Error>> {
    let extent = VoxelExtent::new(257, 2, 2);
    let stone = VoxelMaterialId::new("stone");
    let values: Vec<_> = (0..1028)
        .map(|index| {
            if !(256..1024).contains(&index) {
                VoxelValue::Occupied(stone.clone())
            } else {
                VoxelValue::Empty
            }
        })
        .collect();
    let view = VoxelFrontend::new().publish(DenseVoxelScene::new(
        VoxelSceneId::new("mixed-tiers"),
        VoxelSceneRevision::new(0),
        vec![VoxelMaterial::new(stone, [1.0; 4])],
        [StorageTier::Dense, StorageTier::SparsePages]
            .into_iter()
            .enumerate()
            .map(|(index, tier)| {
                DenseVoxelVolume::new(
                    VoxelVolumeMetadata::new(
                        VoxelVolumeId::new(index.to_string()),
                        extent,
                        [0.0; 3],
                        1.0,
                    ),
                    vec![DenseVoxelBatch::new(
                        VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), extent),
                        values.clone(),
                    )],
                )
                .with_storage_tier(tier)
            })
            .collect(),
    ))?;
    for identity in [VoxelVolumeId::new("0"), VoxelVolumeId::new("1")] {
        for origin in [-2, 0, 1, 255, 256, 257] {
            for height in [-1, 0, 1, 2] {
                for width in [1, 2, 257] {
                    let region = VoxelRegion::new(
                        VoxelCoordinate::new(origin, height, 0),
                        VoxelExtent::new(width, 2, 3),
                    );
                    let samples = view.read_region(&identity, region)?;
                    let first = samples.first().ok_or("empty read")?.value();
                    let expected = if samples.iter().all(|sample| sample.value() == first) {
                        VoxelRegionContent::Uniform(first.clone())
                    } else {
                        VoxelRegionContent::Mixed
                    };
                    assert_eq!(view.region_content(&identity, region)?, expected);
                    let mut buffer = vec![VoxelValue::Empty; samples.len()];
                    view.read_region_into(&identity, region, &mut buffer)?;
                    assert_eq!(
                        buffer,
                        samples
                            .iter()
                            .map(|sample| sample.value().clone())
                            .collect::<Vec<_>>()
                    );
                }
            }
        }
        for (origin, width) in [(0, 256), (256, 1)] {
            let region = VoxelRegion::new(
                VoxelCoordinate::new(origin, 0, 0),
                VoxelExtent::new(width, 1, 1),
            );
            assert!(matches!(
                view.region_content(&identity, region)?,
                VoxelRegionContent::Uniform(_)
            ));
        }
        assert!(matches!(
            view.region_content(
                &identity,
                VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), VoxelExtent::new(0, 1, 1))
            ),
            Err(VoxelFrontendError::EmptyRegionRequest { .. })
        ));
    }
    Ok(())
}

#[test]
fn tiers_agree_across_brick_boundaries_under_edits() -> Result<(), Box<dyn std::error::Error>> {
    let extent = VoxelExtent::new(37, 20, 33);
    let materials = ["stone", "grass"].map(VoxelMaterialId::new);
    let mut state = 0x2545_f491_u32;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        state
    };
    let [width, height, depth] = extent.dimensions();
    let values: Vec<_> = (0..depth)
        .flat_map(|z| (0..height).flat_map(move |y| (0..width).map(move |x| (x, y, z))))
        .map(|(x, y, z)| {
            // Solid, empty, and noisy slabs give uniform, absent, and mixed bricks.
            if y < 8 && z >= 16 {
                VoxelValue::Occupied(materials[0].clone())
            } else if y >= 16 || x >= 32 {
                VoxelValue::Empty
            } else {
                match next() % 3 {
                    0 => VoxelValue::Empty,
                    index => VoxelValue::Occupied(materials[index as usize - 1].clone()),
                }
            }
        })
        .collect();
    let volume = VoxelVolumeId::new("volume");
    let frontends = [StorageTier::Dense, StorageTier::SparsePages].map(|tier| {
        let frontend = VoxelFrontend::new();
        let published = frontend.publish(
            DenseVoxelScene::new(
                VoxelSceneId::new("bricks"),
                VoxelSceneRevision::new(0),
                materials
                    .iter()
                    .map(|identity| VoxelMaterial::new(identity.clone(), [1.0; 4]))
                    .collect(),
                vec![DenseVoxelVolume::new(
                    VoxelVolumeMetadata::new(volume.clone(), extent, [0.0; 3], 1.0),
                    vec![DenseVoxelBatch::new(
                        VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), extent),
                        values.clone(),
                    )],
                )],
            )
            .with_storage_tier(tier),
        );
        published.map(|_| frontend)
    });
    let [dense, sparse] = frontends;
    let (dense, sparse) = (dense?, sparse?);
    let regions: Vec<_> = [
        ((0, 0, 0), (37, 20, 33)),
        ((0, 0, 16), (16, 8, 16)),
        ((3, 1, 17), (20, 6, 15)),
        ((0, 16, 0), (37, 4, 33)),
        ((-4, 15, -2), (50, 10, 40)),
        ((15, 7, 15), (2, 2, 2)),
        ((30, 0, 0), (7, 16, 33)),
        ((32, 0, 0), (5, 20, 33)),
        ((36, 19, 32), (1, 1, 1)),
    ]
    .into_iter()
    .map(|(origin, size)| {
        VoxelRegion::new(
            VoxelCoordinate::new(origin.0, origin.1, origin.2),
            VoxelExtent::new(size.0, size.1, size.2),
        )
    })
    .collect();
    let compare = |dense: &VoxelSceneView,
                   sparse: &VoxelSceneView|
     -> Result<(), Box<dyn std::error::Error>> {
        for region in &regions {
            assert_eq!(
                dense.region_content(&volume, *region)?,
                sparse.region_content(&volume, *region)?,
                "{region:?}"
            );
            assert_eq!(
                dense.read_region(&volume, *region)?,
                sparse.read_region(&volume, *region)?,
                "{region:?}"
            );
        }
        Ok(())
    };
    let initial = (dense.scene_view()?, sparse.scene_view()?);
    compare(&initial.0, &initial.1)?;
    for step in 0..40 {
        let edits: Vec<_> = (0..1 + next() % 40)
            .map(|_| {
                let coordinate = VoxelCoordinate::new(
                    (next() % width) as i32,
                    (next() % height) as i32,
                    (next() % depth) as i32,
                );
                let value = match next() % 3 {
                    0 => VoxelValue::Empty,
                    index => VoxelValue::Occupied(materials[index as usize - 1].clone()),
                };
                VoxelEdit::new(volume.clone(), coordinate, value)
            })
            // Periodically clearing or filling a whole brick normalizes it.
            .chain(
                (step % 10 == 9)
                    .then(|| {
                        (0..16).flat_map(|y| {
                            (0..16).flat_map(move |z| (0..16).map(move |x| (x, y, z)))
                        })
                    })
                    .into_iter()
                    .flatten()
                    .map(|(x, y, z)| {
                        VoxelEdit::new(
                            volume.clone(),
                            VoxelCoordinate::new(x, y, z),
                            if step % 20 == 9 {
                                VoxelValue::Empty
                            } else {
                                VoxelValue::Occupied(materials[1].clone())
                            },
                        )
                    }),
            )
            .collect();
        let dense_outcome = dense.edit(VoxelEditCommand::from_edits(edits.clone()))?;
        let sparse_outcome = sparse.edit(VoxelEditCommand::from_edits(edits))?;
        assert_eq!(
            dense_outcome
                .change_set()
                .map(|change_set| change_set.changed_regions().len()),
            sparse_outcome
                .change_set()
                .map(|change_set| change_set.changed_regions().len())
        );
        compare(dense_outcome.view(), sparse_outcome.view())?;
    }
    compare(&initial.0, &initial.1)?;
    Ok(())
}

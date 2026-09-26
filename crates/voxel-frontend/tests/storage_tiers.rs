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

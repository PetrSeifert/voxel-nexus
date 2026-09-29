use canonical_scene::generate_large_terrain;
use voxel_frontend::{
    VoxelCoordinate, VoxelExtent, VoxelFrontend, VoxelMaterialId, VoxelRegion, VoxelRegionContent,
    VoxelValue, VoxelVolumeId,
};

#[test]
fn repeated_generation_has_identical_content_and_bounded_sparse_publication()
-> Result<(), Box<dyn std::error::Error>> {
    let mut fingerprints = Vec::new();
    for _ in 0..2 {
        let terrain = generate_large_terrain()?;
        fingerprints.push(terrain.content_fingerprint());
        assert_eq!(terrain.fill_batch_count(), 4160);
        assert_eq!(terrain.detail_batch_count(), 4096);
        let detail_values = terrain.detail_value_count();
        let (view, counters) = voxel_frontend::count_storage_work(|| {
            VoxelFrontend::new().publish_sparse(terrain.into_scene())
        });
        let view = view?;
        assert_eq!(counters.validation.batches_validated, 8256);
        assert_eq!(counters.validation.voxel_values_validated, detail_values);
        assert_eq!(counters.validation.sweeps_by_axis[1], 0);
        assert_eq!(counters.validation.sweeps_by_axis.iter().sum::<usize>(), 1);
        assert!(counters.validation.candidate_pairs_examined < 600_000);
        // Staging is limited to surface-intersecting bricks, far below the 2^30 logical values.
        assert!(counters.publication.staged_values_allocated < 80 * 1024 * 1024);
        assert!(view.storage_bytes(&VoxelVolumeId::new("large-terrain"))? < 320 * 1024 * 1024);
    }
    assert_eq!(fingerprints[0], fingerprints[1]);
    Ok(())
}

#[test]
fn large_terrain_publishes_surface_air_and_an_enclosed_cavity()
-> Result<(), Box<dyn std::error::Error>> {
    let scene = generate_large_terrain()?;
    let view = VoxelFrontend::new().publish_sparse(scene.into_scene())?;
    assert_eq!(view.volumes().len(), 1);
    assert_eq!(view.volumes()[0].extent().dimensions(), [2048, 256, 2048]);
    let volume = VoxelVolumeId::new("large-terrain");
    for (origin, extent, expected) in [
        (
            [0, 0, 0],
            [2048, 32, 2048],
            VoxelRegionContent::Uniform(VoxelValue::Occupied(VoxelMaterialId::new(
                "terrain-stone",
            ))),
        ),
        (
            [0, 128, 0],
            [2048, 128, 2048],
            VoxelRegionContent::Uniform(VoxelValue::Empty),
        ),
        (
            [768, 32, 768],
            [256, 32, 256],
            VoxelRegionContent::Uniform(VoxelValue::Empty),
        ),
        (
            [768, 64, 768],
            [256, 16, 256],
            VoxelRegionContent::Uniform(VoxelValue::Occupied(VoxelMaterialId::new(
                "terrain-stone",
            ))),
        ),
        ([0, 79, 0], [32, 3, 32], VoxelRegionContent::Mixed),
    ] {
        let [x, y, z] = origin;
        let [width, height, depth] = extent;
        assert_eq!(
            view.region_content(
                &volume,
                VoxelRegion::new(
                    VoxelCoordinate::new(x, y, z),
                    VoxelExtent::new(width, height, depth)
                )
            )?,
            expected
        );
    }
    for (coordinate, expected) in [
        ([0, 80, 0], "terrain-grass"),
        ([2047, 80, 2047], "terrain-grass"),
        ([1023, 111, 1023], "terrain-grass"),
        ([767, 48, 800], "terrain-stone"),
        ([1024, 48, 800], "terrain-stone"),
        ([800, 48, 767], "terrain-stone"),
        ([800, 48, 1024], "terrain-stone"),
    ] {
        let [x, y, z] = coordinate;
        assert_eq!(
            view.region_content(
                &volume,
                VoxelRegion::new(VoxelCoordinate::new(x, y, z), VoxelExtent::new(1, 1, 1))
            )?,
            VoxelRegionContent::Uniform(VoxelValue::Occupied(VoxelMaterialId::new(expected)))
        );
    }
    Ok(())
}

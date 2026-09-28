use std::error::Error;

use super::storage_counters::{EnumerationWorkCounters, StorageWorkCounters, count_storage_work};
use super::*;

fn stone() -> VoxelValue {
    VoxelValue::Occupied(VoxelMaterialId::new("stone"))
}

fn region(origin: [i32; 3], extent: [u32; 3]) -> VoxelRegion {
    let [x, y, z] = origin;
    let [width, height, depth] = extent;
    VoxelRegion::new(
        VoxelCoordinate::new(x, y, z),
        VoxelExtent::new(width, height, depth),
    )
}

fn volume_identity() -> VoxelVolumeId {
    VoxelVolumeId::new("volume")
}

fn publish(
    extent: [u32; 3],
    batches: Vec<SparseVoxelBatch>,
) -> Result<VoxelSceneView, VoxelFrontendError> {
    let [width, height, depth] = extent;
    VoxelFrontend::new().publish_sparse(
        SparseVoxelScene::new(
            VoxelSceneId::new("cells"),
            VoxelSceneRevision::new(0),
            vec![VoxelMaterial::new(VoxelMaterialId::new("stone"), [1.0; 4])],
            vec![SparseVoxelVolume::new(
                VoxelVolumeMetadata::new(
                    volume_identity(),
                    VoxelExtent::new(width, height, depth),
                    [0.0; 3],
                    1.0,
                ),
                SparseVoxelBackground::Empty,
                batches,
            )],
        )
        .with_storage_tier(StorageTier::SparsePages),
    )
}

fn enumerate(
    view: &VoxelSceneView,
    cell_edge: u32,
) -> Result<(Vec<VoxelCell>, StorageWorkCounters), Box<dyn Error>> {
    let enumeration = view.enumerate_cells(&volume_identity(), cell_edge, 3)?;
    let (cells, counters) = count_storage_work(|| {
        enumeration
            .collect::<Result<Vec<_>, _>>()
            .map(|batches| batches.concat())
    });
    Ok((cells?, counters))
}

/// Three small occupied regions whose separation along every axis is `spacing` voxels.
fn scattered_regions(spacing: i32) -> Vec<SparseVoxelBatch> {
    [0, 1, 2]
        .into_iter()
        .map(|index| {
            let offset = index * spacing;
            SparseVoxelBatch::Fill(VoxelRegionFill::new(
                region([offset, offset + 12, offset], [16, 5, 16]),
                stone(),
            ))
        })
        .collect()
}

#[test]
fn enumeration_work_does_not_grow_with_the_space_between_occupied_regions()
-> Result<(), Box<dyn Error>> {
    let extent = [1 << 20; 3];
    for cell_edge in [4, 16, 64, 1 << 20] {
        let (near_cells, near) = enumerate(&publish(extent, scattered_regions(64))?, cell_edge)?;
        let (far_cells, far) = enumerate(
            &publish(extent, scattered_regions((1 << 19) - 64))?,
            cell_edge,
        )?;
        // Seeking the next resident brick visits page-table nodes along the search path,
        // whose number depends on how keys share branches rather than on the gaps between them.
        // A 2^48-brick key space gives depth 8, and each of the 7 seeks, including the final
        // miss, visits at most two nodes per level.
        for counters in [&near, &far] {
            assert!(
                counters.enumeration.nodes_visited <= 7 * 2 * 8,
                "{counters:?}"
            );
        }
        assert_eq!(
            EnumerationWorkCounters {
                nodes_visited: 0,
                ..near.enumeration
            },
            EnumerationWorkCounters {
                nodes_visited: 0,
                ..far.enumeration
            },
            "cell edge {cell_edge}"
        );
        assert_eq!(near_cells.len(), far_cells.len());
        // Each region straddles two bricks vertically.
        assert_eq!(near.enumeration.bricks_examined, 6);
        assert_eq!(near.enumeration.cells_emitted, near_cells.len());
        assert_eq!(near.bricks_examined, 0);
        assert_eq!(near.voxel_values_examined, 0);
    }
    let (cells, counters) = enumerate(&publish(extent, scattered_regions(64))?, 1 << 20)?;
    assert_eq!(cells.len(), 1);
    assert_eq!(counters.enumeration.peak_working_cells, 1);
    Ok(())
}

#[test]
fn working_cells_are_bounded_by_one_slab_of_cells_and_counted_apart_from_output()
-> Result<(), Box<dyn Error>> {
    let view = publish(
        [64, 32, 96],
        vec![SparseVoxelBatch::Fill(VoxelRegionFill::new(
            region([0, 0, 0], [64, 32, 96]),
            stone(),
        ))],
    )?;
    let bricks = 4 * 2 * 6;
    for (cell_edge, cells, peak_working_cells, voxel_values_examined) in [
        (1, 64 * 32 * 96, 0, 0),
        (8, 8 * 4 * 12, 0, 0),
        (16, bricks, 8, 0),
        (32, 2 * 3, 2, 0),
        (64, 2, 1, 0),
    ] {
        let (enumerated, counters) = enumerate(&view, cell_edge)?;
        assert_eq!(enumerated.len(), cells);
        assert!(
            enumerated
                .iter()
                .all(|cell| cell.content() == &VoxelRegionContent::Uniform(stone()))
        );
        assert_eq!(
            counters.enumeration,
            EnumerationWorkCounters {
                nodes_visited: counters.enumeration.nodes_visited,
                bricks_examined: bricks,
                voxel_values_examined,
                cells_emitted: cells,
                peak_working_cells,
            },
            "cell edge {cell_edge}"
        );
    }
    Ok(())
}

#[test]
fn cells_inside_mixed_bricks_read_only_that_bricks_values() -> Result<(), Box<dyn Error>> {
    let values = (0..16 * 16 * 16)
        .map(|index| {
            if index % 16 < 4 {
                stone()
            } else {
                VoxelValue::Empty
            }
        })
        .collect();
    let view = publish(
        [1 << 16; 3],
        vec![SparseVoxelBatch::Detail(DenseVoxelBatch::new(
            region([4096, 0, 4096], [16, 16, 16]),
            values,
        ))],
    )?;
    let (cells, counters) = enumerate(&view, 4)?;
    assert_eq!(cells.len(), 16);
    assert!(
        cells
            .iter()
            .all(|cell| cell.content() == &VoxelRegionContent::Uniform(stone()))
    );
    assert_eq!(counters.enumeration.bricks_examined, 1);
    // Every cell of the brick is uniform, so each reads all of its own values and nothing
    // outside the brick is read.
    assert_eq!(counters.enumeration.voxel_values_examined, 64 * 64);
    Ok(())
}

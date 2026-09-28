use std::collections::HashMap;
use voxel_frontend::*;

fn stone() -> VoxelValue {
    VoxelValue::Occupied(VoxelMaterialId::new("stone"))
}

fn grass() -> VoxelValue {
    VoxelValue::Occupied(VoxelMaterialId::new("grass"))
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
    VoxelVolumeId::new("terrain")
}

fn materials() -> Vec<VoxelMaterial> {
    ["stone", "grass"]
        .map(|identity| VoxelMaterial::new(VoxelMaterialId::new(identity), [1.0; 4]))
        .into()
}

// A non-aligned 37 x 20 x 33 volume with solid, noisy, sparse, and empty areas gives
// absent, uniform, and mixed bricks, some of them clipped at the volume bounds.
const EXTENT: [u32; 3] = [37, 20, 33];

fn terrain_value(x: u32, y: u32, z: u32) -> VoxelValue {
    if y < 8 && z >= 16 {
        stone()
    } else if y < 4 && x < 16 && z < 16 {
        grass()
    } else if (8..12).contains(&y) && x < 20 && z < 10 {
        match (x * 7 + y * 3 + z * 5) % 4 {
            0 => stone(),
            1 => grass(),
            _ => VoxelValue::Empty,
        }
    } else if (x, y, z) == (36, 19, 32) || (x, y, z) == (33, 17, 2) {
        grass()
    } else {
        VoxelValue::Empty
    }
}

fn publish(tier: StorageTier) -> Result<(VoxelFrontend, VoxelSceneView), VoxelFrontendError> {
    let [width, height, depth] = EXTENT;
    let values = (0..depth)
        .flat_map(|z| (0..height).flat_map(move |y| (0..width).map(move |x| (x, y, z))))
        .map(|(x, y, z)| terrain_value(x, y, z))
        .collect();
    let frontend = VoxelFrontend::new();
    let view = frontend.publish(
        DenseVoxelScene::new(
            VoxelSceneId::new("cells"),
            VoxelSceneRevision::new(0),
            materials(),
            vec![DenseVoxelVolume::new(
                VoxelVolumeMetadata::new(
                    volume_identity(),
                    VoxelExtent::new(width, height, depth),
                    [0.0; 3],
                    1.0,
                ),
                vec![DenseVoxelBatch::new(region([0; 3], EXTENT), values)],
            )],
        )
        .with_storage_tier(tier),
    )?;
    Ok((frontend, view))
}

type CellsByCoordinate = HashMap<[u32; 3], (VoxelRegion, VoxelRegionContent)>;

/// Collects every enumerated cell, failing on duplicates or oversized batches.
fn enumerate(
    view: &VoxelSceneView,
    cell_edge: u32,
    batch_capacity: usize,
) -> Result<CellsByCoordinate, Box<dyn std::error::Error>> {
    collect(
        view.enumerate_cells(&volume_identity(), cell_edge, batch_capacity)?,
        batch_capacity,
    )
}

fn collect(
    enumeration: VoxelCellEnumeration,
    batch_capacity: usize,
) -> Result<CellsByCoordinate, Box<dyn std::error::Error>> {
    let mut cells = HashMap::new();
    for batch in enumeration {
        let batch = batch?;
        assert!(!batch.is_empty());
        assert!(batch.len() <= batch_capacity);
        for cell in batch {
            let coordinate = cell.coordinate().components();
            let previous = cells.insert(coordinate, (cell.region(), cell.content().clone()));
            assert!(previous.is_none(), "cell {coordinate:?} emitted twice");
        }
    }
    Ok(cells)
}

/// Classifies every cell of the grid one by one, keeping only the non-empty cells.
fn expected_cells(
    view: &VoxelSceneView,
    extent: [u32; 3],
    cell_edge: u32,
) -> Result<CellsByCoordinate, Box<dyn std::error::Error>> {
    let [cells_x, cells_y, cells_z] = extent.map(|dimension| dimension.div_ceil(cell_edge));
    let mut cells = HashMap::new();
    for z in 0..cells_z {
        for y in 0..cells_y {
            for x in 0..cells_x {
                let origin = [x, y, z].map(|cell| cell * cell_edge);
                let clipped = [0, 1, 2].map(|axis| cell_edge.min(extent[axis] - origin[axis]));
                let cell_region = region(origin.map(|value| value as i32), clipped);
                let content = view.region_content(&volume_identity(), cell_region)?;
                if content != VoxelRegionContent::Uniform(VoxelValue::Empty) {
                    cells.insert([x, y, z], (cell_region, content));
                }
            }
        }
    }
    Ok(cells)
}

#[test]
fn enumeration_emits_every_non_empty_cell_once_on_either_tier()
-> Result<(), Box<dyn std::error::Error>> {
    let (_, dense) = publish(StorageTier::Dense)?;
    let (_, sparse) = publish(StorageTier::SparsePages)?;
    for cell_edge in [1, 2, 4, 8, 16, 32, 64, 1 << 31] {
        let expected = expected_cells(&dense, EXTENT, cell_edge)?;
        assert!(!expected.is_empty());
        for view in [&dense, &sparse] {
            let cells = enumerate(view, cell_edge, 7)?;
            assert_eq!(cells, expected, "cell edge {cell_edge}");
            for (cell_region, content) in cells.values() {
                assert_eq!(
                    &view.region_content(&volume_identity(), *cell_region)?,
                    content
                );
            }
        }
    }
    Ok(())
}

#[test]
fn edge_cells_keep_their_grid_coordinate_and_report_their_clipped_region()
-> Result<(), Box<dyn std::error::Error>> {
    for tier in [StorageTier::Dense, StorageTier::SparsePages] {
        let (_, view) = publish(tier)?;
        let cells = enumerate(&view, 32, 100)?;
        assert_eq!(
            cells.get(&[1, 0, 1]),
            Some(&(region([32, 0, 32], [5, 20, 1]), VoxelRegionContent::Mixed))
        );
        assert_eq!(
            cells.get(&[0, 0, 0]).map(|(cell_region, _)| *cell_region),
            Some(region([0, 0, 0], [32, 20, 32]))
        );
        let cells = enumerate(&view, 8, 100)?;
        assert_eq!(
            cells.get(&[4, 2, 4]),
            Some(&(region([32, 16, 32], [5, 4, 1]), VoxelRegionContent::Mixed))
        );
        assert_eq!(
            cells.get(&[0, 0, 2]),
            Some(&(
                region([0, 0, 16], [8, 8, 8]),
                VoxelRegionContent::Uniform(stone())
            ))
        );
        let cells = enumerate(&view, 1 << 31, 1)?;
        assert_eq!(
            cells.get(&[0, 0, 0]),
            Some(&(region([0; 3], EXTENT), VoxelRegionContent::Mixed))
        );
    }
    Ok(())
}

#[test]
fn batches_never_exceed_the_capacity_and_zero_capacity_is_rejected()
-> Result<(), Box<dyn std::error::Error>> {
    for tier in [StorageTier::Dense, StorageTier::SparsePages] {
        let (_, view) = publish(tier)?;
        let expected = enumerate(&view, 4, usize::MAX)?;
        for batch_capacity in [1, 2, 5, expected.len(), expected.len() + 1] {
            let enumeration = view.enumerate_cells(&volume_identity(), 4, batch_capacity)?;
            let batch_lengths: Vec<_> = enumeration
                .map(|batch| batch.map(|batch| batch.len()))
                .collect::<Result<_, _>>()?;
            assert_eq!(batch_lengths.iter().sum::<usize>(), expected.len());
            assert!(batch_lengths.iter().all(|length| *length <= batch_capacity));
            // Every batch but the last is full, so a small capacity cannot truncate output.
            if let Some((_, full)) = batch_lengths.split_last() {
                assert!(full.iter().all(|length| *length == batch_capacity));
            }
        }
        assert!(matches!(
            view.enumerate_cells(&volume_identity(), 4, 0),
            Err(VoxelFrontendError::ZeroCellBatchCapacity { identity }) if identity == volume_identity()
        ));
    }
    Ok(())
}

#[test]
fn cell_edges_that_are_not_powers_of_two_and_unknown_volumes_are_rejected()
-> Result<(), Box<dyn std::error::Error>> {
    for tier in [StorageTier::Dense, StorageTier::SparsePages] {
        let (_, view) = publish(tier)?;
        for cell_edge in [0, 3, 12, 48, u32::MAX] {
            let error = match view.enumerate_cells(&volume_identity(), cell_edge, 4) {
                Ok(_) => return Err(format!("cell edge {cell_edge} was accepted").into()),
                Err(error) => error,
            };
            assert!(
                matches!(
                    &error,
                    VoxelFrontendError::InvalidCellEdge { identity, cell_edge: rejected }
                        if identity == &volume_identity() && *rejected == cell_edge
                ),
                "{error}"
            );
            assert!(error.to_string().contains("terrain"));
        }
        assert!(matches!(
            view.enumerate_cells(&VoxelVolumeId::new("missing"), 4, 4),
            Err(VoxelFrontendError::UnknownVolumeIdentity { .. })
        ));
    }
    Ok(())
}

#[test]
fn a_retained_enumeration_keeps_producing_its_original_views_cells()
-> Result<(), Box<dyn std::error::Error>> {
    fn assert_send<T: Send + 'static>(value: T) -> T {
        value
    }
    for tier in [StorageTier::Dense, StorageTier::SparsePages] {
        let (frontend, view) = publish(tier)?;
        let expected = enumerate(&view, 8, 3)?;
        let mut retained = assert_send(view.enumerate_cells(&volume_identity(), 8, 3)?);
        let first_batch = retained.next().ok_or("no first batch")??;
        drop(view);
        // Clear a solid region, fill empty space, and mix a uniform cell.
        let edits = (16..24)
            .flat_map(|z| (0..8).flat_map(move |y| (0..8).map(move |x| (x, y, z))))
            .map(|(x, y, z)| {
                VoxelEdit::new(
                    volume_identity(),
                    VoxelCoordinate::new(x, y, z),
                    VoxelValue::Empty,
                )
            })
            .chain([
                VoxelEdit::new(volume_identity(), VoxelCoordinate::new(30, 18, 20), stone()),
                VoxelEdit::new(volume_identity(), VoxelCoordinate::new(9, 1, 17), grass()),
            ])
            .collect();
        let outcome = frontend.edit(VoxelEditCommand::from_edits(edits))?;
        let edited = enumerate(outcome.view(), 8, 3)?;
        assert!(!edited.contains_key(&[0, 0, 2]));
        assert_eq!(
            edited.get(&[3, 2, 2]),
            Some(&(region([24, 16, 16], [8, 4, 8]), VoxelRegionContent::Mixed))
        );
        assert_eq!(
            edited.get(&[1, 0, 2]).map(|(_, content)| content),
            Some(&VoxelRegionContent::Mixed)
        );

        let mut retained_cells = collect(retained, 3)?;
        for cell in first_batch {
            retained_cells.insert(
                cell.coordinate().components(),
                (cell.region(), cell.content().clone()),
            );
        }
        assert_eq!(retained_cells, expected);
    }
    Ok(())
}

#[test]
fn sparse_input_enumerates_identically_on_either_tier() -> Result<(), Box<dyn std::error::Error>> {
    let extent = [70, 40, 50];
    let batches = || {
        vec![
            SparseVoxelBatch::Fill(VoxelRegionFill::new(
                region([0, 0, 0], [64, 16, 48]),
                stone(),
            )),
            SparseVoxelBatch::Fill(VoxelRegionFill::new(region([3, 20, 5], [3, 5, 7]), grass())),
            SparseVoxelBatch::Fill(VoxelRegionFill::new(
                region([64, 0, 0], [6, 40, 50]),
                VoxelValue::Empty,
            )),
            SparseVoxelBatch::Detail(DenseVoxelBatch::new(
                region([40, 30, 40], [2, 1, 1]),
                vec![grass(), stone()],
            )),
        ]
    };
    let views = [StorageTier::Dense, StorageTier::SparsePages].map(|tier| {
        let [width, height, depth] = extent;
        VoxelFrontend::new().publish_sparse(
            SparseVoxelScene::new(
                VoxelSceneId::new("cells"),
                VoxelSceneRevision::new(0),
                materials(),
                vec![SparseVoxelVolume::new(
                    VoxelVolumeMetadata::new(
                        volume_identity(),
                        VoxelExtent::new(width, height, depth),
                        [0.0; 3],
                        1.0,
                    ),
                    SparseVoxelBackground::Empty,
                    batches(),
                )],
            )
            .with_storage_tier(tier),
        )
    });
    let [dense, sparse] = views;
    let (dense, sparse) = (dense?, sparse?);
    for cell_edge in [1, 4, 16, 32, 128] {
        let expected = expected_cells(&dense, extent, cell_edge)?;
        assert_eq!(enumerate(&dense, cell_edge, 5)?, expected);
        assert_eq!(enumerate(&sparse, cell_edge, 5)?, expected);
    }
    assert_eq!(
        enumerate(&sparse, 16, 5)?.get(&[1, 0, 1]),
        Some(&(
            region([16, 0, 16], [16, 16, 16]),
            VoxelRegionContent::Uniform(stone())
        ))
    );
    Ok(())
}

#[test]
fn the_largest_cell_edges_and_coordinates_are_enumerated_without_overflow()
-> Result<(), Box<dyn std::error::Error>> {
    let axis = 1u32 << 31;
    let far = i32::MAX;
    let view = VoxelFrontend::new().publish_sparse(
        SparseVoxelScene::new(
            VoxelSceneId::new("cells"),
            VoxelSceneRevision::new(0),
            materials(),
            vec![SparseVoxelVolume::new(
                VoxelVolumeMetadata::new(
                    volume_identity(),
                    VoxelExtent::new(axis, 1, 1),
                    [0.0; 3],
                    1.0,
                ),
                SparseVoxelBackground::Empty,
                vec![SparseVoxelBatch::Fill(VoxelRegionFill::new(
                    region([far, 0, 0], [1, 1, 1]),
                    stone(),
                ))],
            )],
        )
        .with_storage_tier(StorageTier::SparsePages),
    )?;
    let uniform = VoxelRegionContent::Uniform(stone());
    for (cell_edge, coordinate, expected) in [
        (
            1,
            [axis - 1, 0, 0],
            (region([far, 0, 0], [1, 1, 1]), uniform.clone()),
        ),
        (
            16,
            [(axis - 1) / 16, 0, 0],
            (
                region([far - 15, 0, 0], [16, 1, 1]),
                VoxelRegionContent::Mixed,
            ),
        ),
        (
            1 << 30,
            [1, 0, 0],
            (
                region([1 << 30, 0, 0], [1 << 30, 1, 1]),
                VoxelRegionContent::Mixed,
            ),
        ),
        (
            1 << 31,
            [0, 0, 0],
            (region([0, 0, 0], [axis, 1, 1]), VoxelRegionContent::Mixed),
        ),
    ] {
        let cells = enumerate(&view, cell_edge, usize::MAX)?;
        assert_eq!(
            cells,
            HashMap::from([(coordinate, expected)]),
            "{cell_edge}"
        );
    }
    Ok(())
}

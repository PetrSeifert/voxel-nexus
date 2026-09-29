use super::storage_counters::count_storage_work;
use super::storage_tier::{Brick, SparseStorage};
use super::*;

fn publish_sparse(
    extent: VoxelExtent,
    value: impl Fn(u32, u32, u32) -> VoxelValue,
) -> Result<(VoxelFrontend, VoxelSceneView), VoxelFrontendError> {
    let [width, height, depth] = extent.dimensions();
    let values = (0..depth)
        .flat_map(|z| (0..height).flat_map(move |y| (0..width).map(move |x| (x, y, z))))
        .map(|(x, y, z)| value(x, y, z))
        .collect();
    let frontend = VoxelFrontend::new();
    let view = frontend.publish(
        DenseVoxelScene::new(
            VoxelSceneId::new("bricks"),
            VoxelSceneRevision::new(0),
            vec![VoxelMaterial::new(VoxelMaterialId::new("stone"), [1.0; 4])],
            vec![DenseVoxelVolume::new(
                VoxelVolumeMetadata::new(VoxelVolumeId::new("volume"), extent, [0.0; 3], 1.0),
                vec![DenseVoxelBatch::new(
                    VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), extent),
                    values,
                )],
            )],
        )
        .with_storage_tier(StorageTier::SparsePages),
    )?;
    Ok((frontend, view))
}

fn sparse_storage(view: &VoxelSceneView) -> Result<&SparseStorage, Box<dyn std::error::Error>> {
    Ok(view
        .published
        .volumes
        .get(&VoxelVolumeId::new("volume"))
        .ok_or("missing volume")?
        .as_any()
        .downcast_ref::<SparseStorage>()
        .ok_or("expected sparse storage")?)
}

fn stone() -> VoxelValue {
    VoxelValue::Occupied(VoxelMaterialId::new("stone"))
}

fn edit_row(
    frontend: &VoxelFrontend,
    x_coordinates: impl Iterator<Item = i32>,
    value: VoxelValue,
) -> Result<VoxelSceneView, VoxelFrontendError> {
    let outcome = frontend.edit(VoxelEditCommand::from_edits(
        x_coordinates
            .map(|x| {
                VoxelEdit::new(
                    VoxelVolumeId::new("volume"),
                    VoxelCoordinate::new(x, 0, 0),
                    value.clone(),
                )
            })
            .collect(),
    ))?;
    Ok(outcome.view().clone())
}

#[derive(Debug)]
enum ExpectedBrick {
    Absent,
    Uniform,
    Mixed,
}

fn assert_brick(
    view: &VoxelSceneView,
    key: usize,
    expected: ExpectedBrick,
) -> Result<(), Box<dyn std::error::Error>> {
    let brick = sparse_storage(view)?.brick(key);
    let matches = matches!(
        (&expected, brick),
        (ExpectedBrick::Absent, None)
            | (ExpectedBrick::Uniform, Some(Brick::Uniform(_)))
            | (ExpectedBrick::Mixed, Some(Brick::Mixed(_)))
    );
    if matches {
        Ok(())
    } else {
        Err(format!("brick {key} is not {expected:?}").into())
    }
}

#[test]
fn edits_copy_nodes_bounded_by_tree_depth_not_by_stored_bricks()
-> Result<(), Box<dyn std::error::Error>> {
    // A 1 x 1 x 1-brick-thick row keeps the input small while still spanning
    // several page-table levels.
    for (brick_count, expected_depth) in [(64usize, 1usize), (4096, 2)] {
        let width = u32::try_from(brick_count * 16)?;
        let last_brick = i32::try_from(brick_count - 1)?;
        for stored_bricks in [1, brick_count / 2, brick_count - 1] {
            let (frontend, initial) = publish_sparse(VoxelExtent::new(width, 1, 1), |x, _, _| {
                if (x as usize) < stored_bricks * 16 && x % 2 == 0 {
                    stone()
                } else {
                    VoxelValue::Empty
                }
            })?;
            let storage = sparse_storage(&initial)?;
            assert_eq!(storage.depth(), expected_depth);
            assert_eq!(storage.stored_bricks(), stored_bricks);
            assert_brick(&initial, 0, ExpectedBrick::Mixed)?;

            let (modified, modification) =
                count_storage_work(|| edit_row(&frontend, 0..1, VoxelValue::Empty));
            let modified = modified?;
            assert_brick(&modified, 0, ExpectedBrick::Mixed)?;

            let (normalized, normalization) =
                count_storage_work(|| edit_row(&frontend, 0..16, stone()));
            let normalized = normalized?;
            assert_brick(&normalized, 0, ExpectedBrick::Uniform)?;

            let (removed, removal) =
                count_storage_work(|| edit_row(&frontend, 0..16, VoxelValue::Empty));
            let removed = removed?;
            assert_brick(&removed, 0, ExpectedBrick::Absent)?;

            let (inserted, insertion) = count_storage_work(|| {
                edit_row(&frontend, (last_brick * 16)..(last_brick * 16 + 1), stone())
            });
            let inserted = inserted?;
            assert_brick(&inserted, brick_count - 1, ExpectedBrick::Mixed)?;

            // Mixed-to-empty normalization of a brick that still has other bricks beside it.
            let (cleared, clearing) = count_storage_work(|| {
                edit_row(
                    &frontend,
                    (last_brick * 16)..(last_brick * 16 + 1),
                    VoxelValue::Empty,
                )
            });
            let cleared = cleared?;
            assert_brick(&cleared, brick_count - 1, ExpectedBrick::Absent)?;

            assert_brick(&initial, 0, ExpectedBrick::Mixed)?;
            assert_brick(&normalized, 0, ExpectedBrick::Uniform)?;
            assert_brick(&inserted, brick_count - 1, ExpectedBrick::Mixed)?;
            assert_eq!(
                initial.region_content(
                    &VoxelVolumeId::new("volume"),
                    VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), VoxelExtent::new(1, 1, 1))
                )?,
                VoxelRegionContent::Uniform(stone())
            );

            for counter in [modification, normalization, removal, insertion, clearing] {
                assert_eq!(counter.copied_brick_payloads, 1);
                assert!(counter.copied_nodes >= 1);
                assert!(counter.copied_nodes <= expected_depth);
            }
        }
    }
    Ok(())
}

#[test]
fn one_command_copies_each_edited_brick_payload_once() -> Result<(), Box<dyn std::error::Error>> {
    let (frontend, _) = publish_sparse(VoxelExtent::new(64, 1, 1), |_, _, _| VoxelValue::Empty)?;
    let (view, counters) = count_storage_work(|| edit_row(&frontend, (0..64).step_by(3), stone()));
    let view = view?;
    assert_eq!(counters.copied_brick_payloads, 4);
    for key in 0..4 {
        assert_brick(&view, key, ExpectedBrick::Mixed)?;
    }
    Ok(())
}

#[test]
fn classifying_uniform_and_absent_bricks_examines_no_voxel_values()
-> Result<(), Box<dyn std::error::Error>> {
    // Three bricks per axis with partial edge bricks: y < 16 is solid, 16..20 is
    // a mixed layer, and everything above is empty.
    let extent = VoxelExtent::new(40, 45, 33);
    let (_, view) = publish_sparse(
        extent,
        |_, y, _| {
            if y < 20 { stone() } else { VoxelValue::Empty }
        },
    )?;
    let volume = VoxelVolumeId::new("volume");
    for (origin, size, expected, bricks) in [
        (
            (0, 0, 0),
            (40, 16, 33),
            VoxelRegionContent::Uniform(stone()),
            9,
        ),
        (
            (3, 2, 5),
            (28, 10, 20),
            VoxelRegionContent::Uniform(stone()),
            4,
        ),
        (
            (0, 32, 0),
            (40, 13, 33),
            VoxelRegionContent::Uniform(VoxelValue::Empty),
            0,
        ),
        (
            (-5, 40, 20),
            (50, 20, 10),
            VoxelRegionContent::Uniform(VoxelValue::Empty),
            0,
        ),
        (
            (100, 100, 100),
            (4, 4, 4),
            VoxelRegionContent::Uniform(VoxelValue::Empty),
            0,
        ),
    ] {
        let region = VoxelRegion::new(
            VoxelCoordinate::new(origin.0, origin.1, origin.2),
            VoxelExtent::new(size.0, size.1, size.2),
        );
        let (content, counters) = count_storage_work(|| view.region_content(&volume, region));
        assert_eq!(content?, expected, "{region:?}");
        assert_eq!(counters.voxel_values_examined, 0, "{region:?}");
        assert_eq!(counters.bricks_examined, bricks, "{region:?}");
    }

    let region = VoxelRegion::new(VoxelCoordinate::new(5, 17, 5), VoxelExtent::new(4, 2, 4));
    let (content, counters) = count_storage_work(|| view.region_content(&volume, region));
    assert_eq!(content?, VoxelRegionContent::Uniform(stone()));
    assert_eq!(counters.bricks_examined, 1);
    assert_eq!(counters.voxel_values_examined, 32);

    let region = VoxelRegion::new(VoxelCoordinate::new(5, 0, 5), VoxelExtent::new(4, 30, 4));
    let (content, counters) = count_storage_work(|| view.region_content(&volume, region));
    assert_eq!(content?, VoxelRegionContent::Mixed);
    assert!(counters.voxel_values_examined <= 4 * 14 * 4);
    Ok(())
}

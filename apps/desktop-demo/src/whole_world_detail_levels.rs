//! PROTOTYPE (issue #137): direct-vote detail levels shared by the throwaway profiles.
use super::streamed_fixture_recipe::{EDGE, volume_identity};
use voxel_frontend::{
    DenseVoxelBatch, SparseVoxelBackground, SparseVoxelBatch, SparseVoxelVolume, StorageTier,
    VoxelCoordinate, VoxelExtent, VoxelMaterialId, VoxelRegion, VoxelSceneView, VoxelValue,
    VoxelVolumeMetadata,
};

pub const LEVEL_EDGES: [u32; 4] = [64, 32, 16, 8];
const BLOCK: u32 = 8;

/// Direct vote: any occupied value makes the coarse voxel solid, empty values do not vote,
/// and a tie goes to the lowest Voxel Material identity.
pub fn vote(values: impl Iterator<Item = VoxelValue>) -> VoxelValue {
    let mut tallies: Vec<(VoxelMaterialId, u32)> = Vec::new();
    for value in values {
        if let VoxelValue::Occupied(material) = value {
            match tallies
                .iter_mut()
                .find(|(identity, _)| *identity == material)
            {
                Some((_, count)) => *count += 1,
                None => tallies.push((material, 1)),
            }
        }
    }
    tallies
        .into_iter()
        .max_by(|(left, left_count), (right, right_count)| {
            // VoxelMaterialId has no order yet; the prototype stands in its lexicographic identity.
            left_count
                .cmp(right_count)
                .then_with(|| format!("{right:?}").cmp(&format!("{left:?}")))
        })
        .map_or(VoxelValue::Empty, |(material, _)| {
            VoxelValue::Occupied(material)
        })
}

/// Coarse values for every level below full detail, in x-fastest/y/z order.
pub fn downsample(view: &VoxelSceneView, x: u32, z: u32) -> Result<Vec<Vec<VoxelValue>>, String> {
    let identity = volume_identity(x, z);
    let mut levels = LEVEL_EDGES[1..]
        .iter()
        .map(|&edge| vec![VoxelValue::Empty; (edge * edge * edge) as usize])
        .collect::<Vec<_>>();
    let blocks = EDGE / BLOCK;
    for block_z in 0..blocks {
        for block_y in 0..blocks {
            for block_x in 0..blocks {
                let origin = [block_x * BLOCK, block_y * BLOCK, block_z * BLOCK];
                let samples = view
                    .read_region(
                        &identity,
                        VoxelRegion::new(
                            VoxelCoordinate::new(
                                origin[0] as i32,
                                origin[1] as i32,
                                origin[2] as i32,
                            ),
                            VoxelExtent::new(BLOCK, BLOCK, BLOCK),
                        ),
                    )
                    .map_err(|error| error.to_string())?;
                for (level, &edge) in levels.iter_mut().zip(&LEVEL_EDGES[1..]) {
                    let scale = EDGE / edge;
                    let cells = BLOCK / scale;
                    for cell_z in 0..cells {
                        for cell_y in 0..cells {
                            for cell_x in 0..cells {
                                let covered = samples.iter().filter_map(|sample| {
                                    let [sx, sy, sz] = sample.coordinate().components();
                                    let local = [
                                        sx as u32 - origin[0],
                                        sy as u32 - origin[1],
                                        sz as u32 - origin[2],
                                    ];
                                    (local[0] / scale == cell_x
                                        && local[1] / scale == cell_y
                                        && local[2] / scale == cell_z)
                                        .then(|| sample.value().clone())
                                });
                                let coarse = [
                                    origin[0] / scale + cell_x,
                                    origin[1] / scale + cell_y,
                                    origin[2] / scale + cell_z,
                                ];
                                let index =
                                    (coarse[0] + edge * (coarse[1] + edge * coarse[2])) as usize;
                                let slot = level
                                    .get_mut(index)
                                    .expect("coarse coordinates lie inside the level");
                                *slot = vote(covered);
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(levels)
}

pub fn coarse_volume(x: u32, z: u32, edge: u32, values: Vec<VoxelValue>) -> SparseVoxelVolume {
    SparseVoxelVolume::new(
        VoxelVolumeMetadata::new(
            volume_identity(x, z),
            VoxelExtent::new(edge, edge, edge),
            [(x * EDGE) as f32, 0.0, (z * EDGE) as f32],
            (EDGE / edge) as f32,
        ),
        SparseVoxelBackground::Empty,
        vec![SparseVoxelBatch::Detail(DenseVoxelBatch::new(
            VoxelRegion::new(
                VoxelCoordinate::new(0, 0, 0),
                VoxelExtent::new(edge, edge, edge),
            ),
            values,
        ))],
    )
    .with_storage_tier(StorageTier::SparsePages)
}

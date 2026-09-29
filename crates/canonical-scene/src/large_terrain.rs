use voxel_frontend::{
    DenseVoxelBatch, SparseVoxelBackground, SparseVoxelBatch, SparseVoxelScene, SparseVoxelVolume,
    StorageTier, VoxelCoordinate, VoxelExtent, VoxelMaterial, VoxelMaterialId, VoxelRegion,
    VoxelRegionFill, VoxelSceneId, VoxelSceneRevision, VoxelValue, VoxelVolumeId,
    VoxelVolumeMetadata,
};

use crate::CanonicalSceneError;

const TILE_EDGE: i32 = 32;
const HORIZONTAL_EDGE: i32 = 2048;

pub struct LargeTerrain {
    batches: Vec<SparseVoxelBatch>,
    fingerprint: u64,
    fill_batches: usize,
    detail_values: usize,
}

impl LargeTerrain {
    /// FNV-1a of the ordered batch stream: kind, little-endian i32 origin and extent,
    /// then material codes (0 = air, 1 = stone, 2 = grass), x fastest.
    pub fn content_fingerprint(&self) -> u64 {
        self.fingerprint
    }

    pub fn fill_batch_count(&self) -> usize {
        self.fill_batches
    }

    pub fn detail_batch_count(&self) -> usize {
        self.batches.len() - self.fill_batches
    }

    pub fn detail_value_count(&self) -> usize {
        self.detail_values
    }

    pub fn into_scene(self) -> SparseVoxelScene {
        SparseVoxelScene::new(
            VoxelSceneId::new("large-terrain-v1"),
            VoxelSceneRevision::new(1),
            vec![
                VoxelMaterial::new(
                    VoxelMaterialId::new("terrain-stone"),
                    [0.35, 0.30, 0.25, 1.0],
                ),
                VoxelMaterial::new(
                    VoxelMaterialId::new("terrain-grass"),
                    [0.12, 0.55, 0.18, 1.0],
                ),
            ],
            vec![
                SparseVoxelVolume::new(
                    VoxelVolumeMetadata::new(
                        VoxelVolumeId::new("large-terrain"),
                        VoxelExtent::new(HORIZONTAL_EDGE as u32, 256, HORIZONTAL_EDGE as u32),
                        [0.0; 3],
                        1.0,
                    ),
                    SparseVoxelBackground::Empty,
                    self.batches,
                )
                .with_storage_tier(StorageTier::SparsePages),
            ],
        )
    }

    fn hash(&mut self, bytes: impl IntoIterator<Item = u8>) {
        for byte in bytes {
            self.fingerprint = (self.fingerprint ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3);
        }
    }

    fn batch_region(&mut self, kind: u8, x: i32, z: i32, bottom: i32, top: i32) -> VoxelRegion {
        self.hash([kind]);
        self.hash(
            [x, bottom, z, TILE_EDGE, top - bottom, TILE_EDGE]
                .into_iter()
                .flat_map(i32::to_le_bytes),
        );
        VoxelRegion::new(
            VoxelCoordinate::new(x, bottom, z),
            VoxelExtent::new(TILE_EDGE as u32, (top - bottom) as u32, TILE_EDGE as u32),
        )
    }

    fn fill(&mut self, x: i32, z: i32, bottom: i32, top: i32, stone: &VoxelValue) {
        let region = self.batch_region(0, x, z, bottom, top);
        self.hash([1]);
        self.batches
            .push(SparseVoxelBatch::Fill(VoxelRegionFill::new(
                region,
                stone.clone(),
            )));
        self.fill_batches += 1;
    }
}

/// Builds a fixed integer heightfield tile by tile, with a 256 x 32 x 256 enclosed cavity.
/// Only the surface band has dense values; all air above it is omitted.
pub fn generate_large_terrain() -> Result<LargeTerrain, CanonicalSceneError> {
    let mut terrain = LargeTerrain {
        batches: Vec::new(),
        fingerprint: 0xcbf2_9ce4_8422_2325,
        fill_batches: 0,
        detail_values: 0,
    };
    terrain
        .batches
        .try_reserve_exact(8256)
        .map_err(CanonicalSceneError::Allocation)?;
    let stone = VoxelValue::Occupied(VoxelMaterialId::new("terrain-stone"));
    let grass = VoxelValue::Occupied(VoxelMaterialId::new("terrain-grass"));
    for z in (0..HORIZONTAL_EDGE).step_by(TILE_EDGE as usize) {
        for x in (0..HORIZONTAL_EDGE).step_by(TILE_EDGE as usize) {
            let heights = [
                height(x, z),
                height(x + TILE_EDGE - 1, z),
                height(x, z + TILE_EDGE - 1),
                height(x + TILE_EDGE - 1, z + TILE_EDGE - 1),
            ];
            // Tiles end at the central ridge, so corner extrema bound every column in a tile.
            let minimum = *heights.iter().min().expect("a tile has four corners");
            let maximum = *heights.iter().max().expect("a tile has four corners");
            if (768..1024).contains(&x) && (768..1024).contains(&z) {
                terrain.fill(x, z, 0, 32, &stone);
                terrain.fill(x, z, 64, minimum, &stone);
            } else {
                terrain.fill(x, z, 0, minimum, &stone);
            }
            let region = terrain.batch_region(1, x, z, minimum, maximum + 1);
            let count = (TILE_EDGE * TILE_EDGE * (maximum + 1 - minimum)) as usize;
            let mut values = Vec::new();
            values
                .try_reserve_exact(count)
                .map_err(CanonicalSceneError::Allocation)?;
            for local_z in z..z + TILE_EDGE {
                for y in minimum..=maximum {
                    for local_x in x..x + TILE_EDGE {
                        let surface = height(local_x, local_z);
                        let (code, value) = if y > surface {
                            (0, VoxelValue::Empty)
                        } else if y == surface {
                            (2, grass.clone())
                        } else {
                            (1, stone.clone())
                        };
                        terrain.hash([code]);
                        values.push(value);
                    }
                }
            }
            terrain.detail_values += values.len();
            terrain
                .batches
                .push(SparseVoxelBatch::Detail(DenseVoxelBatch::new(
                    region, values,
                )));
        }
    }
    Ok(terrain)
}

fn height(x: i32, z: i32) -> i32 {
    80 + (x.min(HORIZONTAL_EDGE - 1 - x) + z.min(HORIZONTAL_EDGE - 1 - z)) / 64
}

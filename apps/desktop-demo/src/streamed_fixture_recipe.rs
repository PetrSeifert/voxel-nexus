use voxel_frontend::{
    DenseVoxelBatch, SparseVoxelBackground, SparseVoxelBatch, SparseVoxelScene, SparseVoxelVolume,
    StorageTier, VoxelCoordinate, VoxelEdit, VoxelEditCommand, VoxelExtent, VoxelMaterial,
    VoxelMaterialId, VoxelRegion, VoxelRegionFill, VoxelSceneId, VoxelSceneRevision, VoxelValue,
    VoxelVolumeId, VoxelVolumeMetadata,
};

pub const EDGE: u32 = 64;
pub const EDIT_COORDINATES: [[i32; 3]; 3] = [[4, 40, 4], [32, 20, 32], [20, 12, 20]];

pub fn volume_identity(x: u32, z: u32) -> VoxelVolumeId {
    VoxelVolumeId::new(format!("volume-{x:02}-{z:02}"))
}

pub fn material(code: u8) -> VoxelValue {
    match code {
        0 => VoxelValue::Empty,
        1 => VoxelValue::Occupied(VoxelMaterialId::new("stone")),
        _ => VoxelValue::Occupied(VoxelMaterialId::new("grass")),
    }
}

pub fn generated_code(x: i32, y: i32, z: i32) -> u8 {
    let height = 24 + (x / 8 + z / 8) % 9;
    if y > height || ((16..24).contains(&x) && (8..16).contains(&y) && (16..24).contains(&z)) {
        0
    } else if y == height {
        2
    } else {
        1
    }
}

#[allow(dead_code)]
pub fn fingerprint(edited: bool) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for z in 0..64 {
        for y in 0..64 {
            for x in 0..64 {
                let code = if edited {
                    EDIT_COORDINATES
                        .iter()
                        .position(|coordinate| *coordinate == [x, y, z])
                        .and_then(|index| [1, 0, 2].get(index).copied())
                        .unwrap_or_else(|| generated_code(x, y, z))
                } else {
                    generated_code(x, y, z)
                };
                hash = (hash ^ u64::from(code)).wrapping_mul(0x100_0000_01b3);
            }
        }
    }
    hash
}

#[allow(dead_code)]
pub fn scene(coordinates: &[(u32, u32)]) -> SparseVoxelScene {
    scene_contents(coordinates, false)
}

#[allow(dead_code)]
pub fn scene_contents(coordinates: &[(u32, u32)], edited: bool) -> SparseVoxelScene {
    let volumes = coordinates
        .iter()
        .map(|&(x, z)| volume_contents(x, z, edited))
        .collect();
    SparseVoxelScene::new(
        VoxelSceneId::new("streamed-qualification-v1"),
        VoxelSceneRevision::new(1),
        materials(),
        volumes,
    )
}

pub fn volume_contents(x: u32, z: u32, edited: bool) -> SparseVoxelVolume {
    let mut batches = Vec::new();
    for local_z in 0..64 {
        for local_x in 0..64 {
            let height = 24 + (local_x / 8 + local_z / 8) % 9;
            for (bottom, top) in if (16..24).contains(&local_x) && (16..24).contains(&local_z) {
                vec![(0, 8), (16, height)]
            } else {
                vec![(0, height)]
            } {
                let intervals =
                    if edited && local_x == 32 && local_z == 32 && bottom <= 20 && top > 20 {
                        vec![(bottom, 20), (21, top)]
                    } else {
                        vec![(bottom, top)]
                    };
                for (bottom, top) in intervals {
                    batches.push(SparseVoxelBatch::Fill(VoxelRegionFill::new(
                        VoxelRegion::new(
                            VoxelCoordinate::new(local_x, bottom, local_z),
                            VoxelExtent::new(1, (top - bottom) as u32, 1),
                        ),
                        material(1),
                    )));
                }
            }
            batches.push(SparseVoxelBatch::Detail(DenseVoxelBatch::new(
                VoxelRegion::new(
                    VoxelCoordinate::new(local_x, height, local_z),
                    VoxelExtent::new(1, 1, 1),
                ),
                vec![material(2)],
            )));
        }
    }
    if edited {
        for (coordinate, code) in [([4, 40, 4], 1), ([20, 12, 20], 2)] {
            let [local_x, y, local_z] = coordinate;
            batches.push(SparseVoxelBatch::Fill(VoxelRegionFill::new(
                VoxelRegion::new(
                    VoxelCoordinate::new(local_x, y, local_z),
                    VoxelExtent::new(1, 1, 1),
                ),
                material(code),
            )));
        }
    }
    SparseVoxelVolume::new(
        VoxelVolumeMetadata::new(
            volume_identity(x, z),
            VoxelExtent::new(EDGE, EDGE, EDGE),
            [(x * EDGE) as f32, 0.0, (z * EDGE) as f32],
            1.0,
        ),
        SparseVoxelBackground::Empty,
        batches,
    )
    .with_storage_tier(StorageTier::SparsePages)
}

pub fn materials() -> Vec<VoxelMaterial> {
    vec![
        VoxelMaterial::new(VoxelMaterialId::new("stone"), [0.35, 0.30, 0.25, 1.0]),
        VoxelMaterial::new(VoxelMaterialId::new("grass"), [0.12, 0.55, 0.18, 1.0]),
    ]
}

pub fn catalog(side: u32) -> Vec<VoxelVolumeMetadata> {
    (0..side)
        .flat_map(|z| {
            (0..side).map(move |x| {
                VoxelVolumeMetadata::new(
                    volume_identity(x, z),
                    VoxelExtent::new(EDGE, EDGE, EDGE),
                    [(x * EDGE) as f32, 0.0, (z * EDGE) as f32],
                    1.0,
                )
            })
        })
        .collect()
}

#[allow(dead_code)]
pub fn edit(x: u32, z: u32, restore: bool) -> VoxelEditCommand {
    VoxelEditCommand::from_edits(
        EDIT_COORDINATES
            .iter()
            .zip([1, 0, 2])
            .map(|(&[local_x, y, local_z], changed)| {
                VoxelEdit::new(
                    volume_identity(x, z),
                    VoxelCoordinate::new(local_x, y, local_z),
                    material(if restore {
                        generated_code(local_x, y, local_z)
                    } else {
                        changed
                    }),
                )
            })
            .collect(),
    )
}

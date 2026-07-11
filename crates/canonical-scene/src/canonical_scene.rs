use std::collections::TryReserveError;

use thiserror::Error;
use voxel_frontend::{
    DenseVoxelBatch, DenseVoxelScene, DenseVoxelVolume, VoxelCoordinate, VoxelExtent,
    VoxelMaterial, VoxelMaterialId, VoxelRegion, VoxelSceneId, VoxelSceneRevision, VoxelValue,
    VoxelVolumeId, VoxelVolumeMetadata,
};

const GENERATOR_IDENTITY: &str = "voxel-nexus-canonical-dense";
const GENERATOR_VERSION: u32 = 1;
const GENERATOR_SEED: u64 = 0x564f_5845_4c4e_5853;
const SCENE_ORIGIN: [f32; 3] = [-8.0, -4.0, -8.0];
const BASE_VOXEL_SIZE: f32 = 0.25;
const BASE_EXPOSED_FACE_LIMIT: u64 = 14_000;
const OVERHANG_START_Z: i64 = 20 + ((GENERATOR_SEED ^ (GENERATOR_SEED >> 32)) % 4) as i64;
const WARM_MATERIAL: CanonicalMaterialMetadata = CanonicalMaterialMetadata {
    identity: "canonical-warm",
    linear_base_color: [0.95, 0.22, 0.1, 1.0],
};
const GREEN_MATERIAL: CanonicalMaterialMetadata = CanonicalMaterialMetadata {
    identity: "canonical-green",
    linear_base_color: [0.12, 0.75, 0.28, 1.0],
};
const BLUE_MATERIAL: CanonicalMaterialMetadata = CanonicalMaterialMetadata {
    identity: "canonical-blue",
    linear_base_color: [0.1, 0.32, 0.95, 1.0],
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CanonicalSceneScale {
    Small,
    Medium,
    Large,
}

impl CanonicalSceneScale {
    pub fn factor(self) -> u32 {
        match self {
            Self::Small => 1,
            Self::Medium => 2,
            Self::Large => 4,
        }
    }

    pub fn dimensions(self) -> [u32; 3] {
        let factor = self.factor();
        [64 * factor, 32 * factor, 64 * factor]
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CanonicalSceneMetadata {
    dimensions: [u32; 3],
    voxel_size: f32,
    material_catalogue: [CanonicalMaterialMetadata; 3],
    volume_identity: VoxelVolumeId,
    occupied_count: u64,
    exposed_face_count: u64,
    exposed_face_limit: u64,
}

impl CanonicalSceneMetadata {
    pub fn generator_identity(&self) -> &'static str {
        GENERATOR_IDENTITY
    }

    pub fn generator_version(&self) -> u32 {
        GENERATOR_VERSION
    }

    pub fn seed(&self) -> u64 {
        GENERATOR_SEED
    }

    pub fn dimensions(&self) -> [u32; 3] {
        self.dimensions
    }

    pub fn scene_origin(&self) -> [f32; 3] {
        SCENE_ORIGIN
    }

    pub fn voxel_size(&self) -> f32 {
        self.voxel_size
    }

    pub fn material_catalogue(&self) -> &[CanonicalMaterialMetadata] {
        &self.material_catalogue
    }

    pub fn volume_identity(&self) -> &VoxelVolumeId {
        &self.volume_identity
    }

    pub fn occupied_count(&self) -> u64 {
        self.occupied_count
    }

    pub fn exposed_face_count(&self) -> u64 {
        self.exposed_face_count
    }

    pub fn exposed_face_limit(&self) -> u64 {
        self.exposed_face_limit
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CanonicalMaterialMetadata {
    identity: &'static str,
    linear_base_color: [f32; 4],
}

impl CanonicalMaterialMetadata {
    pub fn identity(self) -> &'static str {
        self.identity
    }

    pub fn linear_base_color(self) -> [f32; 4] {
        self.linear_base_color
    }
}

pub struct CanonicalScene {
    scene: DenseVoxelScene,
    metadata: CanonicalSceneMetadata,
}

impl CanonicalScene {
    pub fn metadata(&self) -> &CanonicalSceneMetadata {
        &self.metadata
    }

    pub fn into_scene(self) -> DenseVoxelScene {
        self.scene
    }
}

#[derive(Debug, Error)]
pub enum CanonicalSceneError {
    #[error("canonical scene dimensions or counts overflowed")]
    ArithmeticOverflow,
    #[error("canonical dense Voxel Volume allocation failed")]
    Allocation(#[source] TryReserveError),
    #[error("canonical exposed-face count {actual} exceeds the generator bound {limit}")]
    ExposedFaceLimit { actual: u64, limit: u64 },
}

pub fn generate_canonical_scene(
    scale: CanonicalSceneScale,
) -> Result<CanonicalScene, CanonicalSceneError> {
    let dimensions = scale.dimensions();
    let [width, height, depth] = dimensions;
    let value_count = usize::try_from(width)
        .ok()
        .and_then(|value| value.checked_mul(usize::try_from(height).ok()?))
        .and_then(|value| value.checked_mul(usize::try_from(depth).ok()?))
        .ok_or(CanonicalSceneError::ArithmeticOverflow)?;
    let scale_factor = i64::from(scale.factor());
    let material_catalogue = [WARM_MATERIAL, GREEN_MATERIAL, BLUE_MATERIAL];
    let warm_material_identity = VoxelMaterialId::new(WARM_MATERIAL.identity);
    let green_material_identity = VoxelMaterialId::new(GREEN_MATERIAL.identity);
    let blue_material_identity = VoxelMaterialId::new(BLUE_MATERIAL.identity);
    let mut values = Vec::new();
    values
        .try_reserve_exact(value_count)
        .map_err(CanonicalSceneError::Allocation)?;
    let mut occupied_count = 0_u64;
    let mut exposed_face_count = 0_u64;
    for coordinate_z in 0..i64::from(depth) {
        for coordinate_y in 0..i64::from(height) {
            for coordinate_x in 0..i64::from(width) {
                if is_occupied(
                    coordinate_x,
                    coordinate_y,
                    coordinate_z,
                    scale_factor,
                    dimensions,
                ) {
                    occupied_count = occupied_count
                        .checked_add(1)
                        .ok_or(CanonicalSceneError::ArithmeticOverflow)?;
                    for [offset_x, offset_y, offset_z] in [
                        [-1, 0, 0],
                        [1, 0, 0],
                        [0, -1, 0],
                        [0, 1, 0],
                        [0, 0, -1],
                        [0, 0, 1],
                    ] {
                        if !is_occupied(
                            coordinate_x + offset_x,
                            coordinate_y + offset_y,
                            coordinate_z + offset_z,
                            scale_factor,
                            dimensions,
                        ) {
                            exposed_face_count = exposed_face_count
                                .checked_add(1)
                                .ok_or(CanonicalSceneError::ArithmeticOverflow)?;
                        }
                    }
                    values.push(VoxelValue::Occupied(material_identity(
                        coordinate_x,
                        scale_factor,
                        &warm_material_identity,
                        &green_material_identity,
                        &blue_material_identity,
                    )));
                } else {
                    values.push(VoxelValue::Empty);
                }
            }
        }
    }
    let scale_squared = u64::from(scale.factor())
        .checked_mul(u64::from(scale.factor()))
        .ok_or(CanonicalSceneError::ArithmeticOverflow)?;
    let exposed_face_limit = BASE_EXPOSED_FACE_LIMIT
        .checked_mul(scale_squared)
        .ok_or(CanonicalSceneError::ArithmeticOverflow)?;
    if exposed_face_count > exposed_face_limit {
        return Err(CanonicalSceneError::ExposedFaceLimit {
            actual: exposed_face_count,
            limit: exposed_face_limit,
        });
    }

    let volume_identity = VoxelVolumeId::new("canonical-volume");
    let extent = VoxelExtent::new(width, height, depth);
    let voxel_size = BASE_VOXEL_SIZE / scale.factor() as f32;
    let materials = vec![
        VoxelMaterial::new(warm_material_identity, WARM_MATERIAL.linear_base_color),
        VoxelMaterial::new(green_material_identity, GREEN_MATERIAL.linear_base_color),
        VoxelMaterial::new(blue_material_identity, BLUE_MATERIAL.linear_base_color),
    ];
    let metadata = CanonicalSceneMetadata {
        dimensions,
        voxel_size,
        material_catalogue,
        volume_identity: volume_identity.clone(),
        occupied_count,
        exposed_face_count,
        exposed_face_limit,
    };
    let scene = DenseVoxelScene::new(
        VoxelSceneId::new("canonical-dense-scene"),
        VoxelSceneRevision::new(1),
        materials,
        vec![DenseVoxelVolume::new(
            VoxelVolumeMetadata::new(volume_identity, extent, SCENE_ORIGIN, voxel_size),
            vec![DenseVoxelBatch::new(
                VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), extent),
                values,
            )],
        )],
    );
    Ok(CanonicalScene { scene, metadata })
}

fn material_identity(
    coordinate_x: i64,
    scale_factor: i64,
    warm: &VoxelMaterialId,
    green: &VoxelMaterialId,
    blue: &VoxelMaterialId,
) -> VoxelMaterialId {
    if coordinate_x < 28 * scale_factor {
        warm.clone()
    } else if coordinate_x < 40 * scale_factor {
        green.clone()
    } else {
        blue.clone()
    }
}

fn is_occupied(
    coordinate_x: i64,
    coordinate_y: i64,
    coordinate_z: i64,
    scale_factor: i64,
    dimensions: [u32; 3],
) -> bool {
    let [width, height, depth] = dimensions;
    if coordinate_x < 0
        || coordinate_y < 0
        || coordinate_z < 0
        || coordinate_x >= i64::from(width)
        || coordinate_y >= i64::from(height)
        || coordinate_z >= i64::from(depth)
    {
        return false;
    }
    let base = coordinate_x < 56 * scale_factor
        && coordinate_y < 8 * scale_factor
        && coordinate_z >= 4 * scale_factor
        && coordinate_z < 60 * scale_factor;
    let mesa = coordinate_x >= 8 * scale_factor
        && coordinate_x < 48 * scale_factor
        && coordinate_y >= 8 * scale_factor
        && coordinate_y < 26 * scale_factor
        && coordinate_z >= 10 * scale_factor
        && coordinate_z < 54 * scale_factor;
    let tunnel = coordinate_x >= 24 * scale_factor
        && coordinate_x < 34 * scale_factor
        && coordinate_y >= 8 * scale_factor
        && coordinate_y < 20 * scale_factor
        && coordinate_z >= 10 * scale_factor
        && coordinate_z < 54 * scale_factor;
    let isolated_overhang = coordinate_x >= 50 * scale_factor
        && coordinate_x < 64 * scale_factor
        && coordinate_y >= 20 * scale_factor
        && coordinate_y < 24 * scale_factor
        && coordinate_z >= OVERHANG_START_Z * scale_factor
        && coordinate_z < (OVERHANG_START_Z + 20) * scale_factor;
    base || (mesa && !tunnel) || isolated_overhang
}

use std::collections::TryReserveError;

use semantic_ray_oracle::{SemanticRay, SemanticRayError, SemanticRayProbe, SemanticRayProbeError};
use thiserror::Error;
use voxel_frontend::{
    DenseVoxelBatch, DenseVoxelScene, DenseVoxelVolume, SparseVoxelBackground, SparseVoxelBatch,
    SparseVoxelScene, SparseVoxelVolume, VoxelCoordinate, VoxelExtent, VoxelMaterial,
    VoxelMaterialId, VoxelRegion, VoxelRegionFill, VoxelSceneId, VoxelSceneRevision, VoxelValue,
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
    fills: Vec<(VoxelRegion, VoxelValue)>,
    metadata: CanonicalSceneMetadata,
}

impl CanonicalScene {
    pub fn metadata(&self) -> &CanonicalSceneMetadata {
        &self.metadata
    }

    pub fn into_scene(self) -> SparseVoxelScene {
        SparseVoxelScene::new(
            VoxelSceneId::new("canonical-dense-scene"),
            VoxelSceneRevision::new(1),
            self.materials(),
            vec![SparseVoxelVolume::new(
                self.volume_metadata(),
                SparseVoxelBackground::Empty,
                self.fills
                    .into_iter()
                    .map(|(region, value)| {
                        SparseVoxelBatch::Fill(VoxelRegionFill::new(region, value))
                    })
                    .collect(),
            )],
        )
    }

    pub fn into_dense_scene(self) -> Result<DenseVoxelScene, CanonicalSceneError> {
        let [width, height, depth] = self.metadata.dimensions.map(|value| value as usize);
        let value_count = width
            .checked_mul(height)
            .and_then(|count| count.checked_mul(depth))
            .ok_or(CanonicalSceneError::ArithmeticOverflow)?;
        let mut values = Vec::new();
        values
            .try_reserve_exact(value_count)
            .map_err(CanonicalSceneError::Allocation)?;
        values.resize(value_count, VoxelValue::Empty);
        for (region, value) in &self.fills {
            let [origin_x, origin_y, origin_z] =
                region.origin().components().map(|value| value as usize);
            let [region_width, region_height, region_depth] =
                region.extent().dimensions().map(|value| value as usize);
            for z in origin_z..origin_z + region_depth {
                for y in origin_y..origin_y + region_height {
                    let start = (z * height + y) * width + origin_x;
                    values
                        .get_mut(start..start + region_width)
                        .expect("canonical fills are within the allocated volume bounds")
                        .fill(value.clone());
                }
            }
        }
        let volume_metadata = self.volume_metadata();
        let extent = volume_metadata.extent();
        Ok(DenseVoxelScene::new(
            VoxelSceneId::new("canonical-dense-scene"),
            VoxelSceneRevision::new(1),
            self.materials(),
            vec![DenseVoxelVolume::new(
                volume_metadata,
                vec![DenseVoxelBatch::new(
                    VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), extent),
                    values,
                )],
            )],
        ))
    }

    fn materials(&self) -> Vec<VoxelMaterial> {
        self.metadata
            .material_catalogue
            .iter()
            .map(|material| {
                VoxelMaterial::new(
                    VoxelMaterialId::new(material.identity),
                    material.linear_base_color,
                )
            })
            .collect()
    }

    fn volume_metadata(&self) -> VoxelVolumeMetadata {
        let [width, height, depth] = self.metadata.dimensions;
        VoxelVolumeMetadata::new(
            self.metadata.volume_identity.clone(),
            VoxelExtent::new(width, height, depth),
            SCENE_ORIGIN,
            self.metadata.voxel_size,
        )
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

#[derive(Debug, Error)]
pub enum CanonicalSemanticRayProbeError {
    #[error("canonical Semantic Ray could not be declared")]
    Ray(#[from] SemanticRayError),
    #[error("canonical Semantic Ray probe could not be declared")]
    Probe(#[from] SemanticRayProbeError),
}

pub fn canonical_edit_semantic_ray_probes()
-> Result<[SemanticRayProbe; 3], CanonicalSemanticRayProbeError> {
    Ok([
        canonical_edit_semantic_ray_probe("edit-0-0-0", 0)?,
        canonical_edit_semantic_ray_probe("edit-40-0-0", 40)?,
        canonical_edit_semantic_ray_probe("edit-80-0-0", 80)?,
    ])
}

fn canonical_edit_semantic_ray_probe(
    identity: &str,
    coordinate_x: i32,
) -> Result<SemanticRayProbe, CanonicalSemanticRayProbeError> {
    let [scene_origin_x, scene_origin_y, scene_origin_z] = SCENE_ORIGIN.map(f64::from);
    let voxel_size = f64::from(BASE_VOXEL_SIZE) / f64::from(CanonicalSceneScale::Large.factor());
    let ray = SemanticRay::new(
        [
            scene_origin_x + (f64::from(coordinate_x) + 0.5) * voxel_size,
            scene_origin_y + 0.5 * voxel_size,
            scene_origin_z - voxel_size,
        ],
        [0.0, 0.0, 1.0],
        0.0,
        2.0 * voxel_size,
    )?;
    Ok(SemanticRayProbe::new(identity, ray)?)
}

pub fn generate_canonical_scene(
    scale: CanonicalSceneScale,
) -> Result<CanonicalScene, CanonicalSceneError> {
    let factor = scale.factor();
    let dimensions = scale.dimensions();
    let warm = VoxelMaterialId::new(WARM_MATERIAL.identity);
    let green = VoxelMaterialId::new(GREEN_MATERIAL.identity);
    let blue = VoxelMaterialId::new(BLUE_MATERIAL.identity);
    let mut fills = Vec::new();
    let mut occupied_count = 0_u64;
    let mut exposed_face_count = 0_u64;
    // Every geometry and material boundary is a partition plane, so each box is uniform
    // and its neighbours have constant occupancy across each entire face.
    let x_boundaries = [0, 8, 24, 28, 34, 40, 48, 50, 56, 64];
    let y_boundaries = [0, 8, 20, 24, 26, 32];
    let z_boundaries = [
        0,
        4,
        10,
        OVERHANG_START_Z,
        OVERHANG_START_Z + 20,
        54,
        60,
        64,
    ];
    for (&minimum_z, &maximum_z) in z_boundaries.iter().zip(z_boundaries.iter().skip(1)) {
        for (&minimum_y, &maximum_y) in y_boundaries.iter().zip(y_boundaries.iter().skip(1)) {
            for (&minimum_x, &maximum_x) in x_boundaries.iter().zip(x_boundaries.iter().skip(1)) {
                if !is_occupied(
                    minimum_x,
                    minimum_y,
                    minimum_z,
                    1,
                    CanonicalSceneScale::Small.dimensions(),
                ) {
                    continue;
                }
                let width = (maximum_x - minimum_x) as u64;
                let height = (maximum_y - minimum_y) as u64;
                let depth = (maximum_z - minimum_z) as u64;
                occupied_count += width * height * depth;
                for (neighbour, area) in [
                    ([minimum_x - 1, minimum_y, minimum_z], height * depth),
                    ([maximum_x, minimum_y, minimum_z], height * depth),
                    ([minimum_x, minimum_y - 1, minimum_z], width * depth),
                    ([minimum_x, maximum_y, minimum_z], width * depth),
                    ([minimum_x, minimum_y, minimum_z - 1], width * height),
                    ([minimum_x, minimum_y, maximum_z], width * height),
                ] {
                    let [neighbour_x, neighbour_y, neighbour_z] = neighbour;
                    if !is_occupied(
                        neighbour_x,
                        neighbour_y,
                        neighbour_z,
                        1,
                        CanonicalSceneScale::Small.dimensions(),
                    ) {
                        exposed_face_count += area;
                    }
                }
                fills.push((
                    VoxelRegion::new(
                        VoxelCoordinate::new(
                            minimum_x as i32 * factor as i32,
                            minimum_y as i32 * factor as i32,
                            minimum_z as i32 * factor as i32,
                        ),
                        VoxelExtent::new(
                            width as u32 * factor,
                            height as u32 * factor,
                            depth as u32 * factor,
                        ),
                    ),
                    VoxelValue::Occupied(material_identity(minimum_x, 1, &warm, &green, &blue)),
                ));
            }
        }
    }
    let scale_squared = u64::from(factor).pow(2);
    occupied_count *= u64::from(factor).pow(3);
    exposed_face_count *= scale_squared;
    let exposed_face_limit = BASE_EXPOSED_FACE_LIMIT * scale_squared;
    if exposed_face_count > exposed_face_limit {
        return Err(CanonicalSceneError::ExposedFaceLimit {
            actual: exposed_face_count,
            limit: exposed_face_limit,
        });
    }
    let metadata = CanonicalSceneMetadata {
        dimensions,
        voxel_size: BASE_VOXEL_SIZE / factor as f32,
        material_catalogue: [WARM_MATERIAL, GREEN_MATERIAL, BLUE_MATERIAL],
        volume_identity: VoxelVolumeId::new("canonical-volume"),
        occupied_count,
        exposed_face_count,
        exposed_face_limit,
    };
    Ok(CanonicalScene { fills, metadata })
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

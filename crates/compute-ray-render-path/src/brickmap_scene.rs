use crate::compute_scene::{ComputeVolumeHeader, contact_precedes, trace_volume_with};
use semantic_ray_oracle::{SemanticRay, SemanticRayObservation, SemanticRayResult};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use thiserror::Error;
use voxel_frontend::{
    VoxelCoordinate, VoxelFrontendError, VoxelMaterialId, VoxelRegionContent, VoxelSceneId,
    VoxelSceneRevision, VoxelSceneView, VoxelValue, VoxelVolumeId,
};

const EDGE: u32 = 8;
const POOL_WORDS: usize = 256;
const MIXED: u32 = 1 << 31;

#[derive(Clone, Debug)]
pub struct BrickmapObservations {
    pub coarse_grid_bytes: usize,
    pub mixed_brick_count: usize,
    pub pool_bytes: usize,
    pub enumeration_time: Duration,
    pub construction_time: Duration,
}

#[derive(Debug, Error)]
pub enum BrickmapBuildError {
    #[error("brickmap Voxel Volume {volume:?}: {part} size exceeds representation limits")]
    Size {
        volume: VoxelVolumeId,
        part: &'static str,
    },
    #[error("brickmap Voxel Volume {volume:?}: could not allocate {part}")]
    Allocation {
        volume: VoxelVolumeId,
        part: &'static str,
    },
    #[error(
        "brickmap Voxel Volume {volume:?} uses material {material:?} beyond the 65,535 occupied material identity limit"
    )]
    TooManyMaterials {
        volume: VoxelVolumeId,
        material: VoxelMaterialId,
    },
    #[error("could not read Voxel Cell Grid contents for brickmap construction")]
    Frontend(#[from] VoxelFrontendError),
}

#[derive(Clone, Debug)]
struct BrickmapVolume {
    header: ComputeVolumeHeader,
    dimensions: [u32; 3],
    // Zero is empty, 1..=65535 is uniform, and the high bit tags a pool slot.
    entries: Vec<u32>,
}

#[derive(Clone, Debug)]
pub struct BrickmapSceneBundle {
    scene_identity: VoxelSceneId,
    revision: VoxelSceneRevision,
    volumes: Vec<BrickmapVolume>,
    materials: Vec<VoxelMaterialId>,
    pool: Vec<u32>,
    observations: BrickmapObservations,
}

impl BrickmapSceneBundle {
    pub fn from_view(view: &VoxelSceneView) -> Result<Self, BrickmapBuildError> {
        Self::from_view_with_progress(view, || Ok(()))
    }

    pub(crate) fn from_view_with_progress<E>(
        view: &VoxelSceneView,
        mut progress: impl FnMut() -> Result<(), E>,
    ) -> Result<Self, E>
    where
        E: From<BrickmapBuildError> + From<VoxelFrontendError>,
    {
        let started = Instant::now();
        let mut bundle = Self {
            scene_identity: view.scene_id().clone(),
            revision: view.revision(),
            volumes: Vec::new(),
            materials: Vec::new(),
            pool: Vec::new(),
            observations: BrickmapObservations {
                coarse_grid_bytes: 0,
                mixed_brick_count: 0,
                pool_bytes: 0,
                enumeration_time: Duration::ZERO,
                construction_time: Duration::ZERO,
            },
        };
        let mut indices = HashMap::new();
        let mut volumes = view.volumes().iter().collect::<Vec<_>>();
        volumes.sort_by(|left, right| left.identity().cmp(right.identity()));
        for volume in volumes {
            progress()?;
            let identity = volume.identity();
            let dimensions = volume
                .extent()
                .dimensions()
                .map(|dimension| dimension.div_ceil(EDGE));
            let count = dimensions
                .into_iter()
                .try_fold(1usize, |count, dimension| {
                    count
                        .checked_mul(dimension as usize)
                        .ok_or_else(|| size_error(identity, "coarse grid"))
                })?;
            let bytes = checked_bytes(count, identity, "coarse grid")?;
            bundle.observations.coarse_grid_bytes = bundle
                .observations
                .coarse_grid_bytes
                .checked_add(bytes)
                .ok_or_else(|| size_error(identity, "coarse grid total"))?;
            let mut entries = Vec::new();
            reserve(&mut entries, count, identity, "coarse grid")?;
            entries.resize(count, 0);
            let enumeration_started = Instant::now();
            let mut batches = view.enumerate_cells(identity, EDGE, 64)?;
            bundle.observations.enumeration_time += enumeration_started.elapsed();
            loop {
                let enumeration_started = Instant::now();
                let batch = batches.next();
                bundle.observations.enumeration_time += enumeration_started.elapsed();
                let Some(batch) = batch else { break };
                for cell in batch? {
                    progress()?;
                    let entry = match cell.content() {
                        VoxelRegionContent::Uniform(value) => u32::from(material_index(
                            value,
                            identity,
                            &mut indices,
                            &mut bundle.materials,
                        )?),
                        VoxelRegionContent::Mixed => {
                            let slot = bundle.pool.len() / POOL_WORDS;
                            let slot = u32::try_from(slot)
                                .ok()
                                .filter(|slot| *slot < MIXED)
                                .ok_or_else(|| size_error(identity, "mixed brick pool"))?;
                            let length = bundle
                                .pool
                                .len()
                                .checked_add(POOL_WORDS)
                                .ok_or_else(|| size_error(identity, "mixed brick pool"))?;
                            checked_bytes(length, identity, "mixed brick pool")?;
                            reserve(&mut bundle.pool, POOL_WORDS, identity, "mixed brick pool")?;
                            let start = bundle.pool.len();
                            bundle.pool.resize(length, 0);
                            let [width, height, depth] = cell.region().extent().dimensions();
                            let count = (width * height * depth) as usize;
                            let mut values = vec![VoxelValue::Empty; count];
                            view.read_region_into(identity, cell.region(), &mut values)?;
                            for (index, value) in values.iter().enumerate() {
                                let x = index % width as usize;
                                let y = index / width as usize % height as usize;
                                let z = index / (width * height) as usize;
                                let local = x + 8 * y + 64 * z;
                                let material = material_index(
                                    value,
                                    identity,
                                    &mut indices,
                                    &mut bundle.materials,
                                )?;
                                let word = bundle
                                    .pool
                                    .get_mut(start + local / 2)
                                    .expect("an 8-cubed cell fits its reserved pool slot");
                                *word |= u32::from(material) << (16 * (local % 2));
                            }
                            MIXED | slot
                        }
                    };
                    let index = grid_index(dimensions, cell.coordinate().components())
                        .expect("enumerated cells lie within the volume grid");
                    *entries
                        .get_mut(index)
                        .expect("the full coarse grid was allocated") = entry;
                }
            }
            reserve(&mut bundle.volumes, 1, identity, "volume headers")?;
            bundle.volumes.push(BrickmapVolume {
                header: ComputeVolumeHeader::for_brickmap(volume),
                dimensions,
                entries,
            });
        }
        bundle.observations.mixed_brick_count = bundle.pool.len() / POOL_WORDS;
        bundle.observations.pool_bytes = bundle.pool.len() * size_of::<u32>();
        bundle.observations.construction_time =
            started.elapsed() - bundle.observations.enumeration_time;
        Ok(bundle)
    }

    pub(crate) fn gpu_words(
        &self,
    ) -> Result<(Vec<ComputeVolumeHeader>, Vec<u32>), crate::ComputeSceneBuildError> {
        let allocation_error = |_| crate::ComputeSceneBuildError::Allocation;
        let count = self
            .volumes
            .iter()
            .try_fold(1usize, |count, volume| {
                count.checked_add(volume.entries.len())
            })
            .and_then(|count| count.checked_add(self.pool.len()))
            .ok_or(crate::ComputeSceneBuildError::ArithmeticOverflow)?;
        u32::try_from(count)?;
        let mut headers = Vec::new();
        headers
            .try_reserve_exact(self.volumes.len())
            .map_err(allocation_error)?;
        let mut words = Vec::new();
        words.try_reserve_exact(count).map_err(allocation_error)?;
        words.push(0);
        for volume in &self.volumes {
            let mut header = volume.header.clone();
            header.set_voxel_word_offset(u32::try_from(words.len())?);
            headers.push(header);
            words.extend(&volume.entries);
        }
        *words.first_mut().expect("the pool offset word is present") = u32::try_from(words.len())?;
        words.extend(&self.pool);
        u32::try_from(words.len())?;
        Ok((headers, words))
    }

    pub fn observations(&self) -> &BrickmapObservations {
        &self.observations
    }

    pub fn scene_identity(&self) -> &VoxelSceneId {
        &self.scene_identity
    }

    pub fn revision(&self) -> VoxelSceneRevision {
        self.revision
    }

    pub fn material_identities(&self) -> &[VoxelMaterialId] {
        &self.materials
    }

    pub fn observe(&self, ray: &SemanticRay) -> SemanticRayObservation {
        let mut nearest = None;
        for volume in &self.volumes {
            let Some(contact) =
                trace_volume_with(&volume.header, ray, &self.materials, |coordinate| {
                    self.sample(volume, coordinate)
                })
            else {
                continue;
            };
            if nearest
                .as_ref()
                .is_none_or(|current| contact_precedes(&contact, current))
            {
                nearest = Some(contact);
            }
        }
        SemanticRayObservation::new(
            self.scene_identity.clone(),
            self.revision,
            nearest.map_or(SemanticRayResult::Miss, SemanticRayResult::Contact),
        )
    }

    fn sample(&self, volume: &BrickmapVolume, coordinate: VoxelCoordinate) -> Option<(u32, i32)> {
        let [x, y, z] = coordinate.components().map(u32::try_from);
        let coordinates = [x.ok()?, y.ok()?, z.ok()?];
        if coordinates
            .into_iter()
            .zip(volume.header.extent().dimensions())
            .any(|(coordinate, bound)| coordinate >= bound)
        {
            return None;
        }
        let index = grid_index(volume.dimensions, coordinates.map(|value| value / EDGE))?;
        let entry = *volume.entries.get(index)?;
        if entry & MIXED == 0 {
            return Some((entry, if entry == 0 { 8 } else { 1 }));
        }
        let [x, y, z] = coordinates.map(|value| (value % EDGE) as usize);
        let local = x + 8 * y + 64 * z;
        let word = self
            .pool
            .get((entry & !MIXED) as usize * POOL_WORDS + local / 2)?;
        Some(((word >> (16 * (local % 2))) & 0xffff, 1))
    }
}

fn size_error(volume: &VoxelVolumeId, part: &'static str) -> BrickmapBuildError {
    BrickmapBuildError::Size {
        volume: volume.clone(),
        part,
    }
}

fn checked_bytes(
    words: usize,
    volume: &VoxelVolumeId,
    part: &'static str,
) -> Result<usize, BrickmapBuildError> {
    // Coarse entries and pool word offsets must remain addressable by 32-bit indices.
    if u32::try_from(words).is_err() {
        return Err(size_error(volume, part));
    }
    words
        .checked_mul(size_of::<u32>())
        .filter(|bytes| *bytes <= isize::MAX as usize)
        .ok_or_else(|| size_error(volume, part))
}

fn reserve<T>(
    values: &mut Vec<T>,
    additional: usize,
    volume: &VoxelVolumeId,
    part: &'static str,
) -> Result<(), BrickmapBuildError> {
    values
        .try_reserve(additional)
        .map_err(|_| BrickmapBuildError::Allocation {
            volume: volume.clone(),
            part,
        })
}

fn material_index(
    value: &VoxelValue,
    volume: &VoxelVolumeId,
    indices: &mut HashMap<VoxelMaterialId, u16>,
    materials: &mut Vec<VoxelMaterialId>,
) -> Result<u16, BrickmapBuildError> {
    let VoxelValue::Occupied(identity) = value else {
        return Ok(0);
    };
    if let Some(index) = indices.get(identity) {
        return Ok(*index);
    }
    let index =
        u16::try_from(materials.len() + 1).map_err(|_| BrickmapBuildError::TooManyMaterials {
            volume: volume.clone(),
            material: identity.clone(),
        })?;
    indices
        .try_reserve(1)
        .map_err(|_| BrickmapBuildError::Allocation {
            volume: volume.clone(),
            part: "material table",
        })?;
    reserve(materials, 1, volume, "material table")?;
    materials.push(identity.clone());
    indices.insert(identity.clone(), index);
    Ok(index)
}

fn grid_index(dimensions: [u32; 3], coordinate: [u32; 3]) -> Option<usize> {
    if coordinate
        .into_iter()
        .zip(dimensions)
        .any(|(coordinate, bound)| coordinate >= bound)
    {
        return None;
    }
    let [x, y, z] = coordinate.map(|value| value as usize);
    let [width, height, _] = dimensions.map(|value| value as usize);
    z.checked_mul(height)?
        .checked_add(y)?
        .checked_mul(width)?
        .checked_add(x)
}

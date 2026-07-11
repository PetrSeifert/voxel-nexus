use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};
use thiserror::Error;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct VoxelSceneId(Arc<str>);

impl VoxelSceneId {
    pub fn new(identity: impl Into<String>) -> Self {
        Self(Arc::from(identity.into()))
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct VoxelVolumeId(Arc<str>);

impl VoxelVolumeId {
    pub fn new(identity: impl Into<String>) -> Self {
        Self(Arc::from(identity.into()))
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct VoxelMaterialId(Arc<str>);

impl VoxelMaterialId {
    pub fn new(identity: impl Into<String>) -> Self {
        Self(Arc::from(identity.into()))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VoxelSceneRevision(u64);

impl VoxelSceneRevision {
    pub fn new(revision: u64) -> Self {
        Self(revision)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct VoxelCoordinate {
    x: i32,
    y: i32,
    z: i32,
}

impl VoxelCoordinate {
    pub fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }

    pub fn components(self) -> [i32; 3] {
        [self.x, self.y, self.z]
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VoxelExtent {
    width: u32,
    height: u32,
    depth: u32,
}

impl VoxelExtent {
    pub fn new(width: u32, height: u32, depth: u32) -> Self {
        Self {
            width,
            height,
            depth,
        }
    }

    pub fn dimensions(self) -> [u32; 3] {
        [self.width, self.height, self.depth]
    }

    fn value_count(self) -> Option<usize> {
        usize::try_from(self.width)
            .ok()?
            .checked_mul(usize::try_from(self.height).ok()?)?
            .checked_mul(usize::try_from(self.depth).ok()?)
    }

    fn is_empty(self) -> bool {
        self.width == 0 || self.height == 0 || self.depth == 0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VoxelRegion {
    origin: VoxelCoordinate,
    extent: VoxelExtent,
}

impl VoxelRegion {
    pub fn new(origin: VoxelCoordinate, extent: VoxelExtent) -> Self {
        Self { origin, extent }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum VoxelValue {
    Empty,
    Occupied(VoxelMaterialId),
}

#[derive(Clone, Debug, PartialEq)]
pub struct VoxelMaterial {
    identity: VoxelMaterialId,
    linear_base_color: [f32; 4],
}

impl VoxelMaterial {
    pub fn new(identity: VoxelMaterialId, linear_base_color: [f32; 4]) -> Self {
        Self {
            identity,
            linear_base_color,
        }
    }

    pub fn identity(&self) -> &VoxelMaterialId {
        &self.identity
    }

    pub fn linear_base_color(&self) -> [f32; 4] {
        self.linear_base_color
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct VoxelVolumeMetadata {
    identity: VoxelVolumeId,
    extent: VoxelExtent,
    scene_origin: [f32; 3],
    voxel_size: f32,
}

impl VoxelVolumeMetadata {
    pub fn new(
        identity: VoxelVolumeId,
        extent: VoxelExtent,
        scene_origin: [f32; 3],
        voxel_size: f32,
    ) -> Self {
        Self {
            identity,
            extent,
            scene_origin,
            voxel_size,
        }
    }

    pub fn identity(&self) -> &VoxelVolumeId {
        &self.identity
    }

    pub fn extent(&self) -> VoxelExtent {
        self.extent
    }

    pub fn scene_origin(&self) -> [f32; 3] {
        self.scene_origin
    }

    pub fn voxel_size(&self) -> f32 {
        self.voxel_size
    }
}

#[derive(Clone, Debug)]
pub struct DenseVoxelBatch {
    region: VoxelRegion,
    values: Vec<VoxelValue>,
}

impl DenseVoxelBatch {
    pub fn new(region: VoxelRegion, values: Vec<VoxelValue>) -> Self {
        Self { region, values }
    }
}

#[derive(Clone, Debug)]
pub struct DenseVoxelVolume {
    metadata: VoxelVolumeMetadata,
    batches: Vec<DenseVoxelBatch>,
}

impl DenseVoxelVolume {
    pub fn new(metadata: VoxelVolumeMetadata, batches: Vec<DenseVoxelBatch>) -> Self {
        Self { metadata, batches }
    }
}

#[derive(Clone, Debug)]
pub struct DenseVoxelScene {
    identity: VoxelSceneId,
    revision: VoxelSceneRevision,
    materials: Vec<VoxelMaterial>,
    volumes: Vec<DenseVoxelVolume>,
}

impl DenseVoxelScene {
    pub fn new(
        identity: VoxelSceneId,
        revision: VoxelSceneRevision,
        materials: Vec<VoxelMaterial>,
        volumes: Vec<DenseVoxelVolume>,
    ) -> Self {
        Self {
            identity,
            revision,
            materials,
            volumes,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VoxelSample {
    coordinate: VoxelCoordinate,
    value: VoxelValue,
}

impl VoxelSample {
    pub fn coordinate(&self) -> VoxelCoordinate {
        self.coordinate
    }

    pub fn value(&self) -> &VoxelValue {
        &self.value
    }
}

#[derive(Debug, Error)]
pub enum VoxelFrontendError {
    #[error("Voxel Scene identity must not be empty")]
    EmptySceneIdentity,
    #[error("duplicate Voxel Material identity {identity:?}")]
    DuplicateMaterialIdentity { identity: VoxelMaterialId },
    #[error("Voxel Material {identity:?} has an invalid linear base color")]
    InvalidMaterialColor { identity: VoxelMaterialId },
    #[error("duplicate Voxel Volume identity {identity:?}")]
    DuplicateVolumeIdentity { identity: VoxelVolumeId },
    #[error("Voxel Volume identity must not be empty")]
    EmptyVolumeIdentity,
    #[error("Voxel Material identity must not be empty")]
    EmptyMaterialIdentity,
    #[error("Voxel Volume {identity:?} has an empty extent")]
    EmptyVolumeExtent { identity: VoxelVolumeId },
    #[error("Voxel Volume {identity:?} has invalid scene origin or voxel size")]
    InvalidVolumeMetadata { identity: VoxelVolumeId },
    #[error("Voxel Volume {identity:?} is too large to address")]
    VolumeTooLarge { identity: VoxelVolumeId },
    #[error("dense batch {batch_index} for Voxel Volume {identity:?} has an empty region")]
    EmptyBatchRegion {
        identity: VoxelVolumeId,
        batch_index: usize,
    },
    #[error(
        "dense batch {batch_index} for Voxel Volume {identity:?} has invalid coordinate bounds"
    )]
    InvalidBatchBounds {
        identity: VoxelVolumeId,
        batch_index: usize,
    },
    #[error("dense batch {batch_index} for Voxel Volume {identity:?} is outside the volume extent")]
    BatchOutsideVolume {
        identity: VoxelVolumeId,
        batch_index: usize,
    },
    #[error(
        "dense batch {batch_index} for Voxel Volume {identity:?} contains {actual} values but its region requires {expected}"
    )]
    BatchValueCount {
        identity: VoxelVolumeId,
        batch_index: usize,
        expected: usize,
        actual: usize,
    },
    #[error("Voxel Volume {identity:?} provides coordinate {coordinate:?} more than once")]
    DuplicateVoxelCoordinate {
        identity: VoxelVolumeId,
        coordinate: VoxelCoordinate,
    },
    #[error("Voxel Volume {identity:?} does not provide every coordinate in its finite extent")]
    IncompleteVolume { identity: VoxelVolumeId },
    #[error(
        "Voxel Volume {volume_identity:?} coordinate {coordinate:?} references unknown Voxel Material {material_identity:?}"
    )]
    UnknownMaterialReference {
        volume_identity: VoxelVolumeId,
        coordinate: VoxelCoordinate,
        material_identity: VoxelMaterialId,
    },
    #[error("no Voxel Scene Revision has been published")]
    SceneNotPublished,
    #[error("a Voxel Scene Revision has already been published")]
    SceneAlreadyPublished,
    #[error("unknown Voxel Volume identity {identity:?}")]
    UnknownVolumeIdentity { identity: VoxelVolumeId },
    #[error("Voxel Region request for Voxel Volume {identity:?} has an empty extent")]
    EmptyRegionRequest { identity: VoxelVolumeId },
    #[error("Voxel Region request for Voxel Volume {identity:?} has invalid coordinate bounds")]
    InvalidRegionBounds { identity: VoxelVolumeId },
    #[error("Voxel Region result for Voxel Volume {identity:?} could not be allocated")]
    RegionReadAllocation { identity: VoxelVolumeId },
    #[error("Voxel Frontend state could not be accessed")]
    StateUnavailable,
}

#[derive(Default)]
pub struct VoxelFrontend {
    published: RwLock<Option<Arc<PublishedScene>>>,
}

impl VoxelFrontend {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn publish(&self, scene: DenseVoxelScene) -> Result<VoxelSceneView, VoxelFrontendError> {
        let published = Arc::new(PublishedScene::try_from(scene)?);
        let mut current = self
            .published
            .write()
            .map_err(|_| VoxelFrontendError::StateUnavailable)?;
        if current.is_some() {
            return Err(VoxelFrontendError::SceneAlreadyPublished);
        }
        *current = Some(Arc::clone(&published));
        Ok(VoxelSceneView { published })
    }

    pub fn scene_view(&self) -> Result<VoxelSceneView, VoxelFrontendError> {
        let current = self
            .published
            .read()
            .map_err(|_| VoxelFrontendError::StateUnavailable)?;
        let published = current
            .as_ref()
            .ok_or(VoxelFrontendError::SceneNotPublished)?;
        Ok(VoxelSceneView {
            published: Arc::clone(published),
        })
    }
}

#[derive(Clone)]
pub struct VoxelSceneView {
    published: Arc<PublishedScene>,
}

impl VoxelSceneView {
    pub fn scene_id(&self) -> &VoxelSceneId {
        &self.published.identity
    }

    pub fn revision(&self) -> VoxelSceneRevision {
        self.published.revision
    }

    pub fn materials(&self) -> &[VoxelMaterial] {
        &self.published.materials
    }

    pub fn volumes(&self) -> &[VoxelVolumeMetadata] {
        &self.published.volume_metadata
    }

    pub fn read_region(
        &self,
        volume_identity: &VoxelVolumeId,
        region: VoxelRegion,
    ) -> Result<Vec<VoxelSample>, VoxelFrontendError> {
        let volume = self.published.volumes.get(volume_identity).ok_or_else(|| {
            VoxelFrontendError::UnknownVolumeIdentity {
                identity: volume_identity.clone(),
            }
        })?;
        let bounds = RegionBounds::new(region).ok_or_else(|| {
            if region.extent.is_empty() {
                VoxelFrontendError::EmptyRegionRequest {
                    identity: volume_identity.clone(),
                }
            } else {
                VoxelFrontendError::InvalidRegionBounds {
                    identity: volume_identity.clone(),
                }
            }
        })?;
        let capacity =
            region
                .extent
                .value_count()
                .ok_or_else(|| VoxelFrontendError::InvalidRegionBounds {
                    identity: volume_identity.clone(),
                })?;
        let mut samples = Vec::new();
        samples.try_reserve_exact(capacity).map_err(|_| {
            VoxelFrontendError::RegionReadAllocation {
                identity: volume_identity.clone(),
            }
        })?;
        for coordinate in bounds.coordinates() {
            samples.push(VoxelSample {
                coordinate,
                value: volume.value(coordinate).clone(),
            });
        }
        Ok(samples)
    }
}

struct PublishedScene {
    identity: VoxelSceneId,
    revision: VoxelSceneRevision,
    materials: Vec<VoxelMaterial>,
    volume_metadata: Vec<VoxelVolumeMetadata>,
    volumes: HashMap<VoxelVolumeId, DenseStorage>,
}

impl TryFrom<DenseVoxelScene> for PublishedScene {
    type Error = VoxelFrontendError;

    fn try_from(scene: DenseVoxelScene) -> Result<Self, Self::Error> {
        if scene.identity.0.is_empty() {
            return Err(VoxelFrontendError::EmptySceneIdentity);
        }
        let mut material_identities = HashSet::new();
        for material in &scene.materials {
            if material.identity.0.is_empty() {
                return Err(VoxelFrontendError::EmptyMaterialIdentity);
            }
            if !material_identities.insert(material.identity.clone()) {
                return Err(VoxelFrontendError::DuplicateMaterialIdentity {
                    identity: material.identity.clone(),
                });
            }
            if material
                .linear_base_color
                .iter()
                .any(|component| !component.is_finite())
            {
                return Err(VoxelFrontendError::InvalidMaterialColor {
                    identity: material.identity.clone(),
                });
            }
        }

        let mut volume_identities = HashSet::new();
        let mut volume_metadata = Vec::with_capacity(scene.volumes.len());
        let mut volumes = HashMap::with_capacity(scene.volumes.len());
        for volume in scene.volumes {
            let identity = volume.metadata.identity.clone();
            if identity.0.is_empty() {
                return Err(VoxelFrontendError::EmptyVolumeIdentity);
            }
            if !volume_identities.insert(identity.clone()) {
                return Err(VoxelFrontendError::DuplicateVolumeIdentity { identity });
            }
            validate_volume_metadata(&volume.metadata)?;
            let storage = DenseStorage::from_batches(&volume, &material_identities)?;
            volume_metadata.push(volume.metadata);
            volumes.insert(identity, storage);
        }

        Ok(Self {
            identity: scene.identity,
            revision: scene.revision,
            materials: scene.materials,
            volume_metadata,
            volumes,
        })
    }
}

fn validate_volume_metadata(metadata: &VoxelVolumeMetadata) -> Result<(), VoxelFrontendError> {
    if metadata.extent.is_empty() {
        return Err(VoxelFrontendError::EmptyVolumeExtent {
            identity: metadata.identity.clone(),
        });
    }
    if metadata.extent.value_count().is_none() {
        return Err(VoxelFrontendError::VolumeTooLarge {
            identity: metadata.identity.clone(),
        });
    }
    if metadata.scene_origin.iter().any(|value| !value.is_finite())
        || !metadata.voxel_size.is_finite()
        || metadata.voxel_size <= 0.0
    {
        return Err(VoxelFrontendError::InvalidVolumeMetadata {
            identity: metadata.identity.clone(),
        });
    }
    Ok(())
}

struct DenseStorage {
    extent: VoxelExtent,
    values: Vec<VoxelValue>,
}

impl DenseStorage {
    fn from_batches(
        volume: &DenseVoxelVolume,
        materials: &HashSet<VoxelMaterialId>,
    ) -> Result<Self, VoxelFrontendError> {
        let value_count = volume.metadata.extent.value_count().ok_or_else(|| {
            VoxelFrontendError::VolumeTooLarge {
                identity: volume.metadata.identity.clone(),
            }
        })?;
        let mut values = vec![None; value_count];
        for (batch_index, batch) in volume.batches.iter().enumerate() {
            let bounds = RegionBounds::new(batch.region).ok_or_else(|| {
                if batch.region.extent.is_empty() {
                    VoxelFrontendError::EmptyBatchRegion {
                        identity: volume.metadata.identity.clone(),
                        batch_index,
                    }
                } else {
                    VoxelFrontendError::InvalidBatchBounds {
                        identity: volume.metadata.identity.clone(),
                        batch_index,
                    }
                }
            })?;
            if bounds.start_x < 0
                || bounds.start_y < 0
                || bounds.start_z < 0
                || u32::try_from(bounds.end_x).ok() > Some(volume.metadata.extent.width)
                || u32::try_from(bounds.end_y).ok() > Some(volume.metadata.extent.height)
                || u32::try_from(bounds.end_z).ok() > Some(volume.metadata.extent.depth)
            {
                return Err(VoxelFrontendError::BatchOutsideVolume {
                    identity: volume.metadata.identity.clone(),
                    batch_index,
                });
            }
            let expected = batch.region.extent.value_count().ok_or_else(|| {
                VoxelFrontendError::InvalidBatchBounds {
                    identity: volume.metadata.identity.clone(),
                    batch_index,
                }
            })?;
            if batch.values.len() != expected {
                return Err(VoxelFrontendError::BatchValueCount {
                    identity: volume.metadata.identity.clone(),
                    batch_index,
                    expected,
                    actual: batch.values.len(),
                });
            }

            for (batch_value_index, coordinate) in bounds.coordinates().enumerate() {
                let value = batch.values.get(batch_value_index).ok_or_else(|| {
                    VoxelFrontendError::BatchValueCount {
                        identity: volume.metadata.identity.clone(),
                        batch_index,
                        expected,
                        actual: batch.values.len(),
                    }
                })?;
                if let VoxelValue::Occupied(material_identity) = value
                    && !materials.contains(material_identity)
                {
                    return Err(VoxelFrontendError::UnknownMaterialReference {
                        volume_identity: volume.metadata.identity.clone(),
                        coordinate,
                        material_identity: material_identity.clone(),
                    });
                }
                let storage_index =
                    dense_index(volume.metadata.extent, coordinate).ok_or_else(|| {
                        VoxelFrontendError::BatchOutsideVolume {
                            identity: volume.metadata.identity.clone(),
                            batch_index,
                        }
                    })?;
                let destination = values.get_mut(storage_index).ok_or_else(|| {
                    VoxelFrontendError::BatchOutsideVolume {
                        identity: volume.metadata.identity.clone(),
                        batch_index,
                    }
                })?;
                if destination.is_some() {
                    return Err(VoxelFrontendError::DuplicateVoxelCoordinate {
                        identity: volume.metadata.identity.clone(),
                        coordinate,
                    });
                }
                *destination = Some(value.clone());
            }
        }
        let values = values
            .into_iter()
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| VoxelFrontendError::IncompleteVolume {
                identity: volume.metadata.identity.clone(),
            })?;
        Ok(Self {
            extent: volume.metadata.extent,
            values,
        })
    }

    fn value(&self, coordinate: VoxelCoordinate) -> &VoxelValue {
        match dense_index(self.extent, coordinate).and_then(|index| self.values.get(index)) {
            Some(value) => value,
            None => &VoxelValue::Empty,
        }
    }
}

fn dense_index(extent: VoxelExtent, coordinate: VoxelCoordinate) -> Option<usize> {
    let x = usize::try_from(coordinate.x).ok()?;
    let y = usize::try_from(coordinate.y).ok()?;
    let z = usize::try_from(coordinate.z).ok()?;
    let width = usize::try_from(extent.width).ok()?;
    let height = usize::try_from(extent.height).ok()?;
    let depth = usize::try_from(extent.depth).ok()?;
    if x >= width || y >= height || z >= depth {
        return None;
    }
    z.checked_mul(height)?
        .checked_add(y)?
        .checked_mul(width)?
        .checked_add(x)
}

struct RegionBounds {
    start_x: i64,
    start_y: i64,
    start_z: i64,
    end_x: i64,
    end_y: i64,
    end_z: i64,
}

impl RegionBounds {
    fn new(region: VoxelRegion) -> Option<Self> {
        if region.extent.is_empty() {
            return None;
        }
        let end_x = i64::from(region.origin.x).checked_add(i64::from(region.extent.width))?;
        let end_y = i64::from(region.origin.y).checked_add(i64::from(region.extent.height))?;
        let end_z = i64::from(region.origin.z).checked_add(i64::from(region.extent.depth))?;
        let exclusive_coordinate_limit = i64::from(i32::MAX) + 1;
        if end_x > exclusive_coordinate_limit
            || end_y > exclusive_coordinate_limit
            || end_z > exclusive_coordinate_limit
        {
            return None;
        }
        Some(Self {
            start_x: i64::from(region.origin.x),
            start_y: i64::from(region.origin.y),
            start_z: i64::from(region.origin.z),
            end_x,
            end_y,
            end_z,
        })
    }

    fn coordinates(&self) -> impl Iterator<Item = VoxelCoordinate> + '_ {
        (self.start_z..self.end_z).flat_map(move |z| {
            (self.start_y..self.end_y).flat_map(move |y| {
                (self.start_x..self.end_x).filter_map(move |x| {
                    Some(VoxelCoordinate::new(
                        i32::try_from(x).ok()?,
                        i32::try_from(y).ok()?,
                        i32::try_from(z).ok()?,
                    ))
                })
            })
        })
    }
}

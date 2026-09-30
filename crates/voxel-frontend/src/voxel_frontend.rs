use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::{Arc, RwLock};
use thiserror::Error;

#[cfg(test)]
mod brick_storage_tests;
mod cell_enumeration;
#[cfg(test)]
mod cell_enumeration_tests;
mod page_table;
mod residency;
pub use residency::{VoxelResidencyCopies, VoxelResidencySelection, VoxelResidencySelectionId};
#[cfg(feature = "qualification")]
mod qualification_views;
#[cfg(feature = "qualification")]
pub use qualification_views::QualificationViewError;
#[cfg(test)]
mod sharing_tests;
mod sparse_publication;
#[cfg(test)]
mod sparse_publication_tests;
mod storage_counters;
mod storage_tier;
mod streamed_edits;
mod streamed_publication;
pub use cell_enumeration::{VoxelCell, VoxelCellCoordinate, VoxelCellEnumeration};
use page_table::PageTable;
use sparse_publication::ValidatedBatch;
#[cfg(feature = "qualification")]
pub use storage_counters::{
    EnumerationWorkCounters, PublicationWorkCounters, StorageWorkCounters, ValidationWorkCounters,
    count_storage_work,
};
use storage_counters::{record_publication_values_written, record_staged_values_allocated};
use storage_tier::{BrickGrid, SparseStorage, Storage};
pub use streamed_edits::StreamedEditStatistics;
use streamed_publication::{MaterializationCache, ReadStorage, StreamedScene};
pub use streamed_publication::{
    MaterializationCacheStats, StreamedVoxelScene, StreamedVoxelVolume, VoxelSourceError,
    VoxelVolumeSource,
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum StorageTier {
    #[default]
    Dense,
    SparsePages,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum VoxelRegionContent {
    Uniform(VoxelValue),
    Mixed,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct VoxelSceneId(Arc<str>);

impl VoxelSceneId {
    pub fn new(identity: impl Into<String>) -> Self {
        Self(Arc::from(identity.into()))
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
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

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct VoxelSceneRevision(u64);

impl VoxelSceneRevision {
    pub fn new(revision: u64) -> Self {
        Self(revision)
    }

    pub fn checked_successor(self) -> Option<Self> {
        self.0.checked_add(1).map(Self)
    }

    pub fn is_newer_than(self, other: Self) -> bool {
        self.0 > other.0
    }
}

impl fmt::Display for VoxelSceneRevision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
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

    fn contains(self, coordinate: VoxelCoordinate) -> bool {
        coordinate
            .components()
            .into_iter()
            .zip(self.dimensions())
            .all(|(component, extent)| {
                u32::try_from(component).is_ok_and(|component| component < extent)
            })
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

    pub fn origin(self) -> VoxelCoordinate {
        self.origin
    }

    pub fn extent(self) -> VoxelExtent {
        self.extent
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
    storage_tier: StorageTier,
}

impl DenseVoxelVolume {
    pub fn new(metadata: VoxelVolumeMetadata, batches: Vec<DenseVoxelBatch>) -> Self {
        Self {
            metadata,
            batches,
            storage_tier: StorageTier::Dense,
        }
    }

    pub fn with_storage_tier(mut self, storage_tier: StorageTier) -> Self {
        self.storage_tier = storage_tier;
        self
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
    pub fn with_storage_tier(mut self, storage_tier: StorageTier) -> Self {
        for volume in &mut self.volumes {
            volume.storage_tier = storage_tier;
        }
        self
    }
}

/// The value of every coordinate that a sparse volume's batches omit.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SparseVoxelBackground {
    #[default]
    Empty,
}

#[derive(Clone, Debug)]
pub struct VoxelRegionFill {
    region: VoxelRegion,
    value: VoxelValue,
}

impl VoxelRegionFill {
    pub fn new(region: VoxelRegion, value: VoxelValue) -> Self {
        Self { region, value }
    }
}

/// Every coordinate in a batch's region counts as supplied, including explicitly supplied
/// `Empty`, so the regions of one volume's batches must not intersect.
#[derive(Clone, Debug)]
pub enum SparseVoxelBatch {
    Fill(VoxelRegionFill),
    Detail(DenseVoxelBatch),
}

#[derive(Clone, Debug)]
pub struct SparseVoxelVolume {
    metadata: VoxelVolumeMetadata,
    background: SparseVoxelBackground,
    batches: Vec<SparseVoxelBatch>,
    storage_tier: StorageTier,
}

impl SparseVoxelVolume {
    pub fn new(
        metadata: VoxelVolumeMetadata,
        background: SparseVoxelBackground,
        batches: Vec<SparseVoxelBatch>,
    ) -> Self {
        Self {
            metadata,
            background,
            batches,
            storage_tier: StorageTier::Dense,
        }
    }

    pub fn with_storage_tier(mut self, storage_tier: StorageTier) -> Self {
        self.storage_tier = storage_tier;
        self
    }
}

#[derive(Clone, Debug)]
pub struct SparseVoxelScene {
    identity: VoxelSceneId,
    revision: VoxelSceneRevision,
    materials: Vec<VoxelMaterial>,
    volumes: Vec<SparseVoxelVolume>,
}

impl SparseVoxelScene {
    pub fn new(
        identity: VoxelSceneId,
        revision: VoxelSceneRevision,
        materials: Vec<VoxelMaterial>,
        volumes: Vec<SparseVoxelVolume>,
    ) -> Self {
        Self {
            identity,
            revision,
            materials,
            volumes,
        }
    }

    pub fn with_storage_tier(mut self, storage_tier: StorageTier) -> Self {
        for volume in &mut self.volumes {
            volume.storage_tier = storage_tier;
        }
        self
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VoxelEdit {
    volume_identity: VoxelVolumeId,
    coordinate: VoxelCoordinate,
    value: VoxelValue,
}

impl VoxelEdit {
    pub fn new(
        volume_identity: VoxelVolumeId,
        coordinate: VoxelCoordinate,
        value: VoxelValue,
    ) -> Self {
        Self {
            volume_identity,
            coordinate,
            value,
        }
    }

    pub fn volume_identity(&self) -> &VoxelVolumeId {
        &self.volume_identity
    }

    pub fn coordinate(&self) -> VoxelCoordinate {
        self.coordinate
    }

    pub fn value(&self) -> &VoxelValue {
        &self.value
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VoxelEditCommand {
    edits: Vec<VoxelEdit>,
}

impl VoxelEditCommand {
    pub fn new(
        volume_identity: VoxelVolumeId,
        coordinate: VoxelCoordinate,
        value: VoxelValue,
    ) -> Self {
        Self::from_edits(vec![VoxelEdit::new(volume_identity, coordinate, value)])
    }

    pub fn from_edits(edits: Vec<VoxelEdit>) -> Self {
        Self { edits }
    }

    pub fn edits(&self) -> &[VoxelEdit] {
        &self.edits
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VoxelChangedRegion {
    volume_identity: VoxelVolumeId,
    region: VoxelRegion,
}

impl VoxelChangedRegion {
    pub fn volume_identity(&self) -> &VoxelVolumeId {
        &self.volume_identity
    }

    pub fn region(&self) -> VoxelRegion {
        self.region
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VoxelChangeSet {
    scene_identity: VoxelSceneId,
    predecessor_revision: VoxelSceneRevision,
    successor_revision: VoxelSceneRevision,
    changed_regions: Vec<VoxelChangedRegion>,
}

impl VoxelChangeSet {
    pub fn scene_identity(&self) -> &VoxelSceneId {
        &self.scene_identity
    }

    pub fn predecessor_revision(&self) -> VoxelSceneRevision {
        self.predecessor_revision
    }

    pub fn successor_revision(&self) -> VoxelSceneRevision {
        self.successor_revision
    }

    pub fn changed_regions(&self) -> &[VoxelChangedRegion] {
        &self.changed_regions
    }
}

pub enum VoxelEditOutcome {
    Unchanged(VoxelSceneView),
    Changed {
        view: VoxelSceneView,
        change_set: VoxelChangeSet,
    },
}

impl VoxelEditOutcome {
    pub fn view(&self) -> &VoxelSceneView {
        match self {
            Self::Unchanged(view) | Self::Changed { view, .. } => view,
        }
    }

    pub fn change_set(&self) -> Option<&VoxelChangeSet> {
        match self {
            Self::Unchanged(_) => None,
            Self::Changed { change_set, .. } => Some(change_set),
        }
    }
}

#[derive(Debug, Error)]
pub enum VoxelFrontendError {
    #[error("Voxel Residency Selection must contain at least one Voxel Volume")]
    EmptyResidencySelection,
    #[error("Voxel Residency Selection belongs to a different Voxel Scene")]
    ResidencySceneMismatch,
    #[error("Voxel Residency Selection identity already names a different selection")]
    ResidencyIdentityConflict,
    #[error("a newer Required Voxel Residency Selection superseded materialization")]
    ResidencySuperseded,
    #[error("streamed Voxel Volume {identity:?} requires sparse Storage Tier")]
    StreamedStorageTier { identity: VoxelVolumeId },
    #[error("source for Voxel Volume {identity:?} belongs to a different Voxel Scene")]
    SourceSceneMismatch { identity: VoxelVolumeId },
    #[error("source for Voxel Volume {identity:?} requires a non-empty material palette")]
    EmptySourceMaterialPalette { identity: VoxelVolumeId },
    #[error("source output for Voxel Volume {identity:?} disagrees with its declared metadata")]
    SourceMetadataMismatch { identity: VoxelVolumeId },
    #[error("the query-only materialization copy is already held")]
    QueryOnlyCopyBusy,
    #[error("the materialization cache could not reserve storage for a copy")]
    MaterializationCacheExhausted,
    #[error("the requested materialization is already being generated")]
    MaterializationInProgress,
    #[error("generation of Voxel Volume {identity:?} failed: {source}")]
    VolumeGeneration {
        identity: VoxelVolumeId,
        #[source]
        source: VoxelSourceError,
    },
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
    #[error("Voxel Scene has too many Voxel Materials")]
    TooManyMaterials,
    #[error("Voxel Volume {identity:?} has an empty extent")]
    EmptyVolumeExtent { identity: VoxelVolumeId },
    #[error("Voxel Volume {identity:?} has invalid scene origin or voxel size")]
    InvalidVolumeMetadata { identity: VoxelVolumeId },
    #[error("Voxel Volume {identity:?} is too large to address")]
    VolumeTooLarge { identity: VoxelVolumeId },
    #[error("storage for Voxel Volume {identity:?} could not be allocated")]
    VolumeAllocation { identity: VoxelVolumeId },
    #[error("batch {batch_index} for Voxel Volume {identity:?} has an empty region")]
    EmptyBatchRegion {
        identity: VoxelVolumeId,
        batch_index: usize,
    },
    #[error("batch {batch_index} for Voxel Volume {identity:?} has invalid coordinate bounds")]
    InvalidBatchBounds {
        identity: VoxelVolumeId,
        batch_index: usize,
    },
    #[error("batch {batch_index} for Voxel Volume {identity:?} is outside the volume extent")]
    BatchOutsideVolume {
        identity: VoxelVolumeId,
        batch_index: usize,
    },
    #[error(
        "batch {batch_index} for Voxel Volume {identity:?} contains {actual} values but its region requires {expected}"
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
    #[error(
        "sparse batches {first_batch_index} and {second_batch_index} for Voxel Volume {identity:?} overlap"
    )]
    OverlappingBatches {
        identity: VoxelVolumeId,
        first_batch_index: usize,
        second_batch_index: usize,
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
    #[error("Voxel Edit Command coordinate {coordinate:?} is outside Voxel Volume {identity:?}")]
    EditCoordinateOutsideVolume {
        identity: VoxelVolumeId,
        coordinate: VoxelCoordinate,
    },
    #[error(
        "Voxel Edit Command for Voxel Volume {volume_identity:?} coordinate {coordinate:?} references unknown Voxel Material {material_identity:?}"
    )]
    UnknownEditMaterial {
        volume_identity: VoxelVolumeId,
        coordinate: VoxelCoordinate,
        material_identity: VoxelMaterialId,
    },
    #[error(
        "Voxel Scene {scene_identity:?} revision {revision} has no immediate successor for a value-changing Voxel Edit Command"
    )]
    RevisionOverflow {
        scene_identity: VoxelSceneId,
        revision: VoxelSceneRevision,
    },
    #[error(
        "Voxel Region buffer for Voxel Volume {identity:?} contains {actual} values but requires {expected}"
    )]
    RegionBufferSize {
        identity: VoxelVolumeId,
        expected: usize,
        actual: usize,
    },
    #[error("Voxel Region request for Voxel Volume {identity:?} has an empty extent")]
    EmptyRegionRequest { identity: VoxelVolumeId },
    #[error("Voxel Region request for Voxel Volume {identity:?} has invalid coordinate bounds")]
    InvalidRegionBounds { identity: VoxelVolumeId },
    #[error("Voxel Region result for Voxel Volume {identity:?} could not be allocated")]
    RegionReadAllocation { identity: VoxelVolumeId },
    #[error(
        "Voxel Cell Grid enumeration of Voxel Volume {identity:?} requires a non-zero batch capacity"
    )]
    ZeroCellBatchCapacity { identity: VoxelVolumeId },
    #[error(
        "Voxel Cell Grid cell edge {cell_edge} for Voxel Volume {identity:?} is not a power of two"
    )]
    InvalidCellEdge {
        identity: VoxelVolumeId,
        cell_edge: u32,
    },
    #[error(
        "Voxel Cell Grid enumeration of Voxel Volume {identity:?} could not allocate its working state or output"
    )]
    CellEnumerationAllocation { identity: VoxelVolumeId },
    #[error(
        "Voxel Cell Grid enumeration of Voxel Volume {identity:?} could not traverse its storage"
    )]
    CellEnumerationTraversal { identity: VoxelVolumeId },
    #[error("Voxel Frontend state could not be accessed")]
    StateUnavailable,
}

#[derive(Default)]
pub struct VoxelFrontend {
    published: RwLock<Option<Arc<PublishedScene>>>,
    materialization_cache: Arc<MaterializationCache>,
    residency: std::sync::Mutex<residency::ResidencyState>,
    residency_worker: std::sync::Mutex<()>,
}

impl VoxelFrontend {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn publish(&self, scene: DenseVoxelScene) -> Result<VoxelSceneView, VoxelFrontendError> {
        self.install(PublishedScene::new(
            scene.identity,
            scene.revision,
            scene.materials,
            scene.volumes,
        )?)
    }

    pub fn publish_sparse(
        &self,
        scene: SparseVoxelScene,
    ) -> Result<VoxelSceneView, VoxelFrontendError> {
        self.install(PublishedScene::new(
            scene.identity,
            scene.revision,
            scene.materials,
            scene.volumes,
        )?)
    }

    fn install(&self, published: PublishedScene) -> Result<VoxelSceneView, VoxelFrontendError> {
        let published = Arc::new(published);
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

    pub fn edit(&self, command: VoxelEditCommand) -> Result<VoxelEditOutcome, VoxelFrontendError> {
        let mut publication = self
            .published
            .write()
            .map_err(|_| VoxelFrontendError::StateUnavailable)?;
        let published = publication
            .as_ref()
            .ok_or(VoxelFrontendError::SceneNotPublished)?;
        let mut validated = HashMap::new();
        for edit in command.edits {
            let extent = published.volume_extent(&edit.volume_identity)?;
            if !extent.contains(edit.coordinate) {
                return Err(VoxelFrontendError::EditCoordinateOutsideVolume {
                    identity: edit.volume_identity.clone(),
                    coordinate: edit.coordinate,
                });
            }
            let value_index = match &edit.value {
                VoxelValue::Empty => MaterialIndex::EMPTY,
                VoxelValue::Occupied(material_identity) => *published
                    .material_indices
                    .get(material_identity)
                    .ok_or_else(|| VoxelFrontendError::UnknownEditMaterial {
                        volume_identity: edit.volume_identity.clone(),
                        coordinate: edit.coordinate,
                        material_identity: material_identity.clone(),
                    })?,
            };
            // Validate even entries overwritten later so invalid input always rejects the command.
            validated.insert((edit.volume_identity, edit.coordinate), value_index);
        }
        let mut changes_by_volume: HashMap<VoxelVolumeId, Vec<(VoxelCoordinate, MaterialIndex)>> =
            HashMap::new();
        let mut generated_values = HashMap::new();
        for ((identity, coordinate), value) in validated {
            let current = if let Some(streamed) = &published.streamed {
                let generated =
                    streamed.generated_value(&identity, coordinate, &published.material_indices)?;
                generated_values.insert((identity.clone(), coordinate), generated);
                streamed
                    .overlay
                    .value(&identity, coordinate)?
                    .unwrap_or(generated)
            } else {
                published
                    .volumes
                    .get(&identity)
                    .expect("validated unstreamed volume identities have storage")
                    .value(coordinate)
            };
            if current != value {
                changes_by_volume
                    .entry(identity)
                    .or_default()
                    .push((coordinate, value));
            }
        }
        if changes_by_volume.is_empty() {
            return Ok(VoxelEditOutcome::Unchanged(VoxelSceneView {
                published: Arc::clone(published),
            }));
        }
        let successor_revision = published.revision.checked_successor().ok_or_else(|| {
            VoxelFrontendError::RevisionOverflow {
                scene_identity: published.identity.clone(),
                revision: published.revision,
            }
        })?;
        let mut successor = PublishedScene::clone(published);
        successor.revision = successor_revision;
        if let Some(streamed) = &mut successor.streamed {
            streamed.overlay = streamed.overlay.successor(
                successor_revision,
                &changes_by_volume,
                &generated_values,
            )?;
        }
        let mut changed_regions = Vec::new();
        for (identity, changes) in changes_by_volume {
            if let Some(volume) = successor.volumes.get_mut(&identity) {
                *volume = volume.successor(&changes);
            }
            successor
                .volume_content_versions
                .insert(identity.clone(), successor_revision);
            changed_regions.extend(
                changes
                    .into_iter()
                    .map(|(coordinate, _)| VoxelChangedRegion {
                        volume_identity: identity.clone(),
                        region: VoxelRegion::new(coordinate, VoxelExtent::new(1, 1, 1)),
                    }),
            );
        }
        let change_set = VoxelChangeSet {
            scene_identity: successor.identity.clone(),
            predecessor_revision: published.revision,
            successor_revision,
            changed_regions,
        };
        let published = Arc::new(successor);
        *publication = Some(Arc::clone(&published));
        Ok(VoxelEditOutcome::Changed {
            view: VoxelSceneView { published },
            change_set,
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

    pub fn material(&self, identity: &VoxelMaterialId) -> Option<&VoxelMaterial> {
        let index = self.published.material_indices.get(identity)?;
        self.published.materials.get(index.0 as usize - 1)
    }

    pub fn volumes(&self) -> &[VoxelVolumeMetadata] {
        &self.published.volume_metadata
    }

    pub fn is_streamed(&self) -> bool {
        self.published.streamed.is_some()
    }

    pub fn volume_content_version(
        &self,
        volume_identity: &VoxelVolumeId,
    ) -> Result<VoxelSceneRevision, VoxelFrontendError> {
        self.published
            .volume_content_versions
            .get(volume_identity)
            .copied()
            .ok_or_else(|| VoxelFrontendError::UnknownVolumeIdentity {
                identity: volume_identity.clone(),
            })
    }

    /// Writes values in x-fastest, then y, then z order, relative to the region origin.
    /// The buffer must contain exactly width * height * depth values. Coordinates
    /// outside the volume are empty. Invalid requests leave the buffer unchanged.
    pub fn read_region_into(
        &self,
        volume_identity: &VoxelVolumeId,
        region: VoxelRegion,
        values: &mut [VoxelValue],
    ) -> Result<(), VoxelFrontendError> {
        let (extent, bounds, capacity) = self.region_read(volume_identity, region)?;
        if values.len() != capacity {
            return Err(VoxelFrontendError::RegionBufferSize {
                identity: volume_identity.clone(),
                expected: capacity,
                actual: values.len(),
            });
        }
        match self.region_storage(volume_identity, extent, &bounds)? {
            Some(volume) => {
                volume.read_region_into(&bounds, &self.published.palette_values, values)
            }
            None => values.fill(VoxelValue::Empty),
        }
        Ok(())
    }

    pub fn read_region(
        &self,
        volume_identity: &VoxelVolumeId,
        region: VoxelRegion,
    ) -> Result<Vec<VoxelSample>, VoxelFrontendError> {
        let (extent, bounds, capacity) = self.region_read(volume_identity, region)?;
        let mut samples = Vec::new();
        samples.try_reserve_exact(capacity).map_err(|_| {
            VoxelFrontendError::RegionReadAllocation {
                identity: volume_identity.clone(),
            }
        })?;
        let volume = self.region_storage(volume_identity, extent, &bounds)?;
        for coordinate in bounds.coordinates() {
            samples.push(VoxelSample {
                coordinate,
                value: self
                    .published
                    .palette_values
                    .get(
                        volume
                            .as_ref()
                            .map_or(MaterialIndex::EMPTY, |volume| volume.value(coordinate))
                            .0 as usize,
                    )
                    .cloned()
                    .unwrap_or(VoxelValue::Empty),
            });
        }
        Ok(samples)
    }

    pub fn region_content(
        &self,
        volume_identity: &VoxelVolumeId,
        region: VoxelRegion,
    ) -> Result<VoxelRegionContent, VoxelFrontendError> {
        let (extent, bounds) = self.region_bounds(volume_identity, region)?;
        let Some(volume) = self.region_storage(volume_identity, extent, &bounds)? else {
            return Ok(VoxelRegionContent::Uniform(VoxelValue::Empty));
        };
        Ok(match volume.uniform_region(&bounds) {
            Some(index) => VoxelRegionContent::Uniform(
                self.published
                    .palette_values
                    .get(index.0 as usize)
                    .cloned()
                    .unwrap_or(VoxelValue::Empty),
            ),
            None => VoxelRegionContent::Mixed,
        })
    }

    /// Enumerates the non-empty cells of a volume on a Voxel Cell Grid with the given
    /// power-of-two `cell_edge`, in batches of at most `batch_capacity` cells. The enumeration
    /// holds a clone of this view, so later edits do not change what it produces.
    pub fn enumerate_cells(
        &self,
        volume_identity: &VoxelVolumeId,
        cell_edge: u32,
        batch_capacity: usize,
    ) -> Result<VoxelCellEnumeration, VoxelFrontendError> {
        VoxelCellEnumeration::new(
            self.clone(),
            volume_identity.clone(),
            cell_edge,
            batch_capacity,
        )
    }

    /// Estimated owned storage bytes, excluding allocator overhead and shared scene metadata.
    /// For streamed volumes, reports only the currently cached payload, without generating it.
    pub fn storage_bytes(
        &self,
        volume_identity: &VoxelVolumeId,
    ) -> Result<usize, VoxelFrontendError> {
        if self.published.streamed.is_some() {
            return self.published.streamed_storage_bytes(volume_identity);
        }
        self.published
            .volumes
            .get(volume_identity)
            .map(|volume| volume.storage_bytes())
            .ok_or_else(|| VoxelFrontendError::UnknownVolumeIdentity {
                identity: volume_identity.clone(),
            })
    }

    fn region_read(
        &self,
        volume_identity: &VoxelVolumeId,
        region: VoxelRegion,
    ) -> Result<(VoxelExtent, RegionBounds, usize), VoxelFrontendError> {
        let (extent, bounds) = self.region_bounds(volume_identity, region)?;
        let capacity =
            region
                .extent
                .value_count()
                .ok_or_else(|| VoxelFrontendError::InvalidRegionBounds {
                    identity: volume_identity.clone(),
                })?;
        Ok((extent, bounds, capacity))
    }

    fn region_bounds(
        &self,
        volume_identity: &VoxelVolumeId,
        region: VoxelRegion,
    ) -> Result<(VoxelExtent, RegionBounds), VoxelFrontendError> {
        let extent = self.published.volume_extent(volume_identity)?;
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
        Ok((extent, bounds))
    }

    fn region_storage(
        &self,
        volume_identity: &VoxelVolumeId,
        extent: VoxelExtent,
        bounds: &RegionBounds,
    ) -> Result<Option<ReadStorage>, VoxelFrontendError> {
        if bounds.end_x <= 0
            || bounds.end_y <= 0
            || bounds.end_z <= 0
            || bounds.start_x >= i64::from(extent.width)
            || bounds.start_y >= i64::from(extent.height)
            || bounds.start_z >= i64::from(extent.depth)
        {
            return Ok(None);
        }
        self.published.read_storage(volume_identity).map(Some)
    }
}

#[derive(Clone)]
struct PublishedScene {
    identity: VoxelSceneId,
    revision: VoxelSceneRevision,
    materials: Arc<[VoxelMaterial]>,
    material_indices: Arc<HashMap<VoxelMaterialId, MaterialIndex>>,
    palette_values: Arc<[VoxelValue]>,
    volume_metadata: Arc<[VoxelVolumeMetadata]>,
    volumes: HashMap<VoxelVolumeId, Arc<dyn Storage>>,
    volume_content_versions: HashMap<VoxelVolumeId, VoxelSceneRevision>,
    streamed: Option<StreamedScene>,
}

trait VolumeInput {
    fn metadata(&self) -> &VoxelVolumeMetadata;
    fn storage(
        &self,
        materials: &HashMap<VoxelMaterialId, MaterialIndex>,
    ) -> Result<Arc<dyn Storage>, VoxelFrontendError>;
}

impl VolumeInput for DenseVoxelVolume {
    fn metadata(&self) -> &VoxelVolumeMetadata {
        &self.metadata
    }

    fn storage(
        &self,
        materials: &HashMap<VoxelMaterialId, MaterialIndex>,
    ) -> Result<Arc<dyn Storage>, VoxelFrontendError> {
        Ok(match self.storage_tier {
            StorageTier::Dense => Arc::new(DenseStorage::from_batches(self, materials)?),
            StorageTier::SparsePages => {
                let grid = brick_grid(&self.metadata)?;
                Arc::new(SparseStorage::from_dense(
                    &DenseStorage::from_batches(self, materials)?,
                    grid,
                ))
            }
        })
    }
}

impl VolumeInput for SparseVoxelVolume {
    fn metadata(&self) -> &VoxelVolumeMetadata {
        &self.metadata
    }

    fn storage(
        &self,
        materials: &HashMap<VoxelMaterialId, MaterialIndex>,
    ) -> Result<Arc<dyn Storage>, VoxelFrontendError> {
        // Absent sparse bricks and unwritten dense values both mean Empty.
        let SparseVoxelBackground::Empty = self.background;
        let batches = sparse_publication::validate(self, materials)?;
        let identity = &self.metadata.identity;
        Ok(match self.storage_tier {
            StorageTier::Dense => Arc::new(DenseStorage::from_sparse_batches(
                self.metadata.extent,
                &batches,
                identity,
            )?),
            StorageTier::SparsePages => Arc::new(SparseStorage::from_batches(
                self.metadata.extent,
                brick_grid(&self.metadata)?,
                &batches,
                identity,
            )?),
        })
    }
}

fn brick_grid(metadata: &VoxelVolumeMetadata) -> Result<BrickGrid, VoxelFrontendError> {
    BrickGrid::new(metadata.extent).ok_or_else(|| VoxelFrontendError::VolumeTooLarge {
        identity: metadata.identity.clone(),
    })
}

impl PublishedScene {
    fn new(
        scene_identity: VoxelSceneId,
        revision: VoxelSceneRevision,
        materials: Vec<VoxelMaterial>,
        volumes: Vec<impl VolumeInput>,
    ) -> Result<Self, VoxelFrontendError> {
        if scene_identity.0.is_empty() {
            return Err(VoxelFrontendError::EmptySceneIdentity);
        }
        let mut material_identities = HashMap::new();
        let mut palette_values = vec![VoxelValue::Empty];
        for material in &materials {
            if material.identity.0.is_empty() {
                return Err(VoxelFrontendError::EmptyMaterialIdentity);
            }
            if material_identities.contains_key(&material.identity) {
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
            let index = u32::try_from(palette_values.len())
                .ok()
                .filter(|index| *index != MaterialIndex::UNSUPPLIED.0)
                .ok_or(VoxelFrontendError::TooManyMaterials)?;
            material_identities.insert(material.identity.clone(), MaterialIndex(index));
            palette_values.push(VoxelValue::Occupied(material.identity.clone()));
        }

        let mut volume_identities = HashSet::new();
        let mut volume_metadata = Vec::with_capacity(volumes.len());
        let mut storage_by_volume = HashMap::with_capacity(volumes.len());
        for volume in volumes {
            let metadata = volume.metadata();
            let identity = metadata.identity.clone();
            if identity.0.is_empty() {
                return Err(VoxelFrontendError::EmptyVolumeIdentity);
            }
            if !volume_identities.insert(identity.clone()) {
                return Err(VoxelFrontendError::DuplicateVolumeIdentity { identity });
            }
            validate_volume_metadata(metadata)?;
            let storage = volume.storage(&material_identities)?;
            volume_metadata.push(metadata.clone());
            storage_by_volume.insert(identity, storage);
        }

        let volume_content_versions = storage_by_volume
            .keys()
            .cloned()
            .map(|identity| (identity, revision))
            .collect();
        Ok(Self {
            identity: scene_identity,
            revision,
            materials: materials.into(),
            material_indices: Arc::new(material_identities),
            palette_values: palette_values.into(),
            volume_metadata: volume_metadata.into(),
            volumes: storage_by_volume,
            volume_content_versions,
            streamed: None,
        })
    }
}

fn validate_volume_metadata(metadata: &VoxelVolumeMetadata) -> Result<(), VoxelFrontendError> {
    if metadata.extent.is_empty() {
        return Err(VoxelFrontendError::EmptyVolumeExtent {
            identity: metadata.identity.clone(),
        });
    }
    if RegionBounds::new(VoxelRegion::new(
        VoxelCoordinate::new(0, 0, 0),
        metadata.extent,
    ))
    .is_none()
    {
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct MaterialIndex(u32);

impl MaterialIndex {
    const EMPTY: Self = Self(0);
    const UNSUPPLIED: Self = Self(u32::MAX);
}

#[derive(Clone)]
struct DenseStorage {
    extent: VoxelExtent,
    pages: Arc<PageTable<Arc<Vec<MaterialIndex>>>>,
}

impl DenseStorage {
    // Keep the existing page boundaries so edits retain the same sharing granularity.
    const PAGE_VALUES: usize = 256;

    fn get(&self, index: usize) -> Option<&MaterialIndex> {
        self.pages
            .get(&(index / Self::PAGE_VALUES))?
            .get(index % Self::PAGE_VALUES)
    }

    fn get_mut(&mut self, index: usize) -> Option<&mut MaterialIndex> {
        let page = Arc::make_mut(&mut self.pages).get_mut(&(index / Self::PAGE_VALUES))?;
        Arc::make_mut(page).get_mut(index % Self::PAGE_VALUES)
    }

    fn from_batches(
        volume: &DenseVoxelVolume,
        materials: &HashMap<VoxelMaterialId, MaterialIndex>,
    ) -> Result<Self, VoxelFrontendError> {
        let value_count = volume.metadata.extent.value_count().ok_or_else(|| {
            VoxelFrontendError::VolumeTooLarge {
                identity: volume.metadata.identity.clone(),
            }
        })?;
        // Preserve publication limits for the public dense input representation.
        for element_size in [size_of::<Option<VoxelValue>>(), size_of::<VoxelValue>()] {
            if value_count
                .checked_mul(element_size)
                .is_none_or(|bytes| bytes > isize::MAX as usize)
            {
                return Err(VoxelFrontendError::VolumeTooLarge {
                    identity: volume.metadata.identity.clone(),
                });
            }
        }
        let mut supplied_count = 0usize;
        for (batch_index, batch) in volume.batches.iter().enumerate() {
            validate_dense_batch(
                batch,
                batch_index,
                &volume.metadata.identity,
                volume.metadata.extent,
            )?;
            supplied_count = supplied_count
                .checked_add(batch.values.len())
                .ok_or_else(|| VoxelFrontendError::VolumeTooLarge {
                    identity: volume.metadata.identity.clone(),
                })?;
        }
        if supplied_count < value_count {
            return Err(VoxelFrontendError::IncompleteVolume {
                identity: volume.metadata.identity.clone(),
            });
        }
        let mut storage = Self::filled(
            volume.metadata.extent,
            value_count,
            MaterialIndex::UNSUPPLIED,
            &volume.metadata.identity,
        )?;
        for (batch_index, batch) in volume.batches.iter().enumerate() {
            let bounds = validate_dense_batch(
                batch,
                batch_index,
                &volume.metadata.identity,
                volume.metadata.extent,
            )?;
            let expected = batch.values.len();

            for (batch_value_index, coordinate) in bounds.coordinates().enumerate() {
                let value = batch.values.get(batch_value_index).ok_or_else(|| {
                    VoxelFrontendError::BatchValueCount {
                        identity: volume.metadata.identity.clone(),
                        batch_index,
                        expected,
                        actual: batch.values.len(),
                    }
                })?;
                let value_index = match value {
                    VoxelValue::Empty => MaterialIndex::EMPTY,
                    VoxelValue::Occupied(material_identity) => *materials
                        .get(material_identity)
                        .ok_or_else(|| VoxelFrontendError::UnknownMaterialReference {
                            volume_identity: volume.metadata.identity.clone(),
                            coordinate,
                            material_identity: material_identity.clone(),
                        })?,
                };
                let storage_index =
                    dense_index(volume.metadata.extent, coordinate).ok_or_else(|| {
                        VoxelFrontendError::BatchOutsideVolume {
                            identity: volume.metadata.identity.clone(),
                            batch_index,
                        }
                    })?;
                let destination = storage.get_mut(storage_index).ok_or_else(|| {
                    VoxelFrontendError::BatchOutsideVolume {
                        identity: volume.metadata.identity.clone(),
                        batch_index,
                    }
                })?;
                if *destination != MaterialIndex::UNSUPPLIED {
                    return Err(VoxelFrontendError::DuplicateVoxelCoordinate {
                        identity: volume.metadata.identity.clone(),
                        coordinate,
                    });
                }
                *destination = value_index;
            }
            record_publication_values_written(batch.values.len());
        }
        if storage
            .pages
            .values()
            .any(|page| page.contains(&MaterialIndex::UNSUPPLIED))
        {
            return Err(VoxelFrontendError::IncompleteVolume {
                identity: volume.metadata.identity.clone(),
            });
        }
        Ok(storage)
    }

    fn filled(
        extent: VoxelExtent,
        value_count: usize,
        value: MaterialIndex,
        identity: &VoxelVolumeId,
    ) -> Result<Self, VoxelFrontendError> {
        let mut pages = PageTable::new(value_count.div_ceil(Self::PAGE_VALUES));
        for start in (0..value_count).step_by(Self::PAGE_VALUES) {
            let count = (value_count - start).min(Self::PAGE_VALUES);
            let mut page = Vec::new();
            page.try_reserve_exact(count)
                .map_err(|_| VoxelFrontendError::VolumeAllocation {
                    identity: identity.clone(),
                })?;
            page.resize(count, value);
            pages.insert(start / Self::PAGE_VALUES, Arc::new(page));
        }
        record_staged_values_allocated(value_count);
        Ok(Self {
            extent,
            pages: Arc::new(pages),
        })
    }

    fn from_sparse_batches(
        extent: VoxelExtent,
        batches: &[ValidatedBatch],
        identity: &VoxelVolumeId,
    ) -> Result<Self, VoxelFrontendError> {
        let value_count =
            extent
                .value_count()
                .ok_or_else(|| VoxelFrontendError::VolumeTooLarge {
                    identity: identity.clone(),
                })?;
        let mut storage = Self::filled(extent, value_count, MaterialIndex::EMPTY, identity)?;
        for batch in batches {
            // The storage starts out empty, so an Empty fill writes nothing.
            if batch.fill_value() == Some(MaterialIndex::EMPTY) {
                continue;
            }
            for (coordinate, value) in batch.values() {
                if let Some(destination) =
                    dense_index(extent, coordinate).and_then(|index| storage.get_mut(index))
                {
                    *destination = value;
                }
            }
            record_publication_values_written(batch.value_count());
        }
        Ok(storage)
    }

    fn value(&self, coordinate: VoxelCoordinate) -> MaterialIndex {
        dense_index(self.extent, coordinate)
            .and_then(|index| self.get(index))
            .copied()
            .unwrap_or(MaterialIndex::EMPTY)
    }
}

fn validate_dense_batch(
    batch: &DenseVoxelBatch,
    batch_index: usize,
    volume_identity: &VoxelVolumeId,
    volume_extent: VoxelExtent,
) -> Result<RegionBounds, VoxelFrontendError> {
    let bounds = validate_batch_region(batch.region, batch_index, volume_identity, volume_extent)?;
    let expected = batch.region.extent.value_count().ok_or_else(|| {
        VoxelFrontendError::InvalidBatchBounds {
            identity: volume_identity.clone(),
            batch_index,
        }
    })?;
    if batch.values.len() != expected {
        return Err(VoxelFrontendError::BatchValueCount {
            identity: volume_identity.clone(),
            batch_index,
            expected,
            actual: batch.values.len(),
        });
    }

    Ok(bounds)
}

fn validate_batch_region(
    region: VoxelRegion,
    batch_index: usize,
    volume_identity: &VoxelVolumeId,
    volume_extent: VoxelExtent,
) -> Result<RegionBounds, VoxelFrontendError> {
    let bounds = RegionBounds::new(region).ok_or_else(|| {
        if region.extent.is_empty() {
            VoxelFrontendError::EmptyBatchRegion {
                identity: volume_identity.clone(),
                batch_index,
            }
        } else {
            VoxelFrontendError::InvalidBatchBounds {
                identity: volume_identity.clone(),
                batch_index,
            }
        }
    })?;
    if bounds.start_x < 0
        || bounds.start_y < 0
        || bounds.start_z < 0
        || u32::try_from(bounds.end_x).ok() > Some(volume_extent.width)
        || u32::try_from(bounds.end_y).ok() > Some(volume_extent.height)
        || u32::try_from(bounds.end_z).ok() > Some(volume_extent.depth)
    {
        return Err(VoxelFrontendError::BatchOutsideVolume {
            identity: volume_identity.clone(),
            batch_index,
        });
    }
    Ok(bounds)
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

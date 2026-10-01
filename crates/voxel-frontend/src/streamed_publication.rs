use super::streamed_edits::OverlayVersion;
use super::*;
use std::ops::Deref;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, Weak};

#[derive(Debug, Error)]
pub enum VoxelSourceError {
    #[error("Voxel Volume generation failed: {message}")]
    Generation { message: String },
    #[error("Voxel Volume generation could not allocate its payload")]
    Allocation,
}

/// An immutable recipe bound to one scene identity, including all of its parameters.
/// Implementations must be deterministic within a process run, total over the declared
/// extent, and return only declared materials. Omitted sparse coordinates mean Empty.
pub trait VoxelVolumeSource: Send + Sync {
    fn scene_id(&self) -> &VoxelSceneId;
    fn requires_materials(&self) -> bool;
    /// Evaluates one in-bounds coordinate without building a volume payload.
    /// Its value must agree with `materialize` for the same immutable recipe.
    fn value(&self, coordinate: VoxelCoordinate) -> Result<VoxelValue, VoxelSourceError>;
    fn materialize(&self) -> Result<SparseVoxelVolume, VoxelSourceError>;
}

#[derive(Clone)]
pub struct StreamedVoxelVolume {
    metadata: VoxelVolumeMetadata,
    source: Arc<dyn VoxelVolumeSource>,
    storage_tier: StorageTier,
}

impl StreamedVoxelVolume {
    pub fn new(metadata: VoxelVolumeMetadata, source: Arc<dyn VoxelVolumeSource>) -> Self {
        Self {
            metadata,
            source,
            storage_tier: StorageTier::SparsePages,
        }
    }

    pub fn with_storage_tier(mut self, storage_tier: StorageTier) -> Self {
        self.storage_tier = storage_tier;
        self
    }
}

#[derive(Clone)]
pub struct StreamedVoxelScene {
    identity: VoxelSceneId,
    revision: VoxelSceneRevision,
    materials: Vec<VoxelMaterial>,
    volumes: Vec<StreamedVoxelVolume>,
}

impl StreamedVoxelScene {
    pub fn new(
        identity: VoxelSceneId,
        revision: VoxelSceneRevision,
        materials: Vec<VoxelMaterial>,
        volumes: Vec<StreamedVoxelVolume>,
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

impl VoxelFrontend {
    /// Publishes the complete fixed volume catalog without generating any payload.
    /// Reads share the residency cache bounded by this frontend's residency limits.
    /// Concurrent requests needing a second query-only copy return a typed error.
    pub fn publish_streamed(
        &self,
        scene: StreamedVoxelScene,
    ) -> Result<VoxelSceneView, VoxelFrontendError> {
        if self.residency_limits.is_none() {
            return Err(VoxelFrontendError::ResidencyLimitsUndeclared);
        }
        let mut published = PublishedScene::new(
            scene.identity,
            scene.revision,
            scene.materials,
            Vec::<DenseVoxelVolume>::new(),
        )?;
        let mut metadata = Vec::with_capacity(scene.volumes.len());
        let mut volumes = HashMap::with_capacity(scene.volumes.len());
        for volume in scene.volumes {
            let identity = volume.metadata.identity.clone();
            if identity.0.is_empty() {
                return Err(VoxelFrontendError::EmptyVolumeIdentity);
            }
            if volumes.contains_key(&identity) {
                return Err(VoxelFrontendError::DuplicateVolumeIdentity { identity });
            }
            validate_volume_metadata(&volume.metadata)?;
            brick_grid(&volume.metadata)?;
            if volume.storage_tier != StorageTier::SparsePages {
                return Err(VoxelFrontendError::StreamedStorageTier { identity });
            }
            if volume.source.scene_id() != &published.identity {
                return Err(VoxelFrontendError::SourceSceneMismatch { identity });
            }
            if published.materials.is_empty() && volume.source.requires_materials() {
                return Err(VoxelFrontendError::EmptySourceMaterialPalette { identity });
            }
            metadata.push(volume.metadata.clone());
            published
                .volume_content_versions
                .insert(identity.clone(), published.revision);
            volumes.insert(identity, volume);
        }
        published.volume_metadata = metadata.into();
        published.streamed = Some(StreamedScene {
            volumes: Arc::new(volumes),
            cache: self.materialization_cache.clone(),
            overlay: OverlayVersion::new(published.revision),
        });
        self.install(published)
    }
}

#[derive(Clone)]
pub(super) struct StreamedScene {
    volumes: Arc<HashMap<VoxelVolumeId, StreamedVoxelVolume>>,
    cache: Arc<MaterializationCache>,
    pub(super) overlay: Arc<OverlayVersion>,
}

impl StreamedScene {
    pub(super) fn generated_value(
        &self,
        identity: &VoxelVolumeId,
        coordinate: VoxelCoordinate,
        materials: &HashMap<VoxelMaterialId, MaterialIndex>,
    ) -> Result<MaterialIndex, VoxelFrontendError> {
        let volume = self
            .volumes
            .get(identity)
            .expect("validated streamed volume identities have sources");
        match volume.source.value(coordinate).map_err(|source| {
            VoxelFrontendError::VolumeGeneration {
                identity: identity.clone(),
                source,
            }
        })? {
            VoxelValue::Empty => Ok(MaterialIndex::EMPTY),
            VoxelValue::Occupied(material_identity) => materials
                .get(&material_identity)
                .copied()
                .ok_or_else(|| VoxelFrontendError::UnknownMaterialReference {
                    volume_identity: identity.clone(),
                    coordinate,
                    material_identity,
                }),
        }
    }
}

#[derive(Clone, Eq, Hash, PartialEq)]
struct MaterializationKey {
    scene: VoxelSceneId,
    volume: VoxelVolumeId,
    content_version: VoxelSceneRevision,
}

/// Copies includes reservations inside generation, before any payload allocation.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MaterializationCacheStats {
    pub copies: usize,
    pub peak_copies: usize,
    pub generating: usize,
    pub query_only_copies: usize,
    pub storage_bytes: usize,
}

struct MaterializedCopy {
    storage: Arc<dyn Storage>,
    // Field order releases the payload before releasing its reservation.
    _reservation: CopyReservation,
}

struct CacheEntry {
    copy: Option<Weak<MaterializedCopy>>,
}

#[derive(Default)]
pub(super) struct MaterializationCache {
    entries: Mutex<HashMap<MaterializationKey, CacheEntry>>,
    copies: AtomicUsize,
    query_only_copies: AtomicUsize,
    peak_copies: AtomicUsize,
}

#[derive(Clone)]
pub(super) struct ReadStorage {
    pub(super) storage: Arc<dyn Storage>,
    // Cell enumeration retains this token until its separate storage reference releases.
    _copy: Option<Arc<MaterializedCopy>>,
}

impl ReadStorage {
    fn shared(copy: Arc<MaterializedCopy>) -> Self {
        Self {
            storage: copy.storage.clone(),
            _copy: Some(copy),
        }
    }
}

impl Deref for ReadStorage {
    type Target = dyn Storage;

    fn deref(&self) -> &Self::Target {
        self.storage.as_ref()
    }
}

pub(super) struct CopyReservation {
    cache: Arc<MaterializationCache>,
    key: MaterializationKey,
    generating: bool,
    query_only: AtomicBool,
}

impl Drop for CopyReservation {
    fn drop(&mut self) {
        if self.query_only.load(Ordering::SeqCst) {
            self.cache.query_only_copies.fetch_sub(1, Ordering::SeqCst);
        }
        if self.generating {
            // Failure or unwinding must release admission without publishing partial content.
            let mut entries = match self.cache.entries.lock() {
                Ok(entries) => entries,
                Err(poisoned) => poisoned.into_inner(),
            };
            entries.remove(&self.key);
            self.cache.copies.fetch_sub(1, Ordering::SeqCst);
        } else {
            self.cache.copies.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

pub(super) enum MaterializationAdmission {
    Ready(ReadStorage),
    Generate {
        reservation: CopyReservation,
        volume: StreamedVoxelVolume,
        materials: Arc<HashMap<VoxelMaterialId, MaterialIndex>>,
        overlay: Arc<OverlayVersion>,
    },
}

impl MaterializationAdmission {
    pub(super) fn finish(self) -> Result<ReadStorage, VoxelFrontendError> {
        let (mut reservation, volume, materials, overlay) = match self {
            Self::Ready(storage) => return Ok(storage),
            Self::Generate {
                reservation,
                volume,
                materials,
                overlay,
            } => (reservation, volume, materials, overlay),
        };
        #[cfg(feature = "qualification")]
        let _allocation_scope =
            QualificationAllocationScope::enter(QualificationAllocationCategory::Materialized);
        let identity = &reservation.key.volume;
        let generated = {
            #[cfg(feature = "qualification")]
            let _generation_scope =
                QualificationAllocationScope::enter(QualificationAllocationCategory::Generation);
            volume.source.materialize()
        }
        .map_err(|error| match error {
            VoxelSourceError::Allocation => VoxelFrontendError::MaterializationCacheExhausted,
            source => VoxelFrontendError::VolumeGeneration {
                identity: identity.clone(),
                source,
            },
        })?;
        if generated.metadata != volume.metadata {
            return Err(VoxelFrontendError::SourceMetadataMismatch {
                identity: identity.clone(),
            });
        }
        if generated.storage_tier != StorageTier::SparsePages {
            return Err(VoxelFrontendError::StreamedStorageTier {
                identity: identity.clone(),
            });
        }
        let mut storage = generated.storage(&materials).map_err(|error| match error {
            VoxelFrontendError::VolumeAllocation { .. } => {
                VoxelFrontendError::MaterializationCacheExhausted
            }
            other => other,
        })?;
        let changes = overlay.changes(identity)?;
        if !changes.is_empty() {
            storage = storage.successor(&changes);
        }
        let cache = reservation.cache.clone();
        let key = reservation.key.clone();
        let mut entries = cache
            .entries
            .lock()
            .map_err(|_| VoxelFrontendError::StateUnavailable)?;
        reservation.generating = false;
        let copy = Arc::new(MaterializedCopy {
            storage,
            _reservation: reservation,
        });
        let entry = entries
            .get_mut(&key)
            .expect("a live reservation retains its cache entry");
        entry.copy = Some(Arc::downgrade(&copy));
        Ok(ReadStorage::shared(copy))
    }
}

impl MaterializationCache {
    fn cached(&self, key: &MaterializationKey) -> Result<Option<ReadStorage>, VoxelFrontendError> {
        let entries = self
            .entries
            .lock()
            .map_err(|_| VoxelFrontendError::StateUnavailable)?;
        let Some(entry) = entries.get(key) else {
            return Ok(None);
        };
        let Some(copy) = entry.copy.as_ref().and_then(Weak::upgrade) else {
            return Ok(None);
        };
        Ok(Some(ReadStorage::shared(copy)))
    }

    fn prepare(
        self: &Arc<Self>,
        key: MaterializationKey,
        volume: &StreamedVoxelVolume,
        materials: &Arc<HashMap<VoxelMaterialId, MaterialIndex>>,
        overlay: &Arc<OverlayVersion>,
        copy_cap: usize,
        for_selection: bool,
    ) -> Result<MaterializationAdmission, VoxelFrontendError> {
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| VoxelFrontendError::StateUnavailable)?;
        entries.retain(|_, entry| {
            entry
                .copy
                .as_ref()
                .is_none_or(|copy| copy.strong_count() > 0)
        });
        if let Some(entry) = entries.get_mut(&key) {
            if let Some(copy) = entry.copy.as_ref().and_then(Weak::upgrade) {
                return Ok(MaterializationAdmission::Ready(ReadStorage::shared(copy)));
            }
            if entry.copy.is_none() {
                return Err(if for_selection {
                    VoxelFrontendError::MaterializationInProgress
                } else {
                    VoxelFrontendError::QueryOnlyCopyBusy
                });
            }
        }
        if !for_selection && self.query_only_copies.load(Ordering::SeqCst) > 0 {
            return Err(VoxelFrontendError::QueryOnlyCopyBusy);
        }
        if self.copies.load(Ordering::SeqCst) >= copy_cap {
            return Err(VoxelFrontendError::MaterializationCacheExhausted);
        }
        let copies = self.copies.fetch_add(1, Ordering::SeqCst) + 1;
        if !for_selection {
            self.query_only_copies.fetch_add(1, Ordering::SeqCst);
        }
        self.peak_copies.fetch_max(copies, Ordering::SeqCst);
        entries.insert(key.clone(), CacheEntry { copy: None });
        Ok(MaterializationAdmission::Generate {
            reservation: CopyReservation {
                cache: self.clone(),
                key,
                generating: true,
                query_only: AtomicBool::new(!for_selection),
            },
            volume: volume.clone(),
            materials: materials.clone(),
            overlay: overlay.clone(),
        })
    }

    pub(super) fn stats(&self) -> Result<MaterializationCacheStats, VoxelFrontendError> {
        let entries = self
            .entries
            .lock()
            .map_err(|_| VoxelFrontendError::StateUnavailable)?;
        let mut stats = MaterializationCacheStats {
            copies: self.copies.load(Ordering::SeqCst),
            peak_copies: self.peak_copies.load(Ordering::SeqCst),
            query_only_copies: self.query_only_copies.load(Ordering::SeqCst),
            ..MaterializationCacheStats::default()
        };
        for entry in entries.values() {
            match &entry.copy {
                None => {
                    stats.generating += 1;
                }
                Some(copy) => {
                    if let Some(copy) = copy.upgrade() {
                        stats.storage_bytes += copy.storage.storage_bytes();
                    }
                }
            }
        }
        Ok(stats)
    }
}

impl VoxelFrontend {
    pub fn materialization_cache_stats(
        &self,
    ) -> Result<MaterializationCacheStats, VoxelFrontendError> {
        self.materialization_cache.stats()
    }
}

impl PublishedScene {
    pub(super) fn promote_selection(
        &self,
        identities: &[VoxelVolumeId],
    ) -> Result<(), VoxelFrontendError> {
        let Some(streamed) = &self.streamed else {
            return Ok(());
        };
        let entries = streamed
            .cache
            .entries
            .lock()
            .map_err(|_| VoxelFrontendError::StateUnavailable)?;
        // A failed or superseded partial handout must leave the query slot occupied.
        // Only complete selections holding every copy can transfer that ownership.
        for identity in identities {
            let copy = entries
                .get(&self.materialization_key(identity)?)
                .and_then(|entry| entry.copy.as_ref())
                .and_then(Weak::upgrade)
                .expect("a completed selection retains every materialization copy");
            if copy._reservation.query_only.swap(false, Ordering::SeqCst) {
                streamed
                    .cache
                    .query_only_copies
                    .fetch_sub(1, Ordering::SeqCst);
            }
        }
        Ok(())
    }

    pub(super) fn cached_selection_storage(
        &self,
        identity: &VoxelVolumeId,
    ) -> Result<Option<ReadStorage>, VoxelFrontendError> {
        if let Some(storage) = self.volumes.get(identity) {
            return Ok(Some(ReadStorage {
                storage: storage.clone(),
                _copy: None,
            }));
        }
        let key = self.materialization_key(identity)?;
        self.streamed
            .as_ref()
            .expect("uncached volumes in a valid scene are streamed")
            .cache
            .cached(&key)
    }

    pub(super) fn volume_extent(
        &self,
        identity: &VoxelVolumeId,
    ) -> Result<VoxelExtent, VoxelFrontendError> {
        self.volumes
            .get(identity)
            .map(|storage| storage.extent())
            .or_else(|| {
                self.streamed
                    .as_ref()?
                    .volumes
                    .get(identity)
                    .map(|volume| volume.metadata.extent)
            })
            .ok_or_else(|| VoxelFrontendError::UnknownVolumeIdentity {
                identity: identity.clone(),
            })
    }

    pub(super) fn read_storage(
        &self,
        identity: &VoxelVolumeId,
    ) -> Result<ReadStorage, VoxelFrontendError> {
        self.prepare_storage(identity, false)?.finish()
    }

    pub(super) fn prepare_storage(
        &self,
        identity: &VoxelVolumeId,
        for_selection: bool,
    ) -> Result<MaterializationAdmission, VoxelFrontendError> {
        if let Some(storage) = self.volumes.get(identity) {
            return Ok(MaterializationAdmission::Ready(ReadStorage {
                storage: storage.clone(),
                _copy: None,
            }));
        }
        let streamed =
            self.streamed
                .as_ref()
                .ok_or_else(|| VoxelFrontendError::UnknownVolumeIdentity {
                    identity: identity.clone(),
                })?;
        let volume = streamed.volumes.get(identity).ok_or_else(|| {
            VoxelFrontendError::UnknownVolumeIdentity {
                identity: identity.clone(),
            }
        })?;
        let key = self.materialization_key(identity)?;
        streamed.cache.prepare(
            key,
            volume,
            &self.material_indices,
            &streamed.overlay,
            self.residency_limits
                .ok_or(VoxelFrontendError::ResidencyLimitsUndeclared)?
                .materialization_copy_cap(),
            for_selection,
        )
    }

    fn materialization_key(
        &self,
        identity: &VoxelVolumeId,
    ) -> Result<MaterializationKey, VoxelFrontendError> {
        Ok(MaterializationKey {
            scene: self.identity.clone(),
            volume: identity.clone(),
            content_version: *self.volume_content_versions.get(identity).ok_or_else(|| {
                VoxelFrontendError::UnknownVolumeIdentity {
                    identity: identity.clone(),
                }
            })?,
        })
    }

    pub(super) fn streamed_storage_bytes(
        &self,
        identity: &VoxelVolumeId,
    ) -> Result<usize, VoxelFrontendError> {
        self.volume_extent(identity)?;
        let key = self.materialization_key(identity)?;
        let streamed = self
            .streamed
            .as_ref()
            .expect("streamed storage accounting is only called for streamed volumes");
        let entries = streamed
            .cache
            .entries
            .lock()
            .map_err(|_| VoxelFrontendError::StateUnavailable)?;
        Ok(entries
            .get(&key)
            .and_then(|entry| entry.copy.as_ref())
            .and_then(Weak::upgrade)
            .map_or(0, |copy| copy.storage.storage_bytes()))
    }
}

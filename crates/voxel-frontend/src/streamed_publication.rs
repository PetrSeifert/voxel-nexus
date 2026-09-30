use super::*;
use std::ops::Deref;
use std::sync::Mutex;

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
    /// Reads share one query-only copy, which is evicted when unheld and another volume
    /// is requested. Concurrent requests needing a second copy return a typed error.
    pub fn publish_streamed(
        &self,
        scene: StreamedVoxelScene,
    ) -> Result<VoxelSceneView, VoxelFrontendError> {
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
            volumes,
            cache: self.materialization_cache.clone(),
        });
        self.install(published)
    }
}

#[derive(Clone)]
pub(super) struct StreamedScene {
    volumes: HashMap<VoxelVolumeId, StreamedVoxelVolume>,
    cache: Arc<MaterializationCache>,
}

#[derive(Clone, Eq, PartialEq)]
struct MaterializationKey {
    scene: VoxelSceneId,
    volume: VoxelVolumeId,
    content_version: VoxelSceneRevision,
}

struct QueryCopy {
    key: MaterializationKey,
    storage: Arc<dyn Storage>,
}

#[derive(Default)]
struct QuerySlot {
    copy: Option<Arc<QueryCopy>>,
    generating: bool,
}

#[derive(Default)]
pub(super) struct MaterializationCache {
    query: Mutex<QuerySlot>,
}

pub(super) struct ReadStorage {
    pub(super) storage: Arc<dyn Storage>,
    // A cell enumeration can outlive its initiating read and must keep the query slot held.
    _query_copy: Option<Arc<QueryCopy>>,
}

impl Deref for ReadStorage {
    type Target = dyn Storage;

    fn deref(&self) -> &Self::Target {
        self.storage.as_ref()
    }
}

struct QueryReservation {
    cache: Arc<MaterializationCache>,
}

impl Drop for QueryReservation {
    fn drop(&mut self) {
        // Failure or unwinding must release admission without publishing partial content.
        let mut query = match self.cache.query.lock() {
            Ok(query) => query,
            Err(poisoned) => poisoned.into_inner(),
        };
        query.generating = false;
    }
}

impl MaterializationCache {
    fn materialize(
        self: &Arc<Self>,
        key: MaterializationKey,
        volume: &StreamedVoxelVolume,
        materials: &HashMap<VoxelMaterialId, MaterialIndex>,
    ) -> Result<ReadStorage, VoxelFrontendError> {
        {
            let mut query = self
                .query
                .lock()
                .map_err(|_| VoxelFrontendError::StateUnavailable)?;
            if query.generating {
                return Err(VoxelFrontendError::QueryOnlyCopyBusy);
            }
            if let Some(copy) = &query.copy {
                if copy.key == key {
                    return Ok(ReadStorage {
                        storage: copy.storage.clone(),
                        _query_copy: Some(copy.clone()),
                    });
                }
                if Arc::strong_count(copy) > 1 {
                    return Err(VoxelFrontendError::QueryOnlyCopyBusy);
                }
            }
            // Evict before generation so even construction uses only the one query copy.
            query.copy = None;
            query.generating = true;
        }
        let reservation = QueryReservation {
            cache: self.clone(),
        };
        let generated = volume.source.materialize().map_err(|error| match error {
            VoxelSourceError::Allocation => VoxelFrontendError::MaterializationCacheExhausted,
            source => VoxelFrontendError::VolumeGeneration {
                identity: key.volume.clone(),
                source,
            },
        })?;
        if generated.metadata != volume.metadata {
            return Err(VoxelFrontendError::SourceMetadataMismatch {
                identity: key.volume,
            });
        }
        if generated.storage_tier != StorageTier::SparsePages {
            return Err(VoxelFrontendError::StreamedStorageTier {
                identity: key.volume,
            });
        }
        let storage = generated.storage(materials).map_err(|error| match error {
            VoxelFrontendError::VolumeAllocation { .. } => {
                VoxelFrontendError::MaterializationCacheExhausted
            }
            other => other,
        })?;
        let copy = Arc::new(QueryCopy { key, storage });
        {
            let mut query = self
                .query
                .lock()
                .map_err(|_| VoxelFrontendError::StateUnavailable)?;
            query.copy = Some(copy.clone());
        }
        drop(reservation);
        Ok(ReadStorage {
            storage: copy.storage.clone(),
            _query_copy: Some(copy),
        })
    }
}

impl PublishedScene {
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
        if let Some(storage) = self.volumes.get(identity) {
            return Ok(ReadStorage {
                storage: storage.clone(),
                _query_copy: None,
            });
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
        streamed
            .cache
            .materialize(key, volume, &self.material_indices)
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
        let query = streamed
            .cache
            .query
            .lock()
            .map_err(|_| VoxelFrontendError::StateUnavailable)?;
        Ok(query
            .copy
            .as_ref()
            .filter(|copy| copy.key == key)
            .map_or(0, |copy| copy.storage.storage_bytes()))
    }
}

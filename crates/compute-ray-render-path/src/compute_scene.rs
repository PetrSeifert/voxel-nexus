use crate::scene_words::SceneWords;
use semantic_ray_oracle::{
    AxisNormal, SemanticRay, SemanticRayContact, SemanticRayContactClassification,
    SemanticRayObservation, SemanticRayResult,
};
use std::collections::{BTreeMap, BTreeSet, HashMap};
#[path = "brickmap_patch.rs"]
mod brickmap_patch;
pub use brickmap_patch::BrickmapPatchObservations;
use std::num::TryFromIntError;
use std::sync::Arc;
use thiserror::Error;
use voxel_frontend::{
    VoxelChangeSet, VoxelCoordinate, VoxelExtent, VoxelFrontendError, VoxelMaterialId, VoxelRegion,
    VoxelSceneId, VoxelSceneRevision, VoxelSceneView, VoxelValue, VoxelVolumeId,
    VoxelVolumeMetadata,
};

pub(crate) const SCENE_PREFIX_WORD_COUNT: usize = 4;
pub(crate) const VOLUME_HEADER_WORD_COUNT: usize = 8;
pub(crate) const MATERIAL_WORD_COUNT: usize = 4;
const REGION_READ_EDGE: u32 = 32;

#[derive(Clone, Debug, PartialEq)]
pub struct ComputeVolumeHeader {
    identity: VoxelVolumeId,
    scene_origin: [f32; 3],
    voxel_size: f32,
    extent: VoxelExtent,
    voxel_word_offset: u32,
}

impl ComputeVolumeHeader {
    pub(crate) fn for_brickmap(volume: &VoxelVolumeMetadata) -> Self {
        Self {
            identity: volume.identity().clone(),
            scene_origin: volume.scene_origin(),
            voxel_size: volume.voxel_size(),
            extent: volume.extent(),
            voxel_word_offset: 0,
        }
    }

    pub fn identity(&self) -> &VoxelVolumeId {
        &self.identity
    }

    pub fn scene_origin(&self) -> [f32; 3] {
        self.scene_origin
    }

    pub fn voxel_size(&self) -> f32 {
        self.voxel_size
    }

    pub fn extent(&self) -> VoxelExtent {
        self.extent
    }

    pub(crate) fn set_voxel_word_offset(&mut self, offset: u32) {
        self.voxel_word_offset = offset;
    }

    pub fn voxel_word_offset(&self) -> u32 {
        self.voxel_word_offset
    }

    pub(crate) fn storage_words(&self) -> [u32; VOLUME_HEADER_WORD_COUNT] {
        let [origin_x, origin_y, origin_z] = self.scene_origin;
        let [width, height, depth] = self.extent.dimensions();
        [
            origin_x.to_bits(),
            origin_y.to_bits(),
            origin_z.to_bits(),
            self.voxel_size.to_bits(),
            width,
            height,
            depth,
            self.voxel_word_offset,
        ]
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BrickmapGrowthObservations {
    pub trigger_revision: VoxelSceneRevision,
    pub old_capacity: usize,
    pub new_capacity: usize,
    pub predicted_peak_bytes: u64,
    pub actual_peak_bytes: Option<u64>,
    pub rebuild_time: std::time::Duration,
}

#[derive(Clone, Debug)]
pub struct ComputeSceneBundle {
    representation: crate::ComputeRepresentation,
    pool_offset: Option<usize>,
    pool_allocation: Arc<()>,
    free_slots: Arc<BTreeSet<u32>>,
    reserved_slots: Vec<u32>,
    retired_slots: Vec<u32>,
    patch_observations: BrickmapPatchObservations,
    growth_observations: Option<BrickmapGrowthObservations>,
    rebuild_base: Option<(VoxelSceneRevision, Arc<()>)>,
    scene_identity: VoxelSceneId,
    revision: VoxelSceneRevision,
    volume_headers: Arc<[ComputeVolumeHeader]>,
    material_identities: Arc<[VoxelMaterialId]>,
    material_words: Arc<[u32]>,
    storage_words: SceneWords,
    voxel_start: usize,
    predecessor: Option<VoxelSceneRevision>,
    patches: Arc<BTreeMap<usize, u32>>,
}

impl ComputeSceneBundle {
    pub fn representation(&self) -> crate::ComputeRepresentation {
        self.representation
    }

    pub fn validate_device_limits(
        &self,
        max_storage_buffer_range: u64,
        max_buffer_size: u64,
    ) -> Result<(), crate::BrickmapValidationError> {
        if let crate::ComputeRepresentation::Brickmap { budget_bytes } = self.representation {
            crate::brickmap_validation::validate_buffer_sizes(
                self.storage_word_count() as u64 * 4,
                budget_bytes,
                max_storage_buffer_range,
                max_buffer_size,
            )?;
        }
        Ok(())
    }

    pub fn from_view_with_representation(
        view: &VoxelSceneView,
        representation: crate::ComputeRepresentation,
    ) -> Result<Self, ComputeSceneBuildError> {
        Self::from_view_with_representation_progress(view, representation, || Ok(()))
    }

    fn from_view_with_representation_progress(
        view: &VoxelSceneView,
        representation: crate::ComputeRepresentation,
        progress: impl FnMut() -> Result<(), ComputeSceneBuildError>,
    ) -> Result<Self, ComputeSceneBuildError> {
        let crate::ComputeRepresentation::Brickmap { .. } = representation else {
            return Self::from_view(view);
        };
        Self::build_brickmap(view, representation, None, progress)
    }

    fn build_brickmap(
        view: &VoxelSceneView,
        representation: crate::ComputeRepresentation,
        capacity: Option<usize>,
        progress: impl FnMut() -> Result<(), ComputeSceneBuildError>,
    ) -> Result<Self, ComputeSceneBuildError> {
        Self::build_brickmap_measured(view, representation, capacity, progress, |_, _| {})
    }

    pub(crate) fn from_view_with_preparation_timings(
        view: &VoxelSceneView,
        representation: crate::ComputeRepresentation,
    ) -> Result<(Self, Vec<(crate::ComputeTimingPhase, std::time::Duration)>), ComputeSceneBuildError>
    {
        if !matches!(
            representation,
            crate::ComputeRepresentation::Brickmap { .. }
        ) {
            return Ok((Self::from_view(view)?, Vec::new()));
        }
        let mut timings = Vec::new();
        let bundle = Self::build_brickmap_measured(
            view,
            representation,
            None,
            || Ok(()),
            |observations, serialization| {
                timings.extend([
                    (
                        crate::ComputeTimingPhase::Enumeration,
                        observations.enumeration_time,
                    ),
                    (
                        crate::ComputeTimingPhase::Construction,
                        observations.construction_time,
                    ),
                    (crate::ComputeTimingPhase::Serialization, serialization),
                ]);
            },
        )?;
        Ok((bundle, timings))
    }

    fn build_brickmap_measured(
        view: &VoxelSceneView,
        representation: crate::ComputeRepresentation,
        capacity: Option<usize>,
        progress: impl FnMut() -> Result<(), ComputeSceneBuildError>,
        mut measured: impl FnMut(&crate::BrickmapObservations, std::time::Duration),
    ) -> Result<Self, ComputeSceneBuildError> {
        let crate::ComputeRepresentation::Brickmap { budget_bytes } = representation else {
            return Err(ComputeSceneBuildError::PatchBaseMismatch);
        };
        crate::brickmap_validation::validate_view(view, budget_bytes)?;
        let brickmap = crate::BrickmapSceneBundle::from_view_with_progress(view, progress)?;
        let serialization_started = std::time::Instant::now();
        let (volume_headers, voxel_words) = brickmap.gpu_words()?;
        let mut material_identities = brickmap.material_identities().to_vec();
        for material in view.materials() {
            if !material_identities.contains(material.identity()) {
                material_identities.push(material.identity().clone());
            }
        }
        if material_identities.len() > u16::MAX as usize {
            return Err(ComputeSceneBuildError::BrickmapMaterialCapacity);
        }
        let mut material_words = Vec::new();
        for identity in &material_identities {
            let material = view
                .materials()
                .iter()
                .find(|material| material.identity() == identity)
                .expect("brickmap materials come from the validated scene");
            material_words.extend(material.linear_base_color().map(f32::to_bits));
        }
        let mut storage_words = pack_storage_words(&volume_headers, &material_words, &voxel_words)?;
        // The high bit distinguishes sparse addressing without changing the dense buffer layout.
        *storage_words
            .get_mut(1)
            .expect("the scene prefix contains four words") |= 1 << 31;
        crate::brickmap_validation::validate_buffer_sizes(
            storage_words.len() as u64 * 4,
            budget_bytes,
            u64::MAX,
            u64::MAX,
        )?;
        let voxel_start = storage_words.len() - voxel_words.len();
        let pool_offset = voxel_start + voxel_words.first().copied().unwrap_or(0) as usize;
        let occupied_slots = (storage_words.len() - pool_offset) / 256;
        let capacity = capacity.unwrap_or(occupied_slots + occupied_slots.div_ceil(4));
        let spare_slots = capacity
            .checked_sub(occupied_slots)
            .ok_or(ComputeSceneBuildError::ArithmeticOverflow)?;
        let total_words = pool_offset
            .checked_add(
                capacity
                    .checked_mul(256)
                    .ok_or(ComputeSceneBuildError::ArithmeticOverflow)?,
            )
            .ok_or(ComputeSceneBuildError::ArithmeticOverflow)?;
        crate::brickmap_validation::validate_buffer_sizes(
            total_words as u64 * 4,
            budget_bytes,
            u64::MAX,
            u64::MAX,
        )?;
        storage_words
            .try_reserve_exact(spare_slots * 256)
            .map_err(|_| ComputeSceneBuildError::Allocation)?;
        storage_words.resize(storage_words.len() + spare_slots * 256, 0);
        u32::try_from(storage_words.len())?;
        let free_slots = (occupied_slots..occupied_slots + spare_slots)
            .map(u32::try_from)
            .collect::<Result<BTreeSet<_>, _>>()?;
        let bundle = Self {
            representation,
            pool_offset: Some(pool_offset),
            pool_allocation: Arc::new(()),
            free_slots: Arc::new(free_slots),
            reserved_slots: Vec::new(),
            retired_slots: Vec::new(),
            patch_observations: BrickmapPatchObservations::default(),
            growth_observations: None,
            rebuild_base: None,
            scene_identity: view.scene_id().clone(),
            revision: view.revision(),
            volume_headers: volume_headers.into(),
            material_identities: material_identities.into(),
            material_words: material_words.into(),
            voxel_start,
            storage_words: SceneWords::new(&storage_words),
            predecessor: None,
            patches: Arc::new(BTreeMap::new()),
        };
        measured(brickmap.observations(), serialization_started.elapsed());
        Ok(bundle)
    }

    pub fn from_view(view: &VoxelSceneView) -> Result<Self, ComputeSceneBuildError> {
        Self::from_view_until_cancelled(view, || false)
    }

    pub(crate) fn from_view_until_cancelled(
        view: &VoxelSceneView,
        mut cancellation_requested: impl FnMut() -> bool,
    ) -> Result<Self, ComputeSceneBuildError> {
        Self::from_view_with_block_completion(view, &mut cancellation_requested, || Ok(()))
    }

    pub(crate) fn from_view_with_block_completion(
        view: &VoxelSceneView,
        mut cancellation_requested: impl FnMut() -> bool,
        mut block_completed: impl FnMut() -> Result<(), ComputeSceneBuildError>,
    ) -> Result<Self, ComputeSceneBuildError> {
        let material_count = u32::try_from(view.materials().len())?;
        if material_count == u32::MAX {
            return Err(ComputeSceneBuildError::TooManyMaterials);
        }

        let mut material_indices = HashMap::new();
        material_indices
            .try_reserve(view.materials().len())
            .map_err(|_| ComputeSceneBuildError::Allocation)?;
        let mut material_identities = Vec::new();
        let mut material_words = Vec::new();
        material_identities
            .try_reserve_exact(view.materials().len())
            .map_err(|_| ComputeSceneBuildError::Allocation)?;
        material_words
            .try_reserve_exact(
                view.materials()
                    .len()
                    .checked_mul(MATERIAL_WORD_COUNT)
                    .ok_or(ComputeSceneBuildError::ArithmeticOverflow)?,
            )
            .map_err(|_| ComputeSceneBuildError::Allocation)?;
        for (index, material) in view.materials().iter().enumerate() {
            let local_index = u32::try_from(index)?;
            if material_indices
                .insert(material.identity().clone(), local_index)
                .is_some()
            {
                return Err(ComputeSceneBuildError::DuplicateMaterial(
                    material.identity().clone(),
                ));
            }
            material_identities.push(material.identity().clone());
            material_words.extend(material.linear_base_color().map(f32::to_bits));
        }

        let mut volumes = view.volumes().iter().collect::<Vec<_>>();
        volumes.sort_by(|left, right| left.identity().cmp(right.identity()));
        let mut volume_headers = Vec::new();
        volume_headers
            .try_reserve_exact(volumes.len())
            .map_err(|_| ComputeSceneBuildError::Allocation)?;
        let total_voxel_count = volumes.iter().try_fold(0_usize, |total, volume| {
            total
                .checked_add(extent_value_count(volume.extent())?)
                .ok_or(ComputeSceneBuildError::ArithmeticOverflow)
        })?;
        let mut voxel_words = Vec::new();
        voxel_words
            .try_reserve_exact(total_voxel_count)
            .map_err(|_| ComputeSceneBuildError::Allocation)?;

        for volume in volumes {
            let voxel_word_offset = u32::try_from(voxel_words.len())?;
            let value_count = extent_value_count(volume.extent())?;
            let new_length = voxel_words
                .len()
                .checked_add(value_count)
                .ok_or(ComputeSceneBuildError::ArithmeticOverflow)?;
            voxel_words.resize(new_length, 0);
            populate_volume_words(
                view,
                volume,
                voxel_word_offset,
                &material_indices,
                &mut voxel_words,
                &mut cancellation_requested,
                &mut block_completed,
            )?;
            volume_headers.push(ComputeVolumeHeader {
                identity: volume.identity().clone(),
                scene_origin: volume.scene_origin(),
                voxel_size: volume.voxel_size(),
                extent: volume.extent(),
                voxel_word_offset,
            });
        }

        let storage_words = pack_storage_words(&volume_headers, &material_words, &voxel_words)?;
        Ok(Self {
            representation: crate::ComputeRepresentation::Dense,
            pool_offset: None,
            pool_allocation: Arc::new(()),
            free_slots: Arc::new(BTreeSet::new()),
            reserved_slots: Vec::new(),
            retired_slots: Vec::new(),
            patch_observations: BrickmapPatchObservations::default(),
            growth_observations: None,
            rebuild_base: None,
            scene_identity: view.scene_id().clone(),
            revision: view.revision(),
            volume_headers: volume_headers.into(),
            material_identities: material_identities.into(),
            material_words: material_words.into(),
            voxel_start: storage_words.len() - voxel_words.len(),
            storage_words: SceneWords::new(&storage_words),
            predecessor: None,
            patches: Arc::new(BTreeMap::new()),
        })
    }

    pub fn scene_identity(&self) -> &VoxelSceneId {
        &self.scene_identity
    }

    pub fn revision(&self) -> VoxelSceneRevision {
        self.revision
    }

    pub fn volume_headers(&self) -> &[ComputeVolumeHeader] {
        &self.volume_headers
    }

    pub fn material_identities(&self) -> &[VoxelMaterialId] {
        &self.material_identities
    }

    pub fn material_words(&self) -> &[u32] {
        &self.material_words
    }

    /// Deferred materialization avoids allocating a full voxel buffer during convergence.
    pub fn voxel_words(&self) -> Vec<u32> {
        self.storage_words
            .flatten()
            .into_iter()
            .skip(self.voxel_start)
            .collect()
    }

    /// Deferred packing keeps full-buffer allocation out of incremental uploads.
    pub fn storage_words(&self) -> Vec<u32> {
        self.storage_words.flatten()
    }

    pub(crate) fn storage_word_count(&self) -> usize {
        self.storage_words.len()
    }

    pub(crate) fn predecessor(&self) -> Option<VoxelSceneRevision> {
        self.predecessor
    }

    pub(crate) fn record_growth_allocation(&mut self, peak_bytes: u64) {
        if let Some(observation) = &mut self.growth_observations {
            observation.actual_peak_bytes = Some(peak_bytes);
        }
    }

    pub(crate) fn predict_allocation_peak(
        &self,
        old_allocation_bytes: u64,
        new_allocation_bytes: u64,
        staging_bytes: u64,
    ) -> Result<u64, ComputeSceneBuildError> {
        let predicted_peak_bytes = old_allocation_bytes
            .checked_add(new_allocation_bytes)
            .and_then(|bytes| bytes.checked_add(staging_bytes))
            .ok_or(ComputeSceneBuildError::ArithmeticOverflow)?;
        if let crate::ComputeRepresentation::Brickmap { budget_bytes } = self.representation
            && predicted_peak_bytes > budget_bytes
        {
            return Err(ComputeSceneBuildError::GrowthBudgetExceeded {
                predicted_peak_bytes,
                budget_bytes,
            });
        }
        Ok(predicted_peak_bytes)
    }

    pub(crate) fn record_growth_prediction(&mut self, predicted_peak_bytes: u64) {
        if let Some(observation) = &mut self.growth_observations {
            observation.predicted_peak_bytes = predicted_peak_bytes;
        }
    }

    #[cfg(test)]
    pub(crate) fn allocation_witness(&self) -> std::sync::Weak<[u32]> {
        let mut words = &self.storage_words;
        loop {
            match words {
                SceneWords::Leaf(page) => return Arc::downgrade(page),
                SceneWords::Branch { left, .. } => words = left,
            }
        }
    }

    pub(crate) fn finish_installation(&mut self) {
        Arc::make_mut(&mut self.free_slots).extend(self.retired_slots.drain(..));
        self.reserved_slots.clear();
        self.patches = Arc::new(BTreeMap::new());
        self.predecessor = None;
        self.rebuild_base = None;
    }

    pub(crate) fn patches(&self) -> &BTreeMap<usize, u32> {
        &self.patches
    }

    pub(crate) fn successor_with_block_completion(
        &self,
        view: &VoxelSceneView,
        changes: &[VoxelChangeSet],
        mut cancellation_requested: impl FnMut() -> bool,
        mut block_completed: impl FnMut() -> Result<(), ComputeSceneBuildError>,
    ) -> Result<Self, ComputeSceneBuildError> {
        if self.pool_offset.is_some() {
            return match self.brickmap_successor(
                view,
                changes,
                &mut cancellation_requested,
                &mut block_completed,
            ) {
                Err(ComputeSceneBuildError::PoolCapacityExhausted) => {
                    self.grow_brickmap(view, cancellation_requested, block_completed)
                }
                result => result,
            };
        }
        let mut revision = self.revision;
        let chain_matches = self.scene_identity() == view.scene_id()
            && changes.iter().all(|change| {
                let matches = change.scene_identity() == self.scene_identity()
                    && change.predecessor_revision() == revision
                    && revision.checked_successor() == Some(change.successor_revision());
                revision = change.successor_revision();
                matches
            })
            && revision == view.revision();
        let layout_matches = view.volumes().len() == self.volume_headers.len()
            && self.volume_headers.iter().all(|header| {
                view.volumes().iter().any(|volume| {
                    volume.identity() == header.identity()
                        && volume.extent() == header.extent()
                        && volume.scene_origin() == header.scene_origin()
                        && volume.voxel_size() == header.voxel_size()
                })
            })
            && view.materials().len() == self.material_identities.len()
            && view
                .materials()
                .iter()
                .zip(self.material_identities.iter())
                .all(|(material, identity)| material.identity() == identity)
            && view
                .materials()
                .iter()
                .flat_map(|material| material.linear_base_color().map(f32::to_bits))
                .eq(self.material_words.iter().copied());
        if !chain_matches || !layout_matches {
            return Self::from_view_with_block_completion(
                view,
                cancellation_requested,
                block_completed,
            );
        }
        let mut successor = Self {
            revision: view.revision(),
            predecessor: Some(self.revision),
            patches: Arc::new(BTreeMap::new()),
            ..self.clone()
        };
        for change in changes {
            for changed in change.changed_regions() {
                let header = self
                    .volume_headers
                    .iter()
                    .find(|header| header.identity() == changed.volume_identity())
                    .ok_or(ComputeSceneBuildError::ArithmeticOverflow)?;
                let [origin_x, origin_y, origin_z] = changed.region().origin().components();
                let [width, height, depth] = changed.region().extent().dimensions();
                for local_z in 0..depth {
                    for local_y in 0..height {
                        for local_x in (0..width).step_by(REGION_READ_EDGE as usize) {
                            if cancellation_requested() {
                                return Err(ComputeSceneBuildError::Cancelled);
                            }
                            let coordinate = VoxelCoordinate::new(
                                origin_x
                                    .checked_add(i32::try_from(local_x)?)
                                    .ok_or(ComputeSceneBuildError::ArithmeticOverflow)?,
                                origin_y
                                    .checked_add(i32::try_from(local_y)?)
                                    .ok_or(ComputeSceneBuildError::ArithmeticOverflow)?,
                                origin_z
                                    .checked_add(i32::try_from(local_z)?)
                                    .ok_or(ComputeSceneBuildError::ArithmeticOverflow)?,
                            );
                            let row_width = REGION_READ_EDGE.min(width - local_x);
                            let mut values = vec![VoxelValue::Empty; usize::try_from(row_width)?];
                            view.read_region_into(
                                header.identity(),
                                VoxelRegion::new(coordinate, VoxelExtent::new(row_width, 1, 1)),
                                &mut values,
                            )?;
                            let start = self
                                .voxel_start
                                .checked_add(usize::try_from(header.voxel_word_offset)?)
                                .and_then(|start| {
                                    start.checked_add(dense_index(header.extent, coordinate)?)
                                })
                                .ok_or(ComputeSceneBuildError::ArithmeticOverflow)?;
                            for (offset, value) in values.into_iter().enumerate() {
                                let word = match value {
                                    VoxelValue::Empty => 0,
                                    VoxelValue::Occupied(identity) => u32::try_from(
                                        self.material_identities
                                            .iter()
                                            .position(|material| material == &identity)
                                            .ok_or_else(|| {
                                                ComputeSceneBuildError::UnknownMaterial {
                                                    volume: header.identity.clone(),
                                                    material: identity,
                                                }
                                            })?,
                                    )?
                                    .checked_add(1)
                                    .ok_or(ComputeSceneBuildError::TooManyMaterials)?,
                                };
                                let index = start
                                    .checked_add(offset)
                                    .ok_or(ComputeSceneBuildError::ArithmeticOverflow)?;
                                successor
                                    .storage_words
                                    .set(index, word)
                                    .ok_or(ComputeSceneBuildError::ArithmeticOverflow)?;
                                Arc::make_mut(&mut successor.patches).insert(index, word);
                            }
                            block_completed()?;
                        }
                    }
                }
            }
        }
        if cancellation_requested() {
            return Err(ComputeSceneBuildError::Cancelled);
        }
        Ok(successor)
    }

    pub fn observe(&self, ray: &SemanticRay) -> SemanticRayObservation {
        let mut nearest_contact = None;
        for header in self.volume_headers.iter() {
            let Some(contact) = (if self.pool_offset.is_some() {
                trace_volume_with(header, ray, &self.material_identities, |coordinate| {
                    self.sample_brickmap(header, coordinate)
                })
            } else {
                trace_volume(self, header, ray)
            }) else {
                continue;
            };
            if nearest_contact
                .as_ref()
                .is_none_or(|nearest| contact_precedes(&contact, nearest))
            {
                nearest_contact = Some(contact);
            }
        }
        SemanticRayObservation::new(
            self.scene_identity.clone(),
            self.revision,
            nearest_contact.map_or(SemanticRayResult::Miss, SemanticRayResult::Contact),
        )
    }
}

#[derive(Debug, Error)]
pub enum ComputeSceneBuildError {
    #[error("the brickmap scene palette exceeds the 65,535 occupied material identity limit")]
    BrickmapMaterialCapacity,
    #[error(
        "brickmap growth peak {predicted_peak_bytes} bytes exceeds configured budget {budget_bytes} bytes"
    )]
    GrowthBudgetExceeded {
        predicted_peak_bytes: u64,
        budget_bytes: u64,
    },
    #[error("brickmap pool capacity exhausted while reserving a private candidate slot")]
    PoolCapacityExhausted,
    #[error("brickmap patch base revision, allocation, or scene layout does not match")]
    PatchBaseMismatch,
    #[error(transparent)]
    Brickmap(#[from] crate::BrickmapBuildError),
    #[error(transparent)]
    BrickmapValidation(#[from] crate::BrickmapValidationError),
    #[error("compute-owned Voxel Scene preparation was cancelled")]
    Cancelled,
    #[error("could not allocate the compute-owned Voxel Scene representation")]
    Allocation,
    #[error("compute-owned Voxel Scene representation arithmetic overflowed")]
    ArithmeticOverflow,
    #[error("the compute-owned Voxel Scene representation exceeds 32-bit indexing")]
    IndexOverflow(#[from] TryFromIntError),
    #[error("the Voxel Scene has too many Voxel Materials for zero-reserved 32-bit words")]
    TooManyMaterials,
    #[error("the Voxel Scene contains duplicate Voxel Material identity {0:?}")]
    DuplicateMaterial(VoxelMaterialId),
    #[error("Voxel Volume {volume:?} references unknown Voxel Material {material:?}")]
    UnknownMaterial {
        volume: VoxelVolumeId,
        material: VoxelMaterialId,
    },
    #[error("could not read a bounded Voxel Region")]
    VoxelFrontend(#[from] VoxelFrontendError),
    #[cfg(any(test, feature = "qualification"))]
    #[error("the compute convergence preparation barrier is unavailable")]
    PreparationBarrier,
    #[error("the compute convergence control state is unavailable during preparation")]
    PreparationControl,
    #[cfg(any(test, feature = "qualification"))]
    #[error("injected preparation failure")]
    InjectedPreparationFailure,
}

fn populate_volume_words(
    view: &VoxelSceneView,
    volume: &VoxelVolumeMetadata,
    voxel_word_offset: u32,
    material_indices: &HashMap<VoxelMaterialId, u32>,
    voxel_words: &mut [u32],
    cancellation_requested: &mut impl FnMut() -> bool,
    block_completed: &mut impl FnMut() -> Result<(), ComputeSceneBuildError>,
) -> Result<(), ComputeSceneBuildError> {
    let [width, height, depth] = volume.extent().dimensions();
    let mut values = Vec::new();
    let maximum_count = usize::try_from(REGION_READ_EDGE)?.pow(3);
    values
        .try_reserve_exact(maximum_count)
        .map_err(|_| ComputeSceneBuildError::Allocation)?;
    for origin_z in (0..depth).step_by(REGION_READ_EDGE as usize) {
        for origin_y in (0..height).step_by(REGION_READ_EDGE as usize) {
            for origin_x in (0..width).step_by(REGION_READ_EDGE as usize) {
                if cancellation_requested() {
                    return Err(ComputeSceneBuildError::Cancelled);
                }
                let region = VoxelRegion::new(
                    VoxelCoordinate::new(
                        i32::try_from(origin_x)?,
                        i32::try_from(origin_y)?,
                        i32::try_from(origin_z)?,
                    ),
                    VoxelExtent::new(
                        REGION_READ_EDGE.min(width - origin_x),
                        REGION_READ_EDGE.min(height - origin_y),
                        REGION_READ_EDGE.min(depth - origin_z),
                    ),
                );
                let [region_width, region_height, region_depth] = region.extent().dimensions();
                let region_width = usize::try_from(region_width)?;
                let region_height = usize::try_from(region_height)?;
                let value_count = region_width * region_height * usize::try_from(region_depth)?;
                values.resize(value_count, VoxelValue::Empty);
                view.read_region_into(volume.identity(), region, &mut values)?;
                for (row_index, row) in values.chunks_exact(region_width).enumerate() {
                    let coordinate = VoxelCoordinate::new(
                        i32::try_from(origin_x)?,
                        i32::try_from(origin_y + u32::try_from(row_index % region_height)?)?,
                        i32::try_from(origin_z + u32::try_from(row_index / region_height)?)?,
                    );
                    let local_index = dense_index(volume.extent(), coordinate)
                        .ok_or(ComputeSceneBuildError::ArithmeticOverflow)?;
                    let destination_start = usize::try_from(voxel_word_offset)?
                        .checked_add(local_index)
                        .ok_or(ComputeSceneBuildError::ArithmeticOverflow)?;
                    let destination_end = destination_start
                        .checked_add(region_width)
                        .ok_or(ComputeSceneBuildError::ArithmeticOverflow)?;
                    let destinations = voxel_words
                        .get_mut(destination_start..destination_end)
                        .ok_or(ComputeSceneBuildError::ArithmeticOverflow)?;
                    for (value, destination) in row.iter().zip(destinations) {
                        *destination = match value {
                            VoxelValue::Empty => 0,
                            VoxelValue::Occupied(material_identity) => material_indices
                                .get(material_identity)
                                .copied()
                                .ok_or_else(|| ComputeSceneBuildError::UnknownMaterial {
                                    volume: volume.identity().clone(),
                                    material: material_identity.clone(),
                                })?
                                .checked_add(1)
                                .ok_or(ComputeSceneBuildError::TooManyMaterials)?,
                        };
                    }
                }
                block_completed()?;
            }
        }
    }
    Ok(())
}

fn pack_storage_words(
    headers: &[ComputeVolumeHeader],
    material_words: &[u32],
    voxel_words: &[u32],
) -> Result<Vec<u32>, ComputeSceneBuildError> {
    let header_words = headers
        .len()
        .checked_mul(VOLUME_HEADER_WORD_COUNT)
        .ok_or(ComputeSceneBuildError::ArithmeticOverflow)?;
    let total_words = SCENE_PREFIX_WORD_COUNT
        .checked_add(header_words)
        .and_then(|count| count.checked_add(material_words.len()))
        .and_then(|count| count.checked_add(voxel_words.len()))
        .ok_or(ComputeSceneBuildError::ArithmeticOverflow)?;
    let mut words = Vec::new();
    words
        .try_reserve_exact(total_words)
        .map_err(|_| ComputeSceneBuildError::Allocation)?;
    words.extend([
        u32::try_from(headers.len())?,
        u32::try_from(material_words.len() / MATERIAL_WORD_COUNT)?,
        u32::try_from(SCENE_PREFIX_WORD_COUNT + header_words)?,
        u32::try_from(SCENE_PREFIX_WORD_COUNT + header_words + material_words.len())?,
    ]);
    for header in headers {
        words.extend(header.storage_words());
    }
    words.extend(material_words);
    words.extend(voxel_words);
    Ok(words)
}

fn trace_volume(
    scene: &ComputeSceneBundle,
    header: &ComputeVolumeHeader,
    ray: &SemanticRay,
) -> Option<SemanticRayContact> {
    trace_volume_with(header, ray, &scene.material_identities, |coordinate| {
        Some((voxel_word(scene, header, coordinate)?, 1))
    })
}

pub(crate) fn trace_volume_with(
    header: &ComputeVolumeHeader,
    ray: &SemanticRay,
    materials: &[VoxelMaterialId],
    sample: impl Fn(VoxelCoordinate) -> Option<(u32, i32)>,
) -> Option<SemanticRayContact> {
    let (distance, exit_distance, _) = intersect_volume(header, ray)?;
    let initial = volume_coordinate_at(header, ray, distance)?.components();
    // Slab intersection proves the entry is in bounds; rounding may put it just outside.
    let dimensions = header.extent.dimensions();
    let initial: [i32; 3] =
        std::array::from_fn(|axis| initial[axis].clamp(0, (dimensions[axis] - 1) as i32));
    let mut coordinate = VoxelCoordinate::new(initial[0], initial[1], initial[2]);

    loop {
        let (material_word, edge) = sample(coordinate)?;
        if material_word != 0
            && let Some((contact_distance, classification)) = voxel_contact(header, ray, coordinate)
        {
            let material_index = usize::try_from(material_word.checked_sub(1)?).ok()?;
            let material_identity = materials.get(material_index)?.clone();
            return Some(SemanticRayContact::new(
                header.identity.clone(),
                coordinate,
                material_identity,
                contact_distance,
                classification,
            ));
        }

        let (next_distance, tied_axes) = next_cell_crossing(header, ray, coordinate, edge)?;
        if next_distance > exit_distance || next_distance > ray.maximum_distance() {
            return None;
        }
        let previous = coordinate.components();
        let mut components = if edge == 1 {
            previous
        } else {
            volume_coordinate_at(header, ray, next_distance)?.components()
        };
        for axis in tied_axes {
            let index = axis.index();
            let start = previous[index] / edge * edge;
            components[index] = if axis.step(ray.direction()) > 0 {
                start.checked_add(edge)?
            } else {
                start.checked_sub(1)?
            };
        }
        coordinate = VoxelCoordinate::new(components[0], components[1], components[2]);
        dense_index(header.extent, coordinate)?;
    }
}

fn voxel_contact(
    header: &ComputeVolumeHeader,
    ray: &SemanticRay,
    coordinate: VoxelCoordinate,
) -> Option<(f64, SemanticRayContactClassification)> {
    let minimum = std::array::from_fn::<_, 3, _>(|axis| {
        f64::from(header.scene_origin[axis])
            + f64::from(coordinate.components()[axis]) * f64::from(header.voxel_size)
    });
    let maximum = minimum.map(|value| value + f64::from(header.voxel_size));
    let start = point_at_distance(ray, ray.minimum_distance());
    if (0..3).all(|axis| start[axis] >= minimum[axis] && start[axis] < maximum[axis]) {
        return Some((
            ray.minimum_distance(),
            SemanticRayContactClassification::StartedInside,
        ));
    }
    let (entry, exit, normal) = intersect_bounds(minimum, maximum, ray)?;
    if exit <= entry || exit <= ray.minimum_distance() || entry > ray.maximum_distance() {
        return None;
    }
    Some((
        entry.max(ray.minimum_distance()),
        SemanticRayContactClassification::Entered(normal),
    ))
}

pub(crate) fn contact_precedes(
    candidate: &SemanticRayContact,
    current: &SemanticRayContact,
) -> bool {
    match candidate.distance().total_cmp(&current.distance()) {
        std::cmp::Ordering::Less => true,
        std::cmp::Ordering::Equal => candidate.volume_identity() < current.volume_identity(),
        std::cmp::Ordering::Greater => false,
    }
}

fn intersect_volume(
    header: &ComputeVolumeHeader,
    ray: &SemanticRay,
) -> Option<(f64, f64, AxisNormal)> {
    let minimum = header.scene_origin.map(f64::from);
    let [width, height, depth] = header.extent.dimensions();
    let voxel_size = f64::from(header.voxel_size);
    let maximum = [
        minimum[0] + f64::from(width) * voxel_size,
        minimum[1] + f64::from(height) * voxel_size,
        minimum[2] + f64::from(depth) * voxel_size,
    ];
    let (entry_distance, exit_distance, entry_normal) = intersect_bounds(minimum, maximum, ray)?;
    if exit_distance <= entry_distance
        || exit_distance < ray.minimum_distance()
        || entry_distance > ray.maximum_distance()
    {
        return None;
    }
    Some((
        entry_distance.max(ray.minimum_distance()),
        exit_distance.min(ray.maximum_distance()),
        entry_normal,
    ))
}

fn intersect_bounds(
    minimum: [f64; 3],
    maximum: [f64; 3],
    ray: &SemanticRay,
) -> Option<(f64, f64, AxisNormal)> {
    let mut entry_distance = f64::NEG_INFINITY;
    let mut exit_distance = f64::INFINITY;
    let mut entry_normal = AxisNormal::NegativeX;
    for axis in Axis::ALL {
        let origin = axis.component(ray.origin());
        let direction = axis.component(ray.direction());
        let axis_minimum = axis.component(minimum);
        let axis_maximum = axis.component(maximum);
        if direction == 0.0 {
            if origin < axis_minimum || origin >= axis_maximum {
                return None;
            }
            continue;
        }
        let minimum_distance = (axis_minimum - origin) / direction;
        let maximum_distance = (axis_maximum - origin) / direction;
        let (axis_entry, axis_exit, normal) = if direction > 0.0 {
            (minimum_distance, maximum_distance, axis.negative_normal())
        } else {
            (maximum_distance, minimum_distance, axis.positive_normal())
        };
        if axis_entry > entry_distance {
            entry_distance = axis_entry;
            entry_normal = normal;
        }
        exit_distance = exit_distance.min(axis_exit);
    }
    Some((entry_distance, exit_distance, entry_normal))
}

fn volume_coordinate_at(
    header: &ComputeVolumeHeader,
    ray: &SemanticRay,
    distance: f64,
) -> Option<VoxelCoordinate> {
    let point = point_at_distance(ray, distance);
    let direction = ray.direction();
    let [width, height, depth] = header.extent.dimensions();
    let mut components = [0_i32; 3];
    for (axis, dimension) in Axis::ALL.into_iter().zip([width, height, depth]) {
        let local = (axis.component(point) - axis.component(header.scene_origin.map(f64::from)))
            / f64::from(header.voxel_size);
        let mut coordinate = local.floor();
        if coordinate == f64::from(dimension) && axis.component(direction) < 0.0 {
            coordinate -= 1.0;
        }
        let destination = components.get_mut(axis.index())?;
        *destination = i32::try_from(coordinate as i64).ok()?;
    }
    Some(VoxelCoordinate::new(
        components[0],
        components[1],
        components[2],
    ))
}

fn next_cell_crossing(
    header: &ComputeVolumeHeader,
    ray: &SemanticRay,
    coordinate: VoxelCoordinate,
    edge: i32,
) -> Option<(f64, Vec<Axis>)> {
    let coordinate = coordinate.components();
    let mut crossings = [f64::INFINITY; 3];
    for axis in Axis::ALL {
        let direction = axis.component(ray.direction());
        if direction == 0.0 {
            continue;
        }
        let local_coordinate = f64::from(*coordinate.get(axis.index())? / edge * edge);
        let boundary_coordinate = if direction > 0.0 {
            local_coordinate + f64::from(edge)
        } else {
            local_coordinate
        };
        let boundary = axis.component(header.scene_origin.map(f64::from))
            + boundary_coordinate * f64::from(header.voxel_size);
        crossings[axis.index()] = (boundary - axis.component(ray.origin())) / direction;
    }
    let next_distance = crossings.into_iter().fold(f64::INFINITY, f64::min);
    if !next_distance.is_finite() {
        return None;
    }
    let tolerance = f64::EPSILON * 16.0 * next_distance.abs().max(1.0);
    let tied_axes = Axis::ALL
        .into_iter()
        .filter(|axis| (crossings[axis.index()] - next_distance).abs() <= tolerance)
        .collect::<Vec<_>>();
    Some((next_distance, tied_axes))
}

fn voxel_word(
    scene: &ComputeSceneBundle,
    header: &ComputeVolumeHeader,
    coordinate: VoxelCoordinate,
) -> Option<u32> {
    let index = usize::try_from(header.voxel_word_offset)
        .ok()?
        .checked_add(dense_index(header.extent, coordinate)?)?;
    scene
        .storage_words
        .get(scene.voxel_start.checked_add(index)?)
}

fn dense_index(extent: VoxelExtent, coordinate: VoxelCoordinate) -> Option<usize> {
    let [coordinate_x, coordinate_y, coordinate_z] = coordinate.components();
    let x = usize::try_from(coordinate_x).ok()?;
    let y = usize::try_from(coordinate_y).ok()?;
    let z = usize::try_from(coordinate_z).ok()?;
    let [width, height, depth] = extent.dimensions().map(|value| value as usize);
    if x >= width || y >= height || z >= depth {
        return None;
    }
    z.checked_mul(height)?
        .checked_add(y)?
        .checked_mul(width)?
        .checked_add(x)
}

fn extent_value_count(extent: VoxelExtent) -> Result<usize, ComputeSceneBuildError> {
    let [width, height, depth] = extent.dimensions().map(|value| value as usize);
    width
        .checked_mul(height)
        .and_then(|count| count.checked_mul(depth))
        .ok_or(ComputeSceneBuildError::ArithmeticOverflow)
}

fn point_at_distance(ray: &SemanticRay, distance: f64) -> [f64; 3] {
    let [origin_x, origin_y, origin_z] = ray.origin();
    let [direction_x, direction_y, direction_z] = ray.direction();
    [
        origin_x + direction_x * distance,
        origin_y + direction_y * distance,
        origin_z + direction_z * distance,
    ]
}

#[derive(Clone, Copy)]
enum Axis {
    X,
    Y,
    Z,
}

impl Axis {
    const ALL: [Self; 3] = [Self::X, Self::Y, Self::Z];

    fn index(self) -> usize {
        match self {
            Self::X => 0,
            Self::Y => 1,
            Self::Z => 2,
        }
    }

    fn component(self, value: [f64; 3]) -> f64 {
        value[self.index()]
    }

    fn step(self, direction: [f64; 3]) -> i32 {
        if self.component(direction) > 0.0 {
            1
        } else {
            -1
        }
    }

    fn negative_normal(self) -> AxisNormal {
        match self {
            Self::X => AxisNormal::NegativeX,
            Self::Y => AxisNormal::NegativeY,
            Self::Z => AxisNormal::NegativeZ,
        }
    }

    fn positive_normal(self) -> AxisNormal {
        match self {
            Self::X => AxisNormal::PositiveX,
            Self::Y => AxisNormal::PositiveY,
            Self::Z => AxisNormal::PositiveZ,
        }
    }
}

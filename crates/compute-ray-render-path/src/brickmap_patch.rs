use super::*;
use voxel_frontend::VoxelRegionContent;

const MIXED: u32 = 1 << 31;
const SLOT_WORDS: usize = 256;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BrickmapPatchObservations {
    pub dirty_cells: usize,
    pub slots_reserved: usize,
    pub slots_retired: usize,
    pub uploaded_bytes: usize,
}

impl ComputeSceneBundle {
    pub fn brickmap_patch_observations(&self) -> BrickmapPatchObservations {
        self.patch_observations
    }

    pub(crate) fn patch_base_matches(&self, installed: &Self) -> bool {
        if let Some((revision, allocation)) = &self.rebuild_base {
            return *revision == installed.revision
                && self.scene_identity == installed.scene_identity
                && Arc::ptr_eq(allocation, &installed.pool_allocation);
        }
        self.pool_offset.is_none()
            || (self.predecessor == Some(installed.revision)
                && self.scene_identity == installed.scene_identity
                && Arc::ptr_eq(&self.pool_allocation, &installed.pool_allocation))
    }

    pub(super) fn brickmap_successor(
        &self,
        view: &VoxelSceneView,
        changes: &[VoxelChangeSet],
        mut cancelled: impl FnMut() -> bool,
        mut completed: impl FnMut() -> Result<(), ComputeSceneBuildError>,
    ) -> Result<Self, ComputeSceneBuildError> {
        if self.scene_identity != *view.scene_id()
            || view.volumes().len() != self.volume_headers.len()
            || !self.volume_headers.iter().all(|header| {
                view.volumes().iter().any(|volume| {
                    volume.identity() == header.identity()
                        && volume.extent() == header.extent()
                        && volume.scene_origin() == header.scene_origin()
                        && volume.voxel_size() == header.voxel_size()
                })
            })
            || view.materials().len() != self.material_identities.len()
            || !self
                .material_identities
                .iter()
                .enumerate()
                .all(|(index, identity)| {
                    view.materials().iter().any(|material| {
                        material.identity() == identity
                            && material.linear_base_color().map(f32::to_bits).as_slice()
                                == self
                                    .material_words
                                    .get(index * 4..index * 4 + 4)
                                    .unwrap_or(&[])
                    })
                })
        {
            return Err(ComputeSceneBuildError::PatchBaseMismatch);
        }
        let mut revision = self.revision;
        let complete_chain = changes.iter().all(|change| {
            let matches = change.scene_identity() == view.scene_id()
                && change.predecessor_revision() == revision
                && revision.checked_successor() == Some(change.successor_revision());
            revision = change.successor_revision();
            matches
        }) && revision == view.revision();
        let mut dirty = BTreeSet::new();
        for (volume_index, header) in self.volume_headers.iter().enumerate() {
            let regions = if complete_chain {
                changes
                    .iter()
                    .flat_map(|change| change.changed_regions())
                    .filter(|change| change.volume_identity() == header.identity())
                    .map(|change| change.region())
                    .collect::<Vec<_>>()
            } else {
                // Missing revisions invalidate every cell, but preserve allocation and
                // still use private reservations rather than replacing the GPU buffer.
                vec![VoxelRegion::new(
                    VoxelCoordinate::new(0, 0, 0),
                    header.extent(),
                )]
            };
            for region in regions {
                let origin = region.origin().components();
                let dimensions = region.extent().dimensions();
                let mut first = [0; 3];
                let mut end = [0; 3];
                for axis in 0..3 {
                    first[axis] = u32::try_from(origin[axis])? / 8;
                    end[axis] = u32::try_from(origin[axis])?
                        .checked_add(dimensions[axis])
                        .ok_or(ComputeSceneBuildError::ArithmeticOverflow)?
                        .div_ceil(8);
                }
                for z in first[2]..end[2] {
                    for y in first[1]..end[1] {
                        for x in first[0]..end[0] {
                            if cancelled() {
                                return Err(ComputeSceneBuildError::Cancelled);
                            }
                            dirty.insert((volume_index, [x, y, z]));
                        }
                    }
                }
            }
        }
        let mut successor = Self {
            growth_observations: None,
            rebuild_base: None,
            revision: view.revision(),
            predecessor: Some(self.revision),
            patches: Arc::new(BTreeMap::new()),
            reserved_slots: Vec::new(),
            retired_slots: Vec::new(),
            patch_observations: BrickmapPatchObservations {
                dirty_cells: dirty.len(),
                ..Default::default()
            },
            ..self.clone()
        };
        let pool_offset = self
            .pool_offset
            .ok_or(ComputeSceneBuildError::PatchBaseMismatch)?;
        for (volume_index, cell) in dirty {
            if cancelled() {
                return Err(ComputeSceneBuildError::Cancelled);
            }
            let header = self
                .volume_headers
                .get(volume_index)
                .ok_or(ComputeSceneBuildError::PatchBaseMismatch)?;
            let origin = cell.map(|value| value * 8);
            let bounds = header.extent().dimensions();
            let extent = std::array::from_fn::<_, 3, _>(|axis| {
                8.min(bounds[axis].saturating_sub(origin[axis]))
            });
            let region = VoxelRegion::new(
                VoxelCoordinate::new(
                    i32::try_from(origin[0])?,
                    i32::try_from(origin[1])?,
                    i32::try_from(origin[2])?,
                ),
                VoxelExtent::new(extent[0], extent[1], extent[2]),
            );
            let grid = bounds.map(|value| value.div_ceil(8) as usize);
            let entry_index = self.voxel_start
                + header.voxel_word_offset as usize
                + cell[0] as usize
                + grid[0] * (cell[1] as usize + grid[1] * cell[2] as usize);
            let old = self
                .storage_words
                .get(entry_index)
                .ok_or(ComputeSceneBuildError::PatchBaseMismatch)?;
            let entry = match view.region_content(header.identity(), region)? {
                VoxelRegionContent::Uniform(value) => self.brickmap_material(&value)?,
                VoxelRegionContent::Mixed => {
                    let mut values =
                        vec![VoxelValue::Empty; (extent[0] * extent[1] * extent[2]) as usize];
                    view.read_region_into(header.identity(), region, &mut values)?;
                    let mut words = [0; SLOT_WORDS];
                    for (index, value) in values.iter().enumerate() {
                        let x = index % extent[0] as usize;
                        let y = index / extent[0] as usize % extent[1] as usize;
                        let z = index / (extent[0] * extent[1]) as usize;
                        let local = x + 8 * y + 64 * z;
                        *words
                            .get_mut(local / 2)
                            .expect("clipped cell fits one pool slot") |=
                            self.brickmap_material(value)? << (16 * (local % 2));
                    }
                    if old & MIXED != 0
                        && words.iter().enumerate().all(|(index, word)| {
                            self.storage_words
                                .get(pool_offset + (old & !MIXED) as usize * SLOT_WORDS + index)
                                == Some(*word)
                        })
                    {
                        completed()?;
                        continue;
                    }
                    let slot = Arc::make_mut(&mut successor.free_slots)
                        .pop_first()
                        .ok_or(ComputeSceneBuildError::PoolCapacityExhausted)?;
                    successor.reserved_slots.push(slot);
                    for (index, word) in words.into_iter().enumerate() {
                        successor
                            .write_patch(pool_offset + slot as usize * SLOT_WORDS + index, word)?;
                    }
                    MIXED | slot
                }
            };
            if entry != old {
                if old & MIXED != 0 {
                    successor.retired_slots.push(old & !MIXED);
                }
                successor.write_patch(entry_index, entry)?;
            }
            completed()?;
        }
        if cancelled() {
            return Err(ComputeSceneBuildError::Cancelled);
        }
        successor.patch_observations.slots_reserved = successor.reserved_slots.len();
        successor.patch_observations.slots_retired = successor.retired_slots.len();
        successor.patch_observations.uploaded_bytes = successor.patches.len() * 4;
        Ok(successor)
    }

    pub fn brickmap_growth_observations(&self) -> Option<BrickmapGrowthObservations> {
        self.growth_observations
    }

    pub(super) fn grow_brickmap(
        &self,
        view: &VoxelSceneView,
        mut cancelled: impl FnMut() -> bool,
        mut completed: impl FnMut() -> Result<(), ComputeSceneBuildError>,
    ) -> Result<Self, ComputeSceneBuildError> {
        let started = std::time::Instant::now();
        let mut required = 0usize;
        // Count before allocating any replacement payload, including edits that skip revisions.
        for volume in view.volumes() {
            for batch in view.enumerate_cells(volume.identity(), 8, 64)? {
                for cell in batch? {
                    if cancelled() {
                        return Err(ComputeSceneBuildError::Cancelled);
                    }
                    if matches!(cell.content(), VoxelRegionContent::Mixed) {
                        required = required
                            .checked_add(1)
                            .ok_or(ComputeSceneBuildError::ArithmeticOverflow)?;
                    }
                    completed()?;
                }
            }
        }
        let offset = self
            .pool_offset
            .ok_or(ComputeSceneBuildError::PatchBaseMismatch)?;
        let old_capacity = (self.storage_word_count() - offset) / SLOT_WORDS;
        let mut new_capacity = old_capacity
            .checked_add(old_capacity.div_ceil(2))
            .ok_or(ComputeSceneBuildError::ArithmeticOverflow)?
            .max(1);
        while new_capacity < required {
            new_capacity = new_capacity
                .checked_add(new_capacity.div_ceil(2))
                .ok_or(ComputeSceneBuildError::ArithmeticOverflow)?;
        }
        let new_words = offset
            .checked_add(
                new_capacity
                    .checked_mul(SLOT_WORDS)
                    .ok_or(ComputeSceneBuildError::ArithmeticOverflow)?,
            )
            .ok_or(ComputeSceneBuildError::ArithmeticOverflow)?;
        u32::try_from(new_words)?;
        let predicted_peak_bytes = (self.storage_word_count() as u64 * 4)
            .checked_add(new_words as u64 * 8)
            .ok_or(ComputeSceneBuildError::ArithmeticOverflow)?;
        let crate::ComputeRepresentation::Brickmap { budget_bytes } = self.representation else {
            return Err(ComputeSceneBuildError::PatchBaseMismatch);
        };
        if predicted_peak_bytes > budget_bytes {
            return Err(ComputeSceneBuildError::GrowthBudgetExceeded {
                predicted_peak_bytes,
                budget_bytes,
            });
        }
        let mut bundle =
            Self::build_brickmap(view, self.representation, Some(new_capacity), || {
                if cancelled() {
                    return Err(ComputeSceneBuildError::Cancelled);
                }
                completed()
            })?;
        if cancelled() {
            return Err(ComputeSceneBuildError::Cancelled);
        }
        bundle.rebuild_base = Some((self.revision, self.pool_allocation.clone()));
        bundle.growth_observations = Some(BrickmapGrowthObservations {
            trigger_revision: view.revision(),
            old_capacity,
            new_capacity,
            predicted_peak_bytes,
            actual_peak_bytes: None,
            rebuild_time: started.elapsed(),
        });
        Ok(bundle)
    }

    fn write_patch(&mut self, index: usize, word: u32) -> Result<(), ComputeSceneBuildError> {
        self.storage_words
            .set(index, word)
            .ok_or(ComputeSceneBuildError::PatchBaseMismatch)?;
        Arc::make_mut(&mut self.patches).insert(index, word);
        Ok(())
    }

    fn brickmap_material(&self, value: &VoxelValue) -> Result<u32, ComputeSceneBuildError> {
        match value {
            VoxelValue::Empty => Ok(0),
            VoxelValue::Occupied(identity) => self
                .material_identities
                .iter()
                .position(|material| material == identity)
                .map(|index| (index + 1) as u32)
                .ok_or(ComputeSceneBuildError::PatchBaseMismatch),
        }
    }

    pub(super) fn sample_brickmap(
        &self,
        header: &ComputeVolumeHeader,
        coordinate: VoxelCoordinate,
    ) -> Option<(u32, i32)> {
        dense_index(header.extent, coordinate)?;
        let [x, y, z] = coordinate.components().map(|value| value as usize);
        let [width, height, _] = header
            .extent
            .dimensions()
            .map(|value| value.div_ceil(8) as usize);
        let index = self.voxel_start
            + header.voxel_word_offset as usize
            + x / 8
            + width * (y / 8 + height * (z / 8));
        let entry = self.storage_words.get(index)?;
        if entry & MIXED == 0 {
            return Some((entry, if entry == 0 { 8 } else { 1 }));
        }
        let local = x % 8 + 8 * (y % 8) + 64 * (z % 8);
        let word = self
            .storage_words
            .get(self.pool_offset? + (entry & !MIXED) as usize * SLOT_WORDS + local / 2)?;
        Some(((word >> (16 * (local % 2))) & 0xffff, 1))
    }
}

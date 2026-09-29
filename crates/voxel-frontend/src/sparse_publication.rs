use super::storage_counters::{
    ValidationMemory, record_batch_validated, record_candidate_pair_examined, record_sweep_axis,
    record_values_validated,
};
use super::*;

/// A sparse batch whose region lies inside its volume and whose values are resolved to
/// material indices. Bounds are volume-local and half-open.
pub(super) struct ValidatedBatch {
    pub(super) start: [usize; 3],
    pub(super) end: [usize; 3],
    content: BatchContent,
}

enum BatchContent {
    Fill(MaterialIndex),
    Detail(Vec<MaterialIndex>),
}

impl ValidatedBatch {
    pub(super) fn fill_value(&self) -> Option<MaterialIndex> {
        match self.content {
            BatchContent::Fill(value) => Some(value),
            BatchContent::Detail(_) => None,
        }
    }

    /// `position` is volume-local and must lie inside the batch region.
    pub(super) fn value_at(&self, [x, y, z]: [usize; 3]) -> MaterialIndex {
        match &self.content {
            BatchContent::Fill(value) => *value,
            BatchContent::Detail(values) => {
                let [start_x, start_y, start_z] = self.start;
                let [width, height, _] = self.dimensions();
                values
                    .get(((z - start_z) * height + (y - start_y)) * width + (x - start_x))
                    .copied()
                    .unwrap_or(MaterialIndex::EMPTY)
            }
        }
    }

    /// Values in x-fastest, then y, then z order.
    pub(super) fn values(&self) -> impl Iterator<Item = (VoxelCoordinate, MaterialIndex)> + '_ {
        let [start_x, start_y, start_z] = self.start;
        let [end_x, end_y, end_z] = self.end;
        (start_z..end_z)
            .flat_map(move |z| {
                (start_y..end_y).flat_map(move |y| (start_x..end_x).map(move |x| [x, y, z]))
            })
            .filter_map(move |position| {
                let [x, y, z] = position.map(i32::try_from);
                Some((
                    VoxelCoordinate::new(x.ok()?, y.ok()?, z.ok()?),
                    self.value_at(position),
                ))
            })
    }

    pub(super) fn value_count(&self) -> usize {
        match &self.content {
            BatchContent::Fill(_) => {
                let [width, height, depth] = self.dimensions();
                width.saturating_mul(height).saturating_mul(depth)
            }
            BatchContent::Detail(values) => values.len(),
        }
    }

    fn dimensions(&self) -> [usize; 3] {
        let [start_x, start_y, start_z] = self.start;
        let [end_x, end_y, end_z] = self.end;
        [end_x - start_x, end_y - start_y, end_z - start_z]
    }

    fn intersects(&self, other: &Self) -> bool {
        (0..3).all(|axis| self.start[axis] < other.end[axis] && other.start[axis] < self.end[axis])
    }
}

/// Validation never tracks coordinates across the volume, so its cost follows the supplied
/// batches rather than the volume extent.
pub(super) fn validate(
    volume: &SparseVoxelVolume,
    materials: &HashMap<VoxelMaterialId, MaterialIndex>,
) -> Result<Vec<ValidatedBatch>, VoxelFrontendError> {
    #[cfg(any(test, feature = "qualification"))]
    let _timer = super::storage_counters::ValidationTimer(std::time::Instant::now());
    let memory = ValidationMemory::default();
    let identity = &volume.metadata.identity;
    let mut batches = Vec::new();
    batches
        .try_reserve_exact(volume.batches.len())
        .map_err(|_| VoxelFrontendError::VolumeAllocation {
            identity: identity.clone(),
        })?;
    memory.retain_vector(&batches);
    for (batch_index, batch) in volume.batches.iter().enumerate() {
        record_batch_validated();
        let (bounds, content) = match batch {
            SparseVoxelBatch::Fill(fill) => {
                let bounds = validate_batch_region(
                    fill.region,
                    batch_index,
                    identity,
                    volume.metadata.extent,
                )?;
                let value = material_index(&fill.value, materials, identity, fill.region.origin)?;
                (bounds, BatchContent::Fill(value))
            }
            SparseVoxelBatch::Detail(detail) => {
                let bounds =
                    validate_dense_batch(detail, batch_index, identity, volume.metadata.extent)?;
                let mut values = Vec::new();
                values.try_reserve_exact(detail.values.len()).map_err(|_| {
                    VoxelFrontendError::VolumeAllocation {
                        identity: identity.clone(),
                    }
                })?;
                memory.retain_vector(&values);
                for (value, coordinate) in detail.values.iter().zip(bounds.coordinates()) {
                    values.push(material_index(value, materials, identity, coordinate)?);
                }
                record_values_validated(values.len());
                (bounds, BatchContent::Detail(values))
            }
        };
        let invalid_bounds = || VoxelFrontendError::InvalidBatchBounds {
            identity: identity.clone(),
            batch_index,
        };
        let local = |value: i64| usize::try_from(value).map_err(|_| invalid_bounds());
        batches.push(ValidatedBatch {
            start: [
                local(bounds.start_x)?,
                local(bounds.start_y)?,
                local(bounds.start_z)?,
            ],
            end: [
                local(bounds.end_x)?,
                local(bounds.end_y)?,
                local(bounds.end_z)?,
            ],
            content,
        });
    }
    if let Some((first_batch_index, second_batch_index)) = find_overlap(&batches, &memory) {
        return Err(VoxelFrontendError::OverlappingBatches {
            identity: identity.clone(),
            first_batch_index,
            second_batch_index,
        });
    }
    Ok(batches)
}

fn material_index(
    value: &VoxelValue,
    materials: &HashMap<VoxelMaterialId, MaterialIndex>,
    volume_identity: &VoxelVolumeId,
    coordinate: VoxelCoordinate,
) -> Result<MaterialIndex, VoxelFrontendError> {
    match value {
        VoxelValue::Empty => Ok(MaterialIndex::EMPTY),
        VoxelValue::Occupied(material_identity) => materials
            .get(material_identity)
            .copied()
            .ok_or_else(|| VoxelFrontendError::UnknownMaterialReference {
                volume_identity: volume_identity.clone(),
                coordinate,
                material_identity: material_identity.clone(),
            }),
    }
}

/// Sweep and prune along the axis whose projections overlap least, followed by exact box
/// intersection of the surviving candidate pairs. Ties prefer horizontal axes, because
/// terrain batches stacked in columns share their vertical projections.
fn find_overlap(batches: &[ValidatedBatch], memory: &ValidationMemory) -> Option<(usize, usize)> {
    let order = [0, 2, 1]
        .into_iter()
        .map(|axis| {
            let mut order: Vec<(usize, &ValidatedBatch)> = batches.iter().enumerate().collect();
            let allocation = memory.temporary_vector(&order);
            // The batch index makes each key unique, so an in-place unstable sort preserves
            // the same order while avoiding an unaccounted sorting buffer.
            order.sort_unstable_by_key(|(index, batch)| (batch.start[axis], *index));
            let overlapping_pairs = overlapping_projection_pairs(&order, axis);
            (overlapping_pairs, axis, order, allocation)
        })
        .min_by_key(|(overlapping_pairs, _, _, _)| *overlapping_pairs);
    let (_, axis, order, _allocation) = order?;
    record_sweep_axis(axis);
    for (position, (first_index, first)) in order.iter().enumerate() {
        let later = order.get(position + 1..).unwrap_or_default();
        for (second_index, second) in later
            .iter()
            .take_while(|(_, second)| second.start[axis] < first.end[axis])
        {
            record_candidate_pair_examined();
            if first.intersects(second) {
                return Some((
                    (*first_index).min(*second_index),
                    (*first_index).max(*second_index),
                ));
            }
        }
    }
    None
}

/// Counts the pairs whose projections onto `axis` overlap. `order` is sorted by start, so a
/// pair overlaps exactly when the later batch starts before the earlier one ends.
fn overlapping_projection_pairs(order: &[(usize, &ValidatedBatch)], axis: usize) -> usize {
    order
        .iter()
        .enumerate()
        .map(|(position, (_, batch))| {
            order
                .get(position + 1..)
                .unwrap_or_default()
                .partition_point(|(_, later)| later.start[axis] < batch.end[axis])
        })
        .sum()
}

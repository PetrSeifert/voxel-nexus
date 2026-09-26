use super::*;
use std::collections::BTreeMap;

pub(super) trait Storage: Send + Sync {
    fn extent(&self) -> VoxelExtent;
    fn value(&self, coordinate: VoxelCoordinate) -> MaterialIndex;
    fn successor(&self, coordinate: VoxelCoordinate, value: MaterialIndex) -> Arc<dyn Storage>;
    fn uniform_span(&self, start: usize, count: usize) -> Option<MaterialIndex>;
    fn storage_bytes(&self) -> usize;
    #[cfg(test)]
    fn as_any(&self) -> &dyn std::any::Any;

    fn read_region_into(
        &self,
        bounds: &RegionBounds,
        palette: &[VoxelValue],
        values: &mut [VoxelValue],
    ) {
        if let Some(index) = self.uniform_region(bounds) {
            values.fill(
                palette
                    .get(index.0 as usize)
                    .cloned()
                    .unwrap_or(VoxelValue::Empty),
            );
            return;
        }
        for (destination, coordinate) in values.iter_mut().zip(bounds.coordinates()) {
            *destination = palette
                .get(self.value(coordinate).0 as usize)
                .cloned()
                .unwrap_or(VoxelValue::Empty);
        }
    }

    fn uniform_region(&self, bounds: &RegionBounds) -> Option<MaterialIndex> {
        let extent = self.extent();
        let mut uniform = None;
        for depth in bounds.start_z..bounds.end_z {
            for height in bounds.start_y..bounds.end_y {
                let mut position = bounds.start_x;
                while position < bounds.end_x {
                    let coordinate = VoxelCoordinate::new(
                        i32::try_from(position).ok()?,
                        i32::try_from(height).ok()?,
                        i32::try_from(depth).ok()?,
                    );
                    let (value, count) = if let Some(index) = dense_index(extent, coordinate) {
                        let count = usize::try_from(
                            (bounds.end_x - position).min(i64::from(extent.width) - position),
                        )
                        .ok()?
                        .min(DenseStorage::PAGE_VALUES - index % DenseStorage::PAGE_VALUES);
                        (self.uniform_span(index, count)?, count)
                    } else {
                        let count = if position < 0 {
                            (bounds.end_x.min(0) - position) as usize
                        } else {
                            (bounds.end_x - position) as usize
                        };
                        (MaterialIndex::EMPTY, count)
                    };
                    if uniform.is_some_and(|previous| previous != value) {
                        return None;
                    }
                    uniform = Some(value);
                    position += i64::try_from(count).ok()?;
                }
            }
        }
        uniform
    }
}

impl Storage for DenseStorage {
    fn extent(&self) -> VoxelExtent {
        self.extent
    }
    fn value(&self, coordinate: VoxelCoordinate) -> MaterialIndex {
        self.value(coordinate)
    }
    fn successor(&self, coordinate: VoxelCoordinate, value: MaterialIndex) -> Arc<dyn Storage> {
        let mut successor = self.clone();
        if let Some(destination) =
            dense_index(self.extent, coordinate).and_then(|index| successor.get_mut(index))
        {
            *destination = value;
        }
        Arc::new(successor)
    }
    fn uniform_span(&self, start: usize, count: usize) -> Option<MaterialIndex> {
        let value = *self.get(start)?;
        (start..start + count)
            .all(|index| self.get(index) == Some(&value))
            .then_some(value)
    }
    fn storage_bytes(&self) -> usize {
        size_of::<Self>()
            + self.pages.capacity() * size_of::<Arc<Vec<MaterialIndex>>>()
            + self
                .pages
                .iter()
                .map(|page| {
                    size_of::<Vec<MaterialIndex>>()
                        + 2 * size_of::<usize>()
                        + page.capacity() * size_of::<MaterialIndex>()
                })
                .sum::<usize>()
            + size_of::<Vec<Arc<Vec<MaterialIndex>>>>()
            + 2 * size_of::<usize>()
    }
    #[cfg(test)]
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[derive(Clone)]
enum SparsePage {
    Uniform(MaterialIndex),
    Mixed(Arc<Vec<MaterialIndex>>),
}

impl SparsePage {
    fn compress(values: Arc<Vec<MaterialIndex>>) -> Option<Self> {
        let first = *values.first()?;
        if values.iter().all(|value| *value == first) {
            (first != MaterialIndex::EMPTY).then_some(Self::Uniform(first))
        } else {
            Some(Self::Mixed(values))
        }
    }
}

#[derive(Clone)]
pub(super) struct SparseStorage {
    extent: VoxelExtent,
    pages: Arc<BTreeMap<usize, SparsePage>>,
}

impl SparseStorage {
    pub(super) fn from_dense(dense: DenseStorage) -> Self {
        let pages = dense
            .pages
            .iter()
            .enumerate()
            .filter_map(|(index, page)| {
                SparsePage::compress(Arc::clone(page)).map(|page| (index, page))
            })
            .collect();
        Self {
            extent: dense.extent,
            pages: Arc::new(pages),
        }
    }
}

impl Storage for SparseStorage {
    fn extent(&self) -> VoxelExtent {
        self.extent
    }
    fn value(&self, coordinate: VoxelCoordinate) -> MaterialIndex {
        let Some(index) = dense_index(self.extent, coordinate) else {
            return MaterialIndex::EMPTY;
        };
        match self.pages.get(&(index / DenseStorage::PAGE_VALUES)) {
            None => MaterialIndex::EMPTY,
            Some(SparsePage::Uniform(value)) => *value,
            Some(SparsePage::Mixed(values)) => values
                .get(index % DenseStorage::PAGE_VALUES)
                .copied()
                .unwrap_or(MaterialIndex::EMPTY),
        }
    }
    fn successor(&self, coordinate: VoxelCoordinate, value: MaterialIndex) -> Arc<dyn Storage> {
        let mut successor = self.clone();
        if let Some(index) = dense_index(self.extent, coordinate) {
            let page_index = index / DenseStorage::PAGE_VALUES;
            let count = self
                .extent
                .value_count()
                .unwrap_or(0)
                .saturating_sub(page_index * DenseStorage::PAGE_VALUES)
                .min(DenseStorage::PAGE_VALUES);
            let mut values = match self.pages.get(&page_index) {
                None => Arc::new(vec![MaterialIndex::EMPTY; count]),
                Some(SparsePage::Uniform(value)) => Arc::new(vec![*value; count]),
                Some(SparsePage::Mixed(values)) => Arc::clone(values),
            };
            if let Some(destination) =
                Arc::make_mut(&mut values).get_mut(index % DenseStorage::PAGE_VALUES)
            {
                *destination = value;
            }
            let pages = Arc::make_mut(&mut successor.pages);
            match SparsePage::compress(values) {
                Some(page) => {
                    pages.insert(page_index, page);
                }
                None => {
                    pages.remove(&page_index);
                }
            }
        }
        Arc::new(successor)
    }
    fn uniform_span(&self, start: usize, count: usize) -> Option<MaterialIndex> {
        match self.pages.get(&(start / DenseStorage::PAGE_VALUES)) {
            None => Some(MaterialIndex::EMPTY),
            Some(SparsePage::Uniform(value)) => Some(*value),
            Some(SparsePage::Mixed(values)) => {
                let offset = start % DenseStorage::PAGE_VALUES;
                let values = values.get(offset..offset + count)?;
                let first = *values.first()?;
                values.iter().all(|value| *value == first).then_some(first)
            }
        }
    }
    fn storage_bytes(&self) -> usize {
        // BTreeMap does not expose allocated node capacity; charge one key/value plus
        // three pointers per entry as an explicit estimate, not allocator telemetry.
        size_of::<Self>()
            + size_of::<BTreeMap<usize, SparsePage>>()
            + 2 * size_of::<usize>()
            + self.pages.len()
                * (size_of::<usize>() + size_of::<SparsePage>() + 3 * size_of::<usize>())
            + self
                .pages
                .values()
                .map(|page| match page {
                    SparsePage::Uniform(_) => 0,
                    SparsePage::Mixed(values) => {
                        size_of::<Vec<MaterialIndex>>()
                            + 2 * size_of::<usize>()
                            + values.capacity() * size_of::<MaterialIndex>()
                    }
                })
                .sum::<usize>()
    }
    #[cfg(test)]
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparse_edits_share_mixed_pages_and_collapse_restored_pages()
    -> Result<(), Box<dyn std::error::Error>> {
        let dense = DenseStorage {
            extent: VoxelExtent::new(513, 1, 1),
            pages: Arc::new(vec![
                Arc::new(vec![MaterialIndex::EMPTY; 256]),
                Arc::new((0..256).map(|index| MaterialIndex(index % 2)).collect()),
                Arc::new(vec![MaterialIndex(1)]),
            ]),
        };
        let sparse = SparseStorage::from_dense(dense);
        assert_eq!(sparse.pages.len(), 2);
        let edited = sparse.successor(VoxelCoordinate::new(0, 0, 0), MaterialIndex(1));
        let edited = edited
            .as_any()
            .downcast_ref::<SparseStorage>()
            .ok_or("wrong tier")?;
        let (Some(SparsePage::Mixed(before)), Some(SparsePage::Mixed(after))) =
            (sparse.pages.get(&1), edited.pages.get(&1))
        else {
            return Err("missing mixed page".into());
        };
        assert!(Arc::ptr_eq(before, after));
        assert_eq!(
            sparse.value(VoxelCoordinate::new(0, 0, 0)),
            MaterialIndex::EMPTY
        );
        let restored = edited.successor(VoxelCoordinate::new(0, 0, 0), MaterialIndex::EMPTY);
        let restored = restored
            .as_any()
            .downcast_ref::<SparseStorage>()
            .ok_or("wrong tier")?;
        assert!(!restored.pages.contains_key(&0));
        let cleared = restored.successor(VoxelCoordinate::new(512, 0, 0), MaterialIndex::EMPTY);
        assert_eq!(
            cleared.value(VoxelCoordinate::new(512, 0, 0)),
            MaterialIndex::EMPTY
        );
        assert_eq!(
            sparse.value(VoxelCoordinate::new(512, 0, 0)),
            MaterialIndex(1)
        );
        Ok(())
    }
}

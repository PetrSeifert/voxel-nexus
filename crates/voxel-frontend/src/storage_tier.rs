use super::cell_enumeration::{
    CellGrid, CellSource, CellSourceError, EnumeratedCell, ScanningCellSource,
};
use super::storage_counters::{
    record_brick_examined, record_copied_brick_payload, record_enumeration_brick_examined,
    record_enumeration_value_examined, record_publication_brick_visited,
    record_publication_values_written, record_staged_values_allocated, record_voxel_value_examined,
    record_working_cells,
};
use super::*;
use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, hash_map};
use std::ops::Range;

pub(super) trait Storage: Send + Sync {
    fn extent(&self) -> VoxelExtent;
    fn value(&self, coordinate: VoxelCoordinate) -> MaterialIndex;
    fn successor(&self, changes: &[(VoxelCoordinate, MaterialIndex)]) -> Arc<dyn Storage>;
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

    fn uniform_region(&self, bounds: &RegionBounds) -> Option<MaterialIndex>;

    fn cell_source(self: Arc<Self>, grid: CellGrid) -> Box<dyn CellSource>;
}

impl DenseStorage {
    fn uniform_span(&self, start: usize, count: usize) -> Option<MaterialIndex> {
        let value = *self.get(start)?;
        (start..start + count)
            .all(|index| self.get(index) == Some(&value))
            .then_some(value)
    }
}

impl Storage for DenseStorage {
    fn extent(&self) -> VoxelExtent {
        self.extent
    }
    fn value(&self, coordinate: VoxelCoordinate) -> MaterialIndex {
        self.value(coordinate)
    }
    fn successor(&self, changes: &[(VoxelCoordinate, MaterialIndex)]) -> Arc<dyn Storage> {
        let mut successor = self.clone();
        for &(coordinate, value) in changes {
            if let Some(destination) =
                dense_index(self.extent, coordinate).and_then(|index| successor.get_mut(index))
            {
                *destination = value;
            }
        }
        Arc::new(successor)
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
    fn cell_source(self: Arc<Self>, grid: CellGrid) -> Box<dyn CellSource> {
        Box::new(ScanningCellSource::new(self, grid))
    }
    fn storage_bytes(&self) -> usize {
        size_of::<Self>()
            + self.pages.storage_bytes()
            + self
                .pages
                .values()
                .map(|page| {
                    size_of::<Vec<MaterialIndex>>()
                        + 2 * size_of::<usize>()
                        + page.capacity() * size_of::<MaterialIndex>()
                })
                .sum::<usize>()
            + 2 * size_of::<usize>()
    }
    #[cfg(test)]
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

const BRICK_EDGE: usize = 16;

#[derive(Clone, Copy)]
pub(super) struct BrickGrid {
    extent: [usize; 3],
    bricks: [usize; 3],
    brick_count: usize,
}

struct BrickLocation {
    key: usize,
    offset: usize,
    value_count: usize,
}

struct BrickEdits {
    value_count: usize,
    values: Vec<(usize, MaterialIndex)>,
}

impl BrickGrid {
    /// Returns `None` when a linear brick key for the extent cannot be represented by `usize`.
    pub(super) fn new(extent: VoxelExtent) -> Option<Self> {
        let [width, height, depth] = extent.dimensions();
        let extent = [
            usize::try_from(width).ok()?,
            usize::try_from(height).ok()?,
            usize::try_from(depth).ok()?,
        ];
        let bricks = extent.map(|dimension| dimension.div_ceil(BRICK_EDGE));
        let [bricks_x, bricks_y, bricks_z] = bricks;
        let brick_count = bricks_x.checked_mul(bricks_y)?.checked_mul(bricks_z)?;
        Some(Self {
            extent,
            bricks,
            brick_count,
        })
    }

    // Every in-grid key is below the checked brick count, so this cannot overflow.
    fn key(&self, [brick_x, brick_y, brick_z]: [usize; 3]) -> usize {
        let [bricks_x, bricks_y, _] = self.bricks;
        (brick_z * bricks_y + brick_y) * bricks_x + brick_x
    }

    fn position(&self, key: usize) -> Option<[usize; 3]> {
        let [bricks_x, bricks_y, bricks_z] = self.bricks;
        let row = key.checked_div(bricks_x)?;
        let position = [
            key.checked_rem(bricks_x)?,
            row.checked_rem(bricks_y)?,
            row.checked_div(bricks_y)?,
        ];
        (position[2] < bricks_z).then_some(position)
    }

    fn brick_extent(&self, [brick_x, brick_y, brick_z]: [usize; 3]) -> [usize; 3] {
        let [width, height, depth] = self.extent;
        [
            (width - brick_x * BRICK_EDGE).min(BRICK_EDGE),
            (height - brick_y * BRICK_EDGE).min(BRICK_EDGE),
            (depth - brick_z * BRICK_EDGE).min(BRICK_EDGE),
        ]
    }

    fn locate(&self, coordinate: VoxelCoordinate) -> Option<BrickLocation> {
        let [x, y, z] = coordinate.components();
        let [x, y, z] = [
            usize::try_from(x).ok()?,
            usize::try_from(y).ok()?,
            usize::try_from(z).ok()?,
        ];
        let [width, height, depth] = self.extent;
        if x >= width || y >= height || z >= depth {
            return None;
        }
        let brick = [x / BRICK_EDGE, y / BRICK_EDGE, z / BRICK_EDGE];
        let [brick_width, brick_height, brick_depth] = self.brick_extent(brick);
        Some(BrickLocation {
            key: self.key(brick),
            offset: ((z % BRICK_EDGE) * brick_height + y % BRICK_EDGE) * brick_width
                + x % BRICK_EDGE,
            value_count: brick_width * brick_height * brick_depth,
        })
    }
}

#[derive(Clone)]
pub(super) enum Brick {
    Uniform(MaterialIndex),
    Mixed(Arc<[MaterialIndex]>),
}

impl Brick {
    /// Returns `None` for an all-empty brick, which is represented by its absence.
    fn normalize(values: Vec<MaterialIndex>) -> Option<Self> {
        let first = *values.first()?;
        if values.iter().all(|value| *value == first) {
            (first != MaterialIndex::EMPTY).then_some(Self::Uniform(first))
        } else {
            Some(Self::Mixed(values.into()))
        }
    }
}

#[derive(Clone)]
pub(super) struct SparseStorage {
    extent: VoxelExtent,
    grid: BrickGrid,
    bricks: Arc<PageTable<Brick>>,
}

impl SparseStorage {
    pub(super) fn from_dense(dense: &DenseStorage, grid: BrickGrid) -> Self {
        let mut bricks = PageTable::new(grid.brick_count);
        let [width, height, _] = grid.extent;
        let [bricks_x, bricks_y, bricks_z] = grid.bricks;
        for brick_z in 0..bricks_z {
            for brick_y in 0..bricks_y {
                for brick_x in 0..bricks_x {
                    record_publication_brick_visited();
                    let brick = [brick_x, brick_y, brick_z];
                    let [brick_width, brick_height, brick_depth] = grid.brick_extent(brick);
                    let mut values = Vec::with_capacity(brick_width * brick_height * brick_depth);
                    for local_z in 0..brick_depth {
                        for local_y in 0..brick_height {
                            for local_x in 0..brick_width {
                                let x = brick_x * BRICK_EDGE + local_x;
                                let y = brick_y * BRICK_EDGE + local_y;
                                let z = brick_z * BRICK_EDGE + local_z;
                                values.push(
                                    dense
                                        .get((z * height + y) * width + x)
                                        .copied()
                                        .unwrap_or(MaterialIndex::EMPTY),
                                );
                            }
                        }
                    }
                    if let Some(normalized) = Brick::normalize(values) {
                        bricks.insert(grid.key(brick), normalized);
                    }
                }
            }
        }
        Self {
            extent: dense.extent,
            grid,
            bricks: Arc::new(bricks),
        }
    }

    /// Builds bricks only where batches supply values, so absent space is never visited.
    /// Batches must not overlap: a brick a fill covers entirely receives no other values.
    pub(super) fn from_batches(
        extent: VoxelExtent,
        grid: BrickGrid,
        batches: &[ValidatedBatch],
        identity: &VoxelVolumeId,
    ) -> Result<Self, VoxelFrontendError> {
        let mut bricks = PageTable::new(grid.brick_count);
        let mut partial_bricks: BTreeMap<usize, Vec<MaterialIndex>> = BTreeMap::new();
        for batch in batches {
            // Partial bricks start out empty and absent bricks mean empty, so an Empty fill
            // writes nothing.
            if batch.fill_value() == Some(MaterialIndex::EMPTY) {
                continue;
            }
            let [start_x, start_y, start_z] = batch.start;
            let [end_x, end_y, end_z] = batch.end;
            for brick_z in start_z / BRICK_EDGE..end_z.div_ceil(BRICK_EDGE) {
                for brick_y in start_y / BRICK_EDGE..end_y.div_ceil(BRICK_EDGE) {
                    for brick_x in start_x / BRICK_EDGE..end_x.div_ceil(BRICK_EDGE) {
                        record_publication_brick_visited();
                        let brick = [brick_x, brick_y, brick_z];
                        let brick_extent = grid.brick_extent(brick);
                        let x = brick_local(&(start_x..end_x), brick_x);
                        let y = brick_local(&(start_y..end_y), brick_y);
                        let z = brick_local(&(start_z..end_z), brick_z);
                        let key = grid.key(brick);
                        let covered = [&x, &y, &z]
                            .into_iter()
                            .zip(brick_extent)
                            .all(|(range, edge)| range.start == 0 && range.end == edge);
                        if covered && let Some(value) = batch.fill_value() {
                            bricks.insert(key, Brick::Uniform(value));
                            continue;
                        }
                        let [brick_width, brick_height, brick_depth] = brick_extent;
                        let values = match partial_bricks.entry(key) {
                            Entry::Occupied(entry) => entry.into_mut(),
                            Entry::Vacant(entry) => {
                                let count = brick_width * brick_height * brick_depth;
                                let mut values = Vec::new();
                                values.try_reserve_exact(count).map_err(|_| {
                                    VoxelFrontendError::VolumeAllocation {
                                        identity: identity.clone(),
                                    }
                                })?;
                                values.resize(count, MaterialIndex::EMPTY);
                                record_staged_values_allocated(count);
                                entry.insert(values)
                            }
                        };
                        let origin = brick.map(|index| index * BRICK_EDGE);
                        for local_z in z.clone() {
                            for local_y in y.clone() {
                                let row = (local_z * brick_height + local_y) * brick_width;
                                for local_x in x.clone() {
                                    if let Some(destination) = values.get_mut(row + local_x) {
                                        *destination = batch.value_at([
                                            origin[0] + local_x,
                                            origin[1] + local_y,
                                            origin[2] + local_z,
                                        ]);
                                    }
                                }
                            }
                        }
                        record_publication_values_written(x.len() * y.len() * z.len());
                    }
                }
            }
        }
        for (key, values) in partial_bricks {
            if let Some(brick) = Brick::normalize(values) {
                bricks.insert(key, brick);
            }
        }
        Ok(Self {
            extent,
            grid,
            bricks: Arc::new(bricks),
        })
    }
}

#[cfg(test)]
impl SparseStorage {
    pub(super) fn brick(&self, key: usize) -> Option<&Brick> {
        self.bricks.get(&key)
    }

    pub(super) fn depth(&self) -> usize {
        self.bricks.depth()
    }

    pub(super) fn stored_bricks(&self) -> usize {
        self.bricks.iter().count()
    }
}

fn clip(start: i64, end: i64, limit: usize) -> Option<Range<usize>> {
    let start = usize::try_from(start.max(0)).ok()?;
    let end = usize::try_from(end.max(0)).ok()?.min(limit);
    Some(start..end.max(start))
}

fn brick_local(range: &Range<usize>, brick: usize) -> Range<usize> {
    let brick_start = brick * BRICK_EDGE;
    range.start.max(brick_start) - brick_start
        ..range.end.min(brick_start + BRICK_EDGE) - brick_start
}

/// Returns `None` once the region is known to be mixed.
fn merge_uniform(uniform: &mut Option<MaterialIndex>, value: MaterialIndex) -> Option<()> {
    if uniform.is_some_and(|previous| previous != value) {
        return None;
    }
    *uniform = Some(value);
    Some(())
}

impl Storage for SparseStorage {
    fn extent(&self) -> VoxelExtent {
        self.extent
    }
    fn value(&self, coordinate: VoxelCoordinate) -> MaterialIndex {
        let Some(location) = self.grid.locate(coordinate) else {
            return MaterialIndex::EMPTY;
        };
        match self.bricks.get(&location.key) {
            None => MaterialIndex::EMPTY,
            Some(Brick::Uniform(value)) => *value,
            Some(Brick::Mixed(values)) => values
                .get(location.offset)
                .copied()
                .unwrap_or(MaterialIndex::EMPTY),
        }
    }
    fn successor(&self, changes: &[(VoxelCoordinate, MaterialIndex)]) -> Arc<dyn Storage> {
        let mut edits_by_brick: BTreeMap<usize, BrickEdits> = BTreeMap::new();
        for &(coordinate, value) in changes {
            if let Some(location) = self.grid.locate(coordinate) {
                edits_by_brick
                    .entry(location.key)
                    .or_insert_with(|| BrickEdits {
                        value_count: location.value_count,
                        values: Vec::new(),
                    })
                    .values
                    .push((location.offset, value));
            }
        }
        let mut successor = self.clone();
        let bricks = Arc::make_mut(&mut successor.bricks);
        for (key, edits) in edits_by_brick {
            let mut values = match bricks.get(&key) {
                None => vec![MaterialIndex::EMPTY; edits.value_count],
                Some(Brick::Uniform(value)) => vec![*value; edits.value_count],
                Some(Brick::Mixed(values)) => values.to_vec(),
            };
            record_copied_brick_payload();
            for (offset, value) in edits.values {
                if let Some(destination) = values.get_mut(offset) {
                    *destination = value;
                }
            }
            match Brick::normalize(values) {
                Some(brick) => bricks.insert(key, brick),
                None => bricks.remove(&key),
            }
        }
        Arc::new(successor)
    }
    fn uniform_region(&self, bounds: &RegionBounds) -> Option<MaterialIndex> {
        let [width, height, depth] = self.grid.extent;
        let x = clip(bounds.start_x, bounds.end_x, width)?;
        let y = clip(bounds.start_y, bounds.end_y, height)?;
        let z = clip(bounds.start_z, bounds.end_z, depth)?;
        let inside = bounds.start_x >= 0
            && bounds.start_y >= 0
            && bounds.start_z >= 0
            && bounds.end_x <= i64::try_from(width).ok()?
            && bounds.end_y <= i64::try_from(height).ok()?
            && bounds.end_z <= i64::try_from(depth).ok()?;
        let mut uniform = (!inside).then_some(MaterialIndex::EMPTY);
        if x.is_empty() || y.is_empty() || z.is_empty() {
            return uniform;
        }
        for brick_z in z.start / BRICK_EDGE..z.end.div_ceil(BRICK_EDGE) {
            for brick_y in y.start / BRICK_EDGE..y.end.div_ceil(BRICK_EDGE) {
                for brick_x in x.start / BRICK_EDGE..x.end.div_ceil(BRICK_EDGE) {
                    let brick = [brick_x, brick_y, brick_z];
                    record_brick_examined();
                    match self.bricks.get(&self.grid.key(brick)) {
                        None => merge_uniform(&mut uniform, MaterialIndex::EMPTY)?,
                        Some(Brick::Uniform(value)) => merge_uniform(&mut uniform, *value)?,
                        Some(Brick::Mixed(values)) => {
                            let [brick_width, brick_height, _] = self.grid.brick_extent(brick);
                            for local_z in brick_local(&z, brick_z) {
                                for local_y in brick_local(&y, brick_y) {
                                    let row = (local_z * brick_height + local_y) * brick_width;
                                    for local_x in brick_local(&x, brick_x) {
                                        record_voxel_value_examined();
                                        merge_uniform(
                                            &mut uniform,
                                            values
                                                .get(row + local_x)
                                                .copied()
                                                .unwrap_or(MaterialIndex::EMPTY),
                                        )?;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        uniform
    }
    fn cell_source(self: Arc<Self>, grid: CellGrid) -> Box<dyn CellSource> {
        if grid.edge >= BRICK_EDGE {
            Box::new(MergingCellSource {
                bricks_per_cell: grid.edge / BRICK_EDGE,
                storage: self,
                grid,
                next_key: Some(0),
                slab: None,
                pending: HashMap::new(),
                draining: None,
            })
        } else {
            Box::new(SplittingCellSource {
                storage: self,
                grid,
                next_key: Some(0),
                current: None,
            })
        }
    }
    fn storage_bytes(&self) -> usize {
        size_of::<Self>()
            + self.bricks.storage_bytes()
            + 2 * size_of::<usize>()
            + self
                .bricks
                .values()
                .map(|brick| match brick {
                    Brick::Uniform(_) => 0,
                    Brick::Mixed(values) => {
                        2 * size_of::<usize>() + values.len() * size_of::<MaterialIndex>()
                    }
                })
                .sum::<usize>()
    }
    #[cfg(test)]
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Enumerates cells that each contain whole bricks, because cell and brick edges are both
/// powers of two. Resident bricks are visited in key order, so every brick of one z-slab of
/// cells is seen before any brick of the next, and only that slab's cells are held for merging.
struct MergingCellSource {
    storage: Arc<SparseStorage>,
    grid: CellGrid,
    bricks_per_cell: usize,
    next_key: Option<usize>,
    slab: Option<usize>,
    pending: HashMap<[usize; 3], ResidentBricks>,
    draining: Option<hash_map::IntoIter<[usize; 3], ResidentBricks>>,
}

/// The resident bricks of one cell. `content` is `None` once they disagree or one is mixed.
struct ResidentBricks {
    count: usize,
    content: Option<MaterialIndex>,
}

impl MergingCellSource {
    fn finish(
        &self,
        cell: [usize; 3],
        bricks: ResidentBricks,
    ) -> Result<EnumeratedCell, CellSourceError> {
        let bounds = self.grid.bounds(cell).ok_or(CellSourceError::Traversal)?;
        let mut covered = 1usize;
        for range in &bounds {
            covered = covered
                .checked_mul(range.end.div_ceil(BRICK_EDGE) - range.start / BRICK_EDGE)
                .ok_or(CellSourceError::Traversal)?;
        }
        // Absent bricks are Empty and every resident brick holds a non-empty value, so a cell
        // with any absent brick is mixed.
        let content = if bricks.count < covered {
            None
        } else {
            bricks.content
        };
        Ok(EnumeratedCell {
            cell,
            bounds,
            content,
        })
    }
}

impl CellSource for MergingCellSource {
    fn next_cell(&mut self) -> Result<Option<EnumeratedCell>, CellSourceError> {
        loop {
            if let Some(draining) = self.draining.as_mut() {
                if let Some((cell, bricks)) = draining.next() {
                    return self.finish(cell, bricks).map(Some);
                }
                self.draining = None;
            }
            let Some(start) = self.next_key else {
                return Ok(None);
            };
            let Some((key, brick)) = self.storage.bricks.first_at_or_after(start) else {
                self.next_key = None;
                self.draining = Some(std::mem::take(&mut self.pending).into_iter());
                continue;
            };
            let content = match brick {
                Brick::Uniform(value) => Some(*value),
                Brick::Mixed(_) => None,
            };
            let position = self
                .storage
                .grid
                .position(key)
                .ok_or(CellSourceError::Traversal)?;
            let cell = position.map(|index| index / self.bricks_per_cell);
            if self.slab.is_some_and(|slab| slab != cell[2]) {
                // Revisit this brick after the finished slab has been emitted.
                self.slab = Some(cell[2]);
                self.draining = Some(std::mem::take(&mut self.pending).into_iter());
                continue;
            }
            self.slab = Some(cell[2]);
            record_enumeration_brick_examined();
            self.next_key = key.checked_add(1);
            if let Some(bricks) = self.pending.get_mut(&cell) {
                bricks.count += 1;
                if bricks.content != content {
                    bricks.content = None;
                }
                continue;
            }
            self.pending
                .try_reserve(1)
                .map_err(|_| CellSourceError::Allocation)?;
            self.pending
                .insert(cell, ResidentBricks { count: 1, content });
            record_working_cells(self.pending.len());
        }
    }
}

/// Enumerates cells smaller than a brick, several of which fit inside each resident brick.
struct SplittingCellSource {
    storage: Arc<SparseStorage>,
    grid: CellGrid,
    next_key: Option<usize>,
    current: Option<SplitBrick>,
}

struct SplitBrick {
    brick: Brick,
    position: [usize; 3],
    extent: [usize; 3],
    cells: [usize; 3],
    next_cell: usize,
}

impl SplitBrick {
    /// Returns the next brick-local cell position, or `None` once every cell has been returned.
    fn advance(&mut self) -> Option<[usize; 3]> {
        let [cells_x, cells_y, cells_z] = self.cells;
        let index = self.next_cell;
        if index >= cells_x * cells_y * cells_z {
            return None;
        }
        self.next_cell += 1;
        Some([
            index % cells_x,
            index / cells_x % cells_y,
            index / (cells_x * cells_y),
        ])
    }

    /// Returns `Ok(None)` for a mixed cell. `local` is brick-local.
    fn classify(
        &self,
        local: &[Range<usize>; 3],
    ) -> Result<Option<MaterialIndex>, CellSourceError> {
        let values = match &self.brick {
            Brick::Uniform(value) => return Ok(Some(*value)),
            Brick::Mixed(values) => values,
        };
        let [brick_width, brick_height, _] = self.extent;
        let [x, y, z] = local;
        let mut uniform = None;
        for local_z in z.clone() {
            for local_y in y.clone() {
                let row = (local_z * brick_height + local_y) * brick_width;
                for local_x in x.clone() {
                    record_enumeration_value_examined();
                    let value = values
                        .get(row + local_x)
                        .copied()
                        .ok_or(CellSourceError::Traversal)?;
                    if merge_uniform(&mut uniform, value).is_none() {
                        return Ok(None);
                    }
                }
            }
        }
        Ok(uniform)
    }
}

impl CellSource for SplittingCellSource {
    fn next_cell(&mut self) -> Result<Option<EnumeratedCell>, CellSourceError> {
        let cells_per_brick = BRICK_EDGE / self.grid.edge;
        loop {
            if let Some(split) = self.current.as_mut() {
                let Some(local_cell) = split.advance() else {
                    self.current = None;
                    continue;
                };
                let mut cell = [0; 3];
                for axis in 0..3 {
                    cell[axis] = split.position[axis]
                        .checked_mul(cells_per_brick)
                        .and_then(|first| first.checked_add(local_cell[axis]))
                        .ok_or(CellSourceError::Traversal)?;
                }
                let bounds = self.grid.bounds(cell).ok_or(CellSourceError::Traversal)?;
                let local = [0, 1, 2].map(|axis| {
                    let origin = split.position[axis] * BRICK_EDGE;
                    bounds[axis].start - origin..bounds[axis].end - origin
                });
                let content = split.classify(&local)?;
                if content == Some(MaterialIndex::EMPTY) {
                    continue;
                }
                return Ok(Some(EnumeratedCell {
                    cell,
                    bounds,
                    content,
                }));
            }
            let Some(start) = self.next_key else {
                return Ok(None);
            };
            let Some((key, brick)) = self.storage.bricks.first_at_or_after(start) else {
                self.next_key = None;
                return Ok(None);
            };
            record_enumeration_brick_examined();
            let position = self
                .storage
                .grid
                .position(key)
                .ok_or(CellSourceError::Traversal)?;
            let extent = self.storage.grid.brick_extent(position);
            self.current = Some(SplitBrick {
                brick: brick.clone(),
                position,
                extent,
                cells: extent.map(|dimension| dimension.div_ceil(self.grid.edge)),
                next_cell: 0,
            });
            self.next_key = key.checked_add(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparse_edits_share_mixed_bricks_and_collapse_restored_bricks()
    -> Result<(), Box<dyn std::error::Error>> {
        let extent = VoxelExtent::new(513, 1, 1);
        let dense = DenseStorage {
            extent,
            pages: Arc::new({
                let mut pages = PageTable::new(3);
                for (index, page) in [
                    Arc::new(vec![MaterialIndex::EMPTY; 256]),
                    Arc::new((0..256).map(|index| MaterialIndex(index % 2)).collect()),
                    Arc::new(vec![MaterialIndex(1)]),
                ]
                .into_iter()
                .enumerate()
                {
                    pages.insert(index, page);
                }
                pages
            }),
        };
        let sparse = SparseStorage::from_dense(&dense, BrickGrid::new(extent).ok_or("grid")?);
        assert_eq!(sparse.stored_bricks(), 17);
        let edited = sparse.successor(&[(VoxelCoordinate::new(0, 0, 0), MaterialIndex(1))]);
        let edited = edited
            .as_any()
            .downcast_ref::<SparseStorage>()
            .ok_or("wrong tier")?;
        let (Some(Brick::Mixed(before)), Some(Brick::Mixed(after))) =
            (sparse.brick(16), edited.brick(16))
        else {
            return Err("missing mixed brick".into());
        };
        assert!(Arc::ptr_eq(before, after));
        assert_eq!(
            sparse.value(VoxelCoordinate::new(0, 0, 0)),
            MaterialIndex::EMPTY
        );
        let restored = edited.successor(&[(VoxelCoordinate::new(0, 0, 0), MaterialIndex::EMPTY)]);
        let restored = restored
            .as_any()
            .downcast_ref::<SparseStorage>()
            .ok_or("wrong tier")?;
        assert!(restored.brick(0).is_none());
        let cleared =
            restored.successor(&[(VoxelCoordinate::new(512, 0, 0), MaterialIndex::EMPTY)]);
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

    #[test]
    fn brick_grids_beyond_the_key_address_space_are_rejected() {
        let limit = 1 << 31;
        assert!(BrickGrid::new(VoxelExtent::new(limit, limit, limit)).is_none());
        #[cfg(target_pointer_width = "64")]
        {
            assert!(BrickGrid::new(VoxelExtent::new(limit, limit, 1 << 13)).is_some());
            assert!(BrickGrid::new(VoxelExtent::new(limit, limit, (1 << 14) + 1)).is_none());
        }
    }
}

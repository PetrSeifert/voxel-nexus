use super::storage_counters::record_cell_emitted;
use super::storage_tier::Storage;
use super::*;
use std::ops::Range;

/// The position of a cell on a Voxel Cell Grid, counted in cells from the volume-local origin.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct VoxelCellCoordinate {
    x: u32,
    y: u32,
    z: u32,
}

impl VoxelCellCoordinate {
    pub fn new(x: u32, y: u32, z: u32) -> Self {
        Self { x, y, z }
    }

    pub fn components(self) -> [u32; 3] {
        [self.x, self.y, self.z]
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VoxelCell {
    coordinate: VoxelCellCoordinate,
    region: VoxelRegion,
    content: VoxelRegionContent,
}

impl VoxelCell {
    pub fn coordinate(&self) -> VoxelCellCoordinate {
        self.coordinate
    }

    /// The cell's region clipped to the volume bounds.
    pub fn region(&self) -> VoxelRegion {
        self.region
    }

    pub fn content(&self) -> &VoxelRegionContent {
        &self.content
    }
}

/// Batches of the non-empty cells of one volume at the revision of the view it was created
/// from. Each non-empty cell appears exactly once, in unspecified order. The enumeration ends
/// after the first error.
pub struct VoxelCellEnumeration {
    view: VoxelSceneView,
    identity: VoxelVolumeId,
    batch_capacity: usize,
    source: Option<Box<dyn CellSource>>,
}

impl VoxelCellEnumeration {
    pub(super) fn new(
        view: VoxelSceneView,
        identity: VoxelVolumeId,
        cell_edge: u32,
        batch_capacity: usize,
    ) -> Result<Self, VoxelFrontendError> {
        let storage = view.published.volumes.get(&identity).ok_or_else(|| {
            VoxelFrontendError::UnknownVolumeIdentity {
                identity: identity.clone(),
            }
        })?;
        if batch_capacity == 0 {
            return Err(VoxelFrontendError::ZeroCellBatchCapacity { identity });
        }
        let grid = CellGrid::new(storage.extent(), cell_edge).ok_or_else(|| {
            VoxelFrontendError::InvalidCellEdge {
                identity: identity.clone(),
                cell_edge,
            }
        })?;
        let source = Arc::clone(storage).cell_source(grid);
        Ok(Self {
            view,
            identity,
            batch_capacity,
            source: Some(source),
        })
    }

    fn next_batch(&mut self) -> Result<Vec<VoxelCell>, VoxelFrontendError> {
        let mut batch = Vec::new();
        while batch.len() < self.batch_capacity {
            let Some(source) = self.source.as_mut() else {
                break;
            };
            let Some(cell) = source.next_cell().map_err(|error| match error {
                CellSourceError::Allocation => VoxelFrontendError::CellEnumerationAllocation {
                    identity: self.identity.clone(),
                },
                CellSourceError::Traversal => VoxelFrontendError::CellEnumerationTraversal {
                    identity: self.identity.clone(),
                },
            })?
            else {
                self.source = None;
                break;
            };
            let cell = self.public_cell(cell).ok_or_else(|| {
                VoxelFrontendError::CellEnumerationTraversal {
                    identity: self.identity.clone(),
                }
            })?;
            // Growing on demand keeps a generous capacity from reserving memory up front.
            batch
                .try_reserve(1)
                .map_err(|_| VoxelFrontendError::CellEnumerationAllocation {
                    identity: self.identity.clone(),
                })?;
            batch.push(cell);
            record_cell_emitted();
        }
        Ok(batch)
    }

    fn public_cell(&self, cell: EnumeratedCell) -> Option<VoxelCell> {
        let [x, y, z] = cell.cell.map(u32::try_from);
        let [range_x, range_y, range_z] = cell.bounds;
        let origin = [&range_x, &range_y, &range_z].map(|range| i32::try_from(range.start));
        let extent = [range_x, range_y, range_z].map(|range| u32::try_from(range.len()));
        let [origin_x, origin_y, origin_z] = origin;
        let [width, height, depth] = extent;
        let content = match cell.content {
            Some(index) => VoxelRegionContent::Uniform(
                self.view
                    .published
                    .palette_values
                    .get(index.0 as usize)?
                    .clone(),
            ),
            None => VoxelRegionContent::Mixed,
        };
        Some(VoxelCell {
            coordinate: VoxelCellCoordinate::new(x.ok()?, y.ok()?, z.ok()?),
            region: VoxelRegion::new(
                VoxelCoordinate::new(origin_x.ok()?, origin_y.ok()?, origin_z.ok()?),
                VoxelExtent::new(width.ok()?, height.ok()?, depth.ok()?),
            ),
            content,
        })
    }
}

impl Iterator for VoxelCellEnumeration {
    type Item = Result<Vec<VoxelCell>, VoxelFrontendError>;

    fn next(&mut self) -> Option<Self::Item> {
        self.source.as_ref()?;
        match self.next_batch() {
            Ok(batch) if batch.is_empty() => None,
            Ok(batch) => Some(Ok(batch)),
            Err(error) => {
                self.source = None;
                Some(Err(error))
            }
        }
    }
}

/// A Voxel Cell Grid over one volume. Its edge is a power of two and it has at least one
/// cell along each axis, because volume extents are never empty.
#[derive(Clone, Copy)]
pub(super) struct CellGrid {
    pub(super) edge: usize,
    extent: [usize; 3],
    cells: [usize; 3],
}

impl CellGrid {
    fn new(extent: VoxelExtent, edge: u32) -> Option<Self> {
        if !edge.is_power_of_two() {
            return None;
        }
        let edge = usize::try_from(edge).ok()?;
        let [width, height, depth] = extent.dimensions();
        let extent = [
            usize::try_from(width).ok()?,
            usize::try_from(height).ok()?,
            usize::try_from(depth).ok()?,
        ];
        Some(Self {
            edge,
            extent,
            cells: extent.map(|dimension| dimension.div_ceil(edge)),
        })
    }

    /// The cell's volume-local bounds clipped to the volume, or `None` when the cell lies
    /// outside the grid.
    pub(super) fn bounds(&self, cell: [usize; 3]) -> Option<[Range<usize>; 3]> {
        let bounds = |axis: usize| {
            let start = cell[axis].checked_mul(self.edge)?;
            let remaining = self.extent[axis]
                .checked_sub(start)
                .filter(|remaining| *remaining > 0)?;
            Some(start..start + remaining.min(self.edge))
        };
        Some([bounds(0)?, bounds(1)?, bounds(2)?])
    }
}

/// A non-empty cell found by a Storage Tier. `content` is `None` for a mixed cell.
pub(super) struct EnumeratedCell {
    pub(super) cell: [usize; 3],
    pub(super) bounds: [Range<usize>; 3],
    pub(super) content: Option<MaterialIndex>,
}

pub(super) enum CellSourceError {
    Allocation,
    Traversal,
}

pub(super) trait CellSource: Send {
    /// Returns each non-empty cell once, then `None`.
    fn next_cell(&mut self) -> Result<Option<EnumeratedCell>, CellSourceError>;
}

/// Classifies every cell of the grid in turn. Its cost follows the volume bounds, which only
/// tiers without cost guarantees may use.
pub(super) struct ScanningCellSource {
    storage: Arc<dyn Storage>,
    grid: CellGrid,
    next: Option<[usize; 3]>,
}

impl ScanningCellSource {
    pub(super) fn new(storage: Arc<dyn Storage>, grid: CellGrid) -> Self {
        Self {
            storage,
            grid,
            next: Some([0; 3]),
        }
    }

    fn advance(&mut self, [x, y, z]: [usize; 3]) {
        let [cells_x, cells_y, cells_z] = self.grid.cells;
        self.next = if x + 1 < cells_x {
            Some([x + 1, y, z])
        } else if y + 1 < cells_y {
            Some([0, y + 1, z])
        } else if z + 1 < cells_z {
            Some([0, 0, z + 1])
        } else {
            None
        };
    }
}

impl CellSource for ScanningCellSource {
    fn next_cell(&mut self) -> Result<Option<EnumeratedCell>, CellSourceError> {
        while let Some(cell) = self.next {
            self.advance(cell);
            let bounds = self.grid.bounds(cell).ok_or(CellSourceError::Traversal)?;
            let [x, y, z] = [&bounds[0], &bounds[1], &bounds[2]].map(|range| {
                Some((
                    i64::try_from(range.start).ok()?,
                    i64::try_from(range.end).ok()?,
                ))
            });
            let ((start_x, end_x), (start_y, end_y), (start_z, end_z)) = (
                x.ok_or(CellSourceError::Traversal)?,
                y.ok_or(CellSourceError::Traversal)?,
                z.ok_or(CellSourceError::Traversal)?,
            );
            let region = RegionBounds {
                start_x,
                start_y,
                start_z,
                end_x,
                end_y,
                end_z,
            };
            let content = self.storage.uniform_region(&region);
            if content != Some(MaterialIndex::EMPTY) {
                return Ok(Some(EnumeratedCell {
                    cell,
                    bounds,
                    content,
                }));
            }
        }
        Ok(None)
    }
}

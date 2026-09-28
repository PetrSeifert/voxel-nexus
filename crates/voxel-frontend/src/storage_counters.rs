#[cfg(any(test, feature = "qualification"))]
use std::cell::Cell;

#[cfg(any(test, feature = "qualification"))]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StorageWorkCounters {
    pub copied_nodes: usize,
    pub copied_brick_payloads: usize,
    pub bricks_examined: usize,
    pub voxel_values_examined: usize,
    pub publication: PublicationWorkCounters,
    pub validation: ValidationWorkCounters,
    pub enumeration: EnumerationWorkCounters,
}

/// Work and staging allocations spent building Storage Tier contents from publication input.
#[cfg(any(test, feature = "qualification"))]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PublicationWorkCounters {
    pub bricks_visited: usize,
    pub voxel_values_written: usize,
    pub staged_values_allocated: usize,
}

/// Work spent validating sparse publication input before any storage is built.
#[cfg(any(test, feature = "qualification"))]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ValidationWorkCounters {
    pub batches_validated: usize,
    pub voxel_values_validated: usize,
    pub candidate_pairs_examined: usize,
}

/// Work spent enumerating the cells of a Voxel Cell Grid. Working cells are held for
/// deduplication and are separate from the cells held in output batches.
#[cfg(any(test, feature = "qualification"))]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EnumerationWorkCounters {
    pub nodes_visited: usize,
    pub bricks_examined: usize,
    pub voxel_values_examined: usize,
    pub cells_emitted: usize,
    pub peak_working_cells: usize,
}

#[cfg(any(test, feature = "qualification"))]
impl StorageWorkCounters {
    fn sum(self, other: Self) -> Self {
        Self {
            copied_nodes: self.copied_nodes + other.copied_nodes,
            copied_brick_payloads: self.copied_brick_payloads + other.copied_brick_payloads,
            bricks_examined: self.bricks_examined + other.bricks_examined,
            voxel_values_examined: self.voxel_values_examined + other.voxel_values_examined,
            publication: PublicationWorkCounters {
                bricks_visited: self.publication.bricks_visited + other.publication.bricks_visited,
                voxel_values_written: self.publication.voxel_values_written
                    + other.publication.voxel_values_written,
                staged_values_allocated: self.publication.staged_values_allocated
                    + other.publication.staged_values_allocated,
            },
            validation: ValidationWorkCounters {
                batches_validated: self.validation.batches_validated
                    + other.validation.batches_validated,
                voxel_values_validated: self.validation.voxel_values_validated
                    + other.validation.voxel_values_validated,
                candidate_pairs_examined: self.validation.candidate_pairs_examined
                    + other.validation.candidate_pairs_examined,
            },
            enumeration: EnumerationWorkCounters {
                nodes_visited: self.enumeration.nodes_visited + other.enumeration.nodes_visited,
                bricks_examined: self.enumeration.bricks_examined
                    + other.enumeration.bricks_examined,
                voxel_values_examined: self.enumeration.voxel_values_examined
                    + other.enumeration.voxel_values_examined,
                cells_emitted: self.enumeration.cells_emitted + other.enumeration.cells_emitted,
                peak_working_cells: self
                    .enumeration
                    .peak_working_cells
                    .max(other.enumeration.peak_working_cells),
            },
        }
    }
}

// Storage work always runs on the thread that calls into the Voxel Frontend, so
// per-thread counters isolate concurrent measurements without synchronization.
#[cfg(any(test, feature = "qualification"))]
thread_local! {
    static COUNTERS: Cell<StorageWorkCounters> = Cell::new(StorageWorkCounters::default());
}

/// Runs `operation` and reports the Storage Tier work it performed on the calling thread.
#[cfg(any(test, feature = "qualification"))]
pub fn count_storage_work<R>(operation: impl FnOnce() -> R) -> (R, StorageWorkCounters) {
    let outer = COUNTERS.replace(StorageWorkCounters::default());
    let result = operation();
    let counted = COUNTERS.replace(outer);
    COUNTERS.set(outer.sum(counted));
    (result, counted)
}

#[cfg(any(test, feature = "qualification"))]
fn record(update: impl FnOnce(&mut StorageWorkCounters)) {
    let mut counters = COUNTERS.get();
    update(&mut counters);
    COUNTERS.set(counters);
}

pub(super) fn record_copied_node() {
    #[cfg(any(test, feature = "qualification"))]
    record(|counters| counters.copied_nodes += 1);
}

pub(super) fn record_copied_brick_payload() {
    #[cfg(any(test, feature = "qualification"))]
    record(|counters| counters.copied_brick_payloads += 1);
}

pub(super) fn record_brick_examined() {
    #[cfg(any(test, feature = "qualification"))]
    record(|counters| counters.bricks_examined += 1);
}

pub(super) fn record_voxel_value_examined() {
    #[cfg(any(test, feature = "qualification"))]
    record(|counters| counters.voxel_values_examined += 1);
}

pub(super) fn record_publication_brick_visited() {
    #[cfg(any(test, feature = "qualification"))]
    record(|counters| counters.publication.bricks_visited += 1);
}

#[cfg_attr(not(any(test, feature = "qualification")), allow(unused_variables))]
pub(super) fn record_publication_values_written(count: usize) {
    #[cfg(any(test, feature = "qualification"))]
    record(|counters| counters.publication.voxel_values_written += count);
}

#[cfg_attr(not(any(test, feature = "qualification")), allow(unused_variables))]
pub(super) fn record_staged_values_allocated(count: usize) {
    #[cfg(any(test, feature = "qualification"))]
    record(|counters| counters.publication.staged_values_allocated += count);
}

pub(super) fn record_batch_validated() {
    #[cfg(any(test, feature = "qualification"))]
    record(|counters| counters.validation.batches_validated += 1);
}

#[cfg_attr(not(any(test, feature = "qualification")), allow(unused_variables))]
pub(super) fn record_values_validated(count: usize) {
    #[cfg(any(test, feature = "qualification"))]
    record(|counters| counters.validation.voxel_values_validated += count);
}

pub(super) fn record_candidate_pair_examined() {
    #[cfg(any(test, feature = "qualification"))]
    record(|counters| counters.validation.candidate_pairs_examined += 1);
}

pub(super) fn record_enumeration_node_visited() {
    #[cfg(any(test, feature = "qualification"))]
    record(|counters| counters.enumeration.nodes_visited += 1);
}

pub(super) fn record_enumeration_brick_examined() {
    #[cfg(any(test, feature = "qualification"))]
    record(|counters| counters.enumeration.bricks_examined += 1);
}

pub(super) fn record_enumeration_value_examined() {
    #[cfg(any(test, feature = "qualification"))]
    record(|counters| counters.enumeration.voxel_values_examined += 1);
}

pub(super) fn record_cell_emitted() {
    #[cfg(any(test, feature = "qualification"))]
    record(|counters| counters.enumeration.cells_emitted += 1);
}

#[cfg_attr(not(any(test, feature = "qualification")), allow(unused_variables))]
pub(super) fn record_working_cells(count: usize) {
    #[cfg(any(test, feature = "qualification"))]
    record(|counters| {
        counters.enumeration.peak_working_cells = counters.enumeration.peak_working_cells.max(count)
    });
}

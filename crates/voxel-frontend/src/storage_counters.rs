#[cfg(any(test, feature = "qualification"))]
use std::cell::Cell;

#[cfg(any(test, feature = "qualification"))]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StorageWorkCounters {
    pub copied_nodes: usize,
    pub copied_brick_payloads: usize,
    pub bricks_examined: usize,
    pub voxel_values_examined: usize,
    pub classification_nodes_visited: usize,
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

/// Validation excludes input and storage buffers so construction can be measured separately.
#[cfg(any(test, feature = "qualification"))]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ValidationWorkCounters {
    pub batches_validated: usize,
    pub voxel_values_validated: usize,
    pub candidate_pairs_examined: usize,
    /// Number of overlap sweeps selected on x, y, and z respectively.
    pub sweeps_by_axis: [usize; 3],
    /// Total sparse validation wall time, including rejected input.
    pub elapsed: std::time::Duration,
    /// Total bytes reserved by validation buffers, excluding caller-owned input and storage.
    pub allocated_bytes: usize,
    /// Maximum simultaneously reserved validation buffer bytes, including resolved batches.
    pub peak_working_bytes: usize,
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
            classification_nodes_visited: self.classification_nodes_visited
                + other.classification_nodes_visited,
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
                sweeps_by_axis: std::array::from_fn(|axis| {
                    self.validation.sweeps_by_axis[axis] + other.validation.sweeps_by_axis[axis]
                }),
                elapsed: self.validation.elapsed + other.validation.elapsed,
                allocated_bytes: self.validation.allocated_bytes + other.validation.allocated_bytes,
                peak_working_bytes: self
                    .validation
                    .peak_working_bytes
                    .max(other.validation.peak_working_bytes),
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

pub(super) fn record_classification_node_visited() {
    #[cfg(any(test, feature = "qualification"))]
    record(|counters| counters.classification_nodes_visited += 1);
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

#[cfg_attr(not(any(test, feature = "qualification")), allow(unused_variables))]
pub(super) fn record_sweep_axis(axis: usize) {
    #[cfg(any(test, feature = "qualification"))]
    record(|counters| {
        *counters
            .validation
            .sweeps_by_axis
            .get_mut(axis)
            .expect("overlap sweep selects one of the three axes") += 1;
    });
}

#[cfg(any(test, feature = "qualification"))]
pub(super) struct ValidationTimer(pub(super) std::time::Instant);

#[cfg(any(test, feature = "qualification"))]
impl Drop for ValidationTimer {
    fn drop(&mut self) {
        record(|counters| counters.validation.elapsed += self.0.elapsed());
    }
}

#[derive(Default)]
pub(super) struct ValidationMemory {
    #[cfg(any(test, feature = "qualification"))]
    allocated: Cell<usize>,
    #[cfg(any(test, feature = "qualification"))]
    working: Cell<usize>,
    #[cfg(any(test, feature = "qualification"))]
    peak: Cell<usize>,
}

impl ValidationMemory {
    #[cfg_attr(not(any(test, feature = "qualification")), allow(unused_variables))]
    pub(super) fn retain_vector<T>(&self, vector: &Vec<T>) {
        #[cfg(any(test, feature = "qualification"))]
        {
            let bytes = vector.capacity() * std::mem::size_of::<T>();
            self.allocated.set(self.allocated.get() + bytes);
            self.working.set(self.working.get() + bytes);
            self.peak.set(self.peak.get().max(self.working.get()));
        }
    }

    pub(super) fn temporary_vector<T>(&self, vector: &Vec<T>) -> ValidationAllocation<'_> {
        self.retain_vector(vector);
        ValidationAllocation {
            #[cfg(any(test, feature = "qualification"))]
            memory: self,
            #[cfg(any(test, feature = "qualification"))]
            bytes: vector.capacity() * std::mem::size_of::<T>(),
            #[cfg(not(any(test, feature = "qualification")))]
            marker: std::marker::PhantomData,
        }
    }
}

impl Drop for ValidationMemory {
    fn drop(&mut self) {
        #[cfg(any(test, feature = "qualification"))]
        record(|counters| {
            counters.validation.allocated_bytes += self.allocated.get();
            counters.validation.peak_working_bytes =
                counters.validation.peak_working_bytes.max(self.peak.get());
        });
    }
}

pub(super) struct ValidationAllocation<'a> {
    #[cfg(any(test, feature = "qualification"))]
    memory: &'a ValidationMemory,
    #[cfg(any(test, feature = "qualification"))]
    bytes: usize,
    #[cfg(not(any(test, feature = "qualification")))]
    marker: std::marker::PhantomData<&'a ValidationMemory>,
}

impl Drop for ValidationAllocation<'_> {
    fn drop(&mut self) {
        #[cfg(any(test, feature = "qualification"))]
        self.memory
            .working
            .set(self.memory.working.get() - self.bytes);
    }
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

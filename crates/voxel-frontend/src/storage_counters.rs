#[cfg(any(test, feature = "qualification"))]
use std::cell::Cell;

#[cfg(any(test, feature = "qualification"))]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StorageWorkCounters {
    pub copied_nodes: usize,
    pub copied_brick_payloads: usize,
    pub bricks_examined: usize,
    pub voxel_values_examined: usize,
}

#[cfg(any(test, feature = "qualification"))]
impl StorageWorkCounters {
    fn sum(self, other: Self) -> Self {
        Self {
            copied_nodes: self.copied_nodes + other.copied_nodes,
            copied_brick_payloads: self.copied_brick_payloads + other.copied_brick_payloads,
            bricks_examined: self.bricks_examined + other.bricks_examined,
            voxel_values_examined: self.voxel_values_examined + other.voxel_values_examined,
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

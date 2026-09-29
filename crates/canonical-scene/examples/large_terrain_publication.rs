#[path = "support/allocation.rs"]
mod allocation;

use std::sync::atomic::Ordering;
use std::time::Instant;

use allocation::{LIVE, PEAK};
use canonical_scene::generate_large_terrain;
use voxel_frontend::{VoxelFrontend, VoxelVolumeId, count_storage_work};

fn main() -> Result<(), String> {
    println!(
        "fingerprint,fill_batches,detail_batches,detail_values,generation_ms,publication_ms,validation_ms,sweeps_x,sweeps_y,sweeps_z,candidate_pairs,staged_values,storage_bytes,input_heap_bytes,retained_heap_bytes,publication_peak_heap_bytes"
    );
    let baseline = LIVE.load(Ordering::SeqCst);
    let started = Instant::now();
    let terrain = generate_large_terrain().map_err(|error| error.to_string())?;
    let generation_ms = started.elapsed().as_secs_f64() * 1000.0;
    let fingerprint = terrain.content_fingerprint();
    let fills = terrain.fill_batch_count();
    let details = terrain.detail_batch_count();
    let values = terrain.detail_value_count();
    let scene = terrain.into_scene();
    let input = LIVE.load(Ordering::SeqCst) - baseline;
    PEAK.store(LIVE.load(Ordering::SeqCst), Ordering::SeqCst);
    let frontend = VoxelFrontend::new();
    let started = Instant::now();
    let (view, counters) = count_storage_work(|| frontend.publish_sparse(scene));
    let publication_ms = started.elapsed().as_secs_f64() * 1000.0;
    let view = view.map_err(|error| error.to_string())?;
    let retained = LIVE.load(Ordering::SeqCst) - baseline;
    let peak = PEAK.load(Ordering::SeqCst) - baseline;
    let storage = view
        .storage_bytes(&VoxelVolumeId::new("large-terrain"))
        .map_err(|error| error.to_string())?;
    let validation_ms = counters.validation.elapsed.as_secs_f64() * 1000.0;
    let [sweep_x, sweep_y, sweep_z] = counters.validation.sweeps_by_axis;
    println!(
        "{fingerprint:016x},{fills},{details},{values},{generation_ms:.3},{publication_ms:.3},{validation_ms:.3},{sweep_x},{sweep_y},{sweep_z},{},{},{storage},{input},{retained},{peak}",
        counters.validation.candidate_pairs_examined, counters.publication.staged_values_allocated
    );
    Ok(())
}

use canonical_scene::generate_large_terrain;
use compute_ray_render_path::BrickmapSceneBundle;
use voxel_frontend::VoxelFrontend;

fn main() -> Result<(), String> {
    let terrain = generate_large_terrain().map_err(|error| error.to_string())?;
    let fingerprint = terrain.content_fingerprint();
    let view = VoxelFrontend::new()
        .publish_sparse(terrain.into_scene())
        .map_err(|error| error.to_string())?;
    let bundle = BrickmapSceneBundle::from_view(&view).map_err(|error| error.to_string())?;
    let observations = bundle.observations();
    let coarse_bytes = observations.coarse_grid_bytes as u64;
    let mixed_bricks = observations.mixed_brick_count as u64;
    let pool_bytes = observations.pool_bytes as u64;
    let capacity_slots = sum(mixed_bricks, mixed_bricks.div_ceil(4))?;
    let capacity_bytes = capacity_slots
        .checked_mul(1024)
        .ok_or("pool capacity overflow")?;
    // The GPU ABI is not implemented yet. Reserve an explicit allowance for
    // scene/volume headers, material colors, and allocation alignment per copy.
    let metadata_allowance_bytes = 64 * 1024;
    let representation_bytes = sum(sum(coarse_bytes, capacity_bytes)?, metadata_allowance_bytes)?;
    let coexistence_bytes = sum(representation_bytes, representation_bytes)?;
    // Charge a full padded candidate again for upload staging, even if the
    // eventual uploader only stages occupied slots or uses host memory.
    let peak_with_staging_bytes = sum(coexistence_bytes, representation_bytes)?;
    let budget_bytes = 1u64 << 30;
    // Recorded development RTX 4070 limit from issue #98, not a live query.
    let recorded_max_storage_buffer_range = 4_294_967_295u64;
    let dense_bytes = view.volumes().iter().try_fold(0u64, |total, volume| {
        let bytes =
            volume
                .extent()
                .dimensions()
                .into_iter()
                .try_fold(4u64, |bytes, dimension| {
                    bytes
                        .checked_mul(u64::from(dimension))
                        .ok_or("dense size overflow")
                })?;
        sum(total, bytes)
    })?;
    println!("fingerprint={fingerprint:016x}");
    println!("coarse_grid_bytes={coarse_bytes}");
    println!("mixed_brick_count={mixed_bricks}");
    println!("occupied_pool_bytes={pool_bytes}");
    println!("pool_capacity_slots={capacity_slots}");
    println!("pool_bytes_with_25_percent_headroom={capacity_bytes}");
    println!("metadata_allowance_bytes_per_copy={metadata_allowance_bytes}");
    println!("representation_bytes={representation_bytes}");
    println!("visible_plus_candidate_bytes={coexistence_bytes}");
    println!("peak_with_full_candidate_staging_bytes={peak_with_staging_bytes}");
    println!("budget_bytes={budget_bytes}");
    println!("dense_payload_bytes={dense_bytes}");
    println!("recorded_max_storage_buffer_range={recorded_max_storage_buffer_range}");
    println!(
        "construction_ms={:.3}",
        observations.construction_time.as_secs_f64() * 1000.0
    );
    if dense_bytes <= recorded_max_storage_buffer_range {
        return Err("terrain no longer proves the dense storage-buffer limit is exceeded".into());
    }
    if peak_with_staging_bytes > budget_bytes {
        return Err("predicted GPU scene peak exceeds the 1 GiB budget".into());
    }
    println!("budget_result=pass");
    Ok(())
}

fn sum(left: u64, right: u64) -> Result<u64, String> {
    left.checked_add(right)
        .ok_or_else(|| "footprint size overflow".into())
}

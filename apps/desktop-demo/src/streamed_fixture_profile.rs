#[path = "streamed_fixture_allocations.rs"]
#[allow(dead_code)]
mod allocation;
mod streamed_fixture_recipe;
mod streamed_fixture_vulkan;

use allocation::{LIVE, PEAK};
use ash::vk;
use compute_ray_render_path::{ComputeRepresentation, ComputeSceneBundle};
use raster_render_path::derive_raster_regions;
use serde_json::{Value, json};
use std::{io::Write, sync::atomic::Ordering, time::Instant};
use streamed_fixture_recipe::{EDGE, catalog, edit, fingerprint, scene, volume_identity};
use streamed_fixture_vulkan::Device;
use voxel_frontend::{
    VoxelCoordinate, VoxelExtent, VoxelFrontend, VoxelMaterialId, VoxelRegion, VoxelSceneView,
    VoxelValue,
};

enum CurrentContents {
    Edited,
    Restored,
}

fn verify(view: &VoxelSceneView, edited: bool) -> Result<(), String> {
    let samples = view
        .read_region(
            &volume_identity(3, 3),
            VoxelRegion::new(
                VoxelCoordinate::new(0, 0, 0),
                VoxelExtent::new(EDGE, EDGE, EDGE),
            ),
        )
        .map_err(|error| error.to_string())?;
    let stone = VoxelMaterialId::new("stone");
    let grass = VoxelMaterialId::new("grass");
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for sample in samples {
        let code = match sample.value() {
            VoxelValue::Empty => 0,
            VoxelValue::Occupied(identity) if *identity == stone => 1,
            VoxelValue::Occupied(identity) if *identity == grass => 2,
            _ => return Err("unexpected fixture material".into()),
        };
        hash = (hash ^ code).wrapping_mul(0x100_0000_01b3);
    }
    if hash != fingerprint(edited) {
        return Err(format!(
            "published fixture fingerprint mismatch: {hash:016x}"
        ));
    }
    Ok(())
}

fn heap_start() -> usize {
    let baseline = LIVE.load(Ordering::SeqCst);
    PEAK.store(baseline, Ordering::SeqCst);
    baseline
}

fn measure<T>(operation: impl FnOnce() -> Result<T, String>) -> Result<(T, Value), String> {
    let baseline = heap_start();
    let started = Instant::now();
    let result = operation()?;
    let milliseconds = started.elapsed().as_secs_f64() * 1000.0;
    let live = LIVE
        .load(Ordering::SeqCst)
        .checked_sub(baseline)
        .ok_or("measurement freed pre-existing allocations")?;
    let peak = PEAK.load(Ordering::SeqCst).saturating_sub(baseline);
    Ok((
        result,
        json!({"live_bytes": live, "peak_bytes": peak, "milliseconds": milliseconds}),
    ))
}

fn output(record: Value) -> Result<(), String> {
    let mut stdout = std::io::stdout().lock();
    serde_json::to_writer(&mut stdout, &record).map_err(|error| error.to_string())?;
    writeln!(stdout).map_err(|error| error.to_string())
}

fn profile(view: &VoxelSceneView, name: &str, device: &Device) -> Result<(), String> {
    let (raster, raster_heap) = measure(|| {
        derive_raster_regions(view, VoxelExtent::new(16, 16, 16)).map_err(|error| error.to_string())
    })?;
    let mut allocation_bytes = 0;
    let mut buffers = 0;
    for region in raster.regions().iter().filter(|region| !region.is_empty()) {
        for (bytes, usage) in [
            (
                std::mem::size_of_val(region.vertices()),
                vk::BufferUsageFlags::VERTEX_BUFFER,
            ),
            (
                std::mem::size_of_val(region.indices()),
                vk::BufferUsageFlags::INDEX_BUFFER,
            ),
            (
                std::mem::size_of_val(region.material_colors()),
                vk::BufferUsageFlags::STORAGE_BUFFER,
            ),
        ] {
            allocation_bytes += device.allocate_buffer(bytes, usage)?;
            buffers += 1;
        }
    }
    output(
        json!({"kind": "raster", "state": name, "volumes": view.volumes().len(),
        "heap": raster_heap, "faces": raster.semantic_face_count(),
        "isolated_vulkan_allocation_bytes_sum": allocation_bytes, "buffer_count": buffers,
        "vertex_payload_bytes": raster.vertex_byte_size(),
        "index_payload_bytes": raster.index_byte_size(), "regions": raster.regions().len()}),
    )?;
    drop(raster);
    let (compute, compute_heap) = measure(|| {
        ComputeSceneBundle::from_view_with_representation(
            view,
            ComputeRepresentation::Brickmap {
                budget_bytes: 1 << 30,
            },
        )
        .map_err(|error| error.to_string())
    })?;
    let words = compute.storage_words();
    let allocation_bytes =
        device.allocate_buffer(words.len() * 4, vk::BufferUsageFlags::STORAGE_BUFFER)?;
    output(
        json!({"kind": "brickmap", "state": name, "volumes": view.volumes().len(),
        "heap": compute_heap, "serialized_payload_bytes": words.len() * 4,
        "isolated_vulkan_allocation_bytes_sum": allocation_bytes, "buffer_count": 1}),
    )?;
    Ok(())
}

fn run(
    coordinates: &[(u32, u32)],
    contents: CurrentContents,
    name: &str,
    device: &Device,
) -> Result<(), String> {
    let baseline = heap_start();
    let frontend = VoxelFrontend::new();
    let (generated, publication) = measure(|| {
        frontend
            .publish_sparse(scene(coordinates))
            .map_err(|error| error.to_string())
    })?;
    output(
        json!({"kind": "publication", "state": name, "volumes": coordinates.len(),
        "heap": publication}),
    )?;
    verify(&generated, false)?;
    drop(generated);
    let mut view = frontend
        .edit(edit(3, 3, false))
        .map_err(|error| error.to_string())?
        .view()
        .clone();
    if matches!(contents, CurrentContents::Restored) {
        view = frontend
            .edit(edit(3, 3, true))
            .map_err(|error| error.to_string())?
            .view()
            .clone();
    }
    let installed_live_bytes = LIVE.load(Ordering::SeqCst) - baseline;
    verify(&view, matches!(contents, CurrentContents::Edited))?;
    output(json!({"kind": "installed-cpu", "state": name,
        "volumes": coordinates.len(), "live_bytes": installed_live_bytes}))?;
    profile(&view, name, device)?;
    drop(frontend);
    let retained_bytes = LIVE.load(Ordering::SeqCst).saturating_sub(baseline);
    output(json!({"kind": "view-retention", "state": name,
        "volumes_pinned_after_frontend_drop": view.volumes().len(),
        "live_bytes": retained_bytes}))?;
    Ok(())
}

fn main() -> Result<(), String> {
    let mode = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "calibration".into());
    let device = Device::new()?;
    output(device.context())?;
    output(
        json!({"kind": "fixture", "recipe": "streamed-qualification-v1", "edge": EDGE,
        "generated_fingerprint": format!("{:016x}", fingerprint(false)),
        "edited_fingerprint": format!("{:016x}", fingerprint(true)),
        "mode": mode, "qualification": "incomplete: allocation calibration only"}),
    )?;
    match mode.as_str() {
        "baseline" => {
            let coordinates = (0..16)
                .flat_map(|z| (0..16).map(move |x| (x, z)))
                .collect::<Vec<_>>();
            run(
                &coordinates,
                CurrentContents::Edited,
                "fully-resident-256",
                &device,
            )?;
        }
        "calibration" => {
            let frontend = VoxelFrontend::new();
            let (generated, publication) = measure(|| {
                frontend
                    .publish_sparse(scene(&[(3, 3)]))
                    .map_err(|error| error.to_string())
            })?;
            output(
                json!({"kind": "publication", "state": "generated", "volumes": 1, "heap": publication}),
            )?;
            verify(&generated, false)?;
            profile(&generated, "generated", &device)?;
            let (edited, edit_heap) = measure(|| {
                frontend
                    .edit(edit(3, 3, false))
                    .map_err(|error| error.to_string())
            })?;
            output(json!({"kind": "edit", "state": "edited", "heap": edit_heap}))?;
            verify(edited.view(), true)?;
            verify(&generated, false)?;
            profile(edited.view(), "edited", &device)?;
            let (restored, restore_heap) = measure(|| {
                frontend
                    .edit(edit(3, 3, true))
                    .map_err(|error| error.to_string())
            })?;
            output(json!({"kind": "edit", "state": "restored", "heap": restore_heap}))?;
            verify(restored.view(), false)?;
            verify(edited.view(), true)?;
            profile(restored.view(), "restored", &device)?;
            drop(restored);
            drop(edited);
            drop(generated);
            drop(frontend);
            run(
                &[(3, 3)],
                CurrentContents::Edited,
                "edited-current-only",
                &device,
            )?;
            run(
                &[(3, 3)],
                CurrentContents::Restored,
                "restored-current-only",
                &device,
            )?;
            for side in [8, 16] {
                let (metadata, catalog_heap) = measure(|| Ok(catalog(side)))?;
                output(
                    json!({"kind": "catalog", "side": side, "volumes": metadata.len(),
                    "heap": catalog_heap}),
                )?;
                let coordinates = (2..5)
                    .flat_map(|z| (2..5).map(move |x| (x, z)))
                    .collect::<Vec<_>>();
                run(
                    &coordinates,
                    CurrentContents::Edited,
                    &format!("matched-{side}x{side}-selection"),
                    &device,
                )?;
                drop(metadata);
            }
        }
        _ => return Err("use calibration or baseline".into()),
    }
    Ok(())
}

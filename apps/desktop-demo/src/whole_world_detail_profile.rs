//! PROTOTYPE (issue #137): per-volume cost of each Voxel Volume Detail Level.
//!
//! Throwaway calibration. For the generated, edited and restored fixture volume it builds each
//! detail level by direct-vote downsampling, publishes it as an ordinary coarse Voxel Volume,
//! then measures the summary, raster and Brickmap CPU bytes and actual Vulkan allocation sizes.
#[path = "streamed_fixture_allocations.rs"]
#[allow(dead_code)]
mod allocation;
#[allow(dead_code)]
mod streamed_fixture_recipe;
mod streamed_fixture_vulkan;
#[allow(dead_code)]
mod whole_world_detail_levels;

use allocation::{LIVE, PEAK};
use ash::vk;
use compute_ray_render_path::{ComputeRepresentation, ComputeSceneBundle};
use raster_render_path::derive_raster_regions;
use serde_json::{Value, json};
use std::{io::Write, sync::atomic::Ordering, time::Instant};
use streamed_fixture_recipe::{EDGE, edit, materials, scene};
use streamed_fixture_vulkan::Device;
use voxel_frontend::{
    SparseVoxelScene, VoxelExtent, VoxelFrontend, VoxelSceneId, VoxelSceneRevision, VoxelSceneView,
    VoxelValue,
};
use whole_world_detail_levels::{LEVEL_EDGES, coarse_volume, downsample};

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
    let live = LIVE.load(Ordering::SeqCst).saturating_sub(baseline);
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

fn profile_level(
    state: &str,
    edge: u32,
    view: &VoxelSceneView,
    summary: Value,
    device: &Device,
) -> Result<Value, String> {
    let region_edge = edge.min(16);
    let (raster, raster_heap) = measure(|| {
        derive_raster_regions(
            view,
            VoxelExtent::new(region_edge, region_edge, region_edge),
        )
        .map_err(|error| error.to_string())
    })?;
    let mut raster_vulkan = 0;
    let mut raster_buffers = 0;
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
            raster_vulkan += device.allocate_buffer(bytes, usage)?;
            raster_buffers += 1;
        }
    }
    let faces = raster.semantic_face_count();
    let raster_payload = raster.vertex_byte_size() + raster.index_byte_size();
    drop(raster);
    let (compute, brickmap_heap) = measure(|| {
        ComputeSceneBundle::from_view_with_representation(
            view,
            ComputeRepresentation::Brickmap {
                budget_bytes: 1 << 30,
            },
        )
        .map_err(|error| error.to_string())
    })?;
    let words = compute.storage_words();
    let brickmap_vulkan =
        device.allocate_buffer(words.len() * 4, vk::BufferUsageFlags::STORAGE_BUFFER)?;
    let record = json!({"kind": "level", "state": state, "edge": edge,
        "voxel_size": EDGE / edge, "summary": summary,
        "raster": {"heap": raster_heap, "faces": faces, "payload_bytes": raster_payload,
            "region_edge": region_edge, "vulkan_allocation_bytes": raster_vulkan,
            "buffer_count": raster_buffers},
        "brickmap": {"heap": brickmap_heap, "payload_bytes": words.len() * 4,
            "vulkan_allocation_bytes": brickmap_vulkan, "buffer_count": 1}});
    output(record.clone())?;
    Ok(record)
}

fn profile_state(state: &str, view: &VoxelSceneView, device: &Device) -> Result<(), String> {
    profile_level(state, EDGE, view, json!(null), device)?;
    let (levels, downsampling) = measure(|| downsample(view, 3, 3))?;
    output(json!({"kind": "downsampling", "state": state, "heap": downsampling}))?;
    for (&edge, values) in LEVEL_EDGES[1..].iter().zip(levels) {
        let occupied = values
            .iter()
            .filter(|value| matches!(value, VoxelValue::Occupied(_)))
            .count();
        let coarse_frontend = VoxelFrontend::new();
        let (coarse_view, publication) = measure(|| {
            coarse_frontend
                .publish_sparse(SparseVoxelScene::new(
                    VoxelSceneId::new("streamed-qualification-v1-coarse"),
                    VoxelSceneRevision::new(1),
                    materials(),
                    vec![coarse_volume(3, 3, edge, values)],
                ))
                .map_err(|error| error.to_string())
        })?;
        let record = profile_level(state, edge, &coarse_view, json!(null), device)?;
        let published_live = LIVE.load(Ordering::SeqCst);
        drop(record);
        drop(coarse_view);
        drop(coarse_frontend);
        // Publication consumes its input, so retained summary bytes are what dropping it frees.
        let retained = published_live.saturating_sub(LIVE.load(Ordering::SeqCst));
        output(json!({"kind": "summary", "state": state, "edge": edge,
            "publication": publication, "retained_bytes": retained,
            "occupied_voxels": occupied, "dense_code_bytes": edge * edge * edge}))?;
    }
    Ok(())
}

fn main() -> Result<(), String> {
    let device = Device::new()?;
    output(device.context())?;
    output(
        json!({"kind": "fixture", "recipe": "streamed-qualification-v1",
        "prototype": "issue-137 detail-level calibration"}),
    )?;
    let frontend = VoxelFrontend::new();
    let generated = frontend
        .publish_sparse(scene(&[(3, 3)]))
        .map_err(|error| error.to_string())?;
    profile_state("generated", &generated, &device)?;
    let edited = frontend
        .edit(edit(3, 3, false))
        .map_err(|error| error.to_string())?;
    profile_state("edited", edited.view(), &device)?;
    let restored = frontend
        .edit(edit(3, 3, true))
        .map_err(|error| error.to_string())?;
    profile_state("restored", restored.view(), &device)?;
    Ok(())
}

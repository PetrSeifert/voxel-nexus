use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Instant;
use voxel_frontend::*;

struct CountingAllocator;

static MEASURING: AtomicBool = AtomicBool::new(false);
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
static BYTES: AtomicUsize = AtomicUsize::new(0);

fn record_allocation(bytes: usize) {
    if MEASURING.load(Ordering::Relaxed) {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        BYTES.fetch_add(bytes, Ordering::Relaxed);
    }
}

// Forward the allocator contract unchanged to System; the counters do not
// allocate. This executable runs measurements on one thread.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            record_allocation(layout.size());
        }
        pointer
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() {
            record_allocation(layout.size());
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) };
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let pointer = unsafe { System.realloc(pointer, layout, size) };
        if !pointer.is_null() {
            record_allocation(size);
        }
        pointer
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!(
        "edge,volumes,retained_views,allocations_per_edit,allocated_bytes_per_edit,median_us,p95_us"
    );
    for edge in [32u32, 64, 128, 256] {
        for volume_count in [1, 2] {
            for retain_views in [false, true] {
                measure(edge, volume_count, retain_views)?;
            }
        }
    }
    Ok(())
}

fn measure(
    edge: u32,
    volume_count: usize,
    retain_views: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let extent = VoxelExtent::new(edge, edge, edge);
    let value_count = usize::try_from(edge)?.pow(3);
    let material = VoxelMaterialId::new("stone");
    let volumes = (0..volume_count)
        .map(|index| {
            DenseVoxelVolume::new(
                VoxelVolumeMetadata::new(
                    VoxelVolumeId::new(format!("volume-{index}")),
                    extent,
                    [0.0; 3],
                    1.0,
                ),
                vec![DenseVoxelBatch::new(
                    VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), extent),
                    vec![VoxelValue::Occupied(material.clone()); value_count],
                )],
            )
        })
        .collect();
    let frontend = VoxelFrontend::new();
    frontend.publish(DenseVoxelScene::new(
        VoxelSceneId::new("measurement"),
        VoxelSceneRevision::new(0),
        vec![VoxelMaterial::new(material, [1.0; 4])],
        volumes,
    ))?;
    let volume = VoxelVolumeId::new("volume-0");
    let mut retained = VecDeque::with_capacity(4);
    let mut durations = Vec::with_capacity(31);
    let mut allocations = 0;
    let mut bytes = 0;
    // Four untimed commands ensure every retained-view sample has a full
    // window, so allocation and latency comparisons use the same retention.
    for index in 0..35 {
        if retain_views {
            if retained.len() == 4 {
                retained.pop_front();
            }
            retained.push_back(frontend.scene_view()?);
        }
        let command = VoxelEditCommand::new(
            volume.clone(),
            VoxelCoordinate::new(
                i32::try_from(index % edge)?,
                i32::try_from(index / edge)?,
                i32::try_from(index % edge)?,
            ),
            VoxelValue::Empty,
        );
        ALLOCATIONS.store(0, Ordering::Relaxed);
        BYTES.store(0, Ordering::Relaxed);
        MEASURING.store(true, Ordering::Relaxed);
        let start = Instant::now();
        let result = frontend.edit(command);
        let elapsed = start.elapsed();
        MEASURING.store(false, Ordering::Relaxed);
        let outcome = result?;
        if outcome.change_set().is_none() {
            return Err("measurement edit did not change a value".into());
        }
        if index >= 4 {
            allocations += ALLOCATIONS.load(Ordering::Relaxed);
            bytes += BYTES.load(Ordering::Relaxed);
            durations.push(elapsed.as_secs_f64() * 1_000_000.0);
        }
    }
    durations.sort_by(f64::total_cmp);
    let median = durations.get(15).ok_or("missing median")?;
    let percentile = durations.get(29).ok_or("missing percentile")?;
    println!(
        "{edge},{volume_count},{},{},{},{median:.3},{percentile:.3}",
        if retain_views { 4 } else { 0 },
        allocations / 31,
        bytes / 31
    );
    Ok(())
}

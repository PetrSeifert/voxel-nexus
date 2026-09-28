use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use canonical_scene::{CanonicalSceneScale, generate_canonical_scene};
use voxel_frontend::VoxelFrontend;

struct CountingAllocator;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

// SAFETY: Every call forwards the caller's pointer and layout unchanged to System, and the
// counters never allocate.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            let live = LIVE.fetch_add(layout.size(), Ordering::SeqCst) + layout.size();
            PEAK.fetch_max(live, Ordering::SeqCst);
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Ordering::SeqCst);
        unsafe { System.dealloc(pointer, layout) };
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("scale,input_bytes,retained_scene_bytes,publication_peak_bytes");
    for scale in [
        CanonicalSceneScale::Small,
        CanonicalSceneScale::Medium,
        CanonicalSceneScale::Large,
    ] {
        let baseline = LIVE.load(Ordering::SeqCst);
        let scene = generate_canonical_scene(scale)?.into_scene();
        let input = LIVE.load(Ordering::SeqCst) - baseline;
        PEAK.store(LIVE.load(Ordering::SeqCst), Ordering::SeqCst);
        let frontend = VoxelFrontend::new();
        let view = frontend.publish(scene)?;
        let retained = LIVE.load(Ordering::SeqCst) - baseline;
        let peak = PEAK.load(Ordering::SeqCst) - baseline;
        println!("{},{input},{retained},{peak}", 64 * scale.factor());
        drop(view);
        drop(frontend);
    }
    Ok(())
}

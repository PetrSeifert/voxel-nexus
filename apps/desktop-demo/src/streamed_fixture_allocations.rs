use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Clone, Copy, Debug)]
#[repr(usize)]
pub enum Category {
    Control,
    Materialized,
    Generation,
    Raster,
    Brickmap,
    Metadata,
    History,
}

thread_local! { static CATEGORY: Cell<Category> = const { Cell::new(Category::Control) }; }
pub static LIVE: AtomicUsize = AtomicUsize::new(0);
pub static PEAK: AtomicUsize = AtomicUsize::new(0);
static CATEGORY_LIVE: [AtomicUsize; 7] = [const { AtomicUsize::new(0) }; 7];
static CATEGORY_PEAK: [AtomicUsize; 7] = [const { AtomicUsize::new(0) }; 7];
static COUNTS: [AtomicUsize; 7] = [const { AtomicUsize::new(0) }; 7];

pub fn live(category: Category) -> usize {
    CATEGORY_LIVE[category as usize].load(Ordering::SeqCst)
}
pub fn peak(category: Category) -> usize {
    CATEGORY_PEAK[category as usize].load(Ordering::SeqCst)
}
pub fn count(category: Category) -> usize {
    COUNTS[category as usize].load(Ordering::SeqCst)
}
pub fn reset_peaks() {
    PEAK.store(LIVE.load(Ordering::SeqCst), Ordering::SeqCst);
    for (current, peak) in CATEGORY_LIVE.iter().zip(&CATEGORY_PEAK) {
        peak.store(current.load(Ordering::SeqCst), Ordering::SeqCst);
    }
}

pub fn within<T>(category: Category, operation: impl FnOnce() -> T) -> T {
    struct Restore(Category);
    impl Drop for Restore {
        fn drop(&mut self) {
            CATEGORY.with(|category| category.set(self.0));
        }
    }
    let restore = Restore(CATEGORY.with(|current| current.replace(category)));
    let result = operation();
    drop(restore);
    result
}

struct MeasuringAllocator;
#[repr(C)]
struct Header {
    category: usize,
}

fn extended(layout: Layout) -> Option<(Layout, usize)> {
    Layout::new::<Header>()
        .extend(layout)
        .ok()
        .map(|(layout, offset)| (layout.pad_to_align(), offset))
}

// SAFETY: Each System allocation includes an aligned header before the caller's aligned region.
// Deallocation reconstructs that exact layout and pointer; accounting never allocates.
unsafe impl GlobalAlloc for MeasuringAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let Some((system_layout, offset)) = extended(layout) else {
            return std::ptr::null_mut();
        };
        // SAFETY: extended validates the combined header/payload layout.
        let base = unsafe { System.alloc(system_layout) };
        if base.is_null() {
            return base;
        }
        let category = CATEGORY.try_with(Cell::get).unwrap_or(Category::Control) as usize;
        // SAFETY: base is aligned for Header; offset identifies the caller's region inside this allocation.
        unsafe {
            base.cast::<Header>().write(Header { category });
        }
        let bytes = system_layout.size();
        let total = LIVE.fetch_add(bytes, Ordering::SeqCst) + bytes;
        PEAK.fetch_max(total, Ordering::SeqCst);
        let category_live = CATEGORY_LIVE
            .get(category)
            .expect("the allocator stores a valid category")
            .fetch_add(bytes, Ordering::SeqCst)
            + bytes;
        CATEGORY_PEAK
            .get(category)
            .expect("the allocator stores a valid category")
            .fetch_max(category_live, Ordering::SeqCst);
        COUNTS
            .get(category)
            .expect("the allocator stores a valid category")
            .fetch_add(1, Ordering::SeqCst);
        // SAFETY: offset is the payload offset returned by Layout::extend.
        unsafe { base.add(offset) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        let (system_layout, offset) =
            extended(layout).expect("a live allocation had a valid combined layout");
        // SAFETY: pointer came from alloc with this layout, so subtracting offset recovers its live header.
        let base = unsafe { pointer.sub(offset) };
        // SAFETY: the allocator initialized this header and the allocation remains live.
        let category = unsafe { base.cast::<Header>().read() }.category;
        LIVE.fetch_sub(system_layout.size(), Ordering::SeqCst);
        CATEGORY_LIVE
            .get(category)
            .expect("the live header contains its allocation category")
            .fetch_sub(system_layout.size(), Ordering::SeqCst);
        COUNTS
            .get(category)
            .expect("the live header contains its allocation category")
            .fetch_sub(1, Ordering::SeqCst);
        // SAFETY: base and system_layout exactly match the System allocation.
        unsafe { System.dealloc(base, system_layout) };
    }
}

#[global_allocator]
static ALLOCATOR: MeasuringAllocator = MeasuringAllocator;

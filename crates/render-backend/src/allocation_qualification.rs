use ash::vk::{self, Handle};
use std::{
    cell::Cell,
    collections::HashMap,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};
use thiserror::Error;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GpuAllocationClass {
    Fixed,
    Raster,
    Brickmap,
}
#[derive(Clone, Copy)]
struct Owner {
    identity: u64,
    class: GpuAllocationClass,
    scene_buffer_bytes: u64,
}
thread_local! {
    static OWNER: Cell<Owner> = const { Cell::new(Owner { identity: 0, class: GpuAllocationClass::Fixed, scene_buffer_bytes: 0 }) };
    static NEXT: Cell<GpuAllocationClass> = const { Cell::new(GpuAllocationClass::Fixed) };
}
static ENABLED: AtomicBool = AtomicBool::new(false);
static INVALID: AtomicBool = AtomicBool::new(false);
static LEDGER: OnceLock<Mutex<Ledger>> = OnceLock::new();

#[derive(Clone, Debug, Default)]
pub struct GpuAllocationSnapshot {
    pub live_bytes: [u64; 3],
    pub peak_bytes: [u64; 3],
    pub live_allocations: [usize; 3],
    pub peak_allocations: [usize; 3],
    pub total_peak_bytes: u64,
    pub allocations: Vec<(u64, u64, GpuAllocationClass, u64)>,
}
struct Ledger {
    allocations: HashMap<u64, (u64, GpuAllocationClass, u64)>,
    snapshot: GpuAllocationSnapshot,
}
impl Default for Ledger {
    fn default() -> Self {
        // Reserve qualification bookkeeping before any renderer allocation scope is entered.
        Self {
            allocations: HashMap::with_capacity(65_536),
            snapshot: GpuAllocationSnapshot::default(),
        }
    }
}
#[derive(Debug, Error)]
pub enum GpuAllocationQualificationError {
    #[error("GPU allocation qualification is already active")]
    AlreadyActive,
    #[error("GPU allocation qualification ledger is unavailable or incomplete")]
    Unavailable,
}
pub struct GpuAllocationQualification;
impl GpuAllocationQualification {
    pub fn start() -> Result<Self, GpuAllocationQualificationError> {
        if ENABLED.swap(true, Ordering::SeqCst) {
            return Err(GpuAllocationQualificationError::AlreadyActive);
        }
        let mut ledger = LEDGER
            .get_or_init(|| Mutex::new(Ledger::default()))
            .lock()
            .map_err(|_| {
                ENABLED.store(false, Ordering::SeqCst);
                GpuAllocationQualificationError::Unavailable
            })?;
        *ledger = Ledger::default();
        INVALID.store(false, Ordering::SeqCst);
        Ok(Self)
    }
    pub fn snapshot(&self) -> Result<GpuAllocationSnapshot, GpuAllocationQualificationError> {
        if INVALID.load(Ordering::SeqCst) {
            return Err(GpuAllocationQualificationError::Unavailable);
        }
        let ledger = LEDGER
            .get_or_init(|| Mutex::new(Ledger::default()))
            .lock()
            .map_err(|_| GpuAllocationQualificationError::Unavailable)?;
        let mut snapshot = ledger.snapshot.clone();
        snapshot.allocations = ledger
            .allocations
            .iter()
            .map(|(&memory, &(owner, class, bytes))| (memory, owner, class, bytes))
            .collect();
        snapshot.allocations.sort_by_key(|allocation| allocation.0);
        Ok(snapshot)
    }
}
impl Drop for GpuAllocationQualification {
    fn drop(&mut self) {
        ENABLED.store(false, Ordering::SeqCst);
        if let Some(ledger) = LEDGER.get() {
            match ledger.lock() {
                Ok(mut ledger) => {
                    ledger.allocations.clear();
                    ledger.allocations.shrink_to_fit();
                    ledger.snapshot = GpuAllocationSnapshot::default();
                }
                Err(_) => INVALID.store(true, Ordering::SeqCst),
            }
        }
    }
}

pub fn with_gpu_allocation_owner<T>(
    identity: u64,
    class: GpuAllocationClass,
    scene_buffer_bytes: u64,
    operation: impl FnOnce() -> T,
) -> T {
    struct Restore(Owner);
    impl Drop for Restore {
        fn drop(&mut self) {
            OWNER.with(|owner| owner.set(self.0));
        }
    }
    let restore = Restore(OWNER.with(|owner| {
        owner.replace(Owner {
            identity,
            class,
            scene_buffer_bytes,
        })
    }));
    let result = operation();
    drop(restore);
    result
}
pub(super) fn buffer(bytes: u64) {
    if ENABLED.load(Ordering::Relaxed) {
        let owner = OWNER.with(Cell::get);
        NEXT.with(|next| {
            next.set(match owner.class {
                GpuAllocationClass::Raster => GpuAllocationClass::Raster,
                GpuAllocationClass::Brickmap if bytes == owner.scene_buffer_bytes => {
                    GpuAllocationClass::Brickmap
                }
                _ => GpuAllocationClass::Fixed,
            })
        });
    }
}
pub(super) fn image() {
    if ENABLED.load(Ordering::Relaxed) {
        NEXT.with(|next| next.set(GpuAllocationClass::Fixed));
    }
}
fn class_index(class: GpuAllocationClass) -> usize {
    match class {
        GpuAllocationClass::Fixed => 0,
        GpuAllocationClass::Raster => 1,
        GpuAllocationClass::Brickmap => 2,
    }
}
pub(super) fn allocated(memory: vk::DeviceMemory, bytes: u64) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    match LEDGER.get_or_init(|| Mutex::new(Ledger::default())).lock() {
        Ok(mut ledger) => {
            if ledger.allocations.len() >= 65_536 {
                INVALID.store(true, Ordering::SeqCst);
                return;
            }
            let owner = OWNER.with(Cell::get);
            let class = NEXT.with(Cell::get);
            if ledger
                .allocations
                .insert(memory.as_raw(), (owner.identity, class, bytes))
                .is_some()
            {
                INVALID.store(true, Ordering::SeqCst);
            }
            let index = class_index(class);
            ledger.snapshot.live_bytes[index] += bytes;
            ledger.snapshot.live_allocations[index] += 1;
            ledger.snapshot.peak_bytes[index] =
                ledger.snapshot.peak_bytes[index].max(ledger.snapshot.live_bytes[index]);
            ledger.snapshot.peak_allocations[index] = ledger.snapshot.peak_allocations[index]
                .max(ledger.snapshot.live_allocations[index]);
            ledger.snapshot.total_peak_bytes = ledger
                .snapshot
                .total_peak_bytes
                .max(ledger.snapshot.live_bytes.iter().sum());
        }
        Err(_) => INVALID.store(true, Ordering::SeqCst),
    }
}
pub(super) fn freed(memory: vk::DeviceMemory) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    match LEDGER.get_or_init(|| Mutex::new(Ledger::default())).lock() {
        Ok(mut ledger) => match ledger.allocations.remove(&memory.as_raw()) {
            Some((_, class, bytes)) => {
                let index = class_index(class);
                ledger.snapshot.live_bytes[index] -= bytes;
                ledger.snapshot.live_allocations[index] -= 1;
            }
            None => INVALID.store(true, Ordering::SeqCst),
        },
        Err(_) => INVALID.store(true, Ordering::SeqCst),
    }
}

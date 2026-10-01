use ash::vk;
use std::collections::BTreeMap;
use std::sync::{Mutex, PoisonError};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RenderPathGpuMemory {
    pub live_bytes: u64,
    pub peak_bytes: u64,
}

struct Ledger {
    allocations: BTreeMap<vk::DeviceMemory, u64>,
    usage: RenderPathGpuMemory,
}

/// Counts device memory that Render Paths allocate through their device context, including
/// a replacement and the Presenting Render Path while both are alive during a switch.
pub(super) struct GpuMemoryLedger(Mutex<Ledger>);

impl GpuMemoryLedger {
    pub(super) const fn new() -> Self {
        Self(Mutex::new(Ledger {
            allocations: BTreeMap::new(),
            usage: RenderPathGpuMemory {
                live_bytes: 0,
                peak_bytes: 0,
            },
        }))
    }

    pub(super) fn allocated(&self, memory: vk::DeviceMemory, bytes: u64) {
        // The ledger only holds counters, so a panic elsewhere cannot leave it inconsistent.
        let mut ledger = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        ledger.allocations.insert(memory, bytes);
        ledger.usage.live_bytes = ledger.usage.live_bytes.saturating_add(bytes);
        ledger.usage.peak_bytes = ledger.usage.peak_bytes.max(ledger.usage.live_bytes);
    }

    pub(super) fn freed(&self, memory: vk::DeviceMemory) {
        let mut ledger = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(bytes) = ledger.allocations.remove(&memory) {
            ledger.usage.live_bytes = ledger.usage.live_bytes.saturating_sub(bytes);
        }
    }

    pub(super) fn usage(&self) -> RenderPathGpuMemory {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).usage
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ash::vk::Handle;

    #[test]
    fn live_bytes_follow_allocations_and_peak_holds_the_maximum() {
        let ledger = GpuMemoryLedger::new();
        let first = vk::DeviceMemory::from_raw(1);
        let second = vk::DeviceMemory::from_raw(2);
        ledger.allocated(first, 100);
        ledger.allocated(second, 50);
        ledger.freed(first);
        ledger.freed(vk::DeviceMemory::from_raw(3));
        assert_eq!(
            ledger.usage(),
            RenderPathGpuMemory {
                live_bytes: 50,
                peak_bytes: 150,
            }
        );
    }
}

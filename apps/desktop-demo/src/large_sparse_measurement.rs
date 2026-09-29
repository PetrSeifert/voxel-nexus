#[cfg(windows)]
#[path = "../../../crates/canonical-scene/examples/support/allocation.rs"]
mod allocation;
#[cfg(windows)]
mod large_sparse_probes;
#[cfg(windows)]
mod large_sparse_runner;
#[cfg(windows)]
#[allow(dead_code)]
mod windows_adapter;

fn main() -> Result<(), String> {
    #[cfg(windows)]
    {
        large_sparse_runner::main()
    }
    #[cfg(not(windows))]
    {
        Err("large sparse measurement requires Windows Vulkan".into())
    }
}

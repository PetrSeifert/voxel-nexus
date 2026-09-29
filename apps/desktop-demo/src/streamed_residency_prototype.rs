#[path = "streamed_fixture_allocations.rs"]
mod allocation;
mod streamed_fixture_recipe;
mod streamed_residency_cpu;
mod streamed_residency_paths;
mod streamed_residency_route;
mod streamed_residency_source;

#[cfg(windows)]
#[allow(dead_code)]
mod windows_adapter;

#[cfg(windows)]
mod streamed_residency_runner;

fn main() -> Result<(), String> {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    if arguments
        .first()
        .is_some_and(|mode| mode.starts_with("cpu-"))
    {
        return streamed_residency_cpu::main(&arguments);
    }
    if !arguments.iter().any(|argument| argument == "--allow-gpu") {
        return Err("GPU qualification previously froze the development machine. GPU modes require --allow-gpu on a dedicated test machine; CPU modes remain available.".into());
    }
    #[cfg(windows)]
    {
        streamed_residency_runner::main()
    }
    #[cfg(not(windows))]
    {
        Err("streamed residency qualification requires Windows presentation".into())
    }
}

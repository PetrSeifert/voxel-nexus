#[path = "streamed_fixture_allocations.rs"]
mod allocation;
mod streamed_fixture_recipe;
mod streamed_qualification;
#[cfg(windows)]
mod streamed_qualification_oracle;
// Preserve the frozen route's type path while supplying coordinates from production selections.
use streamed_qualification as streamed_residency_source;
#[allow(dead_code)]
mod streamed_neighbourhood;
#[cfg(windows)]
mod streamed_qualification_observation;
#[cfg(windows)]
mod streamed_residency_probes;
#[cfg_attr(not(windows), allow(dead_code))]
mod streamed_residency_route;
#[cfg(windows)]
mod streamed_residency_runner;
#[allow(dead_code)]
mod streamed_world;
#[cfg(windows)]
#[allow(dead_code)]
mod windows_adapter;

fn main() -> Result<(), String> {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    if arguments
        .first()
        .is_some_and(|mode| mode.starts_with("cpu-"))
    {
        return streamed_qualification::run_cpu(&arguments);
    }
    if !arguments.iter().any(|argument| argument == "--allow-gpu") {
        return Err("GPU qualification requires --allow-gpu".into());
    }
    #[cfg(windows)]
    {
        streamed_residency_runner::main()
    }
    #[cfg(not(windows))]
    {
        Err("GPU qualification requires Windows presentation".into())
    }
}

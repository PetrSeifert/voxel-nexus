use portable_compute_ray_evidence::{read_manifest, verify_bundle};
use std::path::PathBuf;
use std::process::ExitCode;

fn run() -> Result<(), String> {
    let mut arguments = std::env::args().skip(1);
    let bundle_root = PathBuf::from(
        arguments
            .next()
            .ok_or_else(|| "missing portable compute-ray evidence directory".to_owned())?,
    );
    if arguments.next().is_some() {
        return Err("unexpected portable compute-ray evidence verifier argument".to_owned());
    }
    let manifest =
        read_manifest(&bundle_root.join("manifest.json")).map_err(|error| error.to_string())?;
    let summary = verify_bundle(&bundle_root, &manifest).map_err(|error| error.to_string())?;
    println!(
        "portable compute-ray evidence verified: artifacts={} selected_frames={} completed_switches={}",
        summary.artifacts, summary.selected_frames, summary.completed_switches
    );
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

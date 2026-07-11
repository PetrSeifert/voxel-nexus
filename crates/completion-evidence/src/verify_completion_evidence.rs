use completion_evidence::{read_manifest, verify_bundle};
use std::path::PathBuf;
use std::process::ExitCode;

fn run() -> Result<(), String> {
    let mut arguments = std::env::args().skip(1);
    let bundle_root = arguments
        .next()
        .map(PathBuf::from)
        .ok_or_else(|| "missing completion bundle directory".to_owned())?;
    if arguments.next().is_some() {
        return Err("unexpected completion evidence verifier argument".to_owned());
    }
    let manifest =
        read_manifest(&bundle_root.join("manifest.json")).map_err(|error| error.to_string())?;
    let summary = verify_bundle(&bundle_root, &manifest).map_err(|error| error.to_string())?;
    let output = serde_json::to_string_pretty(&summary)
        .map_err(|error| format!("could not serialize verification summary: {error}"))?;
    println!("{output}");
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

use measurement_evidence::{RenderPathEvidenceInput, retain_render_path_evidence};
use std::fs;
use std::process::ExitCode;

fn run() -> Result<(), String> {
    let mut arguments = std::env::args().skip(1);
    let input_path = arguments
        .next()
        .ok_or_else(|| "missing Render Path evidence input path".to_owned())?;
    let output_path = arguments
        .next()
        .ok_or_else(|| "missing Render Path evidence report path".to_owned())?;
    if arguments.next().is_some() {
        return Err("unexpected Render Path evidence report argument".to_owned());
    }
    let input = fs::read_to_string(&input_path).map_err(|error| {
        format!("could not read Render Path evidence input {input_path}: {error}")
    })?;
    let input = serde_json::from_str::<RenderPathEvidenceInput>(&input).map_err(|error| {
        format!("could not parse Render Path evidence input {input_path}: {error}")
    })?;
    let report = retain_render_path_evidence(input).map_err(|error| error.to_string())?;
    let output = serde_json::to_string_pretty(&report)
        .map_err(|error| format!("could not serialize Render Path evidence report: {error}"))?;
    fs::write(&output_path, output).map_err(|error| {
        format!("could not write Render Path evidence report {output_path}: {error}")
    })
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

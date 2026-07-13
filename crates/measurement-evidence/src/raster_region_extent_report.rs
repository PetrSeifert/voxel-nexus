use measurement_evidence::{ExtentSelectionInput, select_raster_region_extent};
use std::fs;
use std::process::ExitCode;

fn run() -> Result<(), String> {
    let mut arguments = std::env::args().skip(1);
    let input_path = arguments
        .next()
        .ok_or_else(|| "missing Raster Region extent input path".to_owned())?;
    let output_path = arguments
        .next()
        .ok_or_else(|| "missing Raster Region extent output path".to_owned())?;
    if arguments.next().is_some() {
        return Err("unexpected Raster Region extent report argument".to_owned());
    }
    let input = fs::read_to_string(&input_path)
        .map_err(|error| format!("could not read extent input {input_path}: {error}"))?;
    let input = serde_json::from_str::<ExtentSelectionInput>(&input)
        .map_err(|error| format!("could not parse extent input {input_path}: {error}"))?;
    let report = select_raster_region_extent(input).map_err(|error| error.to_string())?;
    let output = serde_json::to_string_pretty(&report)
        .map_err(|error| format!("could not serialize extent report: {error}"))?;
    fs::write(&output_path, output)
        .map_err(|error| format!("could not write extent report {output_path}: {error}"))
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

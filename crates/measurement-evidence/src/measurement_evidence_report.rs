use measurement_evidence::{ScaleAggregationInput, aggregate_scales};
use std::fs;
use std::process::ExitCode;

fn run() -> Result<(), String> {
    let mut arguments = std::env::args().skip(1);
    let input_path = arguments
        .next()
        .ok_or_else(|| "missing aggregation input path".to_owned())?;
    let output_path = arguments
        .next()
        .ok_or_else(|| "missing aggregation output path".to_owned())?;
    if arguments.next().is_some() {
        return Err("unexpected aggregation report argument".to_owned());
    }
    let input = fs::read_to_string(&input_path)
        .map_err(|error| format!("could not read aggregation input {input_path}: {error}"))?;
    let scales = serde_json::from_str::<Vec<ScaleAggregationInput>>(&input)
        .map_err(|error| format!("could not parse aggregation input {input_path}: {error}"))?;
    let report = aggregate_scales(scales).map_err(|error| error.to_string())?;
    let output = serde_json::to_string_pretty(&report)
        .map_err(|error| format!("could not serialize aggregation report: {error}"))?;
    fs::write(&output_path, output)
        .map_err(|error| format!("could not write aggregation report {output_path}: {error}"))
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

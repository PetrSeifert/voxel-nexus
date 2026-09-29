use measurement_evidence::large_sparse::{LargeSparseRecord, validate};
use std::io::BufRead;

fn main() -> Result<(), String> {
    let runs = std::env::args()
        .skip(1)
        .map(|path| {
            let file = std::fs::File::open(&path).map_err(|error| format!("{path}: {error}"))?;
            std::io::BufReader::new(file)
                .lines()
                .enumerate()
                .map(|(index, line)| {
                    let line = line.map_err(|error| error.to_string())?;
                    serde_json::from_str::<LargeSparseRecord>(&line)
                        .map_err(|error| format!("{path}:{}: {error}", index + 1))
                })
                .collect::<Result<Vec<_>, String>>()
        })
        .collect::<Result<Vec<_>, _>>()?;
    let report = validate(&runs).map_err(|error| error.to_string())?;
    println!(
        "{}",
        serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?
    );
    Ok(())
}

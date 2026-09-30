#![cfg(feature = "qualification")]

#[test]
fn cpu_lifecycle_uses_production_residency_and_preserves_frozen_edits()
-> Result<(), Box<dyn std::error::Error>> {
    let directory =
        std::env::temp_dir().join(format!("streamed-qualification-{}", std::process::id()));
    std::fs::create_dir_all(&directory)?;
    let path = directory.join("lifecycle.jsonl");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_streamed-residency-qualification"))
        .args([
            "cpu-lifecycle",
            path.to_str().ok_or("invalid temporary path")?,
        ])
        .output()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let records = std::fs::read_to_string(&path)?
        .lines()
        .map(serde_json::from_str::<serde_json::Value>)
        .collect::<Result<Vec<_>, _>>()?;
    let result = records
        .iter()
        .find(|record| record["kind"] == "cpu-lifecycle-result")
        .ok_or("missing lifecycle result")?;
    for field in [
        "evicted_edit",
        "historical_reads",
        "restored",
        "unrelated_edit_reuse",
        "compacted",
        "nineteen_copy_admission",
    ] {
        assert_eq!(result[field], true, "{field}");
    }
    let queries: Vec<_> = records
        .iter()
        .filter(|record| record["kind"] == "fingerprint")
        .collect();
    assert!(
        queries.iter().any(
            |record| record["phase"] == "edited" && record["fingerprint"] == "478ee4a9737e1ad7"
        )
    );
    assert!(
        queries
            .iter()
            .any(|record| record["phase"] == "restored"
                && record["fingerprint"] == "9660d9308d69ede5")
    );
    assert!(
        records
            .iter()
            .filter(|record| record["kind"] == "cpu-residency")
            .all(|record| record["peak_copies"]
                .as_u64()
                .is_some_and(|copies| copies <= 19))
    );
    std::fs::remove_file(path)?;
    std::fs::remove_dir(directory)?;
    Ok(())
}

#[test]
fn gpu_qualification_requires_explicit_opt_in() -> Result<(), Box<dyn std::error::Error>> {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_streamed-residency-qualification"))
        .args(["raster", "unused.jsonl"])
        .output()?;
    assert!(!output.status.success());
    assert!(String::from_utf8(output.stderr)?.contains("--allow-gpu"));
    Ok(())
}

#[cfg(windows)]
#[test]
fn verifier_rejects_incomplete_and_out_of_cap_evidence_without_graphics()
-> Result<(), Box<dyn std::error::Error>> {
    let workspace = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let directory = std::env::temp_dir().join(format!("streamed-verifier-{}", std::process::id()));
    let output = std::process::Command::new("pwsh")
        .current_dir(workspace)
        .args([
            "-NoProfile",
            "-File",
            "scripts/test-streamed-residency-verifier.ps1",
            "-EvidenceDirectory",
        ])
        .arg(directory)
        .output()?;
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

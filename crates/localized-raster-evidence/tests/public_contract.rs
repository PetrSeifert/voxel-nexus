use localized_raster_evidence::{
    ArtifactCategory, ArtifactRecord, BarrierOutcomes, BundleManifest, CommandInput,
    EnvironmentContext, LifecycleOutcomes, RepositoryContext, RequiredProcessOutcome,
    RevisionOutcomes, SceneInput, SelectionContext, SummaryOutcomes, verify_hash_inventory,
    verify_manifest_contract,
};
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

fn valid_manifest() -> BundleManifest {
    BundleManifest {
        schema_version: 1,
        scope: "Descriptive retained evidence for this recorded Windows development machine only."
            .to_owned(),
        environment: EnvironmentContext {
            operating_system: "Microsoft Windows 11 Pro 10.0.26200".to_owned(),
            processor: "AMD Ryzen 5 7600 6-Core Processor".to_owned(),
            graphics_device: "NVIDIA GeForce RTX 4070".to_owned(),
            graphics_driver: "32.0.15.9597".to_owned(),
            rustc: "rustc 1.99.0-nightly".to_owned(),
            cargo: "cargo 1.99.0-nightly".to_owned(),
            powershell: "7.5.8".to_owned(),
        },
        repository: RepositoryContext {
            remote: "https://github.com/PetrSeifert/voxel-nexus.git".to_owned(),
            assembly_revision: "f5d3a93dcfbb81f4719e2afc04be73df63bcf225".to_owned(),
            demo_revision: "c3e23e7e142179b0c6745aac1ec8a638dc3509f2".to_owned(),
            extent_revision: "8aaf8ae95bce56b8e28425adef79c927482b8871".to_owned(),
            scale_revision: "f5d3a93dcfbb81f4719e2afc04be73df63bcf225".to_owned(),
        },
        scene: SceneInput {
            name: "canonical-dense-scene".to_owned(),
            generator: "voxel-nexus-canonical-dense".to_owned(),
            generator_version: 1,
            scale: 256,
            dimensions: [256, 128, 256],
            camera: "overview".to_owned(),
        },
        commands: vec![
            CommandInput::new(1, [0, 0, 0], 2),
            CommandInput::new(2, [40, 0, 0], 3),
            CommandInput::new(3, [80, 0, 0], 4),
        ],
        revisions: RevisionOutcomes {
            initial: 1,
            installed: 1,
            expected_final: 4,
            required_final: 4,
            visible_final: 4,
            intermediate_revision_visible: false,
        },
        selection: SelectionContext {
            candidates: [16, 32, 64],
            selected_extent: [16, 16, 16],
            input_path: "extent-selection/selection-input.json".to_owned(),
            report_path: "extent-selection/selection.json".to_owned(),
        },
        barriers: BarrierOutcomes {
            obsolete_cpu_revision: 2,
            obsolete_cpu_cancelled: true,
            superseded_post_upload_revision: 3,
            superseded_post_upload_rejected: true,
        },
        outcomes: SummaryOutcomes {
            semantic_correctness: true,
            localization: true,
            failure_retry: true,
            lifecycle: LifecycleOutcomes {
                passed: true,
                active_cpu_shutdown_passed: true,
                hidden_candidate_shutdown_passed: true,
                owned_resources_after_shutdown: 0,
            },
            validation_warnings: 0,
            validation_errors: 0,
        },
        reproduction_commands: vec![
            "pwsh -NoProfile -File scripts/verify-edit-burst-demo.ps1 -EvidenceDirectory artifacts/edit-burst-issue-46".to_owned(),
            "pwsh -NoProfile -File scripts/qualify-raster-region-extents.ps1 -EvidenceDirectory artifacts/raster-region-extent-selection-issue-47".to_owned(),
            "pwsh -NoProfile -File scripts/characterize-raster-region-scales.ps1 -EvidenceDirectory artifacts/raster-region-scale-characterization-issue-48 -SelectionManifest artifacts/raster-region-extent-selection-issue-47/manifest.json".to_owned(),
            "pwsh -NoProfile -File scripts/verify-localized-raster-source-processes.ps1".to_owned(),
            "pwsh -NoProfile -File scripts/assemble-localized-raster-evidence.ps1".to_owned(),
        ],
        verification_command: "cargo run --locked --package localized-raster-evidence --bin verify-localized-raster-evidence -- docs/evidence/localized-editable-raster/v1/development-machine".to_owned(),
        required_processes: required_processes(),
        artifacts: required_artifacts(),
    }
}

fn artifact(category: ArtifactCategory, path: &str) -> ArtifactRecord {
    ArtifactRecord {
        category,
        path: path.to_owned(),
        sha256: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".to_owned(),
        bytes: 0,
        process_exit_code: Some(0),
        process_outcome_source: Some("demo/manifest.json#/ProcessExitCode".to_owned()),
    }
}

fn required_artifacts() -> Vec<ArtifactRecord> {
    let mut artifacts = vec![
        artifact(ArtifactCategory::UninterruptedDemo, "demo/manifest.json"),
        artifact(
            ArtifactCategory::RepresentativeFrame,
            "demo/final-visible.png",
        ),
        artifact(
            ArtifactCategory::SemanticLocalizationReport,
            "extent-selection/selection.json",
        ),
        artifact(
            ArtifactCategory::OrchestrationTimeline,
            "demo/desktop-demo.stdout.log",
        ),
        artifact(
            ArtifactCategory::FailureShutdownLog,
            "extent-selection/extent-16/qualification/active-cpu-close.stdout.log",
        ),
        artifact(
            ArtifactCategory::RawMeasurement,
            "scale-characterization/raw-distributions.json",
        ),
        artifact(ArtifactCategory::ComparisonChart, "comparison.svg"),
        artifact(
            ArtifactCategory::ValidationOutput,
            "demo/desktop-demo.stderr.log",
        ),
        artifact(ArtifactCategory::ReproductionInstructions, "README.md"),
        artifact(
            ArtifactCategory::SupportingEvidence,
            "extent-selection/manifest.json",
        ),
        artifact(
            ArtifactCategory::SupportingEvidence,
            "scale-characterization/manifest.json",
        ),
    ];
    for index in 0..3 {
        artifacts.push(artifact(
            ArtifactCategory::SupportingEvidence,
            &format!("demo/supporting-{index}.log"),
        ));
    }
    for index in 0..81 {
        artifacts.push(artifact(
            ArtifactCategory::SupportingEvidence,
            &format!("extent-selection/supporting-{index}.log"),
        ));
    }
    for index in 0..46 {
        artifacts.push(artifact(
            ArtifactCategory::SupportingEvidence,
            &format!("scale-characterization/supporting-{index}.log"),
        ));
    }
    for index in 0..5 {
        artifacts.push(artifact(
            ArtifactCategory::SupportingEvidence,
            &format!("process-verification/artifact-{index}.log"),
        ));
    }
    artifacts
}

fn required_processes() -> Vec<RequiredProcessOutcome> {
    [
        "desktop_build",
        "raster_qualification_tests",
        "extent_selection",
        "selection_comparison",
    ]
    .into_iter()
    .map(|name| RequiredProcessOutcome {
        name: name.to_owned(),
        command: "checked command".to_owned(),
        exit_code: 0,
        evidence: "evidence.log#success".to_owned(),
    })
    .collect()
}

#[test]
fn contract_accepts_the_complete_evidence_summary() -> Result<(), Box<dyn std::error::Error>> {
    let summary = verify_manifest_contract(&valid_manifest())?;

    assert_eq!(summary.artifacts, 146);
    assert_eq!(summary.selected_extent, [16, 16, 16]);
    Ok(())
}

#[test]
fn contract_rejects_a_missing_required_artifact_category() {
    let mut manifest = valid_manifest();
    manifest
        .artifacts
        .retain(|artifact| artifact.category != ArtifactCategory::ComparisonChart);

    let error = verify_manifest_contract(&manifest).expect_err("missing chart must be rejected");

    assert!(error.to_string().contains("ComparisonChart"));
}

#[test]
fn contract_rejects_inconsistent_command_revisions() {
    let mut manifest = valid_manifest();
    manifest.commands[1].published_revision = 4;

    let error = verify_manifest_contract(&manifest).expect_err("revision gap must be rejected");

    assert!(error.to_string().contains("command"));
}

#[test]
fn contract_rejects_failed_semantic_or_localization_summary() {
    let mut manifest = valid_manifest();
    manifest.outcomes.localization = false;

    let error =
        verify_manifest_contract(&manifest).expect_err("failed localization must be rejected");

    assert!(error.to_string().contains("localization"));
}

#[test]
fn contract_rejects_unbalanced_resources() {
    let mut manifest = valid_manifest();
    manifest.outcomes.lifecycle.owned_resources_after_shutdown = 1;

    let error = verify_manifest_contract(&manifest).expect_err("owned resource must be rejected");

    assert!(error.to_string().contains("resource"));
}

#[test]
fn contract_rejects_validation_findings() {
    let mut manifest = valid_manifest();
    manifest.outcomes.validation_warnings = 1;

    let error =
        verify_manifest_contract(&manifest).expect_err("validation warning must be rejected");

    assert!(error.to_string().contains("validation"));
}

#[test]
fn contract_rejects_unsuccessful_artifact_process() {
    let mut manifest = valid_manifest();
    manifest.artifacts[0].process_exit_code = Some(7);

    let error = verify_manifest_contract(&manifest).expect_err("failed process must be rejected");

    assert!(error.to_string().contains("exit"));
}

#[test]
fn contract_accepts_not_applicable_process_metadata_for_static_artifacts()
-> Result<(), Box<dyn std::error::Error>> {
    let mut manifest = valid_manifest();
    let chart = manifest
        .artifacts
        .iter_mut()
        .find(|artifact| artifact.category == ArtifactCategory::ComparisonChart)
        .ok_or("missing comparison chart")?;
    chart.process_exit_code = None;
    chart.process_outcome_source = None;

    verify_manifest_contract(&manifest)?;
    Ok(())
}

#[test]
fn contract_rejects_an_unsuccessful_required_process() {
    let mut manifest = valid_manifest();
    manifest.required_processes[1].exit_code = 1;

    let error =
        verify_manifest_contract(&manifest).expect_err("failed required process must be rejected");

    assert!(error.to_string().contains("raster_qualification_tests"));
}

#[test]
fn contract_rejects_an_inconsistent_reproduction_command() {
    let mut manifest = valid_manifest();
    manifest.reproduction_commands[0] = "cargo test".to_owned();

    let error = verify_manifest_contract(&manifest)
        .expect_err("inconsistent reproduction command must be rejected");

    assert!(error.to_string().contains("commands"));
}

#[test]
fn hash_inventory_rejects_a_missing_artifact() -> Result<(), Box<dyn std::error::Error>> {
    let root = temporary_directory("missing")?;
    let record = artifact(ArtifactCategory::ValidationOutput, "missing.log");

    let error = verify_hash_inventory(&root, &[record]).expect_err("missing file must be rejected");

    assert!(error.to_string().contains("missing.log"));
    fs::remove_dir_all(root)?;
    Ok(())
}

#[test]
fn hash_inventory_rejects_a_checksum_mismatch() -> Result<(), Box<dyn std::error::Error>> {
    let root = temporary_directory("checksum")?;
    fs::write(root.join("changed.log"), "changed")?;
    let record = artifact(ArtifactCategory::ValidationOutput, "changed.log");

    let error = verify_hash_inventory(&root, &[record]).expect_err("changed file must be rejected");

    assert!(error.to_string().contains("changed.log"));
    fs::remove_dir_all(root)?;
    Ok(())
}

#[test]
fn hash_inventory_rejects_a_duplicate_artifact_path() -> Result<(), Box<dyn std::error::Error>> {
    let root = temporary_directory("duplicate")?;
    fs::write(root.join("same.log"), "")?;
    let record = artifact(ArtifactCategory::ValidationOutput, "same.log");

    let error = verify_hash_inventory(&root, &[record.clone(), record])
        .expect_err("duplicate artifact path must be rejected");

    assert!(error.to_string().contains("same.log"));
    fs::remove_dir_all(root)?;
    Ok(())
}

fn temporary_directory(suffix: &str) -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
    let unique = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let root = std::env::temp_dir().join(format!(
        "voxel-nexus-localized-raster-evidence-{suffix}-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir_all(&root)?;
    Ok(root)
}

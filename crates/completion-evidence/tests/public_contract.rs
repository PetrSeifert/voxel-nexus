use completion_evidence::{
    ArtifactCategory, ArtifactRecord, CompletionBundleManifest, ReproductionCategory,
    ReproductionCommand, VideoMetadata, WINDOWS_DEVELOPMENT_MACHINE_SCOPE, verify_hash_inventory,
    verify_manifest_contract,
};
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

fn required_reproduction_commands() -> Vec<ReproductionCommand> {
    [
        ReproductionCategory::GeneratedArtifacts,
        ReproductionCategory::Formatting,
        ReproductionCategory::Lint,
        ReproductionCategory::UnitAndIntegration,
        ReproductionCategory::VoxelFrontendRead,
        ReproductionCategory::DiagnosticSurface,
        ReproductionCategory::Lifecycle,
        ReproductionCategory::DeterministicFailure,
        ReproductionCategory::PrerequisiteRegression,
        ReproductionCategory::BundleVerification,
    ]
    .into_iter()
    .map(|category| ReproductionCommand {
        category,
        command: "cargo test --locked --workspace".to_owned(),
    })
    .collect()
}

fn artifacts() -> Vec<ArtifactRecord> {
    let required = [
        (ArtifactCategory::ContinuousVideo, 1),
        (ArtifactCategory::VideoEventTimeline, 1),
        (ArtifactCategory::FixedPosePng, 6),
        (ArtifactCategory::TolerantComparison, 3),
        (ArtifactCategory::SemanticFaceReport, 1),
        (ArtifactCategory::ValidationLog, 1),
        (ArtifactCategory::DerivationFailureLog, 1),
        (ArtifactCategory::UploadFailureLog, 1),
        (ArtifactCategory::PrerequisiteLog, 2),
        (ArtifactCategory::TimingManifest, 1),
        (ArtifactCategory::FirstCorrectFrameStream, 30),
        (ArtifactCategory::SteadyCpuGpuStream, 3),
        (ArtifactCategory::TimingSummary, 1),
        (ArtifactCategory::GeometryResourceCounts, 1),
        (ArtifactCategory::ComparisonChart, 1),
        (ArtifactCategory::CleanCheckoutLog, 1),
        (ArtifactCategory::ReproductionInstructions, 1),
    ];
    let mut artifacts = Vec::new();
    for (category, count) in required {
        for _ in 0..count {
            let identity = artifacts.len() + 1;
            artifacts.push(ArtifactRecord {
                category,
                path: format!("artifact-{identity}"),
                sha256: EMPTY_SHA256.to_owned(),
                bytes: u64::from(category == ArtifactCategory::ContinuousVideo),
            });
        }
    }
    artifacts
}

fn valid_manifest() -> CompletionBundleManifest {
    CompletionBundleManifest {
        schema_version: 1,
        scope: WINDOWS_DEVELOPMENT_MACHINE_SCOPE.to_owned(),
        repository_revision: "0123456789abcdef0123456789abcdef01234567".to_owned(),
        reproduction_commands: required_reproduction_commands(),
        video: VideoMetadata {
            path: "artifact-1".to_owned(),
            capture_scope: "Voxel Nexus desktop-demo window".to_owned(),
            duration_seconds: 20.0,
            codec: "h264".to_owned(),
            pixel_format: "yuv420p".to_owned(),
            width: 884,
            height: 611,
            average_frame_rate: "30/1".to_owned(),
            validation_warnings: 0,
            validation_errors: 0,
            uninterrupted: true,
            events: completion_evidence::REQUIRED_VIDEO_EVENTS
                .iter()
                .map(|event| (*event).to_owned())
                .collect(),
        },
        artifacts: artifacts(),
    }
}

#[test]
fn completion_contract_accepts_all_required_categories_and_windows_scope()
-> Result<(), Box<dyn std::error::Error>> {
    let summary = verify_manifest_contract(&valid_manifest())?;

    assert_eq!(summary.first_correct_frame_streams, 30);
    assert_eq!(summary.steady_cpu_gpu_streams, 3);
    assert_eq!(summary.fixed_pose_pngs, 6);
    Ok(())
}

#[test]
fn completion_contract_rejects_portable_runtime_claims() {
    let mut manifest = valid_manifest();
    manifest.scope = "Runtime proof applies to Windows systems.".to_owned();

    let error = verify_manifest_contract(&manifest).expect_err("portable claim must be rejected");

    assert!(
        error
            .to_string()
            .contains("recorded Windows development machine")
    );
}

#[test]
fn completion_contract_rejects_missing_raw_streams() {
    let mut manifest = valid_manifest();
    manifest
        .artifacts
        .retain(|artifact| artifact.category != ArtifactCategory::FirstCorrectFrameStream);

    let error = verify_manifest_contract(&manifest).expect_err("raw streams must be required");

    assert!(error.to_string().contains("FirstCorrectFrameStream"));
}

#[test]
fn hash_inventory_rejects_changed_artifacts() -> Result<(), Box<dyn std::error::Error>> {
    let unique = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let root = std::env::temp_dir().join(format!(
        "voxel-nexus-completion-evidence-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir_all(&root)?;
    let path = root.join("proof.log");
    fs::write(&path, "changed")?;
    let artifact = ArtifactRecord {
        category: ArtifactCategory::ValidationLog,
        path: "proof.log".to_owned(),
        sha256: EMPTY_SHA256.to_owned(),
        bytes: 0,
    };

    let error = verify_hash_inventory(&root, &[artifact]).expect_err("changed file must fail");

    assert!(error.to_string().contains("proof.log"));
    fs::remove_dir_all(root)?;
    Ok(())
}

use portable_compute_ray_evidence::{
    ArtifactCategory, ArtifactRecord, BundleManifest, LifecycleOutcomes, MACHINE_LOCAL_SCOPE,
    MeasurementConditions, OwnedResources, REPOSITORY_REMOTE, RepositoryProvenance,
    RevisionOutcomes, SemanticOutcomes, verify_hash_inventory, verify_manifest_contract,
};
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

fn required_artifacts() -> Vec<ArtifactRecord> {
    let requirements = [
        (ArtifactCategory::Provenance, 1),
        (ArtifactCategory::Executable, 1),
        (ArtifactCategory::CapabilityFacts, 1),
        (ArtifactCategory::SceneDefinition, 1),
        (ArtifactCategory::CameraDefinition, 1),
        (ArtifactCategory::ProbeDefinition, 1),
        (ArtifactCategory::OracleSelfTests, 1),
        (ArtifactCategory::SemanticObservations, 1),
        (ArtifactCategory::EventTimeline, 1),
        (ArtifactCategory::LifecycleLog, 1),
        (ArtifactCategory::FailureLog, 1),
        (ArtifactCategory::TimingStream, 1),
        (ArtifactCategory::ResourceLedger, 1),
        (ArtifactCategory::SelectedFrame, 5),
        (ArtifactCategory::SemanticSummary, 1),
        (ArtifactCategory::TimingResourceChart, 1),
        (ArtifactCategory::ValidationLog, 1),
        (ArtifactCategory::UninterruptedVideo, 1),
        (ArtifactCategory::ShutdownLog, 1),
        (ArtifactCategory::ReproductionInstructions, 1),
    ];
    let mut artifacts = Vec::new();
    for (category, count) in requirements {
        for _ in 0..count {
            let path = if category == ArtifactCategory::Executable {
                "bin/desktop-demo.exe".to_owned()
            } else {
                format!("artifact-{}", artifacts.len() + 1)
            };
            let sha256 = if category == ArtifactCategory::Executable {
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_owned()
            } else {
                EMPTY_SHA256.to_owned()
            };
            artifacts.push(ArtifactRecord {
                category,
                path,
                sha256,
                bytes: 1,
            });
        }
    }
    artifacts
}

fn valid_manifest() -> BundleManifest {
    BundleManifest {
        schema_version: 1,
        scope: MACHINE_LOCAL_SCOPE.to_owned(),
        provenance: RepositoryProvenance {
            remote: REPOSITORY_REMOTE.to_owned(),
            revision: "0123456789abcdef0123456789abcdef01234567".to_owned(),
            executable_path: "bin/desktop-demo.exe".to_owned(),
            executable_sha256: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                .to_owned(),
        },
        conditions: MeasurementConditions {
            machine_identity: "development-machine".to_owned(),
            operating_system: "Windows".to_owned(),
            device: "recorded-device".to_owned(),
            driver: "recorded-driver".to_owned(),
            vulkan_api_version: "1.3".to_owned(),
            correctness_validation_enabled: true,
            timing_validation_enabled: false,
            timing_conditions_identity: "development-machine-vulkan".to_owned(),
            resource_conditions_identity: "development-machine-vulkan".to_owned(),
            superiority_claimed: false,
            cross_machine_claimed: false,
        },
        semantics: SemanticOutcomes {
            oracle_self_tests_passed: true,
            compute_observations_passed: true,
            raster_correspondence_passed: true,
            mismatches: 0,
            started_inside_normals: 0,
        },
        revisions: RevisionOutcomes {
            required: vec![1, 2, 3, 4],
            visible: vec![1, 4],
            installed_compute: vec![1, 4],
            obsolete_presented_frames: 0,
            obsolete_semantic_observations: 0,
        },
        lifecycle: LifecycleOutcomes {
            uninterrupted: true,
            completed_switches: 3,
            final_presenter: "compute_ray".to_owned(),
            ownership_balanced: true,
            switching_after_shutdown: OwnedResources::default(),
            raster_after_shutdown: OwnedResources::default(),
            compute_after_shutdown: OwnedResources::default(),
            validation_warnings: 0,
            validation_errors: 0,
        },
        dense_dda_selected: true,
        occupancy_attempted: false,
        artifacts: required_artifacts(),
    }
}

#[test]
fn contract_accepts_attributed_machine_local_outcomes() -> Result<(), Box<dyn std::error::Error>> {
    let summary = verify_manifest_contract(&valid_manifest())?;

    assert_eq!(summary.completed_switches, 3);
    assert_eq!(summary.selected_frames, 5);
    Ok(())
}

#[test]
fn contract_rejects_semantic_mismatch() {
    let mut manifest = valid_manifest();
    manifest.semantics.mismatches = 1;

    let error = verify_manifest_contract(&manifest).expect_err("semantic mismatch was accepted");

    assert!(error.to_string().contains("semantic"));
}

#[test]
fn contract_rejects_revision_regression() {
    let mut manifest = valid_manifest();
    manifest.revisions.visible = vec![1, 4, 3];

    let error = verify_manifest_contract(&manifest).expect_err("revision regression was accepted");

    assert!(error.to_string().contains("revision"));
}

#[test]
fn contract_rejects_substituted_measurement_conditions() {
    let mut manifest = valid_manifest();
    manifest.conditions.resource_conditions_identity = "another-machine".to_owned();

    let error =
        verify_manifest_contract(&manifest).expect_err("substituted conditions were accepted");

    assert!(error.to_string().contains("conditions"));
}

#[test]
fn contract_rejects_unbalanced_ownership() {
    let mut manifest = valid_manifest();
    manifest.lifecycle.compute_after_shutdown.workers = 1;

    let error = verify_manifest_contract(&manifest).expect_err("live worker was accepted");

    assert!(error.to_string().contains("ownership"));
}

#[test]
fn contract_rejects_validation_findings() {
    let mut manifest = valid_manifest();
    manifest.lifecycle.validation_warnings = 1;

    let error = verify_manifest_contract(&manifest).expect_err("validation warning was accepted");

    assert!(error.to_string().contains("validation"));
}

#[test]
fn contract_rejects_superiority_or_cross_machine_claims() {
    let mut manifest = valid_manifest();
    manifest.conditions.superiority_claimed = true;

    let error = verify_manifest_contract(&manifest).expect_err("superiority claim was accepted");

    assert!(error.to_string().contains("claim"));
}

#[test]
fn contract_rejects_missing_required_still() {
    let mut manifest = valid_manifest();
    let selected_frame = manifest
        .artifacts
        .iter()
        .position(|artifact| artifact.category == ArtifactCategory::SelectedFrame)
        .expect("fixture has selected frames");
    manifest.artifacts.remove(selected_frame);

    let error = verify_manifest_contract(&manifest).expect_err("missing still was accepted");

    assert!(error.to_string().contains("SelectedFrame"));
}

#[test]
fn contract_rejects_substituted_executable_hash() {
    let mut manifest = valid_manifest();
    manifest.provenance.executable_sha256 =
        "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff".to_owned();

    let error =
        verify_manifest_contract(&manifest).expect_err("substituted executable was accepted");

    assert!(error.to_string().contains("executable"));
}

#[test]
fn hash_inventory_rejects_changed_artifact() -> Result<(), Box<dyn std::error::Error>> {
    let unique = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let root = std::env::temp_dir().join(format!(
        "voxel-nexus-portable-compute-ray-evidence-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir_all(&root)?;
    fs::write(root.join("proof.log"), "changed")?;
    let artifact = ArtifactRecord {
        category: ArtifactCategory::ValidationLog,
        path: "proof.log".to_owned(),
        sha256: EMPTY_SHA256.to_owned(),
        bytes: 7,
    };

    let error = verify_hash_inventory(&root, &[artifact]).expect_err("changed hash was accepted");

    assert!(error.to_string().contains("proof.log"));
    fs::remove_dir_all(root)?;
    Ok(())
}

#[test]
fn contract_rejects_missing_machine_attribution() {
    let mut manifest = valid_manifest();
    manifest.conditions.machine_identity.clear();

    let error = verify_manifest_contract(&manifest).expect_err("missing attribution was accepted");

    assert!(error.to_string().contains("machine_identity"));
}

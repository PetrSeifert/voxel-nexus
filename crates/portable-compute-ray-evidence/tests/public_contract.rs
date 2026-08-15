use portable_compute_ray_evidence::{
    ArtifactCategory, ArtifactRecord, BundleManifest, LifecycleOutcomes, MACHINE_LOCAL_SCOPE,
    MeasurementConditions, OwnedResources, REPOSITORY_REMOTE, RepositoryProvenance,
    RevisionOutcomes, SemanticOutcomes, verify_bundle, verify_hash_inventory,
    verify_manifest_contract,
};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;
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
    artifacts.push(ArtifactRecord {
        category: ArtifactCategory::SupportingEvidence,
        path: "correctness/desktop-demo.stderr.log".to_owned(),
        sha256: EMPTY_SHA256.to_owned(),
        bytes: 0,
    });
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
            timing_conditions_identity: "development-machine|recorded-device|recorded-driver|vulkan-1.3|scene-64|overview-to-cavity|immediate".to_owned(),
            resource_conditions_identity: "development-machine|recorded-device|recorded-driver|vulkan-1.3|scene-64|overview-to-cavity|immediate".to_owned(),
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

fn artifact_contents(category: ArtifactCategory) -> &'static str {
    match category {
        ArtifactCategory::Provenance => {
            r#"{"repository_remote":"https://github.com/PetrSeifert/voxel-nexus.git","repository_revision":"0123456789abcdef0123456789abcdef01234567","executable_path":"bin/desktop-demo.exe","executable_sha256":"placeholder","machine_identity":"development-machine","operating_system":"Windows"}"#
        }
        ArtifactCategory::CapabilityFacts => {
            r#"{"device":"recorded-device","driver":"recorded-driver","vulkan_api_version":"1.3","correctness_validation":"enabled","timing_validation":"disabled","timing_present_mode":"IMMEDIATE","selected_traversal":"dense_dda","occupancy_attempted":false}"#
        }
        ArtifactCategory::SceneDefinition => "Canonical scene: fixture\n",
        ArtifactCategory::CameraDefinition => concat!(
            "Canonical camera: overview\n",
            "Compute lifecycle qualification published camera=cavity\n",
        ),
        ArtifactCategory::ProbeDefinition => {
            r#"{"schema_version":1,"probes":[{"kind":"probe_definition","probe_identity":"probe-a","origin":[0.0,0.0,0.0],"direction":[0.0,0.0,1.0],"minimum_distance":0.0,"maximum_distance":1.0}]}"#
        }
        ArtifactCategory::SemanticObservations => concat!(
            "{\"kind\":\"observation\",\"render_path\":\"compute_ray\",\"probe_identity\":\"probe-a\",\"revision\":\"1\",\"frame_sequence\":1,\"actual\":{\"result\":\"miss\"},\"oracle\":{\"result\":\"miss\"},\"distance_tolerance\":0.0001,\"passed\":true}\n",
            "{\"kind\":\"observation\",\"render_path\":\"raster\",\"probe_identity\":\"probe-a\",\"revision\":\"1\",\"frame_sequence\":2,\"oracle\":{\"result\":\"miss\"},\"correspondence\":\"NotApplicableMiss\",\"passed\":true}\n",
            "{\"kind\":\"observation\",\"render_path\":\"compute_ray\",\"probe_identity\":\"probe-a\",\"revision\":\"4\",\"frame_sequence\":3,\"actual\":{\"result\":\"contact\",\"volume_identity\":\"volume-a\",\"coordinate\":[1,2,3],\"material_identity\":\"material-a\",\"distance\":0.5,\"classification\":\"entered\",\"outward_normal\":\"PositiveX\"},\"oracle\":{\"result\":\"contact\",\"volume_identity\":\"volume-a\",\"coordinate\":[1,2,3],\"material_identity\":\"material-a\",\"distance\":0.5,\"classification\":\"entered\",\"outward_normal\":\"PositiveX\"},\"distance_tolerance\":0.0001,\"passed\":true}\n",
            "{\"kind\":\"observation\",\"render_path\":\"raster\",\"probe_identity\":\"probe-a\",\"revision\":\"4\",\"frame_sequence\":4,\"oracle\":{\"result\":\"contact\",\"volume_identity\":\"volume-a\",\"coordinate\":[1,2,3],\"material_identity\":\"material-a\",\"distance\":0.5,\"classification\":\"entered\",\"outward_normal\":\"PositiveX\"},\"correspondence\":\"Matched(SemanticFace { volume_identity: volume-a, occupied_coordinate: VoxelCoordinate { x: 1, y: 2, z: 3 }, outward_normal: PositiveX, material_identity: material-a })\",\"passed\":true}\n",
        ),
        ArtifactCategory::SemanticSummary => {
            r#"{"oracle_self_tests_passed":true,"compute_observations_passed":true,"raster_correspondence_passed":true,"observation_count":4,"mismatches":0,"started_inside_normals":0,"table":[{"render_path":"compute_ray","probe_identity":"probe-a","revision":"1","frame_sequence":1,"result":"miss","correspondence":null,"passed":true},{"render_path":"raster","probe_identity":"probe-a","revision":"1","frame_sequence":2,"result":"miss","correspondence":"NotApplicableMiss","passed":true},{"render_path":"compute_ray","probe_identity":"probe-a","revision":"4","frame_sequence":3,"result":"contact","correspondence":null,"passed":true},{"render_path":"raster","probe_identity":"probe-a","revision":"4","frame_sequence":4,"result":"contact","correspondence":"Matched(SemanticFace { volume_identity: volume-a, occupied_coordinate: VoxelCoordinate { x: 1, y: 2, z: 3 }, outward_normal: PositiveX, material_identity: material-a })","passed":true}]}"#
        }
        ArtifactCategory::EventTimeline => {
            r#"[{"event":"raster_revision_1","elapsed_seconds":0.0,"window_title":"Required=1 Visible=1"},{"event":"compute_revision_1","elapsed_seconds":1.0,"window_title":"Required=1 Visible=1"},{"event":"edit_burst_requested","elapsed_seconds":2.0,"window_title":"Required=1 Visible=1"},{"event":"compute_required_4_visible_1","elapsed_seconds":3.0,"window_title":"Required=4 Visible=1"},{"event":"compute_revision_4","elapsed_seconds":4.0,"window_title":"Required=4 Visible=4"},{"event":"raster_revision_4","elapsed_seconds":5.0,"window_title":"Required=4 Visible=4"},{"event":"compute_revision_4_final","elapsed_seconds":6.0,"window_title":"Required=4 Visible=4"},{"event":"clean_close","elapsed_seconds":7.0,"window_title":"closed"}]"#
        }
        ArtifactCategory::LifecycleLog => concat!(
            "Compute revision 2 cancelled after exactly one preparation block\n",
            "Compute revision 3 rejected after upload with Required=4\n",
            "Compute edit burst converged newest-only: Required=4 Visible=4 installed_revisions=[VoxelSceneRevision(4)] obsolete_presented_frames=0 obsolete_semantic_observations=0\n",
            "Render Path round trip complete: raster-to-compute-to-raster-to-compute switches=3 closing_presenter=ComputeRay\n",
            "Render Path-owned raster resources after shutdown: 0\n",
            "Render Path-owned compute resources after shutdown: objects=0 allocations=0 workers=0 views=0\n",
            "Render Path switching resources after shutdown: replacement=0 retiring=0\n",
        ),
        ArtifactCategory::ResourceLedger => concat!(
            "Compute resource observation: sequence=1 point=Shutdown bytes=0 objects=0 allocations=0 workers=0 views=0\n",
            "Render Path-owned raster resources after shutdown: 0\n",
            "Render Path-owned compute resources after shutdown: objects=0 allocations=0 workers=0 views=0\n",
            "Render Path switching resources after shutdown: replacement=0 retiring=0\n",
        ),
        ArtifactCategory::ValidationLog => r#"{"enabled":true,"warnings":0,"errors":0}"#,
        ArtifactCategory::ShutdownLog => {
            r#"{"Cases":[{"ValidationWarnings":0,"ValidationErrors":0}]}"#
        }
        ArtifactCategory::OracleSelfTests | ArtifactCategory::FailureLog => {
            "test result: ok. 7 passed; 0 failed\n"
        }
        ArtifactCategory::TimingStream => concat!(
            "Compute timing event: phase=Preparation elapsed_ms=1\n",
            "Compute timing event: phase=Upload elapsed_ms=1\n",
            "Compute timing event: phase=Installation elapsed_ms=1\n",
            "Compute timing event: phase=Dispatch elapsed_ms=1\n",
            "Compute timing event: phase=Composite elapsed_ms=1\n",
            "Render Path timing event: phase=Switching elapsed_ms=1\n",
            "Render Path timing event: phase=Presentation elapsed_ms=1\n",
        ),
        _ => "evidence\n",
    }
}

fn materialize_bundle(
    root: &Path,
    manifest: &mut BundleManifest,
) -> Result<(), Box<dyn std::error::Error>> {
    for artifact in &mut manifest.artifacts {
        let path = root.join(&artifact.path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let contents = if artifact.path == "correctness/desktop-demo.stderr.log" {
            &[][..]
        } else {
            artifact_contents(artifact.category).as_bytes()
        };
        fs::write(&path, contents)?;
        artifact.bytes = u64::try_from(contents.len())?;
        artifact.sha256 = format!("{:x}", Sha256::digest(contents));
    }
    manifest.provenance.executable_sha256 = manifest
        .artifacts
        .iter()
        .find(|artifact| artifact.category == ArtifactCategory::Executable)
        .ok_or("fixture executable is missing")?
        .sha256
        .clone();
    let provenance = manifest
        .artifacts
        .iter_mut()
        .find(|artifact| artifact.category == ArtifactCategory::Provenance)
        .ok_or("fixture provenance is missing")?;
    let contents = artifact_contents(ArtifactCategory::Provenance)
        .replace("placeholder", &manifest.provenance.executable_sha256);
    fs::write(root.join(&provenance.path), &contents)?;
    provenance.bytes = u64::try_from(contents.len())?;
    provenance.sha256 = format!("{:x}", Sha256::digest(contents.as_bytes()));
    Ok(())
}

fn replace_artifact_text(
    root: &Path,
    manifest: &mut BundleManifest,
    category: ArtifactCategory,
    from: &str,
    to: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let artifact = manifest
        .artifacts
        .iter_mut()
        .find(|artifact| artifact.category == category)
        .ok_or("fixture artifact is missing")?;
    let path = root.join(&artifact.path);
    let original = fs::read_to_string(&path)?;
    let changed = original.replacen(from, to, 1);
    if changed == original {
        return Err("fixture mutation did not match".into());
    }
    fs::write(&path, &changed)?;
    artifact.bytes = u64::try_from(changed.len())?;
    artifact.sha256 = format!("{:x}", Sha256::digest(changed.as_bytes()));
    Ok(())
}

fn rejected(
    manifest: &BundleManifest,
    accepted_message: &'static str,
) -> Result<portable_compute_ray_evidence::EvidenceError, Box<dyn std::error::Error>> {
    match verify_manifest_contract(manifest) {
        Ok(_) => Err(accepted_message.into()),
        Err(error) => Ok(error),
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
fn contract_rejects_semantic_mismatch() -> Result<(), Box<dyn std::error::Error>> {
    let mut manifest = valid_manifest();
    manifest.semantics.mismatches = 1;

    let error = rejected(&manifest, "semantic mismatch was accepted")?;

    assert!(error.to_string().contains("semantic"));
    Ok(())
}

#[test]
fn contract_rejects_revision_regression() -> Result<(), Box<dyn std::error::Error>> {
    let mut manifest = valid_manifest();
    manifest.revisions.visible = vec![1, 4, 3];

    let error = rejected(&manifest, "revision regression was accepted")?;

    assert!(error.to_string().contains("revision"));
    Ok(())
}

#[test]
fn contract_rejects_substituted_measurement_conditions() -> Result<(), Box<dyn std::error::Error>> {
    let mut manifest = valid_manifest();
    manifest.conditions.resource_conditions_identity = "another-machine".to_owned();

    let error = rejected(&manifest, "substituted conditions were accepted")?;

    assert!(error.to_string().contains("conditions"));
    Ok(())
}

#[test]
fn contract_rejects_unbalanced_ownership() -> Result<(), Box<dyn std::error::Error>> {
    let mut manifest = valid_manifest();
    manifest.lifecycle.compute_after_shutdown.workers = 1;

    let error = rejected(&manifest, "live worker was accepted")?;

    assert!(error.to_string().contains("ownership"));
    Ok(())
}

#[test]
fn contract_rejects_validation_findings() -> Result<(), Box<dyn std::error::Error>> {
    let mut manifest = valid_manifest();
    manifest.lifecycle.validation_warnings = 1;

    let error = rejected(&manifest, "validation warning was accepted")?;

    assert!(error.to_string().contains("validation"));
    Ok(())
}

#[test]
fn contract_rejects_superiority_or_cross_machine_claims() -> Result<(), Box<dyn std::error::Error>>
{
    let mut manifest = valid_manifest();
    manifest.conditions.superiority_claimed = true;

    let error = rejected(&manifest, "superiority claim was accepted")?;

    assert!(error.to_string().contains("claim"));
    Ok(())
}

#[test]
fn contract_rejects_missing_required_still() -> Result<(), Box<dyn std::error::Error>> {
    let mut manifest = valid_manifest();
    let selected_frame = manifest
        .artifacts
        .iter()
        .position(|artifact| artifact.category == ArtifactCategory::SelectedFrame)
        .ok_or("fixture has no selected frames")?;
    manifest.artifacts.remove(selected_frame);

    let error = rejected(&manifest, "missing still was accepted")?;

    assert!(error.to_string().contains("SelectedFrame"));
    Ok(())
}

#[test]
fn contract_rejects_substituted_executable_hash() -> Result<(), Box<dyn std::error::Error>> {
    let mut manifest = valid_manifest();
    manifest.provenance.executable_sha256 =
        "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff".to_owned();

    let error = rejected(&manifest, "substituted executable was accepted")?;

    assert!(error.to_string().contains("executable"));
    Ok(())
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

    let error = match verify_hash_inventory(&root, &[artifact]) {
        Ok(()) => return Err("changed hash was accepted".into()),
        Err(error) => error,
    };

    assert!(error.to_string().contains("proof.log"));
    fs::remove_dir_all(root)?;
    Ok(())
}

#[test]
fn bundle_reads_and_accepts_matching_retained_evidence() -> Result<(), Box<dyn std::error::Error>> {
    let unique = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let root = std::env::temp_dir().join(format!(
        "voxel-nexus-portable-compute-ray-bundle-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir_all(&root)?;
    let mut manifest = valid_manifest();
    materialize_bundle(&root, &mut manifest)?;

    verify_bundle(&root, &manifest)?;

    fs::remove_dir_all(root)?;
    Ok(())
}

#[test]
fn bundle_rejects_semantic_mismatch_recorded_in_hashed_stream()
-> Result<(), Box<dyn std::error::Error>> {
    let unique = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let root = std::env::temp_dir().join(format!(
        "voxel-nexus-portable-compute-ray-mismatch-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir_all(&root)?;
    let mut manifest = valid_manifest();
    materialize_bundle(&root, &mut manifest)?;
    let semantic_artifact = manifest
        .artifacts
        .iter_mut()
        .find(|artifact| artifact.category == ArtifactCategory::SemanticObservations)
        .ok_or("fixture semantic stream is missing")?;
    let path = root.join(&semantic_artifact.path);
    let mismatched = artifact_contents(ArtifactCategory::SemanticObservations).replacen(
        "\"actual\":{\"result\":\"miss\"}",
        "\"actual\":{\"result\":\"contact\"}",
        1,
    );
    fs::write(&path, &mismatched)?;
    semantic_artifact.bytes = u64::try_from(mismatched.len())?;
    semantic_artifact.sha256 = format!("{:x}", Sha256::digest(mismatched.as_bytes()));

    let error = match verify_bundle(&root, &manifest) {
        Ok(_) => return Err("semantic mismatch in hashed stream was accepted".into()),
        Err(error) => error,
    };

    assert!(error.to_string().contains("semantic"));
    fs::remove_dir_all(root)?;
    Ok(())
}

#[test]
fn bundle_rejects_coordinated_machine_attribution_substitution()
-> Result<(), Box<dyn std::error::Error>> {
    let unique = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let root = std::env::temp_dir().join(format!(
        "voxel-nexus-attribution-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir_all(&root)?;
    let mut manifest = valid_manifest();
    materialize_bundle(&root, &mut manifest)?;
    manifest.conditions.machine_identity = "substitute-machine".to_owned();
    manifest.conditions.operating_system = "substitute-os".to_owned();
    manifest.conditions.timing_conditions_identity = "substitute".to_owned();
    manifest.conditions.resource_conditions_identity = "substitute".to_owned();

    if verify_bundle(&root, &manifest).is_ok() {
        return Err("coordinated attribution substitution was accepted".into());
    }
    fs::remove_dir_all(root)?;
    Ok(())
}

#[test]
fn bundle_rejects_wrong_raster_face_inside_matched_correspondence()
-> Result<(), Box<dyn std::error::Error>> {
    let unique = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let root = std::env::temp_dir().join(format!(
        "voxel-nexus-raster-mismatch-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir_all(&root)?;
    let mut manifest = valid_manifest();
    materialize_bundle(&root, &mut manifest)?;
    replace_artifact_text(
        &root,
        &mut manifest,
        ArtifactCategory::SemanticObservations,
        "material_identity: material-a })",
        "material_identity: wrong-material })",
    )?;
    replace_artifact_text(
        &root,
        &mut manifest,
        ArtifactCategory::SemanticSummary,
        "material_identity: material-a })",
        "material_identity: wrong-material })",
    )?;

    if verify_bundle(&root, &manifest).is_ok() {
        return Err("wrong raster face inside Matched was accepted".into());
    }
    fs::remove_dir_all(root)?;
    Ok(())
}

#[test]
fn bundle_rejects_ownership_claim_appended_after_zero_ledger()
-> Result<(), Box<dyn std::error::Error>> {
    let unique = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let root = std::env::temp_dir().join(format!(
        "voxel-nexus-ledger-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir_all(&root)?;
    let mut manifest = valid_manifest();
    materialize_bundle(&root, &mut manifest)?;
    replace_artifact_text(
        &root,
        &mut manifest,
        ArtifactCategory::ResourceLedger,
        "Render Path switching resources after shutdown: replacement=0 retiring=0\n",
        "Render Path switching resources after shutdown: replacement=0 retiring=0\nCompute resource observation: sequence=99 point=Shutdown objects=1 allocations=1 workers=1 views=1\n",
    )?;

    if verify_bundle(&root, &manifest).is_ok() {
        return Err("ownership contradiction after zero ledger was accepted".into());
    }
    fs::remove_dir_all(root)?;
    Ok(())
}

#[test]
fn bundle_rejects_validation_finding_in_retained_stderr() -> Result<(), Box<dyn std::error::Error>>
{
    let unique = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let root = std::env::temp_dir().join(format!(
        "voxel-nexus-validation-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir_all(&root)?;
    let mut manifest = valid_manifest();
    materialize_bundle(&root, &mut manifest)?;
    let artifact = manifest
        .artifacts
        .iter_mut()
        .find(|artifact| artifact.path == "correctness/desktop-demo.stderr.log")
        .ok_or("fixture validation stderr is missing")?;
    let finding = b"Vulkan validation ERROR: simulated\n";
    fs::write(root.join(&artifact.path), finding)?;
    artifact.bytes = u64::try_from(finding.len())?;
    artifact.sha256 = format!("{:x}", Sha256::digest(finding));

    if verify_bundle(&root, &manifest).is_ok() {
        return Err("retained validation finding was accepted".into());
    }
    fs::remove_dir_all(root)?;
    Ok(())
}

#[test]
fn contract_rejects_missing_machine_attribution() -> Result<(), Box<dyn std::error::Error>> {
    let mut manifest = valid_manifest();
    manifest.conditions.machine_identity.clear();

    let error = rejected(&manifest, "missing attribution was accepted")?;

    assert!(error.to_string().contains("machine_identity"));
    Ok(())
}

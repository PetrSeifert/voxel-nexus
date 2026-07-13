use localized_raster_evidence::{read_manifest, verify_bundle};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn retained_bundle() -> PathBuf {
    repository_root().join("docs/evidence/localized-editable-raster/v1/development-machine")
}

#[test]
fn retained_bundle_passes_all_verification_gates() -> Result<(), Box<dyn std::error::Error>> {
    let root = retained_bundle();
    let manifest = read_manifest(&root.join("manifest.json"))?;

    let summary = verify_bundle(&root, &manifest)?;

    assert_eq!(summary.artifacts, 146);
    assert_eq!(summary.selected_extent, [16, 16, 16]);
    Ok(())
}

#[test]
fn verifier_rejects_a_source_command_that_disagrees_with_the_bundle_manifest()
-> Result<(), Box<dyn std::error::Error>> {
    let source_root = retained_bundle();
    let mut manifest = read_manifest(&source_root.join("manifest.json"))?;
    let destination_root = linked_bundle_directory()?;
    link_artifacts(&source_root, &destination_root, &manifest)?;

    let relative_path = "demo/manifest.json";
    let destination_path = destination_root.join(relative_path);
    fs::remove_file(&destination_path)?;
    let mut source_manifest: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(source_root.join(relative_path))?)?;
    let published_revision = source_manifest
        .pointer_mut("/Commands/1/PublishedRevision")
        .ok_or("missing source published revision")?;
    *published_revision = serde_json::Value::from(4_u64);
    let changed = serde_json::to_vec_pretty(&source_manifest)?;
    fs::write(&destination_path, &changed)?;

    let record = manifest
        .artifacts
        .iter_mut()
        .find(|artifact| artifact.path == relative_path)
        .ok_or("missing demo manifest artifact record")?;
    record.bytes = u64::try_from(changed.len())?;
    record.sha256 = format!("{:x}", Sha256::digest(&changed));

    let error = verify_bundle(&destination_root, &manifest)
        .expect_err("inconsistent retained command must be rejected");

    assert!(error.to_string().contains("demo/manifest.json"));
    assert!(error.to_string().contains("command"));
    fs::remove_dir_all(destination_root)?;
    Ok(())
}

#[test]
fn verifier_rejects_a_retained_required_process_nonzero_exit()
-> Result<(), Box<dyn std::error::Error>> {
    let source_root = retained_bundle();
    let mut manifest = read_manifest(&source_root.join("manifest.json"))?;
    let destination_root = linked_bundle_directory()?;
    link_artifacts(&source_root, &destination_root, &manifest)?;

    let relative_path = "process-verification/process-outcomes.json";
    let destination_path = destination_root.join(relative_path);
    fs::remove_file(&destination_path)?;
    let mut process_outcomes: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(source_root.join(relative_path))?)?;
    let test_exit = process_outcomes
        .pointer_mut("/processes/1/exit_code")
        .ok_or("missing raster qualification process exit")?;
    *test_exit = serde_json::Value::from(1_i32);
    let changed = serde_json::to_vec_pretty(&process_outcomes)?;
    fs::write(&destination_path, &changed)?;

    let record = manifest
        .artifacts
        .iter_mut()
        .find(|artifact| artifact.path == relative_path)
        .ok_or("missing process outcome artifact record")?;
    record.bytes = u64::try_from(changed.len())?;
    record.sha256 = format!("{:x}", Sha256::digest(&changed));

    let error = verify_bundle(&destination_root, &manifest)
        .expect_err("nonzero retained required process exit must be rejected");

    assert!(error.to_string().contains("process exit"));
    fs::remove_dir_all(destination_root)?;
    Ok(())
}

fn linked_bundle_directory() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let unique = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let root = repository_root().join(format!(
        "target/localized-raster-evidence-test-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir_all(&root)?;
    Ok(root)
}

fn link_artifacts(
    source_root: &Path,
    destination_root: &Path,
    manifest: &localized_raster_evidence::BundleManifest,
) -> Result<(), Box<dyn std::error::Error>> {
    for artifact in &manifest.artifacts {
        let source = source_root.join(&artifact.path);
        let destination = destination_root.join(&artifact.path);
        let parent = destination.parent().ok_or("artifact path has no parent")?;
        fs::create_dir_all(parent)?;
        fs::hard_link(source, destination)?;
    }
    Ok(())
}

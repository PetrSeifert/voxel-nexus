use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::{Component, Path};
use thiserror::Error;

pub const MACHINE_LOCAL_SCOPE: &str = "Runtime and measurements apply only to the attributed machine; no Render Path superiority or cross-machine claim.";
pub const REPOSITORY_REMOTE: &str = "https://github.com/PetrSeifert/voxel-nexus.git";

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactCategory {
    Provenance,
    Executable,
    CapabilityFacts,
    SceneDefinition,
    CameraDefinition,
    ProbeDefinition,
    OracleSelfTests,
    SemanticObservations,
    EventTimeline,
    LifecycleLog,
    FailureLog,
    TimingStream,
    ResourceLedger,
    SelectedFrame,
    SemanticSummary,
    TimingResourceChart,
    ValidationLog,
    UninterruptedVideo,
    ShutdownLog,
    ReproductionInstructions,
    SupportingEvidence,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ArtifactRecord {
    pub category: ArtifactCategory,
    pub path: String,
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RepositoryProvenance {
    pub remote: String,
    pub revision: String,
    pub executable_path: String,
    pub executable_sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MeasurementConditions {
    pub machine_identity: String,
    pub operating_system: String,
    pub device: String,
    pub driver: String,
    pub vulkan_api_version: String,
    pub correctness_validation_enabled: bool,
    pub timing_validation_enabled: bool,
    pub timing_conditions_identity: String,
    pub resource_conditions_identity: String,
    pub superiority_claimed: bool,
    pub cross_machine_claimed: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SemanticOutcomes {
    pub oracle_self_tests_passed: bool,
    pub compute_observations_passed: bool,
    pub raster_correspondence_passed: bool,
    pub mismatches: u32,
    pub started_inside_normals: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RevisionOutcomes {
    pub required: Vec<u64>,
    pub visible: Vec<u64>,
    pub installed_compute: Vec<u64>,
    pub obsolete_presented_frames: u32,
    pub obsolete_semantic_observations: u32,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct OwnedResources {
    pub objects: u64,
    pub allocations: u64,
    pub workers: u64,
    pub views: u64,
}

impl OwnedResources {
    fn is_zero(&self) -> bool {
        self.objects == 0 && self.allocations == 0 && self.workers == 0 && self.views == 0
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LifecycleOutcomes {
    pub uninterrupted: bool,
    pub completed_switches: u32,
    pub final_presenter: String,
    pub ownership_balanced: bool,
    pub switching_after_shutdown: OwnedResources,
    pub raster_after_shutdown: OwnedResources,
    pub compute_after_shutdown: OwnedResources,
    pub validation_warnings: u32,
    pub validation_errors: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BundleManifest {
    pub schema_version: u32,
    pub scope: String,
    pub provenance: RepositoryProvenance,
    pub conditions: MeasurementConditions,
    pub semantics: SemanticOutcomes,
    pub revisions: RevisionOutcomes,
    pub lifecycle: LifecycleOutcomes,
    pub dense_dda_selected: bool,
    pub occupancy_attempted: bool,
    pub artifacts: Vec<ArtifactRecord>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct VerificationSummary {
    pub artifacts: usize,
    pub selected_frames: usize,
    pub completed_switches: u32,
}

#[derive(Debug, Error)]
pub enum EvidenceError {
    #[error("portable compute-ray evidence schema version {actual} is unsupported; expected 1")]
    UnsupportedSchema { actual: u32 },
    #[error("bundle attribution field {field} is missing or invalid")]
    InvalidAttribution { field: &'static str },
    #[error("semantic qualification did not pass exactly: {reason}")]
    SemanticMismatch { reason: &'static str },
    #[error("revision evidence does not prove newest-only convergence: {reason}")]
    InvalidRevisions { reason: &'static str },
    #[error("measurement conditions were substituted or permit an excluded claim: {reason}")]
    InvalidConditions { reason: &'static str },
    #[error("lifecycle ownership is unbalanced or nonzero after shutdown: {reason}")]
    InvalidOwnership { reason: &'static str },
    #[error("validation evidence contains findings or used the wrong validation mode")]
    ValidationFindings,
    #[error("traversal selection must retain dense DDA without an occupancy attempt")]
    InvalidTraversalSelection,
    #[error("artifact category {category:?} has {actual} entries; expected {expected}")]
    InvalidArtifactCount {
        category: ArtifactCategory,
        expected: usize,
        actual: usize,
    },
    #[error("artifact path is unsafe, empty, or duplicated: {path:?}")]
    InvalidArtifactPath { path: String },
    #[error("artifact {path} has an invalid SHA-256 or byte count")]
    InvalidArtifactRecord { path: String },
    #[error("artifact {path} could not be read: {source}")]
    ArtifactRead {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("artifact {path} has {actual} bytes; manifest records {expected}")]
    ArtifactSize {
        path: String,
        expected: u64,
        actual: u64,
    },
    #[error("artifact {path} SHA-256 does not match the manifest")]
    ArtifactHash { path: String },
    #[error("could not parse evidence manifest {path}: {source}")]
    ManifestParse {
        path: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("retained evidence {path} is invalid: {reason}")]
    RetainedEvidence { path: String, reason: String },
}

#[derive(Deserialize)]
struct CapabilityEvidence {
    device: String,
    driver: String,
    vulkan_api_version: String,
    correctness_validation: String,
    timing_validation: String,
    timing_present_mode: String,
    selected_traversal: String,
    occupancy_attempted: bool,
}

#[derive(Deserialize)]
struct ProbeEvidenceFile {
    schema_version: u32,
    probes: Vec<ProbeEvidence>,
}

#[derive(Deserialize)]
struct ProbeEvidence {
    kind: String,
    probe_identity: String,
    origin: [f64; 3],
    direction: [f64; 3],
    minimum_distance: f64,
    maximum_distance: f64,
}

#[derive(Debug, Deserialize, PartialEq)]
struct SemanticResultEvidence {
    result: String,
    #[serde(default)]
    volume_identity: Option<String>,
    #[serde(default)]
    coordinate: Option<[i32; 3]>,
    #[serde(default)]
    material_identity: Option<String>,
    #[serde(default)]
    distance: Option<f64>,
    #[serde(default)]
    classification: Option<String>,
    #[serde(default)]
    outward_normal: Option<String>,
}

#[derive(Deserialize)]
struct SemanticObservationEvidence {
    kind: String,
    render_path: String,
    probe_identity: String,
    revision: String,
    frame_sequence: u64,
    #[serde(default)]
    actual: Option<SemanticResultEvidence>,
    oracle: SemanticResultEvidence,
    #[serde(default)]
    distance_tolerance: Option<f64>,
    #[serde(default)]
    correspondence: Option<String>,
    passed: bool,
}

#[derive(Deserialize)]
struct SemanticSummaryEvidence {
    oracle_self_tests_passed: bool,
    compute_observations_passed: bool,
    raster_correspondence_passed: bool,
    observation_count: usize,
    mismatches: u32,
    started_inside_normals: u32,
    table: Vec<SemanticSummaryRow>,
}

#[derive(Debug, Deserialize, PartialEq)]
struct SemanticSummaryRow {
    render_path: String,
    probe_identity: String,
    revision: String,
    frame_sequence: u64,
    result: String,
    #[serde(default)]
    correspondence: Option<String>,
    passed: bool,
}

#[derive(Deserialize)]
struct TimelineEvidence {
    event: String,
    elapsed_seconds: f64,
    window_title: String,
}

#[derive(Deserialize)]
struct ValidationEvidence {
    enabled: bool,
    warnings: u32,
    errors: u32,
}

#[derive(Deserialize)]
struct ProvenanceEvidence {
    repository_remote: String,
    repository_revision: String,
    executable_path: String,
    executable_sha256: String,
}

#[derive(Deserialize)]
struct ShutdownEvidence {
    #[serde(rename = "Cases")]
    cases: Vec<ShutdownCaseEvidence>,
}

#[derive(Deserialize)]
struct ShutdownCaseEvidence {
    #[serde(rename = "ValidationWarnings")]
    validation_warnings: u32,
    #[serde(rename = "ValidationErrors")]
    validation_errors: u32,
}

pub fn read_manifest(path: &Path) -> Result<BundleManifest, EvidenceError> {
    let contents = fs::read_to_string(path).map_err(|source| EvidenceError::ArtifactRead {
        path: path.display().to_string(),
        source,
    })?;
    serde_json::from_str(&contents).map_err(|source| EvidenceError::ManifestParse {
        path: path.display().to_string(),
        source,
    })
}

pub fn verify_manifest_contract(
    manifest: &BundleManifest,
) -> Result<VerificationSummary, EvidenceError> {
    if manifest.schema_version != 1 {
        return Err(EvidenceError::UnsupportedSchema {
            actual: manifest.schema_version,
        });
    }
    for (field, value) in [
        ("scope", manifest.scope.as_str()),
        ("repository_remote", manifest.provenance.remote.as_str()),
        (
            "machine_identity",
            manifest.conditions.machine_identity.as_str(),
        ),
        (
            "operating_system",
            manifest.conditions.operating_system.as_str(),
        ),
        ("device", manifest.conditions.device.as_str()),
        ("driver", manifest.conditions.driver.as_str()),
        (
            "vulkan_api_version",
            manifest.conditions.vulkan_api_version.as_str(),
        ),
    ] {
        if value.trim().is_empty() {
            return Err(EvidenceError::InvalidAttribution { field });
        }
    }
    if manifest.scope != MACHINE_LOCAL_SCOPE {
        return Err(EvidenceError::InvalidAttribution { field: "scope" });
    }
    if manifest.provenance.remote != REPOSITORY_REMOTE {
        return Err(EvidenceError::InvalidAttribution {
            field: "repository_remote",
        });
    }
    if !is_lower_hex(&manifest.provenance.revision, 40) {
        return Err(EvidenceError::InvalidAttribution {
            field: "repository_revision",
        });
    }
    if manifest.provenance.executable_path.trim().is_empty() {
        return Err(EvidenceError::InvalidAttribution {
            field: "executable_path",
        });
    }
    if !is_lower_hex(&manifest.provenance.executable_sha256, 64) {
        return Err(EvidenceError::InvalidAttribution {
            field: "executable_sha256",
        });
    }
    if !manifest.semantics.oracle_self_tests_passed
        || !manifest.semantics.compute_observations_passed
        || !manifest.semantics.raster_correspondence_passed
        || manifest.semantics.mismatches != 0
        || manifest.semantics.started_inside_normals != 0
    {
        return Err(EvidenceError::SemanticMismatch {
            reason: "oracle, compute, and raster results must pass with no mismatch or StartedInside normal",
        });
    }
    if manifest.revisions.required != [1, 2, 3, 4]
        || manifest.revisions.visible != [1, 4]
        || manifest.revisions.installed_compute != [1, 4]
        || manifest.revisions.obsolete_presented_frames != 0
        || manifest.revisions.obsolete_semantic_observations != 0
        || has_regression(&manifest.revisions.required)
        || has_regression(&manifest.revisions.visible)
        || has_regression(&manifest.revisions.installed_compute)
    {
        return Err(EvidenceError::InvalidRevisions {
            reason: "required must be 1,2,3,4 while visible and installed compute advance directly from 1 to 4",
        });
    }
    if manifest
        .conditions
        .timing_conditions_identity
        .trim()
        .is_empty()
        || manifest.conditions.timing_conditions_identity
            != manifest.conditions.resource_conditions_identity
        || manifest.conditions.superiority_claimed
        || manifest.conditions.cross_machine_claimed
    {
        return Err(EvidenceError::InvalidConditions {
            reason: "timing and resource evidence must share one attributed machine-local condition and make no superiority claim",
        });
    }
    if !manifest.conditions.correctness_validation_enabled
        || manifest.conditions.timing_validation_enabled
        || manifest.lifecycle.validation_warnings != 0
        || manifest.lifecycle.validation_errors != 0
    {
        return Err(EvidenceError::ValidationFindings);
    }
    if !manifest.lifecycle.uninterrupted
        || manifest.lifecycle.completed_switches != 3
        || manifest.lifecycle.final_presenter != "compute_ray"
        || !manifest.lifecycle.ownership_balanced
        || !manifest.lifecycle.switching_after_shutdown.is_zero()
        || !manifest.lifecycle.raster_after_shutdown.is_zero()
        || !manifest.lifecycle.compute_after_shutdown.is_zero()
    {
        return Err(EvidenceError::InvalidOwnership {
            reason: "the uninterrupted three-switch sequence must close with compute-ray presenting and every owner at zero",
        });
    }
    if !manifest.dense_dda_selected || manifest.occupancy_attempted {
        return Err(EvidenceError::InvalidTraversalSelection);
    }

    let mut artifact_counts = BTreeMap::new();
    let mut artifact_paths = BTreeSet::new();
    for artifact in &manifest.artifacts {
        validate_artifact_path(&artifact.path)?;
        if !artifact_paths.insert(artifact.path.clone()) {
            return Err(EvidenceError::InvalidArtifactPath {
                path: artifact.path.clone(),
            });
        }
        if !is_lower_hex(&artifact.sha256, 64)
            || (artifact.bytes == 0
                && matches!(
                    artifact.category,
                    ArtifactCategory::Executable
                        | ArtifactCategory::SelectedFrame
                        | ArtifactCategory::UninterruptedVideo
                ))
        {
            return Err(EvidenceError::InvalidArtifactRecord {
                path: artifact.path.clone(),
            });
        }
        *artifact_counts.entry(artifact.category).or_insert(0usize) += 1;
    }
    for (category, expected) in required_artifact_counts() {
        let actual = artifact_counts.get(&category).copied().unwrap_or_default();
        if actual != expected {
            return Err(EvidenceError::InvalidArtifactCount {
                category,
                expected,
                actual,
            });
        }
    }
    let executable = manifest
        .artifacts
        .iter()
        .find(|artifact| artifact.category == ArtifactCategory::Executable)
        .ok_or(EvidenceError::InvalidArtifactCount {
            category: ArtifactCategory::Executable,
            expected: 1,
            actual: 0,
        })?;
    if executable.path != manifest.provenance.executable_path
        || executable.sha256 != manifest.provenance.executable_sha256
    {
        return Err(EvidenceError::InvalidAttribution {
            field: "executable_hash",
        });
    }
    Ok(VerificationSummary {
        artifacts: manifest.artifacts.len(),
        selected_frames: manifest
            .artifacts
            .iter()
            .filter(|artifact| artifact.category == ArtifactCategory::SelectedFrame)
            .count(),
        completed_switches: manifest.lifecycle.completed_switches,
    })
}

pub fn verify_hash_inventory(
    bundle_root: &Path,
    artifacts: &[ArtifactRecord],
) -> Result<(), EvidenceError> {
    let mut buffer = vec![0_u8; 64 * 1024];
    for artifact in artifacts {
        validate_artifact_path(&artifact.path)?;
        let artifact_path = bundle_root.join(&artifact.path);
        let mut file =
            fs::File::open(&artifact_path).map_err(|source| EvidenceError::ArtifactRead {
                path: artifact.path.clone(),
                source,
            })?;
        let actual = file
            .metadata()
            .map_err(|source| EvidenceError::ArtifactRead {
                path: artifact.path.clone(),
                source,
            })?
            .len();
        if actual != artifact.bytes {
            return Err(EvidenceError::ArtifactSize {
                path: artifact.path.clone(),
                expected: artifact.bytes,
                actual,
            });
        }
        let mut hash = Sha256::new();
        loop {
            let bytes_read =
                file.read(&mut buffer)
                    .map_err(|source| EvidenceError::ArtifactRead {
                        path: artifact.path.clone(),
                        source,
                    })?;
            if bytes_read == 0 {
                break;
            }
            hash.update(&buffer[..bytes_read]);
        }
        if format!("{:x}", hash.finalize()) != artifact.sha256 {
            return Err(EvidenceError::ArtifactHash {
                path: artifact.path.clone(),
            });
        }
    }
    Ok(())
}

pub fn verify_bundle(
    bundle_root: &Path,
    manifest: &BundleManifest,
) -> Result<VerificationSummary, EvidenceError> {
    let summary = verify_manifest_contract(manifest)?;
    verify_hash_inventory(bundle_root, &manifest.artifacts)?;
    verify_retained_evidence(bundle_root, manifest)?;
    Ok(summary)
}

fn verify_retained_evidence(
    bundle_root: &Path,
    manifest: &BundleManifest,
) -> Result<(), EvidenceError> {
    verify_provenance_evidence(bundle_root, manifest)?;
    verify_capability_evidence(bundle_root, manifest)?;
    let probes = verify_probe_evidence(bundle_root, manifest)?;
    let observations = verify_semantic_evidence(bundle_root, manifest, &probes)?;
    verify_semantic_summary(bundle_root, manifest, &observations)?;
    verify_scene_and_camera_evidence(bundle_root, manifest)?;
    verify_timeline_evidence(bundle_root, manifest)?;
    verify_lifecycle_and_resource_evidence(bundle_root, manifest)?;
    verify_timing_evidence(bundle_root, manifest)?;
    verify_validation_evidence(bundle_root, manifest)?;
    verify_shutdown_evidence(bundle_root, manifest)?;
    let oracle = read_artifact_text(bundle_root, manifest, ArtifactCategory::OracleSelfTests)?;
    if !oracle.contains("test result: ok") || oracle.contains("FAILED") {
        return retained_error(
            artifact_path(manifest, ArtifactCategory::OracleSelfTests)?,
            "oracle self-tests did not retain a passing result",
        );
    }
    let failure = read_artifact_text(bundle_root, manifest, ArtifactCategory::FailureLog)?;
    if !failure.contains("test result: ok") || failure.contains("FAILED") {
        return retained_error(
            artifact_path(manifest, ArtifactCategory::FailureLog)?,
            "failure-path qualification did not retain a passing result",
        );
    }
    Ok(())
}

fn verify_provenance_evidence(
    bundle_root: &Path,
    manifest: &BundleManifest,
) -> Result<(), EvidenceError> {
    let (path, evidence) = read_artifact_json::<ProvenanceEvidence>(
        bundle_root,
        manifest,
        ArtifactCategory::Provenance,
    )?;
    if evidence.repository_remote != manifest.provenance.remote
        || evidence.repository_revision != manifest.provenance.revision
        || evidence.executable_path != manifest.provenance.executable_path
        || evidence.executable_sha256 != manifest.provenance.executable_sha256
    {
        return retained_error(path, "provenance differs from the manifest");
    }
    Ok(())
}

fn verify_capability_evidence(
    bundle_root: &Path,
    manifest: &BundleManifest,
) -> Result<(), EvidenceError> {
    let (path, evidence) = read_artifact_json::<CapabilityEvidence>(
        bundle_root,
        manifest,
        ArtifactCategory::CapabilityFacts,
    )?;
    if evidence.device != manifest.conditions.device
        || evidence.driver != manifest.conditions.driver
        || evidence.vulkan_api_version != manifest.conditions.vulkan_api_version
        || evidence.correctness_validation != "enabled"
        || evidence.timing_validation != "disabled"
        || evidence.timing_present_mode != "IMMEDIATE"
        || evidence.selected_traversal != "dense_dda"
        || evidence.occupancy_attempted
    {
        return retained_error(
            path,
            "capability or measurement conditions differ from the manifest",
        );
    }
    Ok(())
}

fn verify_probe_evidence(
    bundle_root: &Path,
    manifest: &BundleManifest,
) -> Result<BTreeSet<String>, EvidenceError> {
    let (path, evidence) = read_artifact_json::<ProbeEvidenceFile>(
        bundle_root,
        manifest,
        ArtifactCategory::ProbeDefinition,
    )?;
    if evidence.schema_version != 1 || evidence.probes.is_empty() {
        return retained_error(path, "probe definitions are empty or use another schema");
    }
    let mut identities = BTreeSet::new();
    for probe in evidence.probes {
        if probe.kind != "probe_definition"
            || probe.probe_identity.is_empty()
            || !identities.insert(probe.probe_identity)
            || probe
                .origin
                .iter()
                .chain(probe.direction.iter())
                .any(|component| !component.is_finite())
            || probe.direction.iter().all(|component| *component == 0.0)
            || !probe.minimum_distance.is_finite()
            || !probe.maximum_distance.is_finite()
            || probe.minimum_distance < 0.0
            || probe.maximum_distance < probe.minimum_distance
        {
            return retained_error(path, "a probe definition is incomplete or invalid");
        }
    }
    Ok(identities)
}

fn verify_semantic_evidence(
    bundle_root: &Path,
    manifest: &BundleManifest,
    probes: &BTreeSet<String>,
) -> Result<Vec<SemanticSummaryRow>, EvidenceError> {
    let path = artifact_path(manifest, ArtifactCategory::SemanticObservations)?;
    let text = read_artifact_text(
        bundle_root,
        manifest,
        ArtifactCategory::SemanticObservations,
    )?;
    let mut compute_revisions = BTreeSet::new();
    let mut raster_revisions = BTreeSet::new();
    let mut summary_rows = Vec::new();
    for line in text.lines() {
        let observation =
            serde_json::from_str::<SemanticObservationEvidence>(line).map_err(|source| {
                EvidenceError::RetainedEvidence {
                    path: path.to_owned(),
                    reason: format!("could not parse observation: {source}"),
                }
            })?;
        let revision = observation.revision.parse::<u64>().map_err(|error| {
            EvidenceError::RetainedEvidence {
                path: path.to_owned(),
                reason: format!("invalid revision: {error}"),
            }
        })?;
        verify_semantic_result_shape(&observation.oracle, path)?;
        if observation.kind != "observation"
            || !observation.passed
            || !probes.contains(&observation.probe_identity)
            || observation.frame_sequence == 0
            || !matches!(revision, 1 | 4)
        {
            return retained_error(path, "an observation is unproved or lacks attribution");
        }
        match observation.render_path.as_str() {
            "compute_ray" => {
                let actual =
                    observation
                        .actual
                        .as_ref()
                        .ok_or_else(|| EvidenceError::RetainedEvidence {
                            path: path.to_owned(),
                            reason: "a compute observation has no actual result".to_owned(),
                        })?;
                verify_semantic_result_shape(actual, path)?;
                let tolerance = observation.distance_tolerance.ok_or_else(|| {
                    EvidenceError::RetainedEvidence {
                        path: path.to_owned(),
                        reason: "a compute observation has no distance tolerance".to_owned(),
                    }
                })?;
                if !semantic_results_agree(actual, &observation.oracle, tolerance) {
                    return retained_error(path, "compute observation disagrees with the oracle");
                }
                compute_revisions.insert(revision);
            }
            "raster" => {
                if observation.actual.is_some()
                    || !raster_correspondence_agrees(
                        observation.correspondence.as_deref(),
                        &observation.oracle,
                    )
                {
                    return retained_error(path, "raster correspondence disagrees with the oracle");
                }
                raster_revisions.insert(revision);
            }
            _ => return retained_error(path, "an observation names an unknown Render Path"),
        }
        let result = observation.actual.as_ref().map_or_else(
            || observation.oracle.result.clone(),
            |actual| actual.result.clone(),
        );
        summary_rows.push(SemanticSummaryRow {
            render_path: observation.render_path,
            probe_identity: observation.probe_identity,
            revision: observation.revision,
            frame_sequence: observation.frame_sequence,
            result,
            correspondence: observation.correspondence,
            passed: observation.passed,
        });
    }
    let expected_revisions = BTreeSet::from([1_u64, 4_u64]);
    if compute_revisions != expected_revisions || raster_revisions != expected_revisions {
        return retained_error(
            path,
            "semantic evidence does not cover both revisions and paths",
        );
    }
    Ok(summary_rows)
}

fn verify_semantic_result_shape(
    result: &SemanticResultEvidence,
    path: &str,
) -> Result<(), EvidenceError> {
    match result.result.as_str() {
        "miss" => {
            if result.volume_identity.is_some()
                || result.coordinate.is_some()
                || result.material_identity.is_some()
                || result.distance.is_some()
                || result.classification.is_some()
                || result.outward_normal.is_some()
            {
                return retained_error(path, "a miss contains contact fields");
            }
        }
        "contact" => {
            if result
                .volume_identity
                .as_ref()
                .is_none_or(|value| value.is_empty())
                || result.coordinate.is_none()
                || result
                    .material_identity
                    .as_ref()
                    .is_none_or(|value| value.is_empty())
                || result.distance.is_none_or(|value| !value.is_finite())
            {
                return retained_error(path, "a contact is missing semantic fields");
            }
            match result.classification.as_deref() {
                Some("entered") if result.outward_normal.is_some() => {}
                Some("started_inside") if result.outward_normal.is_none() => {}
                _ => return retained_error(path, "contact classification and normal disagree"),
            }
        }
        _ => return retained_error(path, "semantic result is neither hit nor miss"),
    }
    Ok(())
}

fn semantic_results_agree(
    actual: &SemanticResultEvidence,
    oracle: &SemanticResultEvidence,
    tolerance: f64,
) -> bool {
    if !tolerance.is_finite() || tolerance < 0.0 {
        return false;
    }
    actual.result == oracle.result
        && actual.volume_identity == oracle.volume_identity
        && actual.coordinate == oracle.coordinate
        && actual.material_identity == oracle.material_identity
        && actual.classification == oracle.classification
        && actual.outward_normal == oracle.outward_normal
        && match (actual.distance, oracle.distance) {
            (Some(actual), Some(oracle)) => (actual - oracle).abs() <= tolerance,
            (None, None) => true,
            _ => false,
        }
}

fn raster_correspondence_agrees(
    correspondence: Option<&str>,
    oracle: &SemanticResultEvidence,
) -> bool {
    match (
        oracle.result.as_str(),
        oracle.classification.as_deref(),
        correspondence,
    ) {
        ("miss", None, Some("NotApplicableMiss")) => true,
        ("contact", Some("started_inside"), Some("NotApplicableStartedInside")) => true,
        ("contact", Some("entered"), Some(value)) => value.starts_with("Matched("),
        _ => false,
    }
}

fn verify_semantic_summary(
    bundle_root: &Path,
    manifest: &BundleManifest,
    observations: &[SemanticSummaryRow],
) -> Result<(), EvidenceError> {
    let (path, summary) = read_artifact_json::<SemanticSummaryEvidence>(
        bundle_root,
        manifest,
        ArtifactCategory::SemanticSummary,
    )?;
    if !summary.oracle_self_tests_passed
        || !summary.compute_observations_passed
        || !summary.raster_correspondence_passed
        || summary.observation_count != observations.len()
        || summary.table != observations
        || summary.mismatches != 0
        || summary.started_inside_normals != 0
    {
        return retained_error(path, "semantic summary disagrees with raw observations");
    }
    Ok(())
}

fn verify_scene_and_camera_evidence(
    bundle_root: &Path,
    manifest: &BundleManifest,
) -> Result<(), EvidenceError> {
    let scene_path = artifact_path(manifest, ArtifactCategory::SceneDefinition)?;
    let scene = read_artifact_text(bundle_root, manifest, ArtifactCategory::SceneDefinition)?;
    if scene
        .lines()
        .filter(|line| line.starts_with("Canonical scene: "))
        .count()
        != 1
    {
        return retained_error(
            scene_path,
            "canonical scene attribution is missing or ambiguous",
        );
    }
    let camera_path = artifact_path(manifest, ArtifactCategory::CameraDefinition)?;
    let camera = read_artifact_text(bundle_root, manifest, ArtifactCategory::CameraDefinition)?;
    if !camera.contains("Canonical camera:")
        || !camera.contains("Compute lifecycle qualification published")
    {
        return retained_error(
            camera_path,
            "initial and lifecycle camera definitions were not retained",
        );
    }
    Ok(())
}

fn verify_timeline_evidence(
    bundle_root: &Path,
    manifest: &BundleManifest,
) -> Result<(), EvidenceError> {
    let (path, timeline) = read_artifact_json::<Vec<TimelineEvidence>>(
        bundle_root,
        manifest,
        ArtifactCategory::EventTimeline,
    )?;
    let expected = [
        "raster_revision_1",
        "compute_revision_1",
        "edit_burst_requested",
        "compute_required_4_visible_1",
        "compute_revision_4",
        "raster_revision_4",
        "compute_revision_4_final",
        "clean_close",
    ];
    if timeline.len() != expected.len()
        || timeline
            .iter()
            .map(|event| event.event.as_str())
            .ne(expected)
        || timeline.iter().any(|event| {
            !event.elapsed_seconds.is_finite()
                || event.elapsed_seconds < 0.0
                || event.window_title.is_empty()
        })
        || timeline
            .windows(2)
            .any(|events| events[1].elapsed_seconds < events[0].elapsed_seconds)
        || !timeline[3].window_title.contains("Required=4 Visible=1")
        || timeline[4..7]
            .iter()
            .any(|event| !event.window_title.contains("Required=4 Visible=4"))
    {
        return retained_error(path, "event order or revision attribution changed");
    }
    Ok(())
}

fn verify_lifecycle_and_resource_evidence(
    bundle_root: &Path,
    manifest: &BundleManifest,
) -> Result<(), EvidenceError> {
    let lifecycle = read_artifact_text(bundle_root, manifest, ArtifactCategory::LifecycleLog)?;
    let resource = read_artifact_text(bundle_root, manifest, ArtifactCategory::ResourceLedger)?;
    for required in [
        "Compute edit burst converged newest-only: Required=4 Visible=4",
        "obsolete_presented_frames=0 obsolete_semantic_observations=0",
        "Render Path round trip complete: raster-to-compute-to-raster-to-compute switches=3 closing_presenter=ComputeRay",
        "Render Path-owned raster resources after shutdown: 0",
        "Render Path-owned compute resources after shutdown: objects=0 allocations=0 workers=0 views=0",
        "Render Path switching resources after shutdown: replacement=0 retiring=0",
    ] {
        if !lifecycle.contains(required) {
            return retained_error(
                artifact_path(manifest, ArtifactCategory::LifecycleLog)?,
                "lifecycle sequence or final ownership evidence is missing",
            );
        }
    }
    for required in [
        "Compute resource observation:",
        "Render Path-owned raster resources after shutdown: 0",
        "Render Path-owned compute resources after shutdown: objects=0 allocations=0 workers=0 views=0",
        "Render Path switching resources after shutdown: replacement=0 retiring=0",
    ] {
        if !resource.contains(required) {
            return retained_error(
                artifact_path(manifest, ArtifactCategory::ResourceLedger)?,
                "resource evidence does not finish at zero ownership",
            );
        }
    }
    Ok(())
}

fn verify_timing_evidence(
    bundle_root: &Path,
    manifest: &BundleManifest,
) -> Result<(), EvidenceError> {
    let path = artifact_path(manifest, ArtifactCategory::TimingStream)?;
    let timing = read_artifact_text(bundle_root, manifest, ArtifactCategory::TimingStream)?;
    for phase in [
        "Preparation",
        "Upload",
        "Installation",
        "Dispatch",
        "Composite",
    ] {
        if !timing.contains(&format!("Compute timing event: phase={phase}")) {
            return retained_error(path, "compute timing phases are incomplete");
        }
    }
    for phase in ["Switching", "Presentation"] {
        if !timing.contains(&format!("Render Path timing event: phase={phase}")) {
            return retained_error(path, "Render Path timing phases are incomplete");
        }
    }
    let mut elapsed_count = 0usize;
    for line in timing.lines() {
        let Some((_, value)) = line.split_once("elapsed_ms=") else {
            continue;
        };
        let value =
            value
                .split_whitespace()
                .next()
                .ok_or_else(|| EvidenceError::RetainedEvidence {
                    path: path.to_owned(),
                    reason: "a timing sample has no elapsed value".to_owned(),
                })?;
        let elapsed = value
            .parse::<f64>()
            .map_err(|error| EvidenceError::RetainedEvidence {
                path: path.to_owned(),
                reason: format!("a timing sample is not numeric: {error}"),
            })?;
        if !elapsed.is_finite() || elapsed < 0.0 {
            return retained_error(path, "a timing sample is negative or non-finite");
        }
        elapsed_count =
            elapsed_count
                .checked_add(1)
                .ok_or_else(|| EvidenceError::RetainedEvidence {
                    path: path.to_owned(),
                    reason: "timing sample count overflowed".to_owned(),
                })?;
    }
    if elapsed_count < 7 {
        return retained_error(path, "timing stream has too few attributed samples");
    }
    Ok(())
}

fn verify_validation_evidence(
    bundle_root: &Path,
    manifest: &BundleManifest,
) -> Result<(), EvidenceError> {
    let (path, validation) = read_artifact_json::<ValidationEvidence>(
        bundle_root,
        manifest,
        ArtifactCategory::ValidationLog,
    )?;
    if !validation.enabled || validation.warnings != 0 || validation.errors != 0 {
        return retained_error(path, "validation was disabled or contains findings");
    }
    Ok(())
}

fn verify_shutdown_evidence(
    bundle_root: &Path,
    manifest: &BundleManifest,
) -> Result<(), EvidenceError> {
    let (path, shutdown) = read_artifact_json::<ShutdownEvidence>(
        bundle_root,
        manifest,
        ArtifactCategory::ShutdownLog,
    )?;
    if shutdown.cases.is_empty()
        || shutdown
            .cases
            .iter()
            .any(|case| case.validation_warnings != 0 || case.validation_errors != 0)
    {
        return retained_error(path, "shutdown qualification contains validation findings");
    }
    Ok(())
}

fn artifact_path(
    manifest: &BundleManifest,
    category: ArtifactCategory,
) -> Result<&str, EvidenceError> {
    manifest
        .artifacts
        .iter()
        .find(|artifact| artifact.category == category)
        .map(|artifact| artifact.path.as_str())
        .ok_or(EvidenceError::InvalidArtifactCount {
            category,
            expected: 1,
            actual: 0,
        })
}

fn read_artifact_text(
    bundle_root: &Path,
    manifest: &BundleManifest,
    category: ArtifactCategory,
) -> Result<String, EvidenceError> {
    let path = artifact_path(manifest, category)?;
    fs::read_to_string(bundle_root.join(path)).map_err(|source| EvidenceError::ArtifactRead {
        path: path.to_owned(),
        source,
    })
}

fn read_artifact_json<'a, T: for<'de> Deserialize<'de>>(
    bundle_root: &Path,
    manifest: &'a BundleManifest,
    category: ArtifactCategory,
) -> Result<(&'a str, T), EvidenceError> {
    let path = artifact_path(manifest, category)?;
    let text = read_artifact_text(bundle_root, manifest, category)?;
    let value = serde_json::from_str(&text).map_err(|source| EvidenceError::RetainedEvidence {
        path: path.to_owned(),
        reason: format!("could not parse JSON: {source}"),
    })?;
    Ok((path, value))
}

fn retained_error<T>(path: &str, reason: &str) -> Result<T, EvidenceError> {
    Err(EvidenceError::RetainedEvidence {
        path: path.to_owned(),
        reason: reason.to_owned(),
    })
}

fn is_lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn validate_artifact_path(path: &str) -> Result<(), EvidenceError> {
    let value = Path::new(path);
    if path.is_empty()
        || value.is_absolute()
        || value.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(EvidenceError::InvalidArtifactPath {
            path: path.to_owned(),
        });
    }
    Ok(())
}

fn has_regression(revisions: &[u64]) -> bool {
    revisions.windows(2).any(|pair| pair[1] < pair[0])
}

fn required_artifact_counts() -> [(ArtifactCategory, usize); 20] {
    [
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
    ]
}

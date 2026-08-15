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
    Ok(summary)
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

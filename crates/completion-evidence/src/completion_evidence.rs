use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::{Component, Path};
use thiserror::Error;

pub const WINDOWS_DEVELOPMENT_MACHINE_SCOPE: &str =
    "Runtime execution proven only on this recorded Windows development machine.";

pub const REQUIRED_VIDEO_EVENTS: &[&str] = &[
    "worker_paused",
    "landscape_resize_while_paused",
    "portrait_resize_while_paused",
    "minimized_while_paused",
    "restored_while_paused",
    "first_matching_revision_frame",
    "fixed_pose_overview",
    "fixed_pose_cavity",
    "fixed_pose_boundary",
    "deterministic_camera_move_started",
    "deterministic_camera_move_completed",
    "clean_close",
];

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReproductionCategory {
    GeneratedArtifacts,
    Formatting,
    Lint,
    UnitAndIntegration,
    VoxelFrontendRead,
    DiagnosticSurface,
    Lifecycle,
    DeterministicFailure,
    PrerequisiteRegression,
    BundleVerification,
}

const REQUIRED_REPRODUCTION_CATEGORIES: &[ReproductionCategory] = &[
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
];

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ReproductionCommand {
    pub category: ReproductionCategory,
    pub command: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactCategory {
    ContinuousVideo,
    VideoEventTimeline,
    FixedPosePng,
    TolerantComparison,
    SemanticFaceReport,
    ValidationLog,
    DerivationFailureLog,
    UploadFailureLog,
    PrerequisiteLog,
    TimingManifest,
    FirstCorrectFrameStream,
    SteadyCpuGpuStream,
    TimingSummary,
    GeometryResourceCounts,
    ComparisonChart,
    CleanCheckoutLog,
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

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct VideoMetadata {
    pub path: String,
    pub capture_scope: String,
    pub duration_seconds: f64,
    pub codec: String,
    pub pixel_format: String,
    pub width: u32,
    pub height: u32,
    pub average_frame_rate: String,
    pub validation_warnings: u32,
    pub validation_errors: u32,
    pub uninterrupted: bool,
    pub events: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct CompletionBundleManifest {
    pub schema_version: u32,
    pub scope: String,
    pub repository_revision: String,
    pub reproduction_commands: Vec<ReproductionCommand>,
    pub video: VideoMetadata,
    pub artifacts: Vec<ArtifactRecord>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct VerificationSummary {
    pub artifacts: usize,
    pub fixed_pose_pngs: usize,
    pub first_correct_frame_streams: usize,
    pub steady_cpu_gpu_streams: usize,
}

#[derive(Debug, Error)]
pub enum EvidenceError {
    #[error("completion evidence schema version {actual} is unsupported; expected 1")]
    UnsupportedSchema { actual: u32 },
    #[error(
        "runtime scope must be exactly limited to the recorded Windows development machine: {actual:?}"
    )]
    InvalidScope { actual: String },
    #[error("repository revision must be a full forty-character lowercase hexadecimal commit")]
    InvalidRepositoryRevision,
    #[error("required reproduction category {category:?} is missing or duplicated")]
    InvalidReproductionCategory { category: ReproductionCategory },
    #[error("reproduction command for {category:?} must not be empty")]
    EmptyReproductionCommand { category: ReproductionCategory },
    #[error("artifact category {category:?} has {actual} entries; expected {expected}")]
    InvalidArtifactCount {
        category: ArtifactCategory,
        expected: usize,
        actual: usize,
    },
    #[error("completion video metadata is invalid: {reason}")]
    InvalidVideo { reason: String },
    #[error("completion video event {event} has {actual} occurrences; expected exactly 1")]
    InvalidVideoEventCount { event: String, actual: usize },
    #[error("artifact path is unsafe or empty: {path:?}")]
    UnsafeArtifactPath { path: String },
    #[error("artifact path occurs more than once: {path}")]
    DuplicateArtifactPath { path: String },
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
    #[error("could not parse completion manifest {path}: {source}")]
    ManifestParse {
        path: String,
        #[source]
        source: serde_json::Error,
    },
}

pub fn read_manifest(path: &Path) -> Result<CompletionBundleManifest, EvidenceError> {
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
    manifest: &CompletionBundleManifest,
) -> Result<VerificationSummary, EvidenceError> {
    if manifest.schema_version != 1 {
        return Err(EvidenceError::UnsupportedSchema {
            actual: manifest.schema_version,
        });
    }
    if manifest.scope != WINDOWS_DEVELOPMENT_MACHINE_SCOPE {
        return Err(EvidenceError::InvalidScope {
            actual: manifest.scope.clone(),
        });
    }
    if manifest.repository_revision.len() != 40
        || !manifest
            .repository_revision
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(EvidenceError::InvalidRepositoryRevision);
    }

    for required_category in REQUIRED_REPRODUCTION_CATEGORIES {
        let commands = manifest
            .reproduction_commands
            .iter()
            .filter(|command| command.category == *required_category)
            .collect::<Vec<_>>();
        if commands.len() != 1 {
            return Err(EvidenceError::InvalidReproductionCategory {
                category: *required_category,
            });
        }
        if commands[0].command.trim().is_empty() {
            return Err(EvidenceError::EmptyReproductionCommand {
                category: *required_category,
            });
        }
    }

    let mut artifact_counts = BTreeMap::new();
    let mut artifact_paths = BTreeSet::new();
    for artifact in &manifest.artifacts {
        validate_relative_path(&artifact.path)?;
        if !artifact_paths.insert(&artifact.path) {
            return Err(EvidenceError::DuplicateArtifactPath {
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

    let video_artifact = manifest
        .artifacts
        .iter()
        .find(|artifact| artifact.category == ArtifactCategory::ContinuousVideo)
        .ok_or(EvidenceError::InvalidArtifactCount {
            category: ArtifactCategory::ContinuousVideo,
            expected: 1,
            actual: 0,
        })?;
    if manifest.video.path != video_artifact.path
        || video_artifact.bytes == 0
        || !manifest.video.duration_seconds.is_finite()
        || manifest.video.duration_seconds <= 0.0
        || manifest.video.codec.trim().is_empty()
        || manifest.video.pixel_format.trim().is_empty()
        || manifest.video.width == 0
        || manifest.video.height == 0
        || manifest.video.average_frame_rate.trim().is_empty()
        || manifest.video.capture_scope.trim().is_empty()
        || manifest.video.validation_warnings != 0
        || manifest.video.validation_errors != 0
        || !manifest.video.uninterrupted
    {
        return Err(EvidenceError::InvalidVideo {
            reason:
                "video must be nonempty, decodable, uninterrupted, scoped, and validation-clean"
                    .to_owned(),
        });
    }
    for event in REQUIRED_VIDEO_EVENTS {
        let actual = manifest
            .video
            .events
            .iter()
            .filter(|actual| actual.as_str() == *event)
            .count();
        if actual != 1 {
            return Err(EvidenceError::InvalidVideoEventCount {
                event: (*event).to_owned(),
                actual,
            });
        }
    }

    Ok(VerificationSummary {
        artifacts: manifest.artifacts.len(),
        fixed_pose_pngs: artifact_counts
            .get(&ArtifactCategory::FixedPosePng)
            .copied()
            .unwrap_or_default(),
        first_correct_frame_streams: artifact_counts
            .get(&ArtifactCategory::FirstCorrectFrameStream)
            .copied()
            .unwrap_or_default(),
        steady_cpu_gpu_streams: artifact_counts
            .get(&ArtifactCategory::SteadyCpuGpuStream)
            .copied()
            .unwrap_or_default(),
    })
}

pub fn verify_hash_inventory(
    bundle_root: &Path,
    artifacts: &[ArtifactRecord],
) -> Result<(), EvidenceError> {
    let mut buffer = vec![0_u8; 64 * 1024];
    for artifact in artifacts {
        validate_relative_path(&artifact.path)?;
        let artifact_path = bundle_root.join(&artifact.path);
        let mut file =
            fs::File::open(&artifact_path).map_err(|source| EvidenceError::ArtifactRead {
                path: artifact.path.clone(),
                source,
            })?;
        let metadata = file
            .metadata()
            .map_err(|source| EvidenceError::ArtifactRead {
                path: artifact.path.clone(),
                source,
            })?;
        if metadata.len() != artifact.bytes {
            return Err(EvidenceError::ArtifactSize {
                path: artifact.path.clone(),
                expected: artifact.bytes,
                actual: metadata.len(),
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
        let actual_hash = format!("{:x}", hash.finalize());
        if actual_hash != artifact.sha256 {
            return Err(EvidenceError::ArtifactHash {
                path: artifact.path.clone(),
            });
        }
    }
    Ok(())
}

pub fn verify_bundle(
    bundle_root: &Path,
    manifest: &CompletionBundleManifest,
) -> Result<VerificationSummary, EvidenceError> {
    let summary = verify_manifest_contract(manifest)?;
    verify_hash_inventory(bundle_root, &manifest.artifacts)?;
    Ok(summary)
}

fn validate_relative_path(path: &str) -> Result<(), EvidenceError> {
    let path_value = Path::new(path);
    if path.is_empty()
        || path_value.is_absolute()
        || path_value.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(EvidenceError::UnsafeArtifactPath {
            path: path.to_owned(),
        });
    }
    Ok(())
}

fn required_artifact_counts() -> [(ArtifactCategory, usize); 17] {
    [
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
    ]
}

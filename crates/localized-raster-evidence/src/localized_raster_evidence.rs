use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs;
use std::io::Read;
use std::path::{Component, Path};
use thiserror::Error;

pub const WINDOWS_DEVELOPMENT_MACHINE_SCOPE: &str =
    "Descriptive retained evidence for this recorded Windows development machine only.";
pub const REPOSITORY_REMOTE: &str = "https://github.com/PetrSeifert/voxel-nexus.git";

const REPRODUCTION_COMMANDS: &[&str] = &[
    "pwsh -NoProfile -File scripts/verify-edit-burst-demo.ps1 -EvidenceDirectory artifacts/edit-burst-issue-46",
    "pwsh -NoProfile -File scripts/qualify-raster-region-extents.ps1 -EvidenceDirectory artifacts/raster-region-extent-selection-issue-47",
    "pwsh -NoProfile -File scripts/characterize-raster-region-scales.ps1 -EvidenceDirectory artifacts/raster-region-scale-characterization-issue-48 -SelectionManifest artifacts/raster-region-extent-selection-issue-47/manifest.json",
    "pwsh -NoProfile -File scripts/assemble-localized-raster-evidence.ps1",
];

const VERIFICATION_COMMAND: &str = "cargo run --locked --package localized-raster-evidence --bin verify-localized-raster-evidence -- docs/evidence/localized-editable-raster/v1/development-machine";

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactCategory {
    UninterruptedDemo,
    RepresentativeFrame,
    SemanticLocalizationReport,
    OrchestrationTimeline,
    FailureShutdownLog,
    RawMeasurement,
    ComparisonChart,
    ValidationOutput,
    ReproductionInstructions,
    SupportingEvidence,
}

const REQUIRED_ARTIFACT_CATEGORIES: &[ArtifactCategory] = &[
    ArtifactCategory::UninterruptedDemo,
    ArtifactCategory::RepresentativeFrame,
    ArtifactCategory::SemanticLocalizationReport,
    ArtifactCategory::OrchestrationTimeline,
    ArtifactCategory::FailureShutdownLog,
    ArtifactCategory::RawMeasurement,
    ArtifactCategory::ComparisonChart,
    ArtifactCategory::ValidationOutput,
    ArtifactCategory::ReproductionInstructions,
];

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ArtifactRecord {
    pub category: ArtifactCategory,
    pub path: String,
    pub sha256: String,
    pub bytes: u64,
    pub process_exit_code: i32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EnvironmentContext {
    pub operating_system: String,
    pub processor: String,
    pub graphics_device: String,
    pub graphics_driver: String,
    pub rustc: String,
    pub cargo: String,
    pub powershell: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RepositoryContext {
    pub remote: String,
    pub assembly_revision: String,
    pub demo_revision: String,
    pub extent_revision: String,
    pub scale_revision: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SceneInput {
    pub name: String,
    pub generator: String,
    pub generator_version: u32,
    pub scale: u32,
    pub dimensions: [u32; 3],
    pub camera: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CommandInput {
    pub order: u32,
    pub coordinate: [i32; 3],
    pub old: String,
    pub requested: String,
    pub published_revision: u64,
}

impl CommandInput {
    pub fn new(order: u32, coordinate: [i32; 3], published_revision: u64) -> Self {
        Self {
            order,
            coordinate,
            old: "empty".to_owned(),
            requested: "occupied:canonical-warm".to_owned(),
            published_revision,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RevisionOutcomes {
    pub initial: u64,
    pub installed: u64,
    pub expected_final: u64,
    pub required_final: u64,
    pub visible_final: u64,
    pub intermediate_revision_visible: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SelectionContext {
    pub candidates: [u32; 3],
    pub selected_extent: [u32; 3],
    pub input_path: String,
    pub report_path: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BarrierOutcomes {
    pub obsolete_cpu_revision: u64,
    pub obsolete_cpu_cancelled: bool,
    pub superseded_post_upload_revision: u64,
    pub superseded_post_upload_rejected: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LifecycleOutcomes {
    pub passed: bool,
    pub active_cpu_shutdown_passed: bool,
    pub hidden_candidate_shutdown_passed: bool,
    pub owned_resources_after_shutdown: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SummaryOutcomes {
    pub semantic_correctness: bool,
    pub localization: bool,
    pub failure_retry: bool,
    pub lifecycle: LifecycleOutcomes,
    pub validation_warnings: u32,
    pub validation_errors: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BundleManifest {
    pub schema_version: u32,
    pub scope: String,
    pub environment: EnvironmentContext,
    pub repository: RepositoryContext,
    pub scene: SceneInput,
    pub commands: Vec<CommandInput>,
    pub revisions: RevisionOutcomes,
    pub selection: SelectionContext,
    pub barriers: BarrierOutcomes,
    pub outcomes: SummaryOutcomes,
    pub reproduction_commands: Vec<String>,
    pub verification_command: String,
    pub artifacts: Vec<ArtifactRecord>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct VerificationSummary {
    pub artifacts: usize,
    pub selected_extent: [u32; 3],
}

#[derive(Debug, Error)]
pub enum EvidenceError {
    #[error("localized raster evidence schema version {actual} is unsupported; expected 1")]
    UnsupportedSchema { actual: u32 },
    #[error("bundle scope is not limited to the recorded Windows development machine")]
    InvalidScope,
    #[error("environment field {field} must not be empty")]
    EmptyEnvironment { field: &'static str },
    #[error("repository {field} revision must be a full lowercase hexadecimal commit")]
    InvalidRepositoryRevision { field: &'static str },
    #[error("repository remote or source revision relationship is inconsistent")]
    InvalidRepositoryContext,
    #[error("canonical scene inputs are inconsistent")]
    InvalidScene,
    #[error("command sequence or published revision is inconsistent")]
    InvalidCommands,
    #[error("Voxel Scene revisions are inconsistent")]
    InvalidRevisions,
    #[error("Raster Region extent selection inputs are inconsistent")]
    InvalidSelection,
    #[error("barrier outcomes are inconsistent with revisions or did not pass")]
    InvalidBarriers,
    #[error("semantic correctness summary failed")]
    FailedSemanticCorrectness,
    #[error("localization summary failed")]
    FailedLocalization,
    #[error("failure/retry summary failed")]
    FailedFailureRetry,
    #[error("lifecycle or shutdown summary failed")]
    FailedLifecycle,
    #[error("resource balance failed: {actual} Render Path-owned resources remain")]
    UnbalancedResources { actual: u64 },
    #[error("validation reported {warnings} warnings and {errors} errors")]
    ValidationFindings { warnings: u32, errors: u32 },
    #[error("reproduction or verification commands are inconsistent")]
    InvalidReproductionCommands,
    #[error("required artifact category {category:?} is missing")]
    MissingArtifactCategory { category: ArtifactCategory },
    #[error("artifact inventory {inventory} has {actual} entries; expected {expected}")]
    InvalidInventoryCount {
        inventory: &'static str,
        expected: usize,
        actual: usize,
    },
    #[error("artifact path is unsafe, empty, or duplicated: {path:?}")]
    InvalidArtifactPath { path: String },
    #[error("artifact {path} has an invalid SHA-256 value")]
    InvalidArtifactHash { path: String },
    #[error("artifact {path} records unsuccessful process exit {exit_code}")]
    FailedArtifactProcess { path: String, exit_code: i32 },
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
    #[error("could not parse manifest {path}: {source}")]
    ManifestParse {
        path: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("retained source evidence {path} is inconsistent: {reason}")]
    SourceEvidence { path: String, reason: String },
}

pub fn verify_hash_inventory(
    bundle_root: &Path,
    artifacts: &[ArtifactRecord],
) -> Result<(), EvidenceError> {
    let mut buffer = vec![0_u8; 64 * 1024];
    for artifact in artifacts {
        let mut paths = BTreeSet::new();
        validate_artifact(artifact, &mut paths)?;
        let artifact_path = bundle_root.join(&artifact.path);
        let mut file =
            fs::File::open(&artifact_path).map_err(|source| EvidenceError::ArtifactRead {
                path: artifact.path.clone(),
                source,
            })?;
        let actual_bytes = file
            .metadata()
            .map_err(|source| EvidenceError::ArtifactRead {
                path: artifact.path.clone(),
                source,
            })?
            .len();
        if actual_bytes != artifact.bytes {
            return Err(EvidenceError::ArtifactSize {
                path: artifact.path.clone(),
                expected: artifact.bytes,
                actual: actual_bytes,
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

pub fn verify_bundle(
    bundle_root: &Path,
    manifest: &BundleManifest,
) -> Result<VerificationSummary, EvidenceError> {
    let summary = verify_manifest_contract(manifest)?;
    verify_hash_inventory(bundle_root, &manifest.artifacts)?;
    verify_retained_sources(bundle_root, manifest)?;
    Ok(summary)
}

fn verify_retained_sources(
    bundle_root: &Path,
    manifest: &BundleManifest,
) -> Result<(), EvidenceError> {
    for artifact in &manifest.artifacts {
        if artifact.category == ArtifactCategory::ValidationOutput && artifact.bytes != 0 {
            return source_error(&artifact.path, "validation output is not empty");
        }
        if artifact.category == ArtifactCategory::OrchestrationTimeline {
            let contents = read_source_text(bundle_root, &artifact.path)?;
            if !contents.contains("Render Path-owned raster resources after shutdown: 0") {
                return source_error(
                    &artifact.path,
                    "shutdown log does not report zero Render Path-owned resources",
                );
            }
        }
    }

    let demo_path = "demo/manifest.json";
    let demo = read_source_json(bundle_root, demo_path)?;
    verify_source_run(
        demo_path,
        &demo,
        manifest,
        &manifest.repository.demo_revision,
        32,
        true,
        false,
    )?;

    verify_extent_selection(bundle_root, manifest)?;
    verify_scale_characterization(bundle_root, manifest)?;

    for artifact in &manifest.artifacts {
        let is_nested_run = artifact.path.ends_with("/manifest.json")
            && (artifact.path.contains("/qualification/") || artifact.path.contains("/sample-"));
        if !is_nested_run {
            continue;
        }
        let source = read_source_json(bundle_root, &artifact.path)?;
        let is_extent = artifact.path.starts_with("extent-selection/");
        let expected_revision = if is_extent {
            &manifest.repository.extent_revision
        } else {
            &manifest.repository.scale_revision
        };
        let expected_extent = if is_extent {
            extent_from_path(&artifact.path)?
        } else {
            manifest.selection.selected_extent[0]
        };
        let require_full_lifecycle = artifact.path.contains("/qualification/");
        verify_source_run(
            &artifact.path,
            &source,
            manifest,
            expected_revision,
            expected_extent,
            false,
            require_full_lifecycle,
        )?;
    }
    Ok(())
}

fn verify_extent_selection(
    bundle_root: &Path,
    manifest: &BundleManifest,
) -> Result<(), EvidenceError> {
    let path = "extent-selection/manifest.json";
    let source = read_source_json(bundle_root, path)?;
    verify_environment_context(&source, path, manifest)?;
    require_string(&source, "/repository_revision", path, "repository revision")?
        .eq(&manifest.repository.extent_revision)
        .then_some(())
        .ok_or_else(|| EvidenceError::SourceEvidence {
            path: path.to_owned(),
            reason: "repository revision differs from the bundle context".to_owned(),
        })?;
    verify_u32_triplet(
        &source,
        "/selected_extent",
        manifest.selection.selected_extent,
        path,
        "selected extent",
    )?;
    verify_extent_canonical_input(&source, path, manifest)?;

    let candidate_runs = require_array(&source, "/candidate_runs", path, "candidate runs")?;
    if candidate_runs.len() != manifest.selection.candidates.len() {
        return source_error(path, "candidate count differs from the selection inputs");
    }
    for (index, candidate) in candidate_runs.iter().enumerate() {
        let expected_extent = manifest.selection.candidates[index];
        verify_u32_triplet(
            candidate,
            "/extent",
            [expected_extent; 3],
            path,
            "candidate extent",
        )?;
        let gates = require_object(candidate, "/gate_outcomes", path, "gate outcomes")?;
        for (gate_name, gate) in gates {
            if !require_bool(gate, "/passed", path, gate_name)? {
                return source_error(
                    path,
                    &format!("candidate {expected_extent} gate {gate_name} failed"),
                );
            }
        }
    }

    let selection_input_path = &manifest.selection.input_path;
    let selection_input = read_source_json(bundle_root, selection_input_path)?;
    let input_candidates = require_array(
        &selection_input,
        "/candidates",
        selection_input_path,
        "selection candidates",
    )?;
    verify_candidate_summaries(input_candidates, selection_input_path, manifest)?;

    let selection_report_path = &manifest.selection.report_path;
    let selection_report = read_source_json(bundle_root, selection_report_path)?;
    if require_u64(
        &selection_report,
        "/selected_extent",
        selection_report_path,
        "selected extent",
    )? != u64::from(manifest.selection.selected_extent[0])
    {
        return source_error(
            selection_report_path,
            "selection report chose a different extent",
        );
    }
    let report_candidates = require_array(
        &selection_report,
        "/candidates",
        selection_report_path,
        "selection candidates",
    )?;
    verify_candidate_summaries(report_candidates, selection_report_path, manifest)
}

fn verify_candidate_summaries(
    candidates: &[serde_json::Value],
    path: &str,
    manifest: &BundleManifest,
) -> Result<(), EvidenceError> {
    if candidates.len() != manifest.selection.candidates.len() {
        return source_error(path, "selection candidate count changed");
    }
    for (index, candidate) in candidates.iter().enumerate() {
        if require_u64(candidate, "/extent", path, "candidate extent")?
            != u64::from(manifest.selection.candidates[index])
        {
            return source_error(path, "selection candidate order or extent changed");
        }
        let qualification =
            require_object(candidate, "/qualification", path, "candidate qualification")?;
        for (gate, outcome) in qualification {
            if outcome.as_bool() != Some(true) {
                return source_error(path, &format!("candidate qualification {gate} failed"));
            }
        }
    }
    Ok(())
}

fn verify_extent_canonical_input(
    source: &serde_json::Value,
    path: &str,
    manifest: &BundleManifest,
) -> Result<(), EvidenceError> {
    if require_string(source, "/canonical_input/scene", path, "scene")? != manifest.scene.name
        || require_string(source, "/canonical_input/generator", path, "generator")?
            != manifest.scene.generator
        || require_u64(
            source,
            "/canonical_input/generator_version",
            path,
            "generator version",
        )? != u64::from(manifest.scene.generator_version)
        || require_string(source, "/canonical_input/camera", path, "camera")?
            != manifest.scene.camera
        || require_u64(
            source,
            "/canonical_input/initial_revision",
            path,
            "initial revision",
        )? != manifest.revisions.initial
        || require_u64(
            source,
            "/canonical_input/expected_final_revision",
            path,
            "expected final revision",
        )? != manifest.revisions.expected_final
    {
        return source_error(path, "canonical scene, camera, or revisions changed");
    }
    verify_u32_triplet(
        source,
        "/canonical_input/dimensions",
        manifest.scene.dimensions,
        path,
        "dimensions",
    )?;
    let commands = require_array(source, "/canonical_input/commands", path, "commands")?;
    if commands.len() != manifest.commands.len() {
        return source_error(path, "command count changed");
    }
    for (index, command) in commands.iter().enumerate() {
        let expected = &manifest.commands[index];
        if require_u64(command, "/order", path, "command order")? != u64::from(expected.order)
            || require_string(command, "/old", path, "old Voxel Value")? != expected.old
            || require_string(command, "/requested", path, "requested Voxel Value")?
                != expected.requested
        {
            return source_error(path, "command order or Voxel Values changed");
        }
        let coordinate = require_array(command, "/coordinate", path, "command coordinate")?;
        if coordinate
            .iter()
            .map(serde_json::Value::as_i64)
            .collect::<Option<Vec<_>>>()
            != Some(
                expected
                    .coordinate
                    .iter()
                    .map(|value| i64::from(*value))
                    .collect(),
            )
        {
            return source_error(path, "command coordinate changed");
        }
    }
    Ok(())
}

fn verify_environment_context(
    source: &serde_json::Value,
    path: &str,
    manifest: &BundleManifest,
) -> Result<(), EvidenceError> {
    let operating_system = format!(
        "{} {}",
        require_string(
            source,
            "/machine/operating_system/Caption",
            path,
            "operating system caption",
        )?,
        require_string(
            source,
            "/machine/operating_system/Version",
            path,
            "operating system version",
        )?
    );
    let processor = require_string(source, "/machine/processors/0/Name", path, "processor")?.trim();
    let video_controllers = require_array(
        source,
        "/machine/video_controllers",
        path,
        "video controllers",
    )?;
    let graphics_controller = video_controllers
        .iter()
        .find(|controller| {
            controller
                .pointer("/Name")
                .and_then(serde_json::Value::as_str)
                == Some(manifest.environment.graphics_device.as_str())
        })
        .ok_or_else(|| EvidenceError::SourceEvidence {
            path: path.to_owned(),
            reason: "recorded graphics device is absent from the machine context".to_owned(),
        })?;
    if operating_system != manifest.environment.operating_system
        || processor != manifest.environment.processor
        || require_string(
            graphics_controller,
            "/DriverVersion",
            path,
            "graphics driver",
        )? != manifest.environment.graphics_driver
        || require_string(source, "/machine/rustc", path, "rustc")? != manifest.environment.rustc
        || require_string(source, "/machine/cargo", path, "cargo")? != manifest.environment.cargo
        || require_string(source, "/machine/powershell", path, "PowerShell")?
            != manifest.environment.powershell
    {
        return source_error(path, "machine context differs from the bundle environment");
    }
    Ok(())
}

fn verify_scale_characterization(
    bundle_root: &Path,
    manifest: &BundleManifest,
) -> Result<(), EvidenceError> {
    let path = "scale-characterization/manifest.json";
    let source = read_source_json(bundle_root, path)?;
    if require_string(&source, "/repository_revision", path, "repository revision")?
        != manifest.repository.scale_revision
    {
        return source_error(path, "repository revision differs from the bundle context");
    }
    verify_u32_triplet(
        &source,
        "/selected_extent_source/selected_extent",
        manifest.selection.selected_extent,
        path,
        "selected extent",
    )?;
    let extent_manifest_record = manifest
        .artifacts
        .iter()
        .find(|artifact| artifact.path == "extent-selection/manifest.json")
        .ok_or_else(|| EvidenceError::SourceEvidence {
            path: path.to_owned(),
            reason: "extent manifest is absent from the artifact inventory".to_owned(),
        })?;
    let extent_report_record = manifest
        .artifacts
        .iter()
        .find(|artifact| artifact.path == manifest.selection.report_path)
        .ok_or_else(|| EvidenceError::SourceEvidence {
            path: path.to_owned(),
            reason: "selection report is absent from the artifact inventory".to_owned(),
        })?;
    if require_string(
        &source,
        "/selected_extent_source/manifest_sha256",
        path,
        "extent manifest hash",
    )? != extent_manifest_record.sha256
        || require_string(
            &source,
            "/selected_extent_source/report_sha256",
            path,
            "selection report hash",
        )? != extent_report_record.sha256
    {
        return source_error(path, "selected extent source hashes changed");
    }
    if require_u64(&source, "/sample_count_per_scale", path, "sample count")? != 5 {
        return source_error(path, "sample count per scale is not five");
    }
    let scales = require_array(&source, "/scales", path, "scales")?;
    verify_scale_list(scales, path)?;

    let raw_path = "scale-characterization/raw-distributions.json";
    let raw = read_source_json(bundle_root, raw_path)?;
    verify_u32_triplet(
        &raw,
        "/selected_extent",
        manifest.selection.selected_extent,
        raw_path,
        "selected extent",
    )?;
    if require_u64(&raw, "/sample_count_per_scale", raw_path, "sample count")? != 5 {
        return source_error(raw_path, "sample count per scale is not five");
    }
    let raw_scales = require_array(&raw, "/scales", raw_path, "scales")?;
    verify_scale_list(raw_scales, raw_path)?;
    for scale in raw_scales {
        let samples = require_array(scale, "/samples", raw_path, "samples")?;
        if samples.len() != 5 {
            return source_error(raw_path, "a scale does not retain five samples");
        }
        for sample in samples {
            let relative_manifest =
                require_string(sample, "/manifest", raw_path, "sample manifest")?;
            let retained_path = format!("scale-characterization/{relative_manifest}");
            if !manifest
                .artifacts
                .iter()
                .any(|artifact| artifact.path == retained_path)
            {
                return source_error(raw_path, "raw sample references an uninventoried manifest");
            }
        }
    }
    Ok(())
}

fn verify_scale_list(scales: &[serde_json::Value], path: &str) -> Result<(), EvidenceError> {
    let expected = [
        (64_u64, [64, 32, 64]),
        (128, [128, 64, 128]),
        (256, [256, 128, 256]),
    ];
    if scales.len() != expected.len() {
        return source_error(path, "scale count changed");
    }
    for (index, scale) in scales.iter().enumerate() {
        if require_u64(scale, "/scale", path, "scale")? != expected[index].0 {
            return source_error(path, "scale order or value changed");
        }
        verify_u32_triplet(scale, "/dimensions", expected[index].1, path, "dimensions")?;
    }
    Ok(())
}

fn verify_source_run(
    path: &str,
    source: &serde_json::Value,
    manifest: &BundleManifest,
    expected_revision: &str,
    expected_extent: u32,
    compare_top_level_commands: bool,
    require_full_lifecycle: bool,
) -> Result<(), EvidenceError> {
    if require_string(source, "/RepositoryRevision", path, "repository revision")?
        != expected_revision
    {
        return source_error(path, "repository revision differs from the bundle context");
    }
    if require_u64(source, "/ProcessExitCode", path, "process exit")? != 0 {
        return source_error(path, "required process did not exit successfully");
    }
    if require_u64(source, "/Validation/Warnings", path, "validation warnings")? != 0
        || require_u64(source, "/Validation/Errors", path, "validation errors")? != 0
    {
        return source_error(path, "validation findings are nonzero");
    }
    let scale = u32::try_from(require_u64(
        source,
        "/CanonicalInput/Scale",
        path,
        "scene scale",
    )?)
    .map_err(|_| EvidenceError::SourceEvidence {
        path: path.to_owned(),
        reason: "scene scale is out of range".to_owned(),
    })?;
    verify_canonical_input(
        source,
        "/CanonicalInput",
        path,
        manifest,
        scale,
        expected_extent,
        true,
    )?;
    verify_source_commands(source, path, manifest, scale, compare_top_level_commands)?;
    if require_u64(
        source,
        "/CpuBarrier/ObsoleteRevision",
        path,
        "CPU barrier revision",
    )? != manifest.barriers.obsolete_cpu_revision
        || !require_bool(source, "/CpuBarrier/Cancelled", path, "CPU cancellation")?
        || require_u64(
            source,
            "/PostUploadBarrier/SupersededRevision",
            path,
            "post-upload revision",
        )? != manifest.barriers.superseded_post_upload_revision
        || !require_bool(
            source,
            "/PostUploadBarrier/RejectedAtCommit",
            path,
            "post-upload rejection",
        )?
    {
        return source_error(path, "barrier outcome changed");
    }
    if require_bool(
        source,
        "/Visibility/IntermediateRevisionVisible",
        path,
        "intermediate visibility",
    )? {
        return source_error(path, "an intermediate Voxel Scene Revision became visible");
    }
    let final_title = require_string(source, "/Visibility/FinalTitle", path, "final title")?;
    if !final_title.contains("Required=4 Visible=4") {
        return source_error(
            path,
            "final required and visible revisions are inconsistent",
        );
    }

    if let Some(qualification) = source
        .pointer("/Qualification")
        .filter(|value| !value.is_null())
    {
        for (field, label) in [
            ("SemanticCorrectness", "semantic correctness"),
            ("Localization", "localization"),
            ("FailureRetry", "failure/retry"),
            ("Shutdown", "shutdown"),
            ("ResourceRetirement", "resource retirement"),
            ("Validation", "validation"),
        ] {
            if !require_bool(qualification, &format!("/{field}"), path, label)? {
                return source_error(path, &format!("{label} summary failed"));
            }
        }
        if require_full_lifecycle && !require_bool(qualification, "/Lifecycle", path, "lifecycle")?
        {
            return source_error(path, "lifecycle summary failed");
        }
    } else if require_full_lifecycle {
        return source_error(path, "qualification summary is missing");
    }

    if require_full_lifecycle {
        for outcome_name in ["ActiveCpuWork", "HiddenPostUploadCandidate"] {
            let prefix = format!("/ShutdownQualification/{outcome_name}");
            if !require_bool(
                source,
                &format!("{prefix}/Passed"),
                path,
                "shutdown outcome",
            )? || require_u64(
                source,
                &format!("{prefix}/ProcessExitCode"),
                path,
                "shutdown exit",
            )? != 0
                || require_u64(
                    source,
                    &format!("{prefix}/OwnedResourcesAfterShutdown"),
                    path,
                    "shutdown resources",
                )? != 0
                || require_u64(
                    source,
                    &format!("{prefix}/ValidationWarnings"),
                    path,
                    "shutdown validation warnings",
                )? != 0
                || require_u64(
                    source,
                    &format!("{prefix}/ValidationErrors"),
                    path,
                    "shutdown validation errors",
                )? != 0
            {
                return source_error(path, &format!("shutdown outcome {outcome_name} failed"));
            }
        }
    }
    Ok(())
}

fn verify_canonical_input(
    source: &serde_json::Value,
    prefix: &str,
    path: &str,
    manifest: &BundleManifest,
    expected_scale: u32,
    expected_extent: u32,
    require_installation: bool,
) -> Result<(), EvidenceError> {
    if require_string(source, &format!("{prefix}/Scene"), path, "scene")? != manifest.scene.name
        || require_string(source, &format!("{prefix}/Generator"), path, "generator")?
            != manifest.scene.generator
        || require_u64(
            source,
            &format!("{prefix}/GeneratorVersion"),
            path,
            "generator version",
        )? != u64::from(manifest.scene.generator_version)
        || require_string(source, &format!("{prefix}/Camera"), path, "camera")?
            != manifest.scene.camera
    {
        return source_error(path, "canonical scene or camera input changed");
    }
    let dimensions = [expected_scale, expected_scale / 2, expected_scale];
    verify_u32_triplet(
        source,
        &format!("{prefix}/Dimensions"),
        dimensions,
        path,
        "dimensions",
    )?;
    verify_u32_triplet(
        source,
        &format!("{prefix}/RasterRegionExtent"),
        [expected_extent; 3],
        path,
        "Raster Region extent",
    )?;
    if require_installation
        && (require_u64(
            source,
            &format!("{prefix}/InitialRevision"),
            path,
            "initial revision",
        )? != manifest.revisions.initial
            || require_u64(
                source,
                &format!("{prefix}/InstalledRevision"),
                path,
                "installed revision",
            )? != manifest.revisions.installed
            || require_u64(
                source,
                &format!("{prefix}/ExpectedFinalRevision"),
                path,
                "expected final revision",
            )? != manifest.revisions.expected_final
            || !require_bool(
                source,
                &format!("{prefix}/InstalledComplete"),
                path,
                "complete installation",
            )?)
    {
        return source_error(path, "canonical Voxel Scene revisions changed");
    }
    Ok(())
}

fn verify_source_commands(
    source: &serde_json::Value,
    path: &str,
    manifest: &BundleManifest,
    scale: u32,
    compare_top_level_commands: bool,
) -> Result<(), EvidenceError> {
    let commands = require_array(source, "/Commands", path, "commands")?;
    if commands.len() != manifest.commands.len() {
        return source_error(path, "command count changed");
    }
    let mut previous_x = None;
    for (index, command) in commands.iter().enumerate() {
        let expected = &manifest.commands[index];
        if require_u64(command, "/Order", path, "command order")? != u64::from(expected.order)
            || require_u64(command, "/PublishedRevision", path, "published revision")?
                != expected.published_revision
            || require_string(command, "/Old", path, "old Voxel Value")? != expected.old
            || require_string(command, "/Requested", path, "requested Voxel Value")?
                != expected.requested
        {
            return source_error(path, "command order, values, or published revision changed");
        }
        let coordinate = require_array(command, "/Coordinate", path, "command coordinate")?;
        if coordinate.len() != 3
            || coordinate.get(1).and_then(serde_json::Value::as_i64) != Some(0)
            || coordinate.get(2).and_then(serde_json::Value::as_i64) != Some(0)
        {
            return source_error(path, "command coordinate is malformed");
        }
        let x = coordinate
            .first()
            .and_then(serde_json::Value::as_i64)
            .ok_or_else(|| EvidenceError::SourceEvidence {
                path: path.to_owned(),
                reason: "command x coordinate is missing".to_owned(),
            })?;
        if let Some(previous) = previous_x {
            if x <= previous {
                return source_error(path, "command coordinates are not strictly increasing");
            }
        } else if x != 0 {
            return source_error(path, "first command coordinate changed");
        }
        previous_x = Some(x);
        if (compare_top_level_commands || scale == manifest.scene.scale)
            && coordinate
                .iter()
                .map(|value| value.as_i64())
                .collect::<Option<Vec<_>>>()
                != Some(
                    expected
                        .coordinate
                        .iter()
                        .map(|value| i64::from(*value))
                        .collect(),
                )
        {
            return source_error(
                path,
                "command coordinates disagree with the bundle manifest",
            );
        }
    }
    Ok(())
}

fn extent_from_path(path: &str) -> Result<u32, EvidenceError> {
    path.split('/')
        .find_map(|component| {
            component
                .strip_prefix("extent-")
                .and_then(|extent| extent.parse().ok())
        })
        .ok_or_else(|| EvidenceError::SourceEvidence {
            path: path.to_owned(),
            reason: "could not determine the candidate extent from the artifact path".to_owned(),
        })
}

fn read_source_json(bundle_root: &Path, path: &str) -> Result<serde_json::Value, EvidenceError> {
    let contents = read_source_text(bundle_root, path)?;
    serde_json::from_str(&contents).map_err(|source| EvidenceError::ManifestParse {
        path: path.to_owned(),
        source,
    })
}

fn read_source_text(bundle_root: &Path, path: &str) -> Result<String, EvidenceError> {
    fs::read_to_string(bundle_root.join(path)).map_err(|source| EvidenceError::ArtifactRead {
        path: path.to_owned(),
        source,
    })
}

fn require_array<'a>(
    source: &'a serde_json::Value,
    pointer: &str,
    path: &str,
    label: &str,
) -> Result<&'a [serde_json::Value], EvidenceError> {
    source
        .pointer(pointer)
        .and_then(serde_json::Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| EvidenceError::SourceEvidence {
            path: path.to_owned(),
            reason: format!("{label} is missing or malformed"),
        })
}

fn require_object<'a>(
    source: &'a serde_json::Value,
    pointer: &str,
    path: &str,
    label: &str,
) -> Result<&'a serde_json::Map<String, serde_json::Value>, EvidenceError> {
    source
        .pointer(pointer)
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| EvidenceError::SourceEvidence {
            path: path.to_owned(),
            reason: format!("{label} is missing or malformed"),
        })
}

fn require_string<'a>(
    source: &'a serde_json::Value,
    pointer: &str,
    path: &str,
    label: &str,
) -> Result<&'a str, EvidenceError> {
    source
        .pointer(pointer)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| EvidenceError::SourceEvidence {
            path: path.to_owned(),
            reason: format!("{label} is missing or malformed"),
        })
}

fn require_u64(
    source: &serde_json::Value,
    pointer: &str,
    path: &str,
    label: &str,
) -> Result<u64, EvidenceError> {
    source
        .pointer(pointer)
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| EvidenceError::SourceEvidence {
            path: path.to_owned(),
            reason: format!("{label} is missing or malformed"),
        })
}

fn require_bool(
    source: &serde_json::Value,
    pointer: &str,
    path: &str,
    label: &str,
) -> Result<bool, EvidenceError> {
    source
        .pointer(pointer)
        .and_then(serde_json::Value::as_bool)
        .ok_or_else(|| EvidenceError::SourceEvidence {
            path: path.to_owned(),
            reason: format!("{label} is missing or malformed"),
        })
}

fn verify_u32_triplet(
    source: &serde_json::Value,
    pointer: &str,
    expected: [u32; 3],
    path: &str,
    label: &str,
) -> Result<(), EvidenceError> {
    let values = require_array(source, pointer, path, label)?;
    if values.len() != expected.len()
        || values
            .iter()
            .zip(expected)
            .any(|(actual, expected)| actual.as_u64() != Some(u64::from(expected)))
    {
        return source_error(path, &format!("{label} changed"));
    }
    Ok(())
}

fn source_error<T>(path: &str, reason: &str) -> Result<T, EvidenceError> {
    Err(EvidenceError::SourceEvidence {
        path: path.to_owned(),
        reason: reason.to_owned(),
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
    if manifest.scope != WINDOWS_DEVELOPMENT_MACHINE_SCOPE {
        return Err(EvidenceError::InvalidScope);
    }
    validate_environment(&manifest.environment)?;
    validate_repository(&manifest.repository)?;
    if manifest.scene.name != "canonical-dense-scene"
        || manifest.scene.generator != "voxel-nexus-canonical-dense"
        || manifest.scene.generator_version != 1
        || manifest.scene.scale != 256
        || manifest.scene.dimensions != [256, 128, 256]
        || manifest.scene.camera != "overview"
    {
        return Err(EvidenceError::InvalidScene);
    }
    validate_commands(&manifest.commands)?;
    if manifest.revisions.initial != 1
        || manifest.revisions.installed != 1
        || manifest.revisions.expected_final != 4
        || manifest.revisions.required_final != 4
        || manifest.revisions.visible_final != 4
        || manifest.revisions.intermediate_revision_visible
    {
        return Err(EvidenceError::InvalidRevisions);
    }
    if manifest.selection.candidates != [16, 32, 64]
        || manifest.selection.selected_extent != [16, 16, 16]
        || manifest.selection.input_path != "extent-selection/selection-input.json"
        || manifest.selection.report_path != "extent-selection/selection.json"
    {
        return Err(EvidenceError::InvalidSelection);
    }
    if manifest.barriers.obsolete_cpu_revision != 2
        || !manifest.barriers.obsolete_cpu_cancelled
        || manifest.barriers.superseded_post_upload_revision != 3
        || !manifest.barriers.superseded_post_upload_rejected
    {
        return Err(EvidenceError::InvalidBarriers);
    }
    validate_outcomes(&manifest.outcomes)?;
    if manifest
        .reproduction_commands
        .iter()
        .map(String::as_str)
        .ne(REPRODUCTION_COMMANDS.iter().copied())
        || manifest.verification_command != VERIFICATION_COMMAND
    {
        return Err(EvidenceError::InvalidReproductionCommands);
    }

    let mut artifact_paths = BTreeSet::new();
    for artifact in &manifest.artifacts {
        validate_artifact(artifact, &mut artifact_paths)?;
    }
    for category in REQUIRED_ARTIFACT_CATEGORIES {
        if !manifest
            .artifacts
            .iter()
            .any(|artifact| artifact.category == *category)
        {
            return Err(EvidenceError::MissingArtifactCategory {
                category: *category,
            });
        }
    }
    for (inventory, prefix, expected) in [
        ("demo", "demo/", 7_usize),
        ("extent selection", "extent-selection/", 84_usize),
        (
            "scale characterization",
            "scale-characterization/",
            48_usize,
        ),
    ] {
        let actual = manifest
            .artifacts
            .iter()
            .filter(|artifact| artifact.path.starts_with(prefix))
            .count();
        if actual != expected {
            return Err(EvidenceError::InvalidInventoryCount {
                inventory,
                expected,
                actual,
            });
        }
    }
    if manifest.artifacts.len() != 141 {
        return Err(EvidenceError::InvalidInventoryCount {
            inventory: "complete bundle",
            expected: 141,
            actual: manifest.artifacts.len(),
        });
    }

    Ok(VerificationSummary {
        artifacts: manifest.artifacts.len(),
        selected_extent: manifest.selection.selected_extent,
    })
}

fn validate_environment(environment: &EnvironmentContext) -> Result<(), EvidenceError> {
    for (field, value) in [
        ("operating_system", environment.operating_system.as_str()),
        ("processor", environment.processor.as_str()),
        ("graphics_device", environment.graphics_device.as_str()),
        ("graphics_driver", environment.graphics_driver.as_str()),
        ("rustc", environment.rustc.as_str()),
        ("cargo", environment.cargo.as_str()),
        ("powershell", environment.powershell.as_str()),
    ] {
        if value.trim().is_empty() {
            return Err(EvidenceError::EmptyEnvironment { field });
        }
    }
    Ok(())
}

fn validate_repository(repository: &RepositoryContext) -> Result<(), EvidenceError> {
    for (field, revision) in [
        ("assembly", repository.assembly_revision.as_str()),
        ("demo", repository.demo_revision.as_str()),
        ("extent", repository.extent_revision.as_str()),
        ("scale", repository.scale_revision.as_str()),
    ] {
        if revision.len() != 40
            || !revision
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(EvidenceError::InvalidRepositoryRevision { field });
        }
    }
    if repository.remote != REPOSITORY_REMOTE
        || repository.assembly_revision != repository.scale_revision
    {
        return Err(EvidenceError::InvalidRepositoryContext);
    }
    Ok(())
}

fn validate_commands(commands: &[CommandInput]) -> Result<(), EvidenceError> {
    let expected_coordinates = [[0, 0, 0], [40, 0, 0], [80, 0, 0]];
    if commands.len() != expected_coordinates.len() {
        return Err(EvidenceError::InvalidCommands);
    }
    for (index, command) in commands.iter().enumerate() {
        let expected_order =
            u32::try_from(index + 1).map_err(|_| EvidenceError::InvalidCommands)?;
        let expected_revision =
            u64::try_from(index + 2).map_err(|_| EvidenceError::InvalidCommands)?;
        if command.order != expected_order
            || command.coordinate != expected_coordinates[index]
            || command.old != "empty"
            || command.requested != "occupied:canonical-warm"
            || command.published_revision != expected_revision
        {
            return Err(EvidenceError::InvalidCommands);
        }
    }
    Ok(())
}

fn validate_outcomes(outcomes: &SummaryOutcomes) -> Result<(), EvidenceError> {
    if !outcomes.semantic_correctness {
        return Err(EvidenceError::FailedSemanticCorrectness);
    }
    if !outcomes.localization {
        return Err(EvidenceError::FailedLocalization);
    }
    if !outcomes.failure_retry {
        return Err(EvidenceError::FailedFailureRetry);
    }
    if !outcomes.lifecycle.passed
        || !outcomes.lifecycle.active_cpu_shutdown_passed
        || !outcomes.lifecycle.hidden_candidate_shutdown_passed
    {
        return Err(EvidenceError::FailedLifecycle);
    }
    if outcomes.lifecycle.owned_resources_after_shutdown != 0 {
        return Err(EvidenceError::UnbalancedResources {
            actual: outcomes.lifecycle.owned_resources_after_shutdown,
        });
    }
    if outcomes.validation_warnings != 0 || outcomes.validation_errors != 0 {
        return Err(EvidenceError::ValidationFindings {
            warnings: outcomes.validation_warnings,
            errors: outcomes.validation_errors,
        });
    }
    Ok(())
}

fn validate_artifact(
    artifact: &ArtifactRecord,
    artifact_paths: &mut BTreeSet<String>,
) -> Result<(), EvidenceError> {
    let path = Path::new(&artifact.path);
    if artifact.path.is_empty()
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
        || !artifact_paths.insert(artifact.path.clone())
    {
        return Err(EvidenceError::InvalidArtifactPath {
            path: artifact.path.clone(),
        });
    }
    if artifact.sha256.len() != 64
        || !artifact
            .sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(EvidenceError::InvalidArtifactHash {
            path: artifact.path.clone(),
        });
    }
    if artifact.process_exit_code != 0 {
        return Err(EvidenceError::FailedArtifactProcess {
            path: artifact.path.clone(),
            exit_code: artifact.process_exit_code,
        });
    }
    Ok(())
}

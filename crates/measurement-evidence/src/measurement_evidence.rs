use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct VoxelSceneRevisionIdentity(String);

impl VoxelSceneRevisionIdentity {
    pub fn new(identity: impl Into<String>) -> Result<Self, EvidenceError> {
        let identity = identity.into();
        if identity.is_empty() {
            return Err(EvidenceError::EmptyVoxelSceneRevisionIdentity);
        }
        Ok(Self(identity))
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum MeasurementEvent {
    SceneRevisionPublished {
        source_revision: VoxelSceneRevisionIdentity,
        #[serde(rename = "elapsed_ms")]
        elapsed_milliseconds: f64,
    },
    ArtifactDerived {
        source_revision: VoxelSceneRevisionIdentity,
        #[serde(rename = "elapsed_ms")]
        elapsed_milliseconds: f64,
        resources: ResourceCounts,
    },
    ArtifactInstalled {
        source_revision: VoxelSceneRevisionIdentity,
        #[serde(rename = "elapsed_ms")]
        elapsed_milliseconds: f64,
    },
    MatchingArtifactPresented {
        source_revision: VoxelSceneRevisionIdentity,
        #[serde(rename = "elapsed_ms")]
        elapsed_milliseconds: f64,
    },
    SteadyFrame {
        sequence: u64,
        #[serde(rename = "cpu_frame_ms")]
        cpu_frame_milliseconds: f64,
        #[serde(rename = "gpu_frame_ms")]
        gpu_frame_milliseconds: f64,
    },
}

impl MeasurementEvent {
    pub fn to_json_line(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResourceCounts {
    pub occupied_voxels: u64,
    pub exposed_quads: u64,
    pub vertices: u64,
    pub indices: u64,
    pub draw_calls: u64,
    pub cpu_artifact_bytes: u64,
    pub gpu_buffer_bytes: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub struct DistributionSummary {
    pub count: usize,
    pub median: f64,
    #[serde(rename = "p95")]
    pub ninety_fifth_percentile: f64,
    pub maximum: f64,
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum EvidenceError {
    #[error("a timing distribution must contain at least one finite sample")]
    EmptyDistribution,
    #[error("timing samples must be finite and non-negative")]
    InvalidSample,
    #[error("a Voxel Scene Revision identity must not be empty")]
    EmptyVoxelSceneRevisionIdentity,
    #[error("scale {scale} has {actual} fresh first-correct-frame samples; expected {expected}")]
    FirstCorrectFrameSampleCount {
        scale: u32,
        expected: usize,
        actual: usize,
    },
    #[error("required scale {scale} is missing")]
    MissingScale { scale: u32 },
    #[error("scale {scale} occurs more than once")]
    DuplicateScale { scale: u32 },
    #[error("the aggregation contains {actual} scales instead of exactly three")]
    UnexpectedScaleCount { actual: usize },
    #[error("extent selection schema version {actual} is unsupported; expected 1")]
    UnsupportedExtentSelectionSchema { actual: u32 },
    #[error("required Raster Region extent {extent} is missing")]
    MissingRasterRegionExtent { extent: u32 },
    #[error("Raster Region extent {extent} occurs more than once")]
    DuplicateRasterRegionExtent { extent: u32 },
    #[error("unexpected Raster Region extent {extent}; expected 16, 32, or 64")]
    UnexpectedRasterRegionExtent { extent: u32 },
    #[error("Raster Region extent {extent} did not pass every qualification gate")]
    UnqualifiedExtent { extent: u32 },
    #[error("Raster Region extent {extent} must retain at least two latency samples")]
    InsufficientExtentLatencySamples { extent: u32 },
    #[error(
        "Raster Region extent selection is ambiguous because extents {first} and {second} have identical selection inputs"
    )]
    AmbiguousRasterRegionExtentSelection { first: u32, second: u32 },
    #[error("timing comparison requires two total samples")]
    TimingComparisonRequiresTotals,
    #[error("timing comparison requires one raster and one compute-ray sample")]
    TimingComparisonRequiresBothRenderPaths,
    #[error("timing samples differ in {field}")]
    UnlikeTimingConditions { field: &'static str },
    #[error("timing samples must be finite and non-negative")]
    InvalidTimingSample,
    #[error("a raw timing stream must contain samples")]
    EmptyTimingStream,
    #[error("a raw timing stream identity must not be empty")]
    EmptyTimingStreamIdentity,
    #[error(
        "raw timing stream {stream_identity:?} mixes conditions, scenarios, or sample sequences"
    )]
    MixedTimingStream { stream_identity: String },
    #[error("timing attribution field {field} is empty or invalid")]
    InvalidTimingAttribution { field: &'static str },
    #[error("raw timing stream is missing phase {phase:?}")]
    MissingTimingPhase { phase: TimingPhase },
    #[error("required {render_path:?} {scenario:?} timing stream is missing")]
    MissingTimingStream {
        render_path: RenderPathStrategy,
        scenario: TimingScenario,
    },
    #[error("resource ledger sequence {sequence} does not follow {previous}")]
    ResourceLedgerSequence { previous: u64, sequence: u64 },
    #[error("resource ledger must contain at least one balanced resource lifetime")]
    EmptyResourceLedger,
    #[error("resource ledger identity {identity:?} is empty")]
    EmptyResourceIdentity { identity: String },
    #[error("resource ledger identity {identity:?} has no owned quantity")]
    EmptyResourceQuantity { identity: String },
    #[error("resource ledger identity {identity:?} has invalid attribution field {field}")]
    InvalidResourceAttribution {
        identity: String,
        field: &'static str,
    },
    #[error("resource ledger identity {identity:?} was created more than once")]
    DuplicateResourceCreation { identity: String },
    #[error("resource ledger identity {identity:?} was not live for {action}")]
    ResourceNotLive {
        identity: String,
        action: &'static str,
    },
    #[error("resource ledger transition for {identity:?} changed its immutable definition")]
    ResourceDefinitionChanged { identity: String },
    #[error("resource ledger quantity overflowed")]
    ResourceQuantityOverflow,
    #[error("resource ledger retained resources after shutdown: {identities:?}")]
    LiveResourcesAtShutdown { identities: Vec<String> },
    #[error("occupancy blocks must be fixed at 16 by 16 by 16")]
    InvalidOccupancyBlockExtent,
    #[error("paired dispatch evidence is missing canonical camera {camera_identity:?}")]
    MissingCanonicalCamera { camera_identity: &'static str },
    #[error("paired dispatch evidence repeats camera {camera_identity:?}")]
    DuplicateCanonicalCamera { camera_identity: String },
    #[error("paired dispatch evidence has unexpected camera {camera_identity:?}")]
    UnexpectedCanonicalCamera { camera_identity: String },
    #[error("paired dispatch samples for {camera_identity:?} must contain at least two pairs")]
    InsufficientPairedDispatchSamples { camera_identity: String },
    #[error("paired dispatch conditions for {camera_identity:?} have invalid field {field}")]
    InvalidPairedDispatchConditions {
        camera_identity: String,
        field: &'static str,
    },
    #[error("paired dispatch sequence {sequence} repeats for {camera_identity:?}")]
    DuplicatePairedDispatchSequence {
        camera_identity: String,
        sequence: u64,
    },
    #[error("paired dispatch samples must be finite and non-negative")]
    InvalidPairedDispatchSample,
    #[error("Render Path evidence schema version {actual} is unsupported; expected 1")]
    UnsupportedRenderPathEvidenceSchema { actual: u32 },
    #[error("timing comparison references missing raw sample index {index}")]
    MissingTimingSampleIndex { index: usize },
    #[error("required {scenario:?} {clock:?} timing comparison is missing")]
    MissingTimingComparison {
        scenario: TimingScenario,
        clock: TimingClock,
    },
    #[error("required lifecycle event {kind:?} is missing")]
    MissingLifecycleEvent { kind: LifecycleEvidenceKind },
    #[error("lifecycle event {identity:?} has invalid attribution field {field}")]
    InvalidLifecycleEvent {
        identity: String,
        field: &'static str,
    },
    #[error("lifecycle event {identity:?} has no matching resource ledger evidence")]
    UnmatchedLifecycleEvent { identity: String },
    #[error("resource evidence does not contain simultaneous Render Path overlap")]
    MissingPeakResourceOverlap,
}

pub fn summarize(samples: &[f64]) -> Result<DistributionSummary, EvidenceError> {
    if samples.is_empty() {
        return Err(EvidenceError::EmptyDistribution);
    }
    if samples
        .iter()
        .any(|sample| !sample.is_finite() || sample.is_sign_negative())
    {
        return Err(EvidenceError::InvalidSample);
    }
    let mut ordered = samples.to_vec();
    ordered.sort_by(f64::total_cmp);
    let count = ordered.len();
    let median = if count.is_multiple_of(2) {
        let upper = count / 2;
        (ordered[upper - 1] + ordered[upper]) / 2.0
    } else {
        ordered[count / 2]
    };
    let ninety_fifth_percentile_index = count.saturating_mul(95).div_ceil(100).saturating_sub(1);
    let ninety_fifth_percentile = ordered[ninety_fifth_percentile_index];
    let maximum = ordered[count - 1];
    Ok(DistributionSummary {
        count,
        median,
        ninety_fifth_percentile,
        maximum,
    })
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct FirstCorrectFramePhases {
    pub derivation_milliseconds: f64,
    pub upload_install_milliseconds: f64,
    pub presentation_milliseconds: f64,
    pub total_milliseconds: f64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ScaleAggregationInput {
    pub scale: u32,
    pub first_correct_frame_samples: Vec<FirstCorrectFramePhases>,
    pub cpu_frame_milliseconds: Vec<f64>,
    pub gpu_frame_milliseconds: Vec<f64>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ScaleAggregation {
    pub scale: u32,
    pub derivation: DistributionSummary,
    pub upload_install: DistributionSummary,
    pub presentation: DistributionSummary,
    pub total: DistributionSummary,
    pub cpu_frame: DistributionSummary,
    pub gpu_frame: DistributionSummary,
}

pub fn aggregate_scales(
    scales: Vec<ScaleAggregationInput>,
) -> Result<Vec<ScaleAggregation>, EvidenceError> {
    let mut scales_by_identity = BTreeMap::new();
    for scale in scales {
        let identity = scale.scale;
        if scales_by_identity.insert(identity, scale).is_some() {
            return Err(EvidenceError::DuplicateScale { scale: identity });
        }
    }
    for required_scale in [64, 128, 256] {
        if !scales_by_identity.contains_key(&required_scale) {
            return Err(EvidenceError::MissingScale {
                scale: required_scale,
            });
        }
    }
    if scales_by_identity.len() != 3 {
        return Err(EvidenceError::UnexpectedScaleCount {
            actual: scales_by_identity.len(),
        });
    }
    scales_by_identity
        .into_values()
        .map(|scale| {
            if scale.first_correct_frame_samples.len() != 10 {
                return Err(EvidenceError::FirstCorrectFrameSampleCount {
                    scale: scale.scale,
                    expected: 10,
                    actual: scale.first_correct_frame_samples.len(),
                });
            }
            let derivation = scale
                .first_correct_frame_samples
                .iter()
                .map(|sample| sample.derivation_milliseconds)
                .collect::<Vec<_>>();
            let upload_install = scale
                .first_correct_frame_samples
                .iter()
                .map(|sample| sample.upload_install_milliseconds)
                .collect::<Vec<_>>();
            let presentation = scale
                .first_correct_frame_samples
                .iter()
                .map(|sample| sample.presentation_milliseconds)
                .collect::<Vec<_>>();
            let total = scale
                .first_correct_frame_samples
                .iter()
                .map(|sample| sample.total_milliseconds)
                .collect::<Vec<_>>();
            Ok(ScaleAggregation {
                scale: scale.scale,
                derivation: summarize(&derivation)?,
                upload_install: summarize(&upload_install)?,
                presentation: summarize(&presentation)?,
                total: summarize(&total)?,
                cpu_frame: summarize(&scale.cpu_frame_milliseconds)?,
                gpu_frame: summarize(&scale.gpu_frame_milliseconds)?,
            })
        })
        .collect()
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExtentQualificationGates {
    pub semantic_correctness: bool,
    pub localization: bool,
    pub failure_retry: bool,
    pub lifecycle: bool,
    pub shutdown: bool,
    pub resource_retirement: bool,
    pub validation: bool,
}

impl ExtentQualificationGates {
    fn all_passed(&self) -> bool {
        self.semantic_correctness
            && self.localization
            && self.failure_retry
            && self.lifecycle
            && self.shutdown
            && self.resource_retirement
            && self.validation
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ExtentCandidateInput {
    pub extent: u32,
    pub qualification: ExtentQualificationGates,
    pub latency_samples_milliseconds: Vec<f64>,
    pub peak_live_gpu_bytes: u64,
    pub peak_live_gpu_resources: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ExtentSelectionInput {
    pub schema_version: u32,
    pub candidates: Vec<ExtentCandidateInput>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ExtentCandidateReport {
    pub extent: u32,
    pub qualification: ExtentQualificationGates,
    pub latency_samples_milliseconds: Vec<f64>,
    pub latency_milliseconds: DistributionSummary,
    pub peak_live_gpu_bytes: u64,
    pub peak_live_gpu_resources: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ExtentSelectionReport {
    pub schema_version: u32,
    pub scope: &'static str,
    pub selection_rule: [&'static str; 4],
    pub candidates: Vec<ExtentCandidateReport>,
    pub selected_extent: u32,
}

fn compare_extent_candidates(
    left: &ExtentCandidateReport,
    right: &ExtentCandidateReport,
) -> std::cmp::Ordering {
    left.latency_milliseconds
        .median
        .total_cmp(&right.latency_milliseconds.median)
        .then_with(|| {
            left.latency_milliseconds
                .ninety_fifth_percentile
                .total_cmp(&right.latency_milliseconds.ninety_fifth_percentile)
        })
        .then_with(|| left.peak_live_gpu_bytes.cmp(&right.peak_live_gpu_bytes))
        .then_with(|| {
            left.peak_live_gpu_resources
                .cmp(&right.peak_live_gpu_resources)
        })
}

pub fn select_raster_region_extent(
    input: ExtentSelectionInput,
) -> Result<ExtentSelectionReport, EvidenceError> {
    if input.schema_version != 1 {
        return Err(EvidenceError::UnsupportedExtentSelectionSchema {
            actual: input.schema_version,
        });
    }
    let mut candidates_by_extent = BTreeMap::new();
    for candidate in input.candidates {
        if ![16, 32, 64].contains(&candidate.extent) {
            return Err(EvidenceError::UnexpectedRasterRegionExtent {
                extent: candidate.extent,
            });
        }
        let extent = candidate.extent;
        if candidates_by_extent.insert(extent, candidate).is_some() {
            return Err(EvidenceError::DuplicateRasterRegionExtent { extent });
        }
    }
    for extent in [16, 32, 64] {
        if !candidates_by_extent.contains_key(&extent) {
            return Err(EvidenceError::MissingRasterRegionExtent { extent });
        }
    }
    let mut candidates = Vec::with_capacity(candidates_by_extent.len());
    for candidate in candidates_by_extent.into_values() {
        if !candidate.qualification.all_passed() {
            return Err(EvidenceError::UnqualifiedExtent {
                extent: candidate.extent,
            });
        }
        if candidate.latency_samples_milliseconds.len() < 2 {
            return Err(EvidenceError::InsufficientExtentLatencySamples {
                extent: candidate.extent,
            });
        }
        candidates.push(ExtentCandidateReport {
            extent: candidate.extent,
            qualification: candidate.qualification,
            latency_milliseconds: summarize(&candidate.latency_samples_milliseconds)?,
            latency_samples_milliseconds: candidate.latency_samples_milliseconds,
            peak_live_gpu_bytes: candidate.peak_live_gpu_bytes,
            peak_live_gpu_resources: candidate.peak_live_gpu_resources,
        });
    }
    let selected = candidates
        .iter()
        .min_by(|left, right| compare_extent_candidates(left, right))
        .ok_or(EvidenceError::MissingRasterRegionExtent { extent: 16 })?;
    if let Some(tied) = candidates.iter().find(|candidate| {
        candidate.extent != selected.extent
            && compare_extent_candidates(candidate, selected).is_eq()
    }) {
        return Err(EvidenceError::AmbiguousRasterRegionExtentSelection {
            first: selected.extent.min(tied.extent),
            second: selected.extent.max(tied.extent),
        });
    }
    Ok(ExtentSelectionReport {
        schema_version: 1,
        scope: "Descriptive comparison for the recorded development machine only.",
        selection_rule: [
            "median_latency_milliseconds",
            "p95_latency_milliseconds",
            "peak_live_gpu_bytes",
            "peak_live_gpu_resources",
        ],
        selected_extent: selected.extent,
        candidates,
    })
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RenderPathStrategy {
    Raster,
    ComputeRay,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ComparableTimingConditions {
    pub machine_identity: String,
    pub operating_system: String,
    pub physical_device: String,
    pub driver: String,
    pub vulkan_version: String,
    pub queue_identity: String,
    pub repository_revision: String,
    pub executable_sha256: String,
    pub scene_identity: String,
    pub scene_revision: u64,
    pub camera_state_revision: u64,
    pub camera_identity: String,
    pub extent: [u32; 2],
    pub presentation_route: String,
    pub validation_enabled: bool,
    pub gpu_timestamps: bool,
    pub render_path: RenderPathStrategy,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TimingPhase {
    Preparation,
    Upload,
    Installation,
    Dispatch,
    Composite,
    Presentation,
    Switching,
    Total,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TimingClock {
    CpuWall,
    GpuTimestamp,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TimingScenario {
    ColdRequestToFirstMatchingFrame,
    ChangedSubmissionToFinalVisibleRevision,
    SteadyFrame,
    Switching,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TimingSample {
    pub stream_identity: String,
    pub conditions: ComparableTimingConditions,
    pub scenario: TimingScenario,
    pub sample_sequence: u64,
    pub phase: TimingPhase,
    pub clock: TimingClock,
    #[serde(rename = "elapsed_ms")]
    pub elapsed_milliseconds: f64,
}

impl TimingSample {
    pub fn to_json_line(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

fn verify_timing_attribution(conditions: &ComparableTimingConditions) -> Result<(), EvidenceError> {
    for (field, value) in [
        ("machine_identity", conditions.machine_identity.as_str()),
        ("operating_system", conditions.operating_system.as_str()),
        ("physical_device", conditions.physical_device.as_str()),
        ("driver", conditions.driver.as_str()),
        ("vulkan_version", conditions.vulkan_version.as_str()),
        ("queue_identity", conditions.queue_identity.as_str()),
        (
            "repository_revision",
            conditions.repository_revision.as_str(),
        ),
        ("executable_sha256", conditions.executable_sha256.as_str()),
        ("scene_identity", conditions.scene_identity.as_str()),
        ("camera_identity", conditions.camera_identity.as_str()),
        ("presentation_route", conditions.presentation_route.as_str()),
    ] {
        if value.is_empty() {
            return Err(EvidenceError::InvalidTimingAttribution { field });
        }
    }
    if conditions.extent.contains(&0) {
        return Err(EvidenceError::InvalidTimingAttribution { field: "extent" });
    }
    Ok(())
}

pub fn verify_timing_phase_coverage(samples: &[TimingSample]) -> Result<(), EvidenceError> {
    samples.first().ok_or(EvidenceError::EmptyTimingStream)?;
    let mut streams = BTreeMap::<String, Vec<&TimingSample>>::new();
    for sample in samples {
        if sample.stream_identity.is_empty() {
            return Err(EvidenceError::EmptyTimingStreamIdentity);
        }
        verify_timing_attribution(&sample.conditions)?;
        if !sample.elapsed_milliseconds.is_finite()
            || sample.elapsed_milliseconds.is_sign_negative()
        {
            return Err(EvidenceError::InvalidTimingSample);
        }
        streams
            .entry(sample.stream_identity.clone())
            .or_default()
            .push(sample);
    }

    let mut stream_kinds = BTreeSet::new();
    for (stream_identity, stream) in streams {
        let Some(first) = stream.first() else {
            return Err(EvidenceError::EmptyTimingStream);
        };
        if stream.iter().any(|sample| {
            sample.conditions != first.conditions
                || sample.scenario != first.scenario
                || sample.sample_sequence != first.sample_sequence
        }) {
            return Err(EvidenceError::MixedTimingStream { stream_identity });
        }
        stream_kinds.insert((first.conditions.render_path, first.scenario));
        let phases = stream
            .iter()
            .map(|sample| sample.phase)
            .collect::<BTreeSet<_>>();
        for phase in required_timing_phases(first.conditions.render_path, first.scenario) {
            if !phases.contains(phase) {
                return Err(EvidenceError::MissingTimingPhase { phase: *phase });
            }
        }
    }
    for render_path in [RenderPathStrategy::Raster, RenderPathStrategy::ComputeRay] {
        for scenario in [
            TimingScenario::ColdRequestToFirstMatchingFrame,
            TimingScenario::ChangedSubmissionToFinalVisibleRevision,
            TimingScenario::SteadyFrame,
            TimingScenario::Switching,
        ] {
            if !stream_kinds.contains(&(render_path, scenario)) {
                return Err(EvidenceError::MissingTimingStream {
                    render_path,
                    scenario,
                });
            }
        }
    }
    Ok(())
}

fn required_timing_phases(
    render_path: RenderPathStrategy,
    scenario: TimingScenario,
) -> &'static [TimingPhase] {
    use RenderPathStrategy::{ComputeRay, Raster};
    use TimingPhase::{
        Composite, Dispatch, Installation, Preparation, Presentation, Switching, Total, Upload,
    };

    match (render_path, scenario) {
        (ComputeRay, TimingScenario::ColdRequestToFirstMatchingFrame) => &[
            Preparation,
            Upload,
            Installation,
            Dispatch,
            Composite,
            Presentation,
            Total,
        ],
        (ComputeRay, TimingScenario::Switching) => &[
            Preparation,
            Upload,
            Installation,
            Dispatch,
            Composite,
            Presentation,
            Switching,
            Total,
        ],
        (ComputeRay, TimingScenario::ChangedSubmissionToFinalVisibleRevision) => &[
            Preparation,
            Upload,
            Installation,
            Dispatch,
            Composite,
            Presentation,
            Total,
        ],
        (ComputeRay, TimingScenario::SteadyFrame) => &[Dispatch, Composite, Presentation, Total],
        (Raster, TimingScenario::ColdRequestToFirstMatchingFrame) => {
            &[Preparation, Upload, Installation, Presentation, Total]
        }
        (Raster, TimingScenario::Switching) => &[
            Preparation,
            Upload,
            Installation,
            Presentation,
            Switching,
            Total,
        ],
        (Raster, TimingScenario::ChangedSubmissionToFinalVisibleRevision) => {
            &[Preparation, Upload, Installation, Presentation, Total]
        }
        (Raster, TimingScenario::SteadyFrame) => &[Presentation, Total],
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct TimingComparison {
    pub raster_milliseconds: f64,
    pub compute_ray_milliseconds: f64,
    pub compute_minus_raster_milliseconds: f64,
}

fn require_matching_timing_field<T: PartialEq>(
    left: &T,
    right: &T,
    field: &'static str,
) -> Result<(), EvidenceError> {
    if left != right {
        return Err(EvidenceError::UnlikeTimingConditions { field });
    }
    Ok(())
}

pub fn compare_like_timing_totals(
    left: &TimingSample,
    right: &TimingSample,
) -> Result<TimingComparison, EvidenceError> {
    if left.phase != TimingPhase::Total || right.phase != TimingPhase::Total {
        return Err(EvidenceError::TimingComparisonRequiresTotals);
    }
    if !left.elapsed_milliseconds.is_finite()
        || left.elapsed_milliseconds.is_sign_negative()
        || !right.elapsed_milliseconds.is_finite()
        || right.elapsed_milliseconds.is_sign_negative()
    {
        return Err(EvidenceError::InvalidTimingSample);
    }
    let left_conditions = &left.conditions;
    let right_conditions = &right.conditions;
    require_matching_timing_field(
        &left_conditions.machine_identity,
        &right_conditions.machine_identity,
        "machine_identity",
    )?;
    require_matching_timing_field(
        &left_conditions.operating_system,
        &right_conditions.operating_system,
        "operating_system",
    )?;
    require_matching_timing_field(
        &left_conditions.physical_device,
        &right_conditions.physical_device,
        "physical_device",
    )?;
    require_matching_timing_field(&left_conditions.driver, &right_conditions.driver, "driver")?;
    require_matching_timing_field(
        &left_conditions.vulkan_version,
        &right_conditions.vulkan_version,
        "vulkan_version",
    )?;
    require_matching_timing_field(
        &left_conditions.queue_identity,
        &right_conditions.queue_identity,
        "queue_identity",
    )?;
    require_matching_timing_field(
        &left_conditions.repository_revision,
        &right_conditions.repository_revision,
        "repository_revision",
    )?;
    require_matching_timing_field(
        &left_conditions.executable_sha256,
        &right_conditions.executable_sha256,
        "executable_sha256",
    )?;
    require_matching_timing_field(
        &left_conditions.scene_identity,
        &right_conditions.scene_identity,
        "scene_identity",
    )?;
    require_matching_timing_field(
        &left_conditions.scene_revision,
        &right_conditions.scene_revision,
        "scene_revision",
    )?;
    require_matching_timing_field(
        &left_conditions.camera_state_revision,
        &right_conditions.camera_state_revision,
        "camera_state_revision",
    )?;
    require_matching_timing_field(
        &left_conditions.camera_identity,
        &right_conditions.camera_identity,
        "camera_identity",
    )?;
    require_matching_timing_field(&left_conditions.extent, &right_conditions.extent, "extent")?;
    require_matching_timing_field(
        &left_conditions.presentation_route,
        &right_conditions.presentation_route,
        "presentation_route",
    )?;
    require_matching_timing_field(
        &left_conditions.validation_enabled,
        &right_conditions.validation_enabled,
        "validation_enabled",
    )?;
    require_matching_timing_field(
        &left_conditions.gpu_timestamps,
        &right_conditions.gpu_timestamps,
        "gpu_timestamps",
    )?;
    require_matching_timing_field(&left.scenario, &right.scenario, "scenario")?;
    require_matching_timing_field(
        &left.sample_sequence,
        &right.sample_sequence,
        "sample_sequence",
    )?;
    require_matching_timing_field(&left.clock, &right.clock, "clock")?;

    let (raster_milliseconds, compute_ray_milliseconds) =
        match (left_conditions.render_path, right_conditions.render_path) {
            (RenderPathStrategy::Raster, RenderPathStrategy::ComputeRay) => {
                (left.elapsed_milliseconds, right.elapsed_milliseconds)
            }
            (RenderPathStrategy::ComputeRay, RenderPathStrategy::Raster) => {
                (right.elapsed_milliseconds, left.elapsed_milliseconds)
            }
            _ => return Err(EvidenceError::TimingComparisonRequiresBothRenderPaths),
        };
    Ok(TimingComparison {
        raster_milliseconds,
        compute_ray_milliseconds,
        compute_minus_raster_milliseconds: compute_ray_milliseconds - raster_milliseconds,
    })
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceRole {
    Presenting,
    Replacement,
    Retiring,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceState {
    Preparing,
    Installed,
    Hidden,
    Retiring,
    CleanupDebt,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceType {
    VoxelWords,
    VolumeHeaders,
    MaterialTable,
    OccupancyDirectory,
    StagingAllocation,
    ComputeOutput,
    Composite,
    RasterVertices,
    RasterIndices,
    Worker,
    VoxelSceneView,
    Other,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceLedgerAction {
    Create,
    Transition,
    Destroy,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResourceLedgerAttribution {
    pub render_path: RenderPathStrategy,
    pub role: ResourceRole,
    pub scene_identity: String,
    pub scene_revision: u64,
    pub generation: u64,
    pub state: ResourceState,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResourceLedgerQuantity {
    pub bytes: u64,
    pub objects: u64,
    pub allocations: u64,
    pub workers: u64,
    pub retained_views: u64,
}

impl ResourceLedgerQuantity {
    pub fn is_zero(self) -> bool {
        self == Self::default()
    }

    fn checked_add(self, other: Self) -> Option<Self> {
        Some(Self {
            bytes: self.bytes.checked_add(other.bytes)?,
            objects: self.objects.checked_add(other.objects)?,
            allocations: self.allocations.checked_add(other.allocations)?,
            workers: self.workers.checked_add(other.workers)?,
            retained_views: self.retained_views.checked_add(other.retained_views)?,
        })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResourceLedgerEntry {
    pub sequence: u64,
    pub resource_identity: String,
    pub resource_type: ResourceType,
    pub action: ResourceLedgerAction,
    pub attribution: ResourceLedgerAttribution,
    pub quantity: ResourceLedgerQuantity,
}

impl ResourceLedgerEntry {
    pub fn to_json_line(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
pub struct ResourceLedgerSummary {
    pub peak: ResourceLedgerQuantity,
    pub peak_overlap: ResourceLedgerQuantity,
    pub transition_count: usize,
}

fn total_live_quantity(
    live: &BTreeMap<String, ResourceLedgerEntry>,
) -> Result<ResourceLedgerQuantity, EvidenceError> {
    live.values()
        .try_fold(ResourceLedgerQuantity::default(), |total, entry| {
            total
                .checked_add(entry.quantity)
                .ok_or(EvidenceError::ResourceQuantityOverflow)
        })
}

fn maximum_quantity(
    left: ResourceLedgerQuantity,
    right: ResourceLedgerQuantity,
) -> ResourceLedgerQuantity {
    ResourceLedgerQuantity {
        bytes: left.bytes.max(right.bytes),
        objects: left.objects.max(right.objects),
        allocations: left.allocations.max(right.allocations),
        workers: left.workers.max(right.workers),
        retained_views: left.retained_views.max(right.retained_views),
    }
}

pub fn verify_resource_ledger(
    entries: &[ResourceLedgerEntry],
) -> Result<ResourceLedgerSummary, EvidenceError> {
    if entries.is_empty() {
        return Err(EvidenceError::EmptyResourceLedger);
    }
    let mut previous_sequence = None;
    let mut seen = BTreeSet::new();
    let mut live = BTreeMap::new();
    let mut summary = ResourceLedgerSummary::default();
    for entry in entries {
        if let Some(previous) = previous_sequence
            && entry.sequence <= previous
        {
            return Err(EvidenceError::ResourceLedgerSequence {
                previous,
                sequence: entry.sequence,
            });
        }
        previous_sequence = Some(entry.sequence);
        if entry.resource_identity.is_empty() {
            return Err(EvidenceError::EmptyResourceIdentity {
                identity: entry.resource_identity.clone(),
            });
        }
        if entry.quantity.is_zero() {
            return Err(EvidenceError::EmptyResourceQuantity {
                identity: entry.resource_identity.clone(),
            });
        }
        if entry.attribution.scene_identity.is_empty() {
            return Err(EvidenceError::InvalidResourceAttribution {
                identity: entry.resource_identity.clone(),
                field: "scene_identity",
            });
        }
        match entry.action {
            ResourceLedgerAction::Create => {
                if !seen.insert(entry.resource_identity.clone()) {
                    return Err(EvidenceError::DuplicateResourceCreation {
                        identity: entry.resource_identity.clone(),
                    });
                }
                live.insert(entry.resource_identity.clone(), entry.clone());
            }
            ResourceLedgerAction::Transition => {
                let Some(current) = live.get_mut(&entry.resource_identity) else {
                    return Err(EvidenceError::ResourceNotLive {
                        identity: entry.resource_identity.clone(),
                        action: "transition",
                    });
                };
                if !same_resource_definition(current, entry) {
                    return Err(EvidenceError::ResourceDefinitionChanged {
                        identity: entry.resource_identity.clone(),
                    });
                }
                *current = entry.clone();
            }
            ResourceLedgerAction::Destroy => {
                let Some(current) = live.remove(&entry.resource_identity) else {
                    return Err(EvidenceError::ResourceNotLive {
                        identity: entry.resource_identity.clone(),
                        action: "destruction",
                    });
                };
                if !same_resource_definition(&current, entry) {
                    return Err(EvidenceError::ResourceDefinitionChanged {
                        identity: entry.resource_identity.clone(),
                    });
                }
            }
        }
        let total = total_live_quantity(&live)?;
        summary.peak = maximum_quantity(summary.peak, total);
        let render_path_count = live
            .values()
            .map(|resource| resource.attribution.render_path)
            .collect::<BTreeSet<_>>()
            .len();
        if render_path_count > 1 {
            summary.peak_overlap = maximum_quantity(summary.peak_overlap, total);
        }
    }
    if !live.is_empty() {
        return Err(EvidenceError::LiveResourcesAtShutdown {
            identities: live.into_keys().collect(),
        });
    }
    summary.transition_count = entries
        .iter()
        .filter(|entry| entry.action == ResourceLedgerAction::Transition)
        .count();
    Ok(summary)
}

fn same_resource_definition(left: &ResourceLedgerEntry, right: &ResourceLedgerEntry) -> bool {
    left.resource_type == right.resource_type
        && left.quantity == right.quantity
        && left.attribution.render_path == right.attribution.render_path
        && left.attribution.scene_identity == right.attribution.scene_identity
        && left.attribution.scene_revision == right.attribution.scene_revision
        && left.attribution.generation == right.attribution.generation
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct OccupancyQualificationGates {
    pub semantic_correctness: bool,
    pub switching: bool,
    pub convergence: bool,
    pub lifecycle: bool,
    pub failure_retry: bool,
    pub shutdown: bool,
    pub validation: bool,
    pub resource_balance: bool,
}

impl OccupancyQualificationGates {
    fn all_passed(&self) -> bool {
        self.semantic_correctness
            && self.switching
            && self.convergence
            && self.lifecycle
            && self.failure_retry
            && self.shutdown
            && self.validation
            && self.resource_balance
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct PairedDispatchSamples {
    pub camera_identity: String,
    pub conditions: ComparableTimingConditions,
    pub samples: Vec<PairedDispatchSample>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub struct PairedDispatchSample {
    pub sample_sequence: u64,
    pub dense_gpu_milliseconds: f64,
    pub occupancy_gpu_milliseconds: f64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct OccupancyExperiment {
    pub block_extent: [u32; 3],
    pub qualification: OccupancyQualificationGates,
    pub paired_dispatch_samples: Vec<PairedDispatchSamples>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct TraversalSelectionInput {
    pub occupancy_experiment: Option<OccupancyExperiment>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TraversalSelection {
    DenseDda,
    FixedOccupancy16,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct BootstrapInterval {
    pub lower: f64,
    pub upper: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PairedDispatchReport {
    pub camera_identity: String,
    pub sample_count: usize,
    pub mean_reduction_milliseconds: f64,
    pub bootstrap_95_percent: BootstrapInterval,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TraversalSelectionReport {
    pub selected: TraversalSelection,
    pub occupancy_attempted: bool,
    pub occupancy_qualification_passed: bool,
    pub camera_reports: Vec<PairedDispatchReport>,
}

fn mean(samples: &[f64]) -> f64 {
    samples.iter().sum::<f64>() / samples.len() as f64
}

fn bootstrap_mean_interval(samples: &[f64]) -> BootstrapInterval {
    const REPLICATE_COUNT: usize = 10_000;
    let mut state = 0x4d59_5df4_d0f3_3173_u64 ^ samples.len() as u64;
    let mut replicate_means = Vec::with_capacity(REPLICATE_COUNT);
    for _ in 0..REPLICATE_COUNT {
        let mut total = 0.0;
        for _ in samples {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let index = ((state >> 32) as usize) % samples.len();
            total += samples[index];
        }
        replicate_means.push(total / samples.len() as f64);
    }
    replicate_means.sort_by(f64::total_cmp);
    let lower_index = REPLICATE_COUNT.saturating_mul(25).div_ceil(1_000) - 1;
    let upper_index = REPLICATE_COUNT.saturating_mul(975).div_ceil(1_000) - 1;
    BootstrapInterval {
        lower: replicate_means[lower_index],
        upper: replicate_means[upper_index],
    }
}

pub fn select_compute_traversal(
    input: TraversalSelectionInput,
) -> Result<TraversalSelectionReport, EvidenceError> {
    let Some(experiment) = input.occupancy_experiment else {
        return Ok(TraversalSelectionReport {
            selected: TraversalSelection::DenseDda,
            occupancy_attempted: false,
            occupancy_qualification_passed: false,
            camera_reports: Vec::new(),
        });
    };
    if experiment.block_extent != [16, 16, 16] {
        return Err(EvidenceError::InvalidOccupancyBlockExtent);
    }
    let mut samples_by_camera = BTreeMap::new();
    for samples in experiment.paired_dispatch_samples {
        if !["overview", "cavity", "boundary"].contains(&samples.camera_identity.as_str()) {
            return Err(EvidenceError::UnexpectedCanonicalCamera {
                camera_identity: samples.camera_identity,
            });
        }
        let camera_identity = samples.camera_identity.clone();
        if samples_by_camera
            .insert(camera_identity.clone(), samples)
            .is_some()
        {
            return Err(EvidenceError::DuplicateCanonicalCamera { camera_identity });
        }
    }
    for camera_identity in ["overview", "cavity", "boundary"] {
        if !samples_by_camera.contains_key(camera_identity) {
            return Err(EvidenceError::MissingCanonicalCamera { camera_identity });
        }
    }
    let mut camera_reports = Vec::with_capacity(3);
    let mut shared_conditions = None;
    for camera_identity in ["overview", "cavity", "boundary"] {
        let samples = samples_by_camera
            .remove(camera_identity)
            .ok_or(EvidenceError::MissingCanonicalCamera { camera_identity })?;
        verify_timing_attribution(&samples.conditions)?;
        if samples.conditions.render_path != RenderPathStrategy::ComputeRay {
            return Err(EvidenceError::InvalidPairedDispatchConditions {
                camera_identity: samples.camera_identity,
                field: "render_path",
            });
        }
        if samples.conditions.camera_identity != samples.camera_identity {
            return Err(EvidenceError::InvalidPairedDispatchConditions {
                camera_identity: samples.camera_identity,
                field: "camera_identity",
            });
        }
        if samples.conditions.validation_enabled || !samples.conditions.gpu_timestamps {
            return Err(EvidenceError::InvalidPairedDispatchConditions {
                camera_identity: samples.camera_identity,
                field: "measurement_mode",
            });
        }
        let mut comparable_conditions = samples.conditions.clone();
        comparable_conditions.camera_identity.clear();
        comparable_conditions.camera_state_revision = 0;
        if let Some(expected) = &shared_conditions {
            if expected != &comparable_conditions {
                return Err(EvidenceError::InvalidPairedDispatchConditions {
                    camera_identity: samples.camera_identity,
                    field: "cross_camera_conditions",
                });
            }
        } else {
            shared_conditions = Some(comparable_conditions);
        }
        if samples.samples.len() < 2 {
            return Err(EvidenceError::InsufficientPairedDispatchSamples {
                camera_identity: samples.camera_identity,
            });
        }
        let mut sequences = BTreeSet::new();
        for sample in &samples.samples {
            if !sequences.insert(sample.sample_sequence) {
                return Err(EvidenceError::DuplicatePairedDispatchSequence {
                    camera_identity: samples.camera_identity,
                    sequence: sample.sample_sequence,
                });
            }
            if !sample.dense_gpu_milliseconds.is_finite()
                || sample.dense_gpu_milliseconds.is_sign_negative()
                || !sample.occupancy_gpu_milliseconds.is_finite()
                || sample.occupancy_gpu_milliseconds.is_sign_negative()
            {
                return Err(EvidenceError::InvalidPairedDispatchSample);
            }
        }
        let reductions = samples
            .samples
            .iter()
            .map(|sample| sample.dense_gpu_milliseconds - sample.occupancy_gpu_milliseconds)
            .collect::<Vec<_>>();
        camera_reports.push(PairedDispatchReport {
            camera_identity: samples.camera_identity,
            sample_count: reductions.len(),
            mean_reduction_milliseconds: mean(&reductions),
            bootstrap_95_percent: bootstrap_mean_interval(&reductions),
        });
    }
    let qualification_passed = experiment.qualification.all_passed();
    let occupancy_is_supported = qualification_passed
        && camera_reports.iter().all(|camera| {
            camera.mean_reduction_milliseconds > 0.0 && camera.bootstrap_95_percent.lower > 0.0
        });
    Ok(TraversalSelectionReport {
        selected: if occupancy_is_supported {
            TraversalSelection::FixedOccupancy16
        } else {
            TraversalSelection::DenseDda
        },
        occupancy_attempted: true,
        occupancy_qualification_passed: qualification_passed,
        camera_reports,
    })
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TimingComparisonPair {
    pub left_sample_index: usize,
    pub right_sample_index: usize,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleEvidenceKind {
    FailureRetry,
    Retirement,
    Shutdown,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LifecycleEvidenceEvent {
    pub event_identity: String,
    pub kind: LifecycleEvidenceKind,
    pub attribution: ResourceLedgerAttribution,
    pub diagnostic_identity: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RenderPathEvidenceInput {
    pub schema_version: u32,
    pub timing_samples: Vec<TimingSample>,
    pub timing_comparisons: Vec<TimingComparisonPair>,
    pub resource_ledger: Vec<ResourceLedgerEntry>,
    pub lifecycle_events: Vec<LifecycleEvidenceEvent>,
    pub traversal_selection: TraversalSelectionInput,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct RetainedTimingComparison {
    pub scenario: TimingScenario,
    pub clock: TimingClock,
    pub sample_sequence: u64,
    pub comparison: TimingComparison,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RenderPathEvidenceReport {
    pub schema_version: u32,
    pub scope: &'static str,
    pub raw_timing_samples: Vec<TimingSample>,
    pub timing_comparisons: Vec<RetainedTimingComparison>,
    pub resources: ResourceLedgerSummary,
    pub lifecycle_events: Vec<LifecycleEvidenceEvent>,
    pub traversal: TraversalSelectionReport,
}

fn same_resource_generation(
    left: &ResourceLedgerAttribution,
    right: &ResourceLedgerAttribution,
) -> bool {
    left.render_path == right.render_path
        && left.scene_identity == right.scene_identity
        && left.scene_revision == right.scene_revision
        && left.generation == right.generation
}

fn verify_lifecycle_events(
    events: &[LifecycleEvidenceEvent],
    resource_ledger: &[ResourceLedgerEntry],
) -> Result<(), EvidenceError> {
    let mut retained_kinds = BTreeSet::new();
    for event in events {
        for (field, value) in [
            ("event_identity", event.event_identity.as_str()),
            ("scene_identity", event.attribution.scene_identity.as_str()),
            ("diagnostic_identity", event.diagnostic_identity.as_str()),
        ] {
            if value.is_empty() {
                return Err(EvidenceError::InvalidLifecycleEvent {
                    identity: event.event_identity.clone(),
                    field,
                });
            }
        }
        let has_matching_ledger_evidence = match event.kind {
            LifecycleEvidenceKind::FailureRetry => resource_ledger
                .iter()
                .any(|entry| same_resource_generation(&entry.attribution, &event.attribution)),
            LifecycleEvidenceKind::Retirement => resource_ledger.iter().any(|entry| {
                same_resource_generation(&entry.attribution, &event.attribution)
                    && entry.attribution.role == ResourceRole::Retiring
                    && entry.attribution.state == ResourceState::Retiring
            }),
            LifecycleEvidenceKind::Shutdown => resource_ledger.iter().any(|entry| {
                same_resource_generation(&entry.attribution, &event.attribution)
                    && entry.action == ResourceLedgerAction::Destroy
            }),
        };
        if !has_matching_ledger_evidence {
            return Err(EvidenceError::UnmatchedLifecycleEvent {
                identity: event.event_identity.clone(),
            });
        }
        retained_kinds.insert(event.kind);
    }
    for kind in [
        LifecycleEvidenceKind::FailureRetry,
        LifecycleEvidenceKind::Retirement,
        LifecycleEvidenceKind::Shutdown,
    ] {
        if !retained_kinds.contains(&kind) {
            return Err(EvidenceError::MissingLifecycleEvent { kind });
        }
    }
    Ok(())
}

pub fn retain_render_path_evidence(
    input: RenderPathEvidenceInput,
) -> Result<RenderPathEvidenceReport, EvidenceError> {
    if input.schema_version != 1 {
        return Err(EvidenceError::UnsupportedRenderPathEvidenceSchema {
            actual: input.schema_version,
        });
    }
    verify_timing_phase_coverage(&input.timing_samples)?;
    let mut retained_comparisons = Vec::with_capacity(input.timing_comparisons.len());
    let mut comparison_kinds = BTreeSet::new();
    for pair in input.timing_comparisons {
        let left = input.timing_samples.get(pair.left_sample_index).ok_or(
            EvidenceError::MissingTimingSampleIndex {
                index: pair.left_sample_index,
            },
        )?;
        let right = input.timing_samples.get(pair.right_sample_index).ok_or(
            EvidenceError::MissingTimingSampleIndex {
                index: pair.right_sample_index,
            },
        )?;
        let comparison = compare_like_timing_totals(left, right)?;
        comparison_kinds.insert((left.scenario, left.clock));
        retained_comparisons.push(RetainedTimingComparison {
            scenario: left.scenario,
            clock: left.clock,
            sample_sequence: left.sample_sequence,
            comparison,
        });
    }
    for (scenario, clock) in [
        (
            TimingScenario::ColdRequestToFirstMatchingFrame,
            TimingClock::CpuWall,
        ),
        (
            TimingScenario::ChangedSubmissionToFinalVisibleRevision,
            TimingClock::CpuWall,
        ),
        (TimingScenario::SteadyFrame, TimingClock::CpuWall),
        (TimingScenario::SteadyFrame, TimingClock::GpuTimestamp),
    ] {
        if !comparison_kinds.contains(&(scenario, clock)) {
            return Err(EvidenceError::MissingTimingComparison { scenario, clock });
        }
    }
    let resources = verify_resource_ledger(&input.resource_ledger)?;
    if resources.peak_overlap.is_zero() {
        return Err(EvidenceError::MissingPeakResourceOverlap);
    }
    verify_lifecycle_events(&input.lifecycle_events, &input.resource_ledger)?;
    let traversal = select_compute_traversal(input.traversal_selection)?;
    Ok(RenderPathEvidenceReport {
        schema_version: 1,
        scope: "Descriptive measurements for the attributed machine only; no Render Path superiority claim.",
        raw_timing_samples: input.timing_samples,
        timing_comparisons: retained_comparisons,
        resources,
        lifecycle_events: input.lifecycle_events,
        traversal,
    })
}

pub mod large_sparse;

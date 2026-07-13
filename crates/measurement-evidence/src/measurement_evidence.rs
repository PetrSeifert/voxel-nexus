use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
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

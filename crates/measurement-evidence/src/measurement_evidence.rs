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

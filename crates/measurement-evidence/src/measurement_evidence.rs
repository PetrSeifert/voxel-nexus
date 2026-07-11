use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use thiserror::Error;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum MeasurementEvent {
    SceneRevisionPublished {
        source_revision: String,
        elapsed_ms: f64,
    },
    ArtifactDerived {
        source_revision: String,
        elapsed_ms: f64,
        resources: ResourceCounts,
    },
    ArtifactInstalled {
        source_revision: String,
        elapsed_ms: f64,
    },
    MatchingArtifactPresented {
        source_revision: String,
        elapsed_ms: f64,
    },
    SteadyFrame {
        sequence: u64,
        cpu_frame_ms: f64,
        gpu_frame_ms: f64,
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
    pub p95: f64,
    pub maximum: f64,
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum EvidenceError {
    #[error("a timing distribution must contain at least one finite sample")]
    EmptyDistribution,
    #[error("timing samples must be finite and non-negative")]
    InvalidSample,
    #[error("scale {scale} has {actual} fresh first-correct-frame samples; expected {expected}")]
    FirstCorrectFrameSampleCount {
        scale: u32,
        expected: usize,
        actual: usize,
    },
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
    let p95_index = count.saturating_mul(95).div_ceil(100).saturating_sub(1);
    let p95 = ordered[p95_index];
    let maximum = ordered[count - 1];
    Ok(DistributionSummary {
        count,
        median,
        p95,
        maximum,
    })
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct FirstCorrectFramePhases {
    pub derivation_ms: f64,
    pub upload_install_ms: f64,
    pub presentation_ms: f64,
    pub total_ms: f64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ScaleEvidence {
    pub scale: u32,
    pub first_correct_frame_samples: Vec<FirstCorrectFramePhases>,
    pub first_correct_frame_summary: DistributionSummary,
}

pub fn aggregate_first_correct_frame(
    samples_by_scale: BTreeMap<u32, Vec<FirstCorrectFramePhases>>,
    expected_samples: usize,
) -> Result<Vec<ScaleEvidence>, EvidenceError> {
    let mut evidence = Vec::with_capacity(samples_by_scale.len());
    for (scale, samples) in samples_by_scale {
        if samples.len() != expected_samples {
            return Err(EvidenceError::FirstCorrectFrameSampleCount {
                scale,
                expected: expected_samples,
                actual: samples.len(),
            });
        }
        let totals = samples
            .iter()
            .map(|sample| sample.total_ms)
            .collect::<Vec<_>>();
        evidence.push(ScaleEvidence {
            scale,
            first_correct_frame_samples: samples,
            first_correct_frame_summary: summarize(&totals)?,
        });
    }
    Ok(evidence)
}

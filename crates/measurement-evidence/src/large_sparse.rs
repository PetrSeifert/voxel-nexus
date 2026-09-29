use crate::{DistributionSummary, summarize};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use thiserror::Error;

pub const GPU_SCENE_BUDGET: u64 = 1 << 30;
pub const TIMINGS: &[&str] = &[
    "first_correct_frame",
    "publication",
    "validation",
    "enumeration",
    "construction",
    "serialization",
    "preparation",
    "upload",
    "uniform_to_mixed",
    "mixed_to_uniform",
    "empty_to_mixed",
    "mixed_to_empty",
    "growth",
    "growth_rebuild",
    "growth_upload",
    "steady_cpu",
    "steady_gpu",
];
pub const TRANSITIONS: &[&str] = &[
    "uniform_to_mixed",
    "mixed_to_uniform",
    "empty_to_mixed",
    "mixed_to_empty",
];

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
pub enum LargeSparseRecord {
    Context {
        schema_version: u32,
        device: String,
        driver_version: u32,
        max_storage_buffer_range_bytes: u64,
        dense_payload_bytes: u64,
        extent: [u32; 3],
        presentation_extent: [u32; 2],
        fingerprint: String,
    },
    Timing {
        name: String,
        milliseconds: f64,
    },
    Edit {
        transition: String,
        required_revision: u64,
        visible_revision: u64,
        uploaded_bytes: u64,
    },
    Growth {
        old_capacity: usize,
        new_capacity: usize,
        predicted_peak_bytes: u64,
        actual_peak_bytes: u64,
    },
    Memory {
        phase: String,
        publication_peak_heap_bytes: u64,
        enumeration_peak_heap_bytes: u64,
        enumeration_output_bytes: u64,
        enumeration_working_state_bytes: u64,
        enumeration_working_cells: usize,
        preparation_peak_heap_bytes: u64,
        staging_bytes: u64,
        visible_candidate_gpu_bytes: u64,
        measured_gpu_scene_peak_bytes: u64,
    },
    PredictedRejection {
        predicted_bytes: u64,
        budget_bytes: u64,
        rejected: bool,
    },
    Completed {
        validation_errors: usize,
        validation_warnings: usize,
        shutdown_resources_zero: bool,
    },
    ValidationAllocation {
        allocated_bytes: u64,
        peak_working_bytes: u64,
    },
    Verified {
        phase: String,
        probe_count: usize,
        installed_revision: u64,
    },
}

#[derive(Debug, Error)]
pub enum LargeSparseEvidenceError {
    #[error("measured GPU scene peak {0} bytes exceeds 1 GiB")]
    OverBudget(u64),
    #[error("invalid large sparse evidence: {0}")]
    Invalid(String),
    #[error(transparent)]
    Distribution(#[from] crate::EvidenceError),
}

#[derive(Debug, Serialize)]
pub struct LargeSparseReport {
    pub runs: usize,
    pub gpu_scene_budget_bytes: u64,
    pub measured_peak_bytes: u64,
    pub timings: BTreeMap<String, DistributionSummary>,
}

pub fn validate(
    runs: &[Vec<LargeSparseRecord>],
) -> Result<LargeSparseReport, LargeSparseEvidenceError> {
    let mut peak = 0;
    for record in runs.iter().flatten() {
        let measured = match record {
            LargeSparseRecord::Memory {
                measured_gpu_scene_peak_bytes,
                ..
            } => *measured_gpu_scene_peak_bytes,
            LargeSparseRecord::Growth {
                actual_peak_bytes, ..
            } => *actual_peak_bytes,
            _ => 0,
        };
        if measured > GPU_SCENE_BUDGET {
            return Err(LargeSparseEvidenceError::OverBudget(measured));
        }
        peak = peak.max(measured);
    }
    if runs.len() < 3 {
        return invalid("at least three fresh runs are required");
    }
    let mut samples: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let mut context = None;
    for run in runs {
        let mut counts = BTreeMap::<String, usize>::new();
        let mut edits = BTreeMap::new();
        let mut memories = BTreeMap::new();
        let mut verified = BTreeMap::new();
        let mut contexts = 0;
        let mut growths = 0;
        let mut growth_peak = None;
        let mut rejections = 0;
        let mut completions = 0;
        let mut validation_allocations = 0;
        for record in run {
            match record {
                LargeSparseRecord::Context {
                    schema_version,
                    device,
                    max_storage_buffer_range_bytes,
                    dense_payload_bytes,
                    extent,
                    presentation_extent,
                    fingerprint,
                    ..
                } => {
                    contexts += 1;
                    if *schema_version != 1
                        || device.is_empty()
                        || *max_storage_buffer_range_bytes == 0
                        || *dense_payload_bytes != 4_294_967_296
                        || dense_payload_bytes <= max_storage_buffer_range_bytes
                        || *extent != [2048, 256, 2048]
                        || *presentation_extent != [1920, 1080]
                        || fingerprint != "58ce86bc1227cf55"
                    {
                        return invalid("device, scale, extent or fingerprint mismatch");
                    }
                    if context.is_some_and(|previous| previous != record) {
                        return invalid("runs have different contexts");
                    }
                    context = Some(record);
                }
                LargeSparseRecord::Timing { name, milliseconds } => {
                    if !TIMINGS.contains(&name.as_str())
                        || !milliseconds.is_finite()
                        || *milliseconds < 0.0
                    {
                        return invalid("unknown or invalid timing");
                    }
                    *counts.entry(name.clone()).or_default() += 1;
                    samples.entry(name.clone()).or_default().push(*milliseconds);
                }
                LargeSparseRecord::Edit {
                    transition,
                    required_revision,
                    visible_revision,
                    uploaded_bytes,
                } => {
                    let expected = TRANSITIONS
                        .iter()
                        .position(|name| *name == transition)
                        .map(|index| index as u64 + 2);
                    if expected != Some(*required_revision)
                        || required_revision != visible_revision
                        || *uploaded_bytes == 0
                        || edits.insert(transition, ()).is_some()
                    {
                        return invalid("invalid or duplicate edit");
                    }
                }
                LargeSparseRecord::Growth {
                    old_capacity,
                    new_capacity,
                    predicted_peak_bytes,
                    actual_peak_bytes,
                } => {
                    growths += 1;
                    growth_peak = Some(*actual_peak_bytes);
                    if *old_capacity != 83_200
                        || *new_capacity != 124_800
                        || *predicted_peak_bytes == 0
                        || actual_peak_bytes > predicted_peak_bytes
                        || *predicted_peak_bytes > GPU_SCENE_BUDGET
                    {
                        return invalid("invalid large workload growth");
                    }
                }
                LargeSparseRecord::Memory {
                    phase,
                    publication_peak_heap_bytes,
                    enumeration_peak_heap_bytes,
                    enumeration_output_bytes,
                    enumeration_working_state_bytes,
                    enumeration_working_cells: _,
                    preparation_peak_heap_bytes,
                    staging_bytes,
                    visible_candidate_gpu_bytes,
                    measured_gpu_scene_peak_bytes,
                } => {
                    if !["initial", "edits", "growth"].contains(&phase.as_str())
                        || memories
                            .insert(phase.as_str(), *measured_gpu_scene_peak_bytes)
                            .is_some()
                        || enumeration_output_bytes > enumeration_peak_heap_bytes
                        || enumeration_working_state_bytes > enumeration_peak_heap_bytes
                        || *visible_candidate_gpu_bytes == 0
                        || *measured_gpu_scene_peak_bytes == 0
                        || visible_candidate_gpu_bytes.checked_add(*staging_bytes)
                            != Some(*measured_gpu_scene_peak_bytes)
                    {
                        return invalid("invalid memory accounting");
                    }
                    if phase == "initial"
                        && (*publication_peak_heap_bytes == 0
                            || *enumeration_peak_heap_bytes == 0
                            || *enumeration_output_bytes == 0
                            || *enumeration_working_state_bytes == 0
                            || *preparation_peak_heap_bytes == 0
                            || *staging_bytes == 0)
                    {
                        return invalid("missing CPU memory categories");
                    }
                    if phase == "growth"
                        && (*preparation_peak_heap_bytes == 0 || *staging_bytes == 0)
                    {
                        return invalid("missing growth memory categories");
                    }
                }
                LargeSparseRecord::PredictedRejection {
                    predicted_bytes,
                    budget_bytes,
                    rejected,
                } => {
                    rejections += 1;
                    if !rejected || *budget_bytes == 0 || predicted_bytes <= budget_bytes {
                        return invalid("predicted over-budget rejection was not verified");
                    }
                }
                LargeSparseRecord::Completed {
                    validation_errors,
                    validation_warnings,
                    shutdown_resources_zero,
                } => {
                    completions += 1;
                    if *validation_errors != 0
                        || *validation_warnings != 0
                        || !shutdown_resources_zero
                    {
                        return invalid("validation or shutdown failed");
                    }
                }
                LargeSparseRecord::ValidationAllocation {
                    allocated_bytes,
                    peak_working_bytes,
                } => {
                    validation_allocations += 1;
                    if *allocated_bytes == 0
                        || *peak_working_bytes == 0
                        || peak_working_bytes > allocated_bytes
                    {
                        return invalid("invalid validation allocation accounting");
                    }
                }
                LargeSparseRecord::Verified {
                    phase,
                    probe_count,
                    installed_revision,
                } => {
                    if ![
                        "initial",
                        "uniform_to_mixed",
                        "mixed_to_uniform",
                        "empty_to_mixed",
                        "mixed_to_empty",
                        "growth",
                    ]
                    .contains(&phase.as_str())
                        || *probe_count != if phase == "growth" { 14 } else { 13 }
                        || *installed_revision
                            != match phase.as_str() {
                                "initial" => 1,
                                "uniform_to_mixed" => 2,
                                "mixed_to_uniform" => 3,
                                "empty_to_mixed" => 4,
                                "mixed_to_empty" => 5,
                                _ => 6,
                            }
                        || verified.insert(phase, ()).is_some()
                    {
                        return invalid("invalid semantic verification");
                    }
                }
            }
        }
        if validation_allocations != 1
            || completions != 1
            || contexts != 1
            || growths != 1
            || rejections != 1
            || edits.len() != 4
            || memories.len() != 3
            || verified.len() != 6
        {
            return invalid("missing or duplicate required records");
        }
        if growth_peak != memories.get("growth").copied() {
            return invalid("growth memory records disagree");
        }
        if counts.get("steady_cpu") != counts.get("steady_gpu") {
            return invalid("steady CPU and GPU sample counts differ");
        }
        for name in TIMINGS {
            let count = counts.get(*name).copied().unwrap_or(0);
            if if name.starts_with("steady_") {
                count < 30
            } else {
                count != 1
            } {
                return invalid(&format!("missing or duplicate timing {name}"));
            }
        }
    }
    let timings = samples
        .into_iter()
        .map(|(name, values)| Ok((name, summarize(&values)?)))
        .collect::<Result<_, crate::EvidenceError>>()?;
    Ok(LargeSparseReport {
        runs: runs.len(),
        gpu_scene_budget_bytes: GPU_SCENE_BUDGET,
        measured_peak_bytes: peak,
        timings,
    })
}

fn invalid<T>(message: &str) -> Result<T, LargeSparseEvidenceError> {
    Err(LargeSparseEvidenceError::Invalid(message.to_owned()))
}

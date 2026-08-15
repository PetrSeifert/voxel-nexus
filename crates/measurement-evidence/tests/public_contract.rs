use measurement_evidence::{
    ComparableTimingConditions, EvidenceError, ExtentCandidateInput, ExtentQualificationGates,
    ExtentSelectionInput, FirstCorrectFramePhases, LifecycleEvidenceEvent, LifecycleEvidenceKind,
    MeasurementEvent, OccupancyExperiment, OccupancyQualificationGates, PairedDispatchSample,
    PairedDispatchSamples, RenderPathEvidenceInput, RenderPathStrategy, ResourceCounts,
    ResourceLedgerAction, ResourceLedgerAttribution, ResourceLedgerEntry, ResourceLedgerQuantity,
    ResourceRole, ResourceState, ResourceType, ScaleAggregationInput, TimingClock,
    TimingComparisonPair, TimingPhase, TimingSample, TimingScenario, TraversalSelection,
    TraversalSelectionInput, VoxelSceneRevisionIdentity, aggregate_scales,
    compare_like_timing_totals, retain_render_path_evidence, select_compute_traversal,
    select_raster_region_extent, summarize, verify_resource_ledger, verify_timing_phase_coverage,
};

#[test]
fn timing_event_serialization_is_machine_readable_and_phase_specific()
-> Result<(), Box<dyn std::error::Error>> {
    let event = MeasurementEvent::ArtifactDerived {
        source_revision: VoxelSceneRevisionIdentity::new("1")?,
        elapsed_milliseconds: 12.5,
        resources: ResourceCounts {
            occupied_voxels: 2,
            exposed_quads: 10,
            vertices: 40,
            indices: 60,
            draw_calls: 1,
            cpu_artifact_bytes: 1840,
            gpu_buffer_bytes: 1840,
        },
    };

    assert_eq!(
        event.to_json_line()?,
        r#"{"event":"artifact_derived","source_revision":"1","elapsed_ms":12.5,"resources":{"occupied_voxels":2,"exposed_quads":10,"vertices":40,"indices":60,"draw_calls":1,"cpu_artifact_bytes":1840,"gpu_buffer_bytes":1840}}"#
    );
    Ok(())
}

#[test]
fn summaries_use_literal_median_nearest_rank_ninety_fifth_percentile_and_maximum()
-> Result<(), Box<dyn std::error::Error>> {
    let summary = summarize(&[10.0, 2.0, 8.0, 4.0, 6.0, 20.0, 12.0, 18.0, 14.0, 16.0])?;

    assert_eq!(summary.count, 10);
    assert_eq!(summary.median, 11.0);
    assert_eq!(summary.ninety_fifth_percentile, 20.0);
    assert_eq!(summary.maximum, 20.0);
    Ok(())
}

fn scale_input(scale: u32, sample_count: usize) -> ScaleAggregationInput {
    ScaleAggregationInput {
        scale,
        first_correct_frame_samples: (0..sample_count)
            .map(|index| FirstCorrectFramePhases {
                derivation_milliseconds: index as f64 + 1.0,
                upload_install_milliseconds: 2.0,
                presentation_milliseconds: 3.0,
                total_milliseconds: index as f64 + 6.0,
            })
            .collect(),
        cpu_frame_milliseconds: vec![2.0, 4.0],
        gpu_frame_milliseconds: vec![1.0, 3.0],
    }
}

#[test]
fn manifest_aggregation_covers_exactly_three_scales_with_literal_summaries()
-> Result<(), Box<dyn std::error::Error>> {
    let report = aggregate_scales(vec![
        scale_input(256, 10),
        scale_input(64, 10),
        scale_input(128, 10),
    ])?;

    assert_eq!(
        report.iter().map(|scale| scale.scale).collect::<Vec<_>>(),
        vec![64, 128, 256]
    );
    assert_eq!(report[0].total.count, 10);
    assert_eq!(report[0].total.median, 10.5);
    assert_eq!(report[0].total.ninety_fifth_percentile, 15.0);
    assert_eq!(report[0].cpu_frame.median, 3.0);
    assert_eq!(report[0].gpu_frame.maximum, 3.0);
    Ok(())
}

#[test]
fn manifest_aggregation_rejects_missing_fresh_runs() {
    let error = aggregate_scales(vec![
        scale_input(64, 1),
        scale_input(128, 10),
        scale_input(256, 10),
    ])
    .expect_err("one sample must not satisfy a ten-run manifest");

    assert_eq!(
        error,
        EvidenceError::FirstCorrectFrameSampleCount {
            scale: 64,
            expected: 10,
            actual: 1,
        }
    );
}

fn qualified_extent(
    extent: u32,
    latency_samples_milliseconds: Vec<f64>,
    peak_live_gpu_bytes: u64,
    peak_live_gpu_resources: u64,
) -> ExtentCandidateInput {
    ExtentCandidateInput {
        extent,
        qualification: ExtentQualificationGates {
            semantic_correctness: true,
            localization: true,
            failure_retry: true,
            lifecycle: true,
            shutdown: true,
            resource_retirement: true,
            validation: true,
        },
        latency_samples_milliseconds,
        peak_live_gpu_bytes,
        peak_live_gpu_resources,
    }
}

#[test]
fn extent_selection_uses_the_resolved_lexicographic_rule() -> Result<(), Box<dyn std::error::Error>>
{
    let report = select_raster_region_extent(ExtentSelectionInput {
        schema_version: 1,
        candidates: vec![
            qualified_extent(64, vec![8.0, 10.0], 1_000, 10),
            qualified_extent(16, vec![8.0, 10.0], 900, 20),
            qualified_extent(32, vec![8.0, 10.0], 900, 10),
        ],
    })?;

    assert_eq!(report.selected_extent, 32);
    assert_eq!(report.candidates[0].extent, 16);
    assert_eq!(report.candidates[1].latency_milliseconds.median, 9.0);
    assert_eq!(
        report.selection_rule,
        [
            "median_latency_milliseconds",
            "p95_latency_milliseconds",
            "peak_live_gpu_bytes",
            "peak_live_gpu_resources",
        ]
    );
    Ok(())
}

#[test]
fn extent_selection_prioritizes_median_then_ninety_fifth_percentile()
-> Result<(), Box<dyn std::error::Error>> {
    let median_report = select_raster_region_extent(ExtentSelectionInput {
        schema_version: 1,
        candidates: vec![
            qualified_extent(16, vec![1.0, 9.0, 9.0], 1, 1),
            qualified_extent(32, vec![8.0, 8.0, 100.0], u64::MAX, u64::MAX),
            qualified_extent(64, vec![10.0, 10.0, 10.0], 1, 1),
        ],
    })?;
    assert_eq!(median_report.selected_extent, 32);

    let p95_report = select_raster_region_extent(ExtentSelectionInput {
        schema_version: 1,
        candidates: vec![
            qualified_extent(16, vec![1.0, 5.0, 9.0], 1, 1),
            qualified_extent(32, vec![4.0, 5.0, 6.0], u64::MAX, u64::MAX),
            qualified_extent(64, vec![5.0, 5.0, 10.0], 1, 1),
        ],
    })?;
    assert_eq!(p95_report.selected_extent, 32);
    Ok(())
}

#[test]
fn extent_selection_rejects_identical_selection_inputs() {
    let error = select_raster_region_extent(ExtentSelectionInput {
        schema_version: 1,
        candidates: vec![
            qualified_extent(16, vec![1.0, 2.0], 100, 10),
            qualified_extent(32, vec![1.0, 2.0], 100, 10),
            qualified_extent(64, vec![3.0, 4.0], 100, 10),
        ],
    })
    .expect_err("fully tied resolved inputs cannot select subjectively");

    assert_eq!(
        error,
        EvidenceError::AmbiguousRasterRegionExtentSelection {
            first: 16,
            second: 32,
        }
    );
}

#[test]
fn extent_selection_rejects_a_candidate_that_failed_qualification() {
    let mut failed = qualified_extent(16, vec![1.0, 2.0], 1_000, 10);
    failed.qualification.failure_retry = false;

    let error = select_raster_region_extent(ExtentSelectionInput {
        schema_version: 1,
        candidates: vec![
            failed,
            qualified_extent(32, vec![1.0, 2.0], 1_000, 10),
            qualified_extent(64, vec![1.0, 2.0], 1_000, 10),
        ],
    })
    .expect_err("an unqualified extent must not enter selection");

    assert_eq!(error, EvidenceError::UnqualifiedExtent { extent: 16 });
}

#[test]
fn extent_selection_requires_exactly_the_fixed_candidate_set() {
    let error = select_raster_region_extent(ExtentSelectionInput {
        schema_version: 1,
        candidates: vec![
            qualified_extent(16, vec![1.0, 2.0], 1_000, 10),
            qualified_extent(32, vec![1.0, 2.0], 1_000, 10),
        ],
    })
    .expect_err("all fixed candidates are required");

    assert_eq!(
        error,
        EvidenceError::MissingRasterRegionExtent { extent: 64 }
    );
}

fn timing_conditions(render_path: RenderPathStrategy) -> ComparableTimingConditions {
    ComparableTimingConditions {
        machine_identity: "development-machine".to_owned(),
        operating_system: "Windows 11".to_owned(),
        physical_device: "Example GPU".to_owned(),
        driver: "1.2.3".to_owned(),
        vulkan_version: "1.3".to_owned(),
        queue_identity: "graphics-compute-0".to_owned(),
        repository_revision: "0123456789abcdef".to_owned(),
        executable_sha256: "ab".repeat(32),
        scene_identity: "canonical-scene".to_owned(),
        scene_revision: 4,
        camera_state_revision: 2,
        camera_identity: "overview".to_owned(),
        extent: [1920, 1080],
        presentation_route: "VK_PRESENT_MODE_IMMEDIATE_KHR".to_owned(),
        validation_enabled: false,
        gpu_timestamps: true,
        render_path,
    }
}

fn total_sample(render_path: RenderPathStrategy) -> TimingSample {
    TimingSample {
        stream_identity: "cold-1".to_owned(),
        conditions: timing_conditions(render_path),
        scenario: TimingScenario::ColdRequestToFirstMatchingFrame,
        sample_sequence: 1,
        phase: TimingPhase::Total,
        clock: TimingClock::CpuWall,
        elapsed_milliseconds: 12.5,
    }
}

#[test]
fn timing_comparison_rejects_substituted_conditions() {
    let raster = total_sample(RenderPathStrategy::Raster);
    let mut compute = total_sample(RenderPathStrategy::ComputeRay);
    compute.conditions.extent = [1280, 720];

    let error = compare_like_timing_totals(&raster, &compute)
        .expect_err("different extents must not produce a comparison");

    assert_eq!(
        error,
        EvidenceError::UnlikeTimingConditions { field: "extent" }
    );
}

#[test]
fn raw_timing_stream_requires_every_attributed_phase() {
    let mut samples = [
        TimingPhase::Preparation,
        TimingPhase::Upload,
        TimingPhase::Installation,
        TimingPhase::Dispatch,
        TimingPhase::Composite,
        TimingPhase::Presentation,
        TimingPhase::Switching,
        TimingPhase::Total,
    ]
    .into_iter()
    .enumerate()
    .map(|(index, phase)| TimingSample {
        stream_identity: "compute-cold-1".to_owned(),
        conditions: timing_conditions(RenderPathStrategy::ComputeRay),
        scenario: TimingScenario::ColdRequestToFirstMatchingFrame,
        sample_sequence: 1,
        phase,
        clock: if matches!(phase, TimingPhase::Dispatch | TimingPhase::Composite) {
            TimingClock::GpuTimestamp
        } else {
            TimingClock::CpuWall
        },
        elapsed_milliseconds: index as f64 + 1.0,
    })
    .collect::<Vec<_>>();
    samples.retain(|sample| sample.phase != TimingPhase::Composite);

    let error = verify_timing_phase_coverage(&samples)
        .expect_err("a stream without composite attribution must be incomplete");

    assert_eq!(
        error,
        EvidenceError::MissingTimingPhase {
            phase: TimingPhase::Composite,
        }
    );
}

#[test]
fn one_timing_stream_cannot_supply_another_streams_missing_phase() {
    let phases = [
        TimingPhase::Preparation,
        TimingPhase::Upload,
        TimingPhase::Installation,
        TimingPhase::Dispatch,
        TimingPhase::Composite,
        TimingPhase::Presentation,
        TimingPhase::Total,
    ];
    let mut samples = phases
        .into_iter()
        .map(|phase| TimingSample {
            stream_identity: "complete".to_owned(),
            conditions: timing_conditions(RenderPathStrategy::ComputeRay),
            scenario: TimingScenario::ColdRequestToFirstMatchingFrame,
            sample_sequence: 1,
            phase,
            clock: TimingClock::CpuWall,
            elapsed_milliseconds: 1.0,
        })
        .collect::<Vec<_>>();
    samples.extend(
        phases
            .into_iter()
            .filter(|phase| *phase != TimingPhase::Composite)
            .map(|phase| TimingSample {
                stream_identity: "incomplete".to_owned(),
                conditions: timing_conditions(RenderPathStrategy::ComputeRay),
                scenario: TimingScenario::ColdRequestToFirstMatchingFrame,
                sample_sequence: 2,
                phase,
                clock: TimingClock::CpuWall,
                elapsed_milliseconds: 1.0,
            }),
    );

    let error = verify_timing_phase_coverage(&samples)
        .expect_err("phase coverage must be checked per attributed stream");

    assert_eq!(
        error,
        EvidenceError::MissingTimingPhase {
            phase: TimingPhase::Composite,
        }
    );
}

fn ledger_attribution(state: ResourceState) -> ResourceLedgerAttribution {
    ResourceLedgerAttribution {
        render_path: RenderPathStrategy::ComputeRay,
        role: ResourceRole::Presenting,
        scene_identity: "canonical-scene".to_owned(),
        scene_revision: 4,
        generation: 7,
        state,
    }
}

#[test]
fn resource_ledger_rejects_a_live_allocation_at_shutdown() {
    let entries = vec![ResourceLedgerEntry {
        sequence: 1,
        resource_identity: "scene-buffer-generation-7".to_owned(),
        resource_type: ResourceType::VoxelWords,
        action: ResourceLedgerAction::Create,
        attribution: ledger_attribution(ResourceState::Installed),
        quantity: ResourceLedgerQuantity {
            bytes: 1024,
            objects: 1,
            allocations: 1,
            workers: 0,
            retained_views: 0,
        },
    }];

    let error = verify_resource_ledger(&entries)
        .expect_err("a created allocation without destruction must remain visible");

    assert_eq!(
        error,
        EvidenceError::LiveResourcesAtShutdown {
            identities: vec!["scene-buffer-generation-7".to_owned()],
        }
    );
}

#[test]
fn resource_ledger_rejects_missing_scene_attribution() {
    let mut attribution = ledger_attribution(ResourceState::Installed);
    attribution.scene_identity.clear();
    let entries = vec![ResourceLedgerEntry {
        sequence: 1,
        resource_identity: "scene-buffer-generation-7".to_owned(),
        resource_type: ResourceType::VoxelWords,
        action: ResourceLedgerAction::Create,
        attribution,
        quantity: ResourceLedgerQuantity {
            bytes: 1024,
            objects: 1,
            allocations: 1,
            workers: 0,
            retained_views: 0,
        },
    }];

    let error = verify_resource_ledger(&entries)
        .expect_err("a resource without scene attribution must be rejected");

    assert_eq!(
        error,
        EvidenceError::InvalidResourceAttribution {
            identity: "scene-buffer-generation-7".to_owned(),
            field: "scene_identity",
        }
    );
}

#[test]
fn empty_resource_ledger_is_not_balanced_evidence() {
    let error = verify_resource_ledger(&[])
        .expect_err("an empty ledger cannot prove that resource lifetimes balance");

    assert_eq!(error, EvidenceError::EmptyResourceLedger);
}

fn occupancy_gates() -> OccupancyQualificationGates {
    OccupancyQualificationGates {
        semantic_correctness: true,
        switching: true,
        convergence: true,
        lifecycle: true,
        failure_retry: true,
        shutdown: true,
        validation: true,
        resource_balance: true,
    }
}

fn paired_dispatch_samples(camera_identity: &str, values: &[(f64, f64)]) -> PairedDispatchSamples {
    let mut conditions = timing_conditions(RenderPathStrategy::ComputeRay);
    conditions.camera_identity = camera_identity.to_owned();
    PairedDispatchSamples {
        camera_identity: camera_identity.to_owned(),
        conditions,
        samples: values
            .iter()
            .enumerate()
            .map(
                |(index, (dense_gpu_milliseconds, occupancy_gpu_milliseconds))| {
                    PairedDispatchSample {
                        sample_sequence: index as u64,
                        dense_gpu_milliseconds: *dense_gpu_milliseconds,
                        occupancy_gpu_milliseconds: *occupancy_gpu_milliseconds,
                    }
                },
            )
            .collect(),
    }
}

#[test]
fn dense_dda_remains_selected_when_an_occupancy_interval_includes_zero()
-> Result<(), Box<dyn std::error::Error>> {
    let positive = paired_dispatch_samples(
        "overview",
        &[(10.0, 8.0), (11.0, 9.0), (12.0, 10.0), (13.0, 11.0)],
    );
    let ambiguous = paired_dispatch_samples(
        "cavity",
        &[(10.0, 9.0), (11.0, 12.0), (10.0, 9.0), (11.0, 12.0)],
    );
    let boundary = paired_dispatch_samples(
        "boundary",
        &[(20.0, 15.0), (20.0, 15.0), (20.0, 15.0), (20.0, 15.0)],
    );

    let report = select_compute_traversal(TraversalSelectionInput {
        occupancy_experiment: Some(OccupancyExperiment {
            block_extent: [16, 16, 16],
            qualification: occupancy_gates(),
            paired_dispatch_samples: vec![positive, ambiguous, boundary],
        }),
    })?;

    assert_eq!(report.selected, TraversalSelection::DenseDda);
    assert_eq!(report.camera_reports.len(), 3);
    assert!(
        report
            .camera_reports
            .iter()
            .find(|camera| camera.camera_identity == "cavity")
            .is_some_and(|camera| camera.bootstrap_95_percent.lower <= 0.0)
    );
    Ok(())
}

#[test]
fn occupancy_selection_rejects_cross_camera_machine_substitution() {
    let values = [(10.0, 8.0), (11.0, 9.0)];
    let overview = paired_dispatch_samples("overview", &values);
    let mut cavity = paired_dispatch_samples("cavity", &values);
    cavity.conditions.machine_identity = "different-machine".to_owned();
    let boundary = paired_dispatch_samples("boundary", &values);

    let error = select_compute_traversal(TraversalSelectionInput {
        occupancy_experiment: Some(OccupancyExperiment {
            block_extent: [16, 16, 16],
            qualification: occupancy_gates(),
            paired_dispatch_samples: vec![overview, cavity, boundary],
        }),
    })
    .expect_err("canonical camera samples must share machine-local conditions");

    assert_eq!(
        error,
        EvidenceError::InvalidPairedDispatchConditions {
            camera_identity: "cavity".to_owned(),
            field: "cross_camera_conditions",
        }
    );
}

fn ledger_entry(
    sequence: u64,
    identity: &str,
    action: ResourceLedgerAction,
    render_path: RenderPathStrategy,
    role: ResourceRole,
    state: ResourceState,
    bytes: u64,
) -> ResourceLedgerEntry {
    ResourceLedgerEntry {
        sequence,
        resource_identity: identity.to_owned(),
        resource_type: ResourceType::Other,
        action,
        attribution: ResourceLedgerAttribution {
            render_path,
            role,
            scene_identity: "canonical-scene".to_owned(),
            scene_revision: 4,
            generation: 1,
            state,
        },
        quantity: ResourceLedgerQuantity {
            bytes,
            objects: 1,
            allocations: 1,
            workers: 0,
            retained_views: 0,
        },
    }
}

#[test]
fn overlap_requires_simultaneous_raster_and_compute_ray_ownership()
-> Result<(), Box<dyn std::error::Error>> {
    let entries = vec![
        ledger_entry(
            1,
            "compute-presenting",
            ResourceLedgerAction::Create,
            RenderPathStrategy::ComputeRay,
            ResourceRole::Presenting,
            ResourceState::Installed,
            100,
        ),
        ledger_entry(
            2,
            "compute-replacement",
            ResourceLedgerAction::Create,
            RenderPathStrategy::ComputeRay,
            ResourceRole::Replacement,
            ResourceState::Preparing,
            200,
        ),
        ledger_entry(
            3,
            "compute-replacement",
            ResourceLedgerAction::Destroy,
            RenderPathStrategy::ComputeRay,
            ResourceRole::Replacement,
            ResourceState::Preparing,
            200,
        ),
        ledger_entry(
            4,
            "compute-presenting",
            ResourceLedgerAction::Destroy,
            RenderPathStrategy::ComputeRay,
            ResourceRole::Presenting,
            ResourceState::Installed,
            100,
        ),
    ];

    let summary = verify_resource_ledger(&entries)?;

    assert_eq!(summary.peak.bytes, 300);
    assert_eq!(summary.peak_overlap, ResourceLedgerQuantity::default());
    Ok(())
}

#[test]
fn resource_identity_cannot_change_render_path_during_transition() {
    let entries = vec![
        ledger_entry(
            1,
            "installed-path",
            ResourceLedgerAction::Create,
            RenderPathStrategy::Raster,
            ResourceRole::Presenting,
            ResourceState::Installed,
            100,
        ),
        ledger_entry(
            2,
            "installed-path",
            ResourceLedgerAction::Transition,
            RenderPathStrategy::ComputeRay,
            ResourceRole::Presenting,
            ResourceState::Installed,
            100,
        ),
    ];

    let error = verify_resource_ledger(&entries)
        .expect_err("a resource identity must keep its Render Path attribution");

    assert_eq!(
        error,
        EvidenceError::ResourceDefinitionChanged {
            identity: "installed-path".to_owned(),
        }
    );
}

fn append_timing_stream(
    samples: &mut Vec<TimingSample>,
    render_path: RenderPathStrategy,
    scenario: TimingScenario,
    clock: TimingClock,
    sample_sequence: u64,
) -> usize {
    let phases: &[TimingPhase] = match (render_path, scenario) {
        (RenderPathStrategy::ComputeRay, TimingScenario::ColdRequestToFirstMatchingFrame) => &[
            TimingPhase::Preparation,
            TimingPhase::Upload,
            TimingPhase::Installation,
            TimingPhase::Dispatch,
            TimingPhase::Composite,
            TimingPhase::Presentation,
            TimingPhase::Total,
        ],
        (
            RenderPathStrategy::ComputeRay,
            TimingScenario::ChangedSubmissionToFinalVisibleRevision,
        ) => &[
            TimingPhase::Preparation,
            TimingPhase::Upload,
            TimingPhase::Installation,
            TimingPhase::Dispatch,
            TimingPhase::Composite,
            TimingPhase::Presentation,
            TimingPhase::Total,
        ],
        (RenderPathStrategy::ComputeRay, TimingScenario::SteadyFrame) => &[
            TimingPhase::Dispatch,
            TimingPhase::Composite,
            TimingPhase::Presentation,
            TimingPhase::Total,
        ],
        (RenderPathStrategy::Raster, TimingScenario::ColdRequestToFirstMatchingFrame) => &[
            TimingPhase::Preparation,
            TimingPhase::Upload,
            TimingPhase::Installation,
            TimingPhase::Presentation,
            TimingPhase::Total,
        ],
        (RenderPathStrategy::Raster, TimingScenario::ChangedSubmissionToFinalVisibleRevision) => &[
            TimingPhase::Preparation,
            TimingPhase::Upload,
            TimingPhase::Installation,
            TimingPhase::Presentation,
            TimingPhase::Total,
        ],
        (RenderPathStrategy::Raster, TimingScenario::SteadyFrame) => {
            &[TimingPhase::Presentation, TimingPhase::Total]
        }
        (RenderPathStrategy::ComputeRay, TimingScenario::Switching) => &[
            TimingPhase::Preparation,
            TimingPhase::Upload,
            TimingPhase::Installation,
            TimingPhase::Dispatch,
            TimingPhase::Composite,
            TimingPhase::Presentation,
            TimingPhase::Switching,
            TimingPhase::Total,
        ],
        (RenderPathStrategy::Raster, TimingScenario::Switching) => &[
            TimingPhase::Preparation,
            TimingPhase::Upload,
            TimingPhase::Installation,
            TimingPhase::Presentation,
            TimingPhase::Switching,
            TimingPhase::Total,
        ],
    };
    let stream_identity = format!("{render_path:?}-{scenario:?}-{clock:?}-{sample_sequence}");
    let mut total_index = samples.len();
    for (index, phase) in phases.iter().copied().enumerate() {
        if phase == TimingPhase::Total {
            total_index = samples.len();
        }
        samples.push(TimingSample {
            stream_identity: stream_identity.clone(),
            conditions: timing_conditions(render_path),
            scenario,
            sample_sequence,
            phase,
            clock: if phase == TimingPhase::Total {
                clock
            } else {
                TimingClock::CpuWall
            },
            elapsed_milliseconds: index as f64 + 1.0,
        });
    }
    total_index
}

#[test]
fn focused_render_path_report_retains_comparisons_overlap_and_dense_baseline()
-> Result<(), Box<dyn std::error::Error>> {
    let mut timing_samples = Vec::new();
    let mut comparison_pairs = Vec::new();
    for (sample_sequence, (scenario, clock)) in [
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
    ]
    .into_iter()
    .enumerate()
    {
        let sample_sequence = sample_sequence as u64 + 1;
        let left_sample_index = append_timing_stream(
            &mut timing_samples,
            RenderPathStrategy::Raster,
            scenario,
            clock,
            sample_sequence,
        );
        let right_sample_index = append_timing_stream(
            &mut timing_samples,
            RenderPathStrategy::ComputeRay,
            scenario,
            clock,
            sample_sequence,
        );
        comparison_pairs.push(TimingComparisonPair {
            left_sample_index,
            right_sample_index,
        });
    }
    append_timing_stream(
        &mut timing_samples,
        RenderPathStrategy::Raster,
        TimingScenario::Switching,
        TimingClock::CpuWall,
        5,
    );
    append_timing_stream(
        &mut timing_samples,
        RenderPathStrategy::ComputeRay,
        TimingScenario::Switching,
        TimingClock::CpuWall,
        5,
    );
    let resource_ledger = vec![
        ledger_entry(
            1,
            "raster-installed",
            ResourceLedgerAction::Create,
            RenderPathStrategy::Raster,
            ResourceRole::Presenting,
            ResourceState::Installed,
            100,
        ),
        ledger_entry(
            2,
            "compute-installed",
            ResourceLedgerAction::Create,
            RenderPathStrategy::ComputeRay,
            ResourceRole::Replacement,
            ResourceState::Installed,
            200,
        ),
        ledger_entry(
            3,
            "compute-installed",
            ResourceLedgerAction::Transition,
            RenderPathStrategy::ComputeRay,
            ResourceRole::Presenting,
            ResourceState::Installed,
            200,
        ),
        ledger_entry(
            4,
            "raster-installed",
            ResourceLedgerAction::Transition,
            RenderPathStrategy::Raster,
            ResourceRole::Retiring,
            ResourceState::Retiring,
            100,
        ),
        ledger_entry(
            5,
            "raster-installed",
            ResourceLedgerAction::Destroy,
            RenderPathStrategy::Raster,
            ResourceRole::Retiring,
            ResourceState::Retiring,
            100,
        ),
        ledger_entry(
            6,
            "compute-installed",
            ResourceLedgerAction::Destroy,
            RenderPathStrategy::ComputeRay,
            ResourceRole::Presenting,
            ResourceState::Installed,
            200,
        ),
    ];

    let report = retain_render_path_evidence(RenderPathEvidenceInput {
        schema_version: 1,
        timing_samples,
        timing_comparisons: comparison_pairs,
        resource_ledger,
        lifecycle_events: vec![
            LifecycleEvidenceEvent {
                event_identity: "failed-install-retried".to_owned(),
                kind: LifecycleEvidenceKind::FailureRetry,
                attribution: ResourceLedgerAttribution {
                    render_path: RenderPathStrategy::ComputeRay,
                    role: ResourceRole::Replacement,
                    scene_identity: "canonical-scene".to_owned(),
                    scene_revision: 4,
                    generation: 1,
                    state: ResourceState::Installed,
                },
                diagnostic_identity: "failure-retry-log-1".to_owned(),
            },
            LifecycleEvidenceEvent {
                event_identity: "raster-retired".to_owned(),
                kind: LifecycleEvidenceKind::Retirement,
                attribution: ResourceLedgerAttribution {
                    render_path: RenderPathStrategy::Raster,
                    role: ResourceRole::Retiring,
                    scene_identity: "canonical-scene".to_owned(),
                    scene_revision: 4,
                    generation: 1,
                    state: ResourceState::Retiring,
                },
                diagnostic_identity: "retirement-log-1".to_owned(),
            },
            LifecycleEvidenceEvent {
                event_identity: "compute-shutdown".to_owned(),
                kind: LifecycleEvidenceKind::Shutdown,
                attribution: ResourceLedgerAttribution {
                    render_path: RenderPathStrategy::ComputeRay,
                    role: ResourceRole::Presenting,
                    scene_identity: "canonical-scene".to_owned(),
                    scene_revision: 4,
                    generation: 1,
                    state: ResourceState::Installed,
                },
                diagnostic_identity: "shutdown-log-1".to_owned(),
            },
        ],
        traversal_selection: TraversalSelectionInput::default(),
    })?;

    assert_eq!(report.timing_comparisons.len(), 4);
    assert!(!report.raw_timing_samples.is_empty());
    assert_eq!(report.lifecycle_events.len(), 3);
    assert_eq!(report.resources.peak.bytes, 300);
    assert_eq!(report.resources.peak_overlap.bytes, 300);
    assert_eq!(report.resources.transition_count, 2);
    assert_eq!(report.traversal.selected, TraversalSelection::DenseDda);
    assert_eq!(
        report.scope,
        "Descriptive measurements for the attributed machine only; no Render Path superiority claim."
    );
    Ok(())
}

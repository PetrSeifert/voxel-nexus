use measurement_evidence::large_sparse::{LargeSparseRecord, validate};

#[test]
fn measured_budget_is_enforced_independently_of_prediction() {
    let records = vec![LargeSparseRecord::Memory {
        phase: "growth".into(),
        publication_peak_heap_bytes: 0,
        enumeration_peak_heap_bytes: 0,
        enumeration_output_bytes: 0,
        enumeration_working_state_bytes: 0,
        enumeration_working_cells: 0,
        preparation_peak_heap_bytes: 0,
        staging_bytes: 128,
        visible_candidate_gpu_bytes: 1_073_741_824,
        measured_gpu_scene_peak_bytes: 1_073_741_952,
    }];
    assert!(
        validate(&[records])
            .unwrap_err()
            .to_string()
            .contains("exceeds")
    );
}

fn complete_run() -> Vec<LargeSparseRecord> {
    use measurement_evidence::large_sparse::{TIMINGS, TRANSITIONS};
    let mut records = vec![
        LargeSparseRecord::ValidationAllocation {
            allocated_bytes: 32,
            peak_working_bytes: 32,
        },
        LargeSparseRecord::Completed {
            validation_errors: 0,
            validation_warnings: 0,
            shutdown_resources_zero: true,
        },
        LargeSparseRecord::Context {
            schema_version: 1,
            device: "test-device".into(),
            driver_version: 1,
            max_storage_buffer_range_bytes: 4_294_967_295,
            dense_payload_bytes: 4_294_967_296,
            extent: [2048, 256, 2048],
            presentation_extent: [1920, 1080],
            fingerprint: "58ce86bc1227cf55".into(),
        },
        LargeSparseRecord::Growth {
            old_capacity: 83_200,
            new_capacity: 124_800,
            predicted_peak_bytes: 340,
            actual_peak_bytes: 320,
        },
        LargeSparseRecord::PredictedRejection {
            predicted_bytes: 8444,
            budget_bytes: 8443,
            rejected: true,
        },
    ];
    for name in TIMINGS {
        for _ in 0..if name.starts_with("steady_") { 30 } else { 1 } {
            records.push(LargeSparseRecord::Timing {
                name: (*name).into(),
                milliseconds: 2.0,
            });
        }
    }
    for (index, transition) in TRANSITIONS.iter().enumerate() {
        records.push(LargeSparseRecord::Edit {
            transition: (*transition).into(),
            required_revision: index as u64 + 2,
            visible_revision: index as u64 + 2,
            uploaded_bytes: 1028,
        });
    }
    for phase in ["initial", "edits", "growth"] {
        records.push(LargeSparseRecord::Memory {
            phase: phase.into(),
            publication_peak_heap_bytes: 50,
            enumeration_peak_heap_bytes: 20,
            enumeration_output_bytes: 10,
            enumeration_working_state_bytes: 10,
            enumeration_working_cells: 1,
            preparation_peak_heap_bytes: 90,
            staging_bytes: 100,
            visible_candidate_gpu_bytes: 220,
            measured_gpu_scene_peak_bytes: 320,
        });
    }
    for phase in [
        "initial",
        "uniform_to_mixed",
        "mixed_to_uniform",
        "empty_to_mixed",
        "mixed_to_empty",
        "growth",
    ] {
        records.push(LargeSparseRecord::Verified {
            phase: phase.into(),
            probe_count: if phase == "growth" { 14 } else { 13 },
            installed_revision: match phase {
                "initial" => 1,
                "uniform_to_mixed" => 2,
                "mixed_to_uniform" => 3,
                "empty_to_mixed" => 4,
                "mixed_to_empty" => 5,
                _ => 6,
            },
        });
    }
    records
}

#[test]
fn every_record_round_trips_and_every_timing_has_a_distribution() {
    let run = complete_run();
    let decoded = run
        .iter()
        .map(|record| serde_json::from_str(&serde_json::to_string(record).unwrap()).unwrap())
        .collect::<Vec<LargeSparseRecord>>();
    assert_eq!(decoded, run);
    let report = validate(&[run.clone(), run.clone(), run]).unwrap();
    assert_eq!(report.timings["publication"].count, 3);
    assert_eq!(report.timings["steady_gpu"].count, 90);
    assert_eq!(report.timings["growth"].median, 2.0);
}

#[test]
fn incomplete_duplicate_or_unqualified_evidence_is_rejected() {
    let run = complete_run();
    assert!(validate(std::slice::from_ref(&run)).is_err());
    for index in 0..run.len() {
        if matches!(run.get(index), Some(LargeSparseRecord::Timing { name, .. }) if name.starts_with("steady_"))
        {
            continue;
        }
        let mut missing = run.clone();
        missing.remove(index);
        assert!(
            validate(&[missing, run.clone(), run.clone()]).is_err(),
            "missing record {index}"
        );
    }
    for bad in [
        LargeSparseRecord::PredictedRejection {
            predicted_bytes: 8444,
            budget_bytes: 8443,
            rejected: false,
        },
        LargeSparseRecord::Timing {
            name: "publication".into(),
            milliseconds: f64::NAN,
        },
        LargeSparseRecord::Edit {
            transition: "empty_to_mixed".into(),
            required_revision: 4,
            visible_revision: 3,
            uploaded_bytes: 1028,
        },
    ] {
        let mut broken = run.clone();
        broken.push(bad);
        assert!(validate(&[broken, run.clone(), run.clone()]).is_err());
    }
}

#[test]
fn contradictory_growth_memory_is_rejected() {
    let mut run = complete_run();
    for record in &mut run {
        if let LargeSparseRecord::Growth {
            actual_peak_bytes, ..
        } = record
        {
            *actual_peak_bytes = 400;
        }
    }
    assert!(validate(&[run.clone(), run.clone(), run]).is_err());
}

#[test]
fn measured_growth_cannot_exceed_prediction() {
    let mut run = complete_run();
    for record in &mut run {
        if let LargeSparseRecord::Growth {
            predicted_peak_bytes,
            ..
        } = record
        {
            *predicted_peak_bytes = 319;
        }
    }
    assert!(validate(&[run.clone(), run.clone(), run]).is_err());
}

#[test]
fn predicted_rejection_uses_observed_values() {
    let mut run = complete_run();
    for record in &mut run {
        if let LargeSparseRecord::PredictedRejection {
            predicted_bytes,
            budget_bytes,
            ..
        } = record
        {
            *predicted_bytes = 8468;
            *budget_bytes = 8444;
        }
    }
    assert!(validate(&[run.clone(), run.clone(), run.clone()]).is_ok());
    for record in &mut run {
        if let LargeSparseRecord::PredictedRejection { budget_bytes, .. } = record {
            *budget_bytes = 8468;
        }
    }
    assert!(validate(&[run.clone(), run.clone(), run]).is_err());
}

#[test]
fn evidence_requires_successful_validation_and_shutdown() {
    let mut run = complete_run();
    run.push(LargeSparseRecord::Completed {
        validation_errors: 1,
        validation_warnings: 0,
        shutdown_resources_zero: true,
    });
    assert!(validate(&[run.clone(), run.clone(), run]).is_err());
}

#[test]
fn budget_boundary_is_inclusive_and_device_limit_is_exact() {
    let mut run = complete_run();
    for record in &mut run {
        match record {
            LargeSparseRecord::Growth {
                actual_peak_bytes,
                predicted_peak_bytes,
                ..
            } => {
                *actual_peak_bytes = 1_073_741_824;
                *predicted_peak_bytes = 1_073_741_824;
            }
            LargeSparseRecord::Memory {
                phase,
                visible_candidate_gpu_bytes,
                measured_gpu_scene_peak_bytes,
                ..
            } if phase == "growth" => {
                *visible_candidate_gpu_bytes = 1_073_741_724;
                *measured_gpu_scene_peak_bytes = 1_073_741_824;
            }
            _ => {}
        }
    }
    assert_eq!(
        validate(&[run.clone(), run.clone(), run.clone()])
            .unwrap()
            .measured_peak_bytes,
        1_073_741_824
    );
    for record in &mut run {
        if let LargeSparseRecord::Context {
            max_storage_buffer_range_bytes,
            ..
        } = record
        {
            *max_storage_buffer_range_bytes = 4_294_967_296;
        }
    }
    assert!(validate(&[run.clone(), run.clone(), run]).is_err());
}
#[test]
fn actual_first_preparation_timings_are_required() {
    let run = complete_run();
    let report = validate(&[run.clone(), run.clone(), run.clone()]).unwrap();
    for name in ["enumeration", "construction", "serialization"] {
        assert!(report.timings.contains_key(name), "missing {name}");
        let missing = run.iter().filter(|record| !matches!(record, LargeSparseRecord::Timing { name: recorded, .. } if recorded == name)).cloned().collect::<Vec<_>>();
        assert!(validate(&[missing, run.clone(), run.clone()]).is_err());
    }
}

#[test]
fn verification_requires_the_installed_revision_and_validation_allocation() {
    for replacement in [
        LargeSparseRecord::Verified {
            phase: "initial".into(),
            probe_count: 13,
            installed_revision: 2,
        },
        LargeSparseRecord::ValidationAllocation {
            allocated_bytes: 32,
            peak_working_bytes: 33,
        },
        LargeSparseRecord::ValidationAllocation {
            allocated_bytes: 0,
            peak_working_bytes: 0,
        },
    ] {
        let mut run = complete_run();
        for record in &mut run {
            if matches!((&*record, &replacement),
                (LargeSparseRecord::Verified { phase, .. }, LargeSparseRecord::Verified { .. }) if phase == "initial")
                || matches!(
                    (&*record, &replacement),
                    (
                        LargeSparseRecord::ValidationAllocation { .. },
                        LargeSparseRecord::ValidationAllocation { .. }
                    )
                )
            {
                *record = replacement.clone();
            }
        }
        assert!(validate(&[run.clone(), run.clone(), run]).is_err());
    }
}

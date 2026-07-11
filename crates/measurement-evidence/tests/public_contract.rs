use measurement_evidence::{
    EvidenceError, FirstCorrectFramePhases, MeasurementEvent, ResourceCounts,
    ScaleAggregationInput, VoxelSceneRevisionIdentity, aggregate_scales, summarize,
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

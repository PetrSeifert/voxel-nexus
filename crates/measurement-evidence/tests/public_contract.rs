use measurement_evidence::{
    EvidenceError, FirstCorrectFramePhases, MeasurementEvent, ResourceCounts,
    aggregate_first_correct_frame, summarize,
};
use std::collections::BTreeMap;

#[test]
fn timing_event_serialization_is_machine_readable_and_phase_specific()
-> Result<(), Box<dyn std::error::Error>> {
    let event = MeasurementEvent::ArtifactDerived {
        source_revision: "1".to_owned(),
        elapsed_ms: 12.5,
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
fn summaries_use_literal_median_nearest_rank_p95_and_maximum()
-> Result<(), Box<dyn std::error::Error>> {
    let summary = summarize(&[10.0, 2.0, 8.0, 4.0, 6.0, 20.0, 12.0, 18.0, 14.0, 16.0])?;

    assert_eq!(summary.count, 10);
    assert_eq!(summary.median, 11.0);
    assert_eq!(summary.p95, 20.0);
    assert_eq!(summary.maximum, 20.0);
    Ok(())
}

#[test]
fn manifest_aggregation_rejects_missing_fresh_runs() {
    let samples = vec![FirstCorrectFramePhases {
        derivation_ms: 1.0,
        upload_install_ms: 2.0,
        presentation_ms: 3.0,
        total_ms: 6.0,
    }];
    let error = aggregate_first_correct_frame(BTreeMap::from([(64, samples)]), 10)
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

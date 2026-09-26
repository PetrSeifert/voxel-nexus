use super::{
    CanonicalCameraPose, CpuFrameMeasurement, MeasurementEvent, SteadyFrameCollection,
    compute_edit_burst_admission, fixed_edit_burst, format_convergence_characterization,
    format_convergence_overlay, format_render_path_overlay, render_path_switch_admission,
    should_request_compute_edit_burst, should_request_render_path_switch, should_start_edit_burst,
};
#[cfg(feature = "qualification")]
use super::{
    ComputeShutdownQualification, parse_render_configuration,
    should_wait_for_initial_raster_artifact,
};

#[test]
#[cfg(feature = "qualification")]
fn fixed_candidate_raster_region_extent_is_explicitly_configurable() -> Result<(), String> {
    let (configuration, report_only) = parse_render_configuration(
        [
            "--scene-scale",
            "256",
            "--raster-region-extent",
            "64",
            "--edit-burst-demo",
        ]
        .into_iter()
        .map(str::to_owned),
    )?;

    assert_eq!(configuration.raster_region_extent, 64);
    assert!(!report_only);
    Ok(())
}

#[test]
#[cfg(feature = "qualification")]
fn compute_switch_demo_is_explicit_and_does_not_replace_raster_qualification_modes()
-> Result<(), String> {
    let (configuration, _) =
        parse_render_configuration(["--compute-switch-demo"].into_iter().map(str::to_owned))?;
    assert!(configuration.compute_switch_demo);
    assert!(!configuration.compute_switch_lifecycle_demo);

    let (lifecycle_configuration, _) = parse_render_configuration(
        ["--compute-switch-lifecycle-demo"]
            .into_iter()
            .map(str::to_owned),
    )?;
    assert!(lifecycle_configuration.compute_switch_demo);
    assert!(lifecycle_configuration.compute_switch_lifecycle_demo);

    let (milestone_configuration, _) = parse_render_configuration(
        ["--portable-compute-ray-milestone-demo"]
            .into_iter()
            .map(str::to_owned),
    )?;
    assert!(milestone_configuration.compute_switch_demo);
    assert!(milestone_configuration.compute_switch_lifecycle_demo);
    assert!(milestone_configuration.portable_milestone_demo);

    let (timing_configuration, _) = parse_render_configuration(
        ["--portable-compute-ray-milestone-timing"]
            .into_iter()
            .map(str::to_owned),
    )?;
    assert!(timing_configuration.portable_milestone_demo);
    assert!(timing_configuration.portable_milestone_timing);

    for (argument, expected) in [
        (
            "active-preparation",
            ComputeShutdownQualification::ActivePreparation,
        ),
        (
            "hidden-candidate",
            ComputeShutdownQualification::HiddenCandidate,
        ),
        ("presenting", ComputeShutdownQualification::Presenting),
        ("replacement", ComputeShutdownQualification::Replacement),
    ] {
        let (qualification, _) = parse_render_configuration(
            ["--compute-shutdown-qualification", argument]
                .into_iter()
                .map(str::to_owned),
        )?;
        assert!(qualification.compute_switch_demo);
        assert_eq!(qualification.compute_shutdown_qualification, Some(expected));
    }

    let incompatible = parse_render_configuration(
        ["--compute-switch-demo", "--edit-burst-demo"]
            .into_iter()
            .map(str::to_owned),
    );
    assert!(incompatible.is_err());
    Ok(())
}

#[test]
#[cfg(feature = "qualification")]
fn every_compute_switch_mode_waits_for_the_initial_raster_artifact() -> Result<(), String> {
    for arguments in [
        vec!["--compute-switch-demo"],
        vec!["--compute-switch-lifecycle-demo"],
        vec!["--portable-compute-ray-milestone-demo"],
        vec!["--compute-shutdown-qualification", "presenting"],
    ] {
        let (configuration, _) =
            parse_render_configuration(arguments.into_iter().map(str::to_owned))?;
        assert!(should_wait_for_initial_raster_artifact(
            &configuration,
            false
        ));
        assert!(!should_wait_for_initial_raster_artifact(
            &configuration,
            true
        ));
    }

    let (raster_configuration, _) = parse_render_configuration(std::iter::empty())?;
    assert!(!should_wait_for_initial_raster_artifact(
        &raster_configuration,
        false
    ));
    Ok(())
}

#[test]
fn characterization_line_retains_every_phase_disposition_and_lifecycle_event() {
    let report = format_convergence_characterization(&RasterConvergenceCharacterization {
        phases: RasterConvergencePhaseTimings {
            submission_bookkeeping_milliseconds: 1.0,
            queued_wait_milliseconds: 2.0,
            cpu_derivation_milliseconds: 3.0,
            upload_milliseconds: 4.0,
            frame_boundary_commit_milliseconds: 5.0,
        },
        work: RasterRegionWorkDisposition {
            scheduled: 6,
            completed: 7,
            cancelled: 8,
            stale: 9,
        },
        installed: RasterGpuResourceUsage {
            bytes: 10,
            resources: 11,
        },
        hidden: RasterGpuResourceUsage {
            bytes: 12,
            resources: 13,
        },
        retired: RasterGpuResourceUsage {
            bytes: 14,
            resources: 15,
        },
        peak: RasterGpuResourceUsage {
            bytes: 16,
            resources: 17,
        },
        cancellation_observations: vec![RasterCancellationObservation {
            revision: VoxelSceneRevision::new(2),
            scheduled_regions: 18,
            completed_regions: 19,
            cancelled_regions: 20,
        }],
        safe_retirements: vec![RasterSafeRetirementEvent {
            revision: VoxelSceneRevision::new(3),
            disposition: RasterSafeRetirementDisposition::StaleCandidate,
            resources: RasterGpuResourceUsage {
                bytes: 21,
                resources: 22,
            },
        }],
    });

    for required in [
        "submission_bookkeeping_ms=1.000000",
        "queued_wait_ms=2.000000",
        "cpu_derivation_ms=3.000000",
        "upload_ms=4.000000",
        "frame_boundary_commit_ms=5.000000",
        "scheduled_regions=6 completed_regions=7 cancelled_regions=8 stale_regions=9",
        "installed_bytes=10 installed_resources=11 hidden_bytes=12 hidden_resources=13",
        "retired_bytes=14 retired_resources=15 peak_bytes=16 peak_resources=17",
        "cancellation_events=2:18:19:20",
        "safe_retirement_events=3:stale-candidate:21:22",
    ] {
        assert!(
            report.contains(required),
            "missing {required:?} from {report}"
        );
    }
}

use canonical_scene::{CanonicalSceneScale, generate_canonical_scene};
use raster_render_path::{
    CameraPose, RasterCancellationObservation, RasterConvergenceCharacterization,
    RasterConvergencePhaseTimings, RasterConvergenceStatus, RasterGpuResourceUsage,
    RasterRegionWorkDisposition, RasterRenderPathAdapter, RasterSafeRetirementDisposition,
    RasterSafeRetirementEvent,
};
use render_backend::{CameraStateRevision, FrameObservation, RenderPathSwitchOwner};
use std::time::{Duration, Instant};
use voxel_frontend::{VoxelEditOutcome, VoxelFrontend, VoxelSceneRevision};
use winit::{
    event::ElementState,
    keyboard::{Key, NamedKey},
};

#[test]
fn only_one_non_repeated_space_press_can_start_an_awaiting_edit_burst() {
    let space = Key::Named(NamedKey::Space);
    assert!(should_start_edit_burst(
        true,
        true,
        ElementState::Pressed,
        false,
        &space
    ));
    assert!(!should_start_edit_burst(
        false,
        true,
        ElementState::Pressed,
        false,
        &space
    ));
    assert!(!should_start_edit_burst(
        true,
        false,
        ElementState::Pressed,
        false,
        &space
    ));
    assert!(!should_start_edit_burst(
        true,
        true,
        ElementState::Pressed,
        true,
        &space
    ));
    assert!(!should_start_edit_burst(
        true,
        true,
        ElementState::Released,
        false,
        &space
    ));
}

#[test]
fn only_one_non_repeated_tab_press_requests_an_interactive_switch() {
    let tab = Key::Named(NamedKey::Tab);
    assert!(should_request_render_path_switch(
        true,
        ElementState::Pressed,
        false,
        &tab
    ));
    assert!(!should_request_render_path_switch(
        false,
        ElementState::Pressed,
        false,
        &tab
    ));
    assert!(!should_request_render_path_switch(
        true,
        ElementState::Pressed,
        true,
        &tab
    ));
    assert!(!should_request_render_path_switch(
        true,
        ElementState::Released,
        false,
        &tab
    ));
    assert!(!should_request_render_path_switch(
        true,
        ElementState::Pressed,
        false,
        &Key::Named(NamedKey::Space)
    ));
}

#[test]
fn space_requests_are_detected_even_when_compute_burst_admission_will_reject_them() {
    let space = Key::Named(NamedKey::Space);
    assert!(should_request_compute_edit_burst(
        true,
        ElementState::Pressed,
        false,
        &space
    ));
    assert!(!should_request_compute_edit_burst(
        false,
        ElementState::Pressed,
        false,
        &space
    ));
    assert!(!should_request_compute_edit_burst(
        true,
        ElementState::Pressed,
        true,
        &space
    ));
    assert!(!should_request_compute_edit_burst(
        true,
        ElementState::Released,
        false,
        &space
    ));
}

#[test]
fn compute_edit_burst_admission_requires_an_idle_compute_presenter_and_awaiting_plan()
-> Result<(), Box<dyn std::error::Error>> {
    let frontend = VoxelFrontend::new();
    let view =
        frontend.publish(generate_canonical_scene(CanonicalSceneScale::Small)?.into_scene())?;
    let compute = compute_ray_render_path::ComputeRayRenderPathAdapter::new(
        view,
        CanonicalCameraPose::Overview.pose()?,
        CameraStateRevision::new(1),
    )?;
    let owner = RenderPathSwitchOwner::new(Box::new(compute));
    let diagnostics = owner.diagnostics();

    assert!(compute_edit_burst_admission(&diagnostics, true).is_ok());
    assert!(compute_edit_burst_admission(&diagnostics, false).is_err());

    let revision = VoxelSceneRevision::new(1);
    let (raster, _, _) = RasterRenderPathAdapter::awaiting_artifact_with_camera_control(
        CanonicalCameraPose::Overview.pose()?,
        CameraStateRevision::new(1),
        voxel_frontend::VoxelSceneId::new("raster"),
        revision,
    );
    let raster_diagnostics = RenderPathSwitchOwner::new(Box::new(raster)).diagnostics();
    assert!(compute_edit_burst_admission(&raster_diagnostics, true).is_err());
    Ok(())
}

#[test]
fn render_path_overlay_reports_the_idle_presenter_and_rejects_a_preparing_presenter()
-> Result<(), Box<dyn std::error::Error>> {
    let revision = VoxelSceneRevision::new(4);
    let (raster, _, _) = RasterRenderPathAdapter::awaiting_artifact_with_camera_control(
        CameraPose::new(
            [2.0, 2.0, 2.0],
            [0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            60.0,
            0.1,
            100.0,
        )?,
        CameraStateRevision::new(7),
        voxel_frontend::VoxelSceneId::new("revision-four"),
        revision,
    );
    let owner = RenderPathSwitchOwner::new(Box::new(raster));
    let diagnostics = owner.diagnostics();

    assert!(render_path_switch_admission(&diagnostics).is_err());
    assert_eq!(
        format_render_path_overlay(
            &diagnostics,
            "inactive",
            "overview",
            "Tab-rejected-presenter-not-ready",
        ),
        "Presenter=voxel-nexus.raster Switch=idle ReplacementRevision=none Required=4 Visible=4 Burst=inactive Camera=overview Control=Tab-rejected-presenter-not-ready"
    );
    Ok(())
}

#[test]
fn in_client_overlay_text_reports_both_revisions_and_region_counts() {
    assert_eq!(
        format_convergence_overlay(
            "cpu-barrier-held",
            RasterConvergenceStatus {
                required_revision: VoxelSceneRevision::new(3),
                visible_revision: VoxelSceneRevision::new(1),
                affected_region_count: 2,
                unaffected_region_count: 254,
            },
            "cavity",
        ),
        "EditBurst=cpu-barrier-held Required=3 Visible=1 Affected=2 Unaffected=254 Camera=cavity"
    );
}

#[test]
fn fixed_edit_burst_has_three_ordered_value_changing_commands_and_checked_final_revision()
-> Result<(), Box<dyn std::error::Error>> {
    for scale in [
        CanonicalSceneScale::Small,
        CanonicalSceneScale::Medium,
        CanonicalSceneScale::Large,
    ] {
        let frontend = VoxelFrontend::new();
        let view = frontend.publish(generate_canonical_scene(scale)?.into_scene())?;
        let mut plan = fixed_edit_burst(&view, 16)?;
        assert_eq!(plan.commands.len(), 3);
        assert_eq!(plan.expected_final_revision, VoxelSceneRevision::new(4));
        assert!(plan.take_next_owned_command().is_err());
        plan.claim_space_keypress()?;
        for expected_revision in 2..=4 {
            let command = plan.take_next_owned_command()?;
            let outcome = frontend.edit(command)?;
            let VoxelEditOutcome::Changed { view, .. } = outcome else {
                return Err("fixed command did not change its Voxel Value".into());
            };
            assert_eq!(view.revision(), VoxelSceneRevision::new(expected_revision));
        }
    }
    Ok(())
}

#[test]
fn steady_collection_pairs_sequences_and_excludes_pre_warmup_frames()
-> Result<(), Box<dyn std::error::Error>> {
    let matching_presentation_at = Instant::now();
    let mut collection = SteadyFrameCollection::new(matching_presentation_at);
    collection.submit(CpuFrameMeasurement {
        sequence: 10,
        started_at: matching_presentation_at + Duration::from_secs(4),
        milliseconds: 2.0,
    });
    assert_eq!(
        collection.complete(FrameObservation {
            sequence: 10,
            gpu_frame_milliseconds: 1.0,
        })?,
        None
    );

    collection.submit(CpuFrameMeasurement {
        sequence: 11,
        started_at: matching_presentation_at + Duration::from_secs(6),
        milliseconds: 2.5,
    });
    assert_eq!(
        collection.complete(FrameObservation {
            sequence: 11,
            gpu_frame_milliseconds: 1.5,
        })?,
        Some(MeasurementEvent::SteadyFrame {
            sequence: 11,
            cpu_frame_milliseconds: 2.5,
            gpu_frame_milliseconds: 1.5,
        })
    );
    Ok(())
}

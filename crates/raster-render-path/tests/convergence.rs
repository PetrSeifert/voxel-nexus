use raster_render_path::{
    RasterConvergence, RasterConvergenceAcceptance, RasterConvergenceEvent, RasterConvergenceRetry,
    RasterPreparationDisposition, RasterRenderPath, derive_raster_regions,
};
use std::thread;
use std::time::{Duration, Instant};
use voxel_frontend::{
    DenseVoxelBatch, DenseVoxelScene, DenseVoxelVolume, VoxelCoordinate, VoxelEditCommand,
    VoxelExtent, VoxelFrontend, VoxelMaterial, VoxelMaterialId, VoxelRegion, VoxelSceneId,
    VoxelSceneRevision, VoxelValue, VoxelVolumeId, VoxelVolumeMetadata,
};

fn frontend(
    scene_identity: &str,
    revision: u64,
    extent: VoxelExtent,
) -> Result<VoxelFrontend, Box<dyn std::error::Error>> {
    let [width, height, depth] = extent.dimensions();
    let value_count = usize::try_from(width)?
        .checked_mul(usize::try_from(height)?)
        .and_then(|count| count.checked_mul(usize::try_from(depth).ok()?))
        .ok_or("test volume size overflow")?;
    let frontend = VoxelFrontend::new();
    frontend.publish(DenseVoxelScene::new(
        VoxelSceneId::new(scene_identity),
        VoxelSceneRevision::new(revision),
        vec![VoxelMaterial::new(
            VoxelMaterialId::new("stone"),
            [0.2, 0.3, 0.4, 1.0],
        )],
        vec![DenseVoxelVolume::new(
            VoxelVolumeMetadata::new(VoxelVolumeId::new("terrain"), extent, [0.0, 0.0, 0.0], 1.0),
            vec![DenseVoxelBatch::new(
                VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), extent),
                vec![VoxelValue::Empty; value_count],
            )],
        )],
    ))?;
    Ok(frontend)
}

fn changed_edit(
    frontend: &VoxelFrontend,
    coordinate: VoxelCoordinate,
) -> Result<voxel_frontend::VoxelEditOutcome, voxel_frontend::VoxelFrontendError> {
    frontend.edit(VoxelEditCommand::new(
        VoxelVolumeId::new("terrain"),
        coordinate,
        VoxelValue::Occupied(VoxelMaterialId::new("stone")),
    ))
}

fn convergence(
    frontend: &VoxelFrontend,
    region_extent: VoxelExtent,
) -> Result<RasterConvergence, Box<dyn std::error::Error>> {
    let mut render_path = RasterRenderPath::new();
    render_path.install_artifact(derive_raster_regions(
        &frontend.scene_view()?,
        region_extent,
    )?);
    Ok(RasterConvergence::from_visible(&render_path)?)
}

fn drain_until_ready(
    convergence: &mut RasterConvergence,
    revision: VoxelSceneRevision,
) -> Result<Vec<RasterConvergenceEvent>, Box<dyn std::error::Error>> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut observed = Vec::new();
    while Instant::now() < deadline {
        observed.extend(convergence.drain_events()?);
        if observed.iter().any(|event| {
            matches!(
                event,
                RasterConvergenceEvent::PreparationReady { revision: actual }
                    if *actual == revision
            )
        }) {
            return Ok(observed);
        }
        thread::yield_now();
    }
    Err(format!("revision {revision} did not become ready").into())
}

#[test]
fn unchanged_outcomes_do_not_advance_the_required_revision()
-> Result<(), Box<dyn std::error::Error>> {
    let frontend = frontend("unchanged", 7, VoxelExtent::new(1, 1, 1))?;
    let unchanged = frontend.edit(VoxelEditCommand::new(
        VoxelVolumeId::new("terrain"),
        VoxelCoordinate::new(0, 0, 0),
        VoxelValue::Empty,
    ))?;
    let mut convergence = convergence(&frontend, VoxelExtent::new(1, 1, 1))?;

    assert_eq!(
        convergence.accept(unchanged)?,
        RasterConvergenceAcceptance::Unchanged {
            revision: VoxelSceneRevision::new(7),
        }
    );
    assert_eq!(convergence.required_revision(), VoxelSceneRevision::new(7));
    assert!(convergence.drain_events()?.is_empty());
    Ok(())
}

#[test]
fn adjacent_outcomes_accumulate_as_one_localized_newest_requirement()
-> Result<(), Box<dyn std::error::Error>> {
    let extent = VoxelExtent::new(64, 1, 1);
    let frontend = frontend("adjacent", 10, extent)?;
    let mut convergence = convergence(&frontend, VoxelExtent::new(1, 1, 1))?;

    convergence.accept(changed_edit(&frontend, VoxelCoordinate::new(0, 0, 0))?)?;
    convergence.accept(changed_edit(&frontend, VoxelCoordinate::new(63, 0, 0))?)?;

    let events = drain_until_ready(&mut convergence, VoxelSceneRevision::new(12))?;
    assert!(events.iter().any(|event| matches!(
        event,
        RasterConvergenceEvent::PreparationDiscarded {
            revision,
            disposition: RasterPreparationDisposition::SupersededBeforeUpload,
        } if *revision == VoxelSceneRevision::new(11)
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        RasterConvergenceEvent::PreparationReady {
            revision,
        } if *revision == VoxelSceneRevision::new(12)
    )));
    assert!(!events.iter().any(|event| matches!(
        event,
        RasterConvergenceEvent::PreparationReady { revision }
            if *revision == VoxelSceneRevision::new(11)
    )));
    Ok(())
}

#[test]
fn a_discontinuous_newest_outcome_replaces_pending_work_with_a_full_rebuild()
-> Result<(), Box<dyn std::error::Error>> {
    let first_frontend = frontend("discontinuous", 3, VoxelExtent::new(32, 1, 1))?;
    let newer_frontend = frontend("discontinuous", 20, VoxelExtent::new(32, 1, 1))?;
    let mut convergence = convergence(&first_frontend, VoxelExtent::new(1, 1, 1))?;

    convergence.accept(changed_edit(
        &first_frontend,
        VoxelCoordinate::new(0, 0, 0),
    )?)?;
    convergence.accept(changed_edit(
        &newer_frontend,
        VoxelCoordinate::new(31, 0, 0),
    )?)?;

    let events = drain_until_ready(&mut convergence, VoxelSceneRevision::new(21))?;
    assert!(events.iter().any(|event| matches!(
        event,
        RasterConvergenceEvent::PreparationReady {
            revision,
        } if *revision == VoxelSceneRevision::new(21)
    )));
    Ok(())
}

#[test]
fn rapid_requirements_keep_only_the_newest_pending_target() -> Result<(), Box<dyn std::error::Error>>
{
    let extent = VoxelExtent::new(128, 1, 1);
    let frontend = frontend("rapid", 40, extent)?;
    let mut convergence = convergence(&frontend, VoxelExtent::new(1, 1, 1))?;
    let mut accepted_revisions = Vec::new();

    for coordinate in 0..128 {
        let acceptance = convergence.accept(changed_edit(
            &frontend,
            VoxelCoordinate::new(coordinate, 0, 0),
        )?)?;
        let RasterConvergenceAcceptance::Accepted { revision } = acceptance else {
            return Err("changed edit was not accepted".into());
        };
        accepted_revisions.push(revision);
    }

    let newest_revision = VoxelSceneRevision::new(168);
    let events = drain_until_ready(&mut convergence, newest_revision)?;
    let ready_revisions = events
        .iter()
        .filter_map(|event| match event {
            RasterConvergenceEvent::PreparationReady { revision } => Some(*revision),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(ready_revisions, vec![newest_revision]);
    assert!(events.len() <= 4);
    assert_eq!(convergence.required_revision(), newest_revision);
    assert!(accepted_revisions.contains(&newest_revision));
    Ok(())
}

#[test]
fn a_new_requirement_discards_an_already_ready_result_before_upload()
-> Result<(), Box<dyn std::error::Error>> {
    let frontend = frontend("ready-stale", 70, VoxelExtent::new(2, 1, 1))?;
    let mut convergence = convergence(&frontend, VoxelExtent::new(1, 1, 1))?;

    convergence.accept(changed_edit(&frontend, VoxelCoordinate::new(0, 0, 0))?)?;
    drain_until_ready(&mut convergence, VoxelSceneRevision::new(71))?;
    convergence.accept(changed_edit(&frontend, VoxelCoordinate::new(1, 0, 0))?)?;

    let events = drain_until_ready(&mut convergence, VoxelSceneRevision::new(72))?;
    assert!(events.iter().any(|event| matches!(
        event,
        RasterConvergenceEvent::PreparationDiscarded {
            revision,
            disposition: RasterPreparationDisposition::SupersededBeforeUpload,
        } if *revision == VoxelSceneRevision::new(71)
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        RasterConvergenceEvent::PreparationReady { revision }
            if *revision == VoxelSceneRevision::new(72)
    )));
    Ok(())
}

#[test]
fn retry_restarts_the_retained_required_view_without_another_edit()
-> Result<(), Box<dyn std::error::Error>> {
    let frontend = frontend("retry", 80, VoxelExtent::new(1, 1, 1))?;
    let mut convergence = convergence(&frontend, VoxelExtent::new(1, 1, 1))?;

    assert_eq!(
        convergence.request_retry()?,
        RasterConvergenceRetry::NoRequiredWork
    );
    convergence.accept(changed_edit(&frontend, VoxelCoordinate::new(0, 0, 0))?)?;
    drain_until_ready(&mut convergence, VoxelSceneRevision::new(81))?;

    assert_eq!(
        convergence.request_retry()?,
        RasterConvergenceRetry::Requested {
            revision: VoxelSceneRevision::new(81),
        }
    );
    let events = drain_until_ready(&mut convergence, VoxelSceneRevision::new(81))?;
    assert!(events.iter().any(|event| matches!(
        event,
        RasterConvergenceEvent::PreparationReady { revision }
            if *revision == VoxelSceneRevision::new(81)
    )));
    Ok(())
}

#[test]
fn an_older_discontinuous_outcome_cannot_replace_the_newest_requirement()
-> Result<(), Box<dyn std::error::Error>> {
    let newest_frontend = frontend("newest", 90, VoxelExtent::new(1, 1, 1))?;
    let older_frontend = frontend("newest", 10, VoxelExtent::new(1, 1, 1))?;
    let mut convergence = convergence(&newest_frontend, VoxelExtent::new(1, 1, 1))?;

    convergence.accept(changed_edit(
        &newest_frontend,
        VoxelCoordinate::new(0, 0, 0),
    )?)?;
    assert_eq!(
        convergence.accept(changed_edit(
            &older_frontend,
            VoxelCoordinate::new(0, 0, 0),
        )?)?,
        RasterConvergenceAcceptance::NotNewer {
            revision: VoxelSceneRevision::new(11),
        }
    );
    assert_eq!(convergence.required_revision(), VoxelSceneRevision::new(91));
    Ok(())
}

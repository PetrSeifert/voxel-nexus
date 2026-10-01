use raster_render_path::{
    RasterArtifactBuildCause, RasterConvergenceError, RasterRenderPath, derive_raster_regions,
    derive_raster_residency,
};
use std::sync::Arc;
#[cfg(feature = "qualification")]
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use voxel_frontend::{
    DenseVoxelBatch, DenseVoxelScene, DenseVoxelVolume, VoxelCoordinate, VoxelExtent,
    VoxelFrontend, VoxelMaterial, VoxelMaterialId, VoxelRegion, VoxelResidencyLimits,
    VoxelResidencySelectionId, VoxelSceneId, VoxelSceneRevision, VoxelValue, VoxelVolumeId,
    VoxelVolumeMetadata,
};

fn frontend() -> Result<Arc<VoxelFrontend>, Box<dyn std::error::Error>> {
    frontend_with(20, 9)
}

fn frontend_with(
    count: u32,
    maximum_selection_volumes: usize,
) -> Result<Arc<VoxelFrontend>, Box<dyn std::error::Error>> {
    let frontend = Arc::new(VoxelFrontend::with_residency_limits(
        VoxelResidencyLimits::new(maximum_selection_volumes)?,
    ));
    frontend.publish(DenseVoxelScene::new(
        VoxelSceneId::new("residency"),
        VoxelSceneRevision::new(1),
        vec![VoxelMaterial::new(VoxelMaterialId::new("stone"), [1.0; 4])],
        (0..count)
            .map(|index| {
                DenseVoxelVolume::new(
                    VoxelVolumeMetadata::new(
                        VoxelVolumeId::new(index.to_string()),
                        VoxelExtent::new(2, 1, 1),
                        [index as f32 * 2.0, 0.0, 0.0],
                        1.0,
                    ),
                    vec![DenseVoxelBatch::new(
                        VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), VoxelExtent::new(2, 1, 1)),
                        vec![VoxelValue::Occupied(VoxelMaterialId::new("stone")); 2],
                    )],
                )
            })
            .collect(),
    ))?;
    Ok(frontend)
}

#[cfg(feature = "qualification")]
fn settle(
    path: &mut raster_render_path::RasterRenderPath,
) -> Result<(), Box<dyn std::error::Error>> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        path.qualification_advance_frame_boundary()?;
        if path.required_residency() == path.installed_residency()
            && path.required_revision() == path.visible_revision()
        {
            return Ok(());
        }
        std::thread::yield_now();
    }
    Err("combined target did not converge".into())
}

#[test]
#[cfg(feature = "qualification")]
fn crossing_reuses_shared_volumes_and_unrelated_edits_acknowledge_without_derivation()
-> Result<(), Box<dyn std::error::Error>> {
    let frontend = frontend()?;
    let view = frontend.scene_view()?;
    let selection = |identity, first, last| {
        view.residency_selection(
            VoxelResidencySelectionId::new(identity),
            (first..last).map(|index: u32| VoxelVolumeId::new(index.to_string())),
        )
    };
    let mut path = raster_render_path::RasterRenderPath::new();
    path.install_artifact(derive_raster_residency(
        frontend.clone(),
        &view,
        selection(1, 0, 9)?,
        VoxelExtent::new(1, 1, 1),
    )?);
    path.begin_convergence()?;
    assert_eq!(path.residency_status()?.derived_volumes, 9);
    path.accept_residency_selection(selection(2, 3, 12)?)?;
    assert_eq!(
        path.installed_residency()
            .map(|selection| selection.identity()),
        Some(VoxelResidencySelectionId::new(1))
    );
    settle(&mut path)?;
    assert_eq!(path.residency_status()?.derived_volumes, 12);
    assert_eq!(path.residency_status()?.representation_copies, 9);
    let edit = frontend.edit(voxel_frontend::VoxelEditCommand::new(
        VoxelVolumeId::new("0"),
        VoxelCoordinate::new(0, 0, 0),
        VoxelValue::Empty,
    ))?;
    path.accept_edit_outcome(edit)?;
    settle(&mut path)?;
    assert_eq!(path.visible_revision(), Some(VoxelSceneRevision::new(2)));
    assert_eq!(path.residency_status()?.derived_volumes, 12);
    assert_eq!(
        path.installed_artifact()
            .ok_or("missing artifact")?
            .regions()
            .len(),
        18
    );
    Ok(())
}

#[test]
#[cfg(feature = "qualification")]
fn uploaded_selection_is_superseded_by_the_combined_newest_revision_and_selection()
-> Result<(), Box<dyn std::error::Error>> {
    let frontend = frontend()?;
    let view = frontend.scene_view()?;
    let selection = |identity, first, last| {
        view.residency_selection(
            VoxelResidencySelectionId::new(identity),
            (first..last).map(|index: u32| VoxelVolumeId::new(index.to_string())),
        )
    };
    let old = selection(1, 0, 9)?;
    let stale = selection(2, 9, 18)?;
    let newest = selection(3, 3, 12)?;
    let mut path = raster_render_path::RasterRenderPath::new();
    path.install_artifact(derive_raster_residency(
        frontend.clone(),
        &view,
        old.clone(),
        VoxelExtent::new(1, 1, 1),
    )?);
    path.begin_convergence()?;
    let controller = path.enable_lifecycle_control_with_hold(true);
    path.accept_residency_selection(stale)?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while controller.post_upload_revision()?.is_none() && std::time::Instant::now() < deadline {
        path.qualification_advance_frame_boundary()?;
        assert!(path.residency_status()?.representation_copies <= 18);
        std::thread::yield_now();
    }
    assert_eq!(
        controller.post_upload_revision()?,
        Some(VoxelSceneRevision::new(1))
    );
    assert_eq!(path.residency_status()?.representation_copies, 18);
    path.accept_edit_outcome(frontend.edit(voxel_frontend::VoxelEditCommand::new(
        VoxelVolumeId::new("3"),
        VoxelCoordinate::new(0, 0, 0),
        VoxelValue::Empty,
    ))?)?;
    path.accept_residency_selection(newest.clone())?;
    assert!(!path.accept_residency_selection(selection(2, 0, 9)?)?);
    assert_eq!(path.installed_residency(), Some(&old));
    controller.release_post_upload()?;
    while path.visible_revision() != Some(VoxelSceneRevision::new(2))
        || path.installed_residency() != Some(&newest)
    {
        if std::time::Instant::now() >= deadline {
            return Err("newest combined target did not install".into());
        }
        path.qualification_advance_frame_boundary()?;
        let status = path.residency_status()?;
        assert!(status.representation_copies <= 18);
        assert_eq!(status.workers, 1);
        if path.installed_residency() != Some(&newest) {
            assert_eq!(path.installed_residency(), Some(&old));
        }
        std::thread::yield_now();
    }
    let artifact = path
        .installed_artifact()
        .ok_or("missing installed artifact")?;
    let expected = derive_raster_residency(
        frontend.clone(),
        &frontend.scene_view()?,
        newest,
        VoxelExtent::new(1, 1, 1),
    )?;
    assert_eq!(
        artifact.flatten_geometry()?.vertices(),
        expected.flatten_geometry()?.vertices()
    );
    assert_eq!(
        artifact.flatten_geometry()?.indices(),
        expected.flatten_geometry()?.indices()
    );
    Ok(())
}

#[test]
#[cfg(feature = "qualification")]
fn failures_preserve_presenting_selection_and_retry_then_retirement_releases_every_copy()
-> Result<(), Box<dyn std::error::Error>> {
    use raster_render_path::{RasterConvergenceEvent, RasterConvergenceFailurePhase};
    for phase in [
        RasterConvergenceFailurePhase::Derivation,
        RasterConvergenceFailurePhase::Upload,
        RasterConvergenceFailurePhase::Commit,
    ] {
        let frontend = frontend()?;
        let view = frontend.scene_view()?;
        let old = view.residency_selection(
            VoxelResidencySelectionId::new(1),
            (0..9).map(|index| VoxelVolumeId::new(index.to_string())),
        )?;
        let newest = view.residency_selection(
            VoxelResidencySelectionId::new(2),
            (9..18).map(|index| VoxelVolumeId::new(index.to_string())),
        )?;
        let mut path = raster_render_path::RasterRenderPath::new();
        path.install_artifact(derive_raster_residency(
            frontend.clone(),
            &view,
            old.clone(),
            VoxelExtent::new(1, 1, 1),
        )?);
        path.begin_convergence()?;
        path.qualification_fail_next_convergence(phase)?;
        path.accept_residency_selection(newest.clone())?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if std::time::Instant::now() >= deadline {
                return Err("injected failure was not reported".into());
            }
            path.qualification_advance_frame_boundary()?;
            assert_eq!(path.installed_residency(), Some(&old));
            assert_eq!(path.visible_revision(), Some(VoxelSceneRevision::new(1)));
            assert!(path.residency_status()?.representation_copies <= 18);
            if path.drain_convergence_events()?.iter().any(|event| matches!(event, RasterConvergenceEvent::Failure { failure } if failure.phase() == phase)) { break; }
            std::thread::yield_now();
        }
        path.request_convergence_retry()?;
        settle(&mut path)?;
        assert_eq!(path.installed_residency(), Some(&newest));
        path.qualification_retire()?;
        let retired = path.residency_status()?;
        assert_eq!(retired.representation_copies, 0);
        assert_eq!(retired.workers, 0);
        assert_eq!(retired.gpu_allocations, 0);
    }
    Ok(())
}

#[cfg(feature = "qualification")]
struct Recipe {
    metadata: VoxelVolumeMetadata,
    scene: VoxelSceneId,
    generations: Arc<AtomicUsize>,
    fail_next: Arc<AtomicBool>,
}

#[cfg(feature = "qualification")]
impl voxel_frontend::VoxelVolumeSource for Recipe {
    fn scene_id(&self) -> &VoxelSceneId {
        &self.scene
    }
    fn requires_materials(&self) -> bool {
        true
    }
    fn value(&self, _: VoxelCoordinate) -> Result<VoxelValue, voxel_frontend::VoxelSourceError> {
        Ok(VoxelValue::Occupied(VoxelMaterialId::new("stone")))
    }
    fn materialize(
        &self,
    ) -> Result<voxel_frontend::SparseVoxelVolume, voxel_frontend::VoxelSourceError> {
        self.generations.fetch_add(1, Ordering::SeqCst);
        if self.fail_next.swap(false, Ordering::SeqCst) {
            return Err(voxel_frontend::VoxelSourceError::Generation {
                message: "injected source failure".into(),
            });
        }
        Ok(voxel_frontend::SparseVoxelVolume::new(
            self.metadata.clone(),
            voxel_frontend::SparseVoxelBackground::Empty,
            vec![voxel_frontend::SparseVoxelBatch::Fill(
                voxel_frontend::VoxelRegionFill::new(
                    VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), self.metadata.extent()),
                    self.value(VoxelCoordinate::new(0, 0, 0))?,
                ),
            )],
        )
        .with_storage_tier(voxel_frontend::StorageTier::SparsePages))
    }
}

#[test]
#[cfg(feature = "qualification")]
fn streamed_sources_share_materializations_and_generation_failure_is_retryable()
-> Result<(), Box<dyn std::error::Error>> {
    let frontend = Arc::new(VoxelFrontend::with_residency_limits(
        VoxelResidencyLimits::new(9)?,
    ));
    let scene = VoxelSceneId::new("streamed-residency");
    let generations: Vec<_> = (0..20).map(|_| Arc::new(AtomicUsize::new(0))).collect();
    let fail_next = Arc::new(AtomicBool::new(false));
    let volumes = generations
        .iter()
        .enumerate()
        .map(|(index, generations)| {
            let metadata = VoxelVolumeMetadata::new(
                VoxelVolumeId::new(index.to_string()),
                VoxelExtent::new(2, 1, 1),
                [index as f32 * 2.0, 0.0, 0.0],
                1.0,
            );
            voxel_frontend::StreamedVoxelVolume::new(
                metadata.clone(),
                Arc::new(Recipe {
                    metadata,
                    scene: scene.clone(),
                    generations: generations.clone(),
                    fail_next: fail_next.clone(),
                }),
            )
        })
        .collect();
    let view = frontend.publish_streamed(voxel_frontend::StreamedVoxelScene::new(
        scene,
        VoxelSceneRevision::new(1),
        vec![VoxelMaterial::new(VoxelMaterialId::new("stone"), [1.0; 4])],
        volumes,
    ))?;
    let selection = |identity, first, last| {
        view.residency_selection(
            VoxelResidencySelectionId::new(identity),
            (first..last).map(|index: u32| VoxelVolumeId::new(index.to_string())),
        )
    };
    let old = selection(1, 0, 9)?;
    frontend.require_residency(old.clone())?;
    frontend.establish_residency()?;
    let (sender, receiver) = std::sync::mpsc::channel();
    let mut preparation = raster_render_path::RasterArtifactPreparation::start_residency(
        frontend.clone(),
        view.clone(),
        old.clone(),
        VoxelExtent::new(1, 1, 1),
        move |_| {
            sender
                .send(())
                .expect("preparation notification receiver remains alive");
        },
    )?;
    receiver.recv_timeout(std::time::Duration::from_secs(10))?;
    let mut path = raster_render_path::RasterRenderPath::new();
    path.install_artifact(
        preparation
            .try_complete()?
            .ok_or("initial preparation did not complete")?,
    );
    path.begin_convergence()?;
    for counter in generations.iter().take(9) {
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }
    let newest = selection(2, 3, 12)?;
    fail_next.store(true, Ordering::SeqCst);
    path.accept_residency_selection(newest.clone())?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if std::time::Instant::now() >= deadline {
            return Err("source failure was not reported".into());
        }
        path.qualification_advance_frame_boundary()?;
        if path.drain_convergence_events()?.iter().any(|event| {
            matches!(
                event,
                raster_render_path::RasterConvergenceEvent::Failure { .. }
            )
        }) {
            break;
        }
        std::thread::yield_now();
    }
    assert_eq!(path.installed_residency(), Some(&old));
    path.request_convergence_retry()?;
    settle(&mut path)?;
    assert_eq!(path.installed_residency(), Some(&newest));
    assert_eq!(path.residency_status()?.derived_volumes, 12);
    assert_eq!(
        generations
            .iter()
            .take(9)
            .map(|counter| counter.load(Ordering::SeqCst))
            .sum::<usize>(),
        9
    );
    assert_eq!(
        generations
            .iter()
            .skip(9)
            .take(3)
            .map(|counter| counter.load(Ordering::SeqCst))
            .sum::<usize>(),
        4
    );
    assert!(
        generations
            .iter()
            .skip(12)
            .all(|counter| counter.load(Ordering::SeqCst) == 0)
    );
    path.qualification_retire()?;
    assert_eq!(path.residency_status()?.representation_copies, 0);
    Ok(())
}

#[test]
#[cfg(feature = "qualification")]
fn superseded_worker_drains_one_volume_before_the_newest_target_and_retirement_joins_held_work()
-> Result<(), Box<dyn std::error::Error>> {
    let frontend = frontend()?;
    let view = frontend.scene_view()?;
    let selection = |identity, first, last| {
        view.residency_selection(
            VoxelResidencySelectionId::new(identity),
            (first..last).map(|index: u32| VoxelVolumeId::new(index.to_string())),
        )
    };
    let old = selection(1, 0, 9)?;
    let mut path = raster_render_path::RasterRenderPath::new();
    path.install_artifact(derive_raster_residency(
        frontend.clone(),
        &view,
        old.clone(),
        VoxelExtent::new(1, 1, 1),
    )?);
    path.begin_convergence()?;
    let controller = path.enable_lifecycle_control();
    controller.hold_next_cpu_generation_after_regions(1)?;
    path.qualification_advance_frame_boundary()?;
    path.accept_residency_selection(selection(2, 9, 18)?)?;
    let wait_for_hold = || -> Result<(), Box<dyn std::error::Error>> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            if controller
                .cpu_barrier_observation()?
                .is_some_and(|observation| observation.reached_revision.is_some())
            {
                return Ok(());
            }
            std::thread::yield_now();
        }
        Err("worker did not reach the hold".into())
    };
    wait_for_hold()?;
    for identity in 3..25 {
        path.accept_residency_selection(selection(identity, 10, 19)?)?;
    }
    path.accept_edit_outcome(frontend.edit(voxel_frontend::VoxelEditCommand::new(
        VoxelVolumeId::new("3"),
        VoxelCoordinate::new(0, 0, 0),
        VoxelValue::Empty,
    ))?)?;
    let newest = selection(25, 3, 12)?;
    path.accept_residency_selection(newest.clone())?;
    assert_eq!(path.installed_residency(), Some(&old));
    assert_eq!(path.residency_status()?.representation_copies, 10);
    assert_eq!(path.residency_status()?.workers, 1);
    controller.release_cpu_barrier()?;
    settle(&mut path)?;
    assert_eq!(path.installed_residency(), Some(&newest));
    assert_eq!(path.visible_revision(), Some(VoxelSceneRevision::new(2)));
    // Nine initial volumes, one drained volume, and four changed or entering volumes.
    assert_eq!(path.residency_status()?.derived_volumes, 14);

    controller.hold_next_cpu_generation_after_regions(1)?;
    path.qualification_advance_frame_boundary()?;
    path.accept_residency_selection(selection(26, 0, 9)?)?;
    wait_for_hold()?;
    path.qualification_retire()?;
    assert_eq!(path.residency_status()?.representation_copies, 0);
    assert_eq!(path.residency_status()?.workers, 0);
    assert!(
        controller
            .cpu_barrier_observation()?
            .is_some_and(|observation| observation.finished && observation.cancelled)
    );
    Ok(())
}

#[test]
fn selection_derives_exactly_its_volumes_with_volume_local_region_identities()
-> Result<(), Box<dyn std::error::Error>> {
    let frontend = frontend()?;
    let view = frontend.scene_view()?;
    let selection = view.residency_selection(
        VoxelResidencySelectionId::new(1),
        [VoxelVolumeId::new("3"), VoxelVolumeId::new("4")],
    )?;
    let extent = VoxelExtent::new(1, 1, 1);
    let whole = derive_raster_regions(&view, extent)?;
    let selected = derive_raster_residency(frontend.clone(), &view, selection.clone(), extent)?;
    assert_eq!(selected.regions().len(), 4);
    assert_eq!(selected.residency_selection(), Some(&selection));
    for region in selected.regions() {
        let original = whole
            .regions()
            .iter()
            .find(|original| original.identity() == region.identity())
            .ok_or("missing whole-scene region")?;
        assert_eq!(region.vertices(), original.vertices());
        assert_eq!(region.indices(), original.indices());
    }
    Ok(())
}

#[test]
fn a_configured_maximum_selection_size_bounds_raster_selections()
-> Result<(), Box<dyn std::error::Error>> {
    let maximum = 25;
    let frontend = frontend_with(maximum + 1, 25)?;
    let view = frontend.scene_view()?;
    let selection = |identity, count| {
        view.residency_selection(
            VoxelResidencySelectionId::new(identity),
            (0..count).map(|index: u32| VoxelVolumeId::new(index.to_string())),
        )
    };
    let extent = VoxelExtent::new(1, 1, 1);
    let oversized =
        derive_raster_residency(frontend.clone(), &view, selection(1, maximum + 1)?, extent);
    assert!(matches!(
        oversized.as_ref().map_err(|error| error.cause_detail()),
        Err(RasterArtifactBuildCause::ResidencySelectionTooLarge { maximum: 25 })
    ));
    let full = selection(2, maximum)?;
    let mut path = RasterRenderPath::new();
    path.install_artifact(derive_raster_residency(
        frontend.clone(),
        &view,
        full.clone(),
        extent,
    )?);
    assert_eq!(path.installed_residency(), Some(&full));
    assert!(matches!(
        path.accept_residency_selection(selection(3, maximum + 1)?),
        Err(RasterConvergenceError::ResidencySelectionTooLarge { maximum: 25 })
    ));
    assert!(path.accept_residency_selection(selection(4, maximum)?)?);
    Ok(())
}

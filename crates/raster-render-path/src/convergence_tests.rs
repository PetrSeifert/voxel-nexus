use super::*;
use ash::vk::Handle;
use canonical_scene::{CanonicalSceneScale, generate_canonical_scene};
use std::collections::HashSet;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};
use voxel_frontend::VoxelCoordinate;
use voxel_frontend::VoxelExtent;
use voxel_frontend::VoxelFrontendError;
use voxel_frontend::VoxelMaterialId;
use voxel_frontend::VoxelRegion;
use voxel_frontend::VoxelValue;
use voxel_frontend::VoxelVolumeId;
use voxel_frontend::VoxelVolumeMetadata;
use voxel_frontend::{
    DenseVoxelBatch, DenseVoxelScene, DenseVoxelVolume, VoxelEditCommand, VoxelFrontend,
    VoxelMaterial,
};

fn frontend(revision: u64, width: u32) -> Result<VoxelFrontend, Box<dyn std::error::Error>> {
    let extent = VoxelExtent::new(width, 1, 1);
    let material_identity = VoxelMaterialId::new("stone");
    let frontend = VoxelFrontend::new();
    frontend.publish(DenseVoxelScene::new(
        VoxelSceneId::new("convergence-unit"),
        VoxelSceneRevision::new(revision),
        vec![VoxelMaterial::new(material_identity, [0.2, 0.3, 0.4, 1.0])],
        vec![DenseVoxelVolume::new(
            VoxelVolumeMetadata::new(VoxelVolumeId::new("terrain"), extent, [0.0, 0.0, 0.0], 1.0),
            vec![DenseVoxelBatch::new(
                VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), extent),
                vec![VoxelValue::Empty; usize::try_from(width)?],
            )],
        )],
    ))?;
    Ok(frontend)
}

fn convergence(frontend: &VoxelFrontend) -> Result<RasterConvergence, Box<dyn std::error::Error>> {
    let render_path = render_path(frontend)?;
    Ok(RasterConvergence::from_visible(&render_path)?)
}

fn render_path(frontend: &VoxelFrontend) -> Result<RasterRenderPath, Box<dyn std::error::Error>> {
    let mut render_path = RasterRenderPath::new();
    render_path.install_artifact(derive_raster_regions(
        &frontend.scene_view()?,
        VoxelExtent::new(1, 1, 1),
    )?);
    Ok(render_path)
}

fn changed(frontend: &VoxelFrontend, x: i32) -> Result<VoxelEditOutcome, VoxelFrontendError> {
    frontend.edit(VoxelEditCommand::new(
        VoxelVolumeId::new("terrain"),
        VoxelCoordinate::new(x, 0, 0),
        VoxelValue::Occupied(VoxelMaterialId::new("stone")),
    ))
}

fn canonical_frontend() -> Result<VoxelFrontend, Box<dyn std::error::Error>> {
    let frontend = VoxelFrontend::new();
    frontend.publish_sparse(generate_canonical_scene(CanonicalSceneScale::Large)?.into_scene())?;
    Ok(frontend)
}

fn canonical_changed(
    frontend: &VoxelFrontend,
    coordinate_x: i32,
) -> Result<VoxelEditOutcome, VoxelFrontendError> {
    frontend.edit(VoxelEditCommand::new(
        VoxelVolumeId::new("canonical-volume"),
        VoxelCoordinate::new(coordinate_x, 0, 0),
        VoxelValue::Occupied(VoxelMaterialId::new("canonical-warm")),
    ))
}

fn wait_until_ready(convergence: &mut RasterConvergence) -> Result<(), Box<dyn std::error::Error>> {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if convergence
            .drain_events()?
            .iter()
            .any(|event| matches!(event, RasterConvergenceEvent::PreparationReady { .. }))
        {
            return Ok(());
        }
        thread::yield_now();
    }
    Err("preparation did not become ready".into())
}

fn wait_until_ready_without_draining(
    convergence: &mut RasterConvergence,
) -> Result<(), Box<dyn std::error::Error>> {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        convergence.poll_preparation()?;
        if convergence.active_is_ready() {
            return Ok(());
        }
        thread::yield_now();
    }
    Err("preparation did not become ready".into())
}

fn wait_until_canonical_candidate_is_ready(
    convergence: &mut RasterConvergence,
) -> Result<(), Box<dyn std::error::Error>> {
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        convergence.poll_preparation()?;
        if convergence.active_is_ready() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err("canonical candidate preparation did not become ready".into());
        }
        thread::yield_now();
    }
}

#[test]
fn path_neutral_edit_submission_starts_convergence_without_external_control()
-> Result<(), Box<dyn std::error::Error>> {
    let frontend = frontend(1, 4)?;
    let mut render_path = render_path(&frontend)?;
    let path: &mut dyn RenderPath = &mut render_path;
    path.submit_edit_outcome(changed(&frontend, 0)?)
        .map_err(|error| -> Box<dyn std::error::Error> { error })?;
    render_path.advance_convergence_at_frame_boundary(None)?;
    assert_eq!(
        render_path.required_revision(),
        Some(VoxelSceneRevision::new(2))
    );
    assert_eq!(
        render_path.visible_revision(),
        Some(VoxelSceneRevision::new(1))
    );
    Ok(())
}

#[test]
fn controller_barrier_proves_cancelled_generation_schedules_no_later_region()
-> Result<(), Box<dyn std::error::Error>> {
    let frontend = frontend(1, 4)?;
    let mut render_path = render_path(&frontend)?;
    let controller = render_path.enable_lifecycle_control();
    controller.hold_next_cpu_generation_after_regions(1)?;
    render_path
        .submit_edit_outcome(changed(&frontend, 0)?)
        .map_err(|error| -> Box<dyn std::error::Error> { error })?;
    render_path.advance_convergence_at_frame_boundary(None)?;

    for _ in 0..10_000 {
        if controller
            .cpu_barrier_observation()?
            .is_some_and(|observation| {
                observation.reached_revision == Some(VoxelSceneRevision::new(2))
            })
        {
            break;
        }
        thread::yield_now();
    }
    assert_eq!(
        controller.cpu_barrier_observation()?,
        Some(RasterConvergenceCpuBarrierObservation {
            reached_revision: Some(VoxelSceneRevision::new(2)),
            scheduled_region_count: 1,
            finished: false,
            cancelled: false,
        })
    );

    controller.submit(changed(&frontend, 3)?)?;
    render_path.advance_convergence_at_frame_boundary(None)?;
    assert_eq!(
        controller
            .convergence_status()?
            .ok_or("missing convergence status")?
            .required_revision,
        VoxelSceneRevision::new(3)
    );
    controller.release_cpu_barrier()?;
    for _ in 0..10_000 {
        render_path.advance_convergence_at_frame_boundary(None)?;
        if controller
            .cpu_barrier_observation()?
            .is_some_and(|observation| observation.finished)
        {
            break;
        }
        thread::yield_now();
    }
    assert_eq!(
        controller.cpu_barrier_observation()?,
        Some(RasterConvergenceCpuBarrierObservation {
            reached_revision: Some(VoxelSceneRevision::new(2)),
            scheduled_region_count: 1,
            finished: true,
            cancelled: true,
        })
    );
    Ok(())
}

#[test]
fn controller_observes_superseded_post_upload_rejection_before_final_visibility()
-> Result<(), Box<dyn std::error::Error>> {
    let frontend = frontend(1, 2)?;
    let mut render_path = render_path(&frontend)?;
    let controller = render_path.enable_lifecycle_control_with_hold(true);
    controller.submit(changed(&frontend, 0)?)?;
    for _ in 0..10_000 {
        render_path.advance_convergence_at_frame_boundary(None)?;
        if controller.post_upload_revision()? == Some(VoxelSceneRevision::new(2)) {
            break;
        }
        thread::yield_now();
    }
    assert_eq!(
        controller.post_upload_revision()?,
        Some(VoxelSceneRevision::new(2))
    );
    assert_eq!(
        controller
            .convergence_status()?
            .ok_or("missing held-candidate status")?
            .visible_revision,
        VoxelSceneRevision::new(1)
    );

    controller.submit(changed(&frontend, 1)?)?;
    render_path.advance_convergence_at_frame_boundary(None)?;
    controller.release_post_upload()?;
    render_path.advance_convergence_at_frame_boundary(None)?;
    assert_eq!(
        controller.rejected_candidate()?,
        Some(RasterRejectedCandidate {
            revision: VoxelSceneRevision::new(2),
            retired_resource_count: 0,
        })
    );
    assert_eq!(
        controller
            .convergence_status()?
            .ok_or("missing rejection status")?
            .visible_revision,
        VoxelSceneRevision::new(1)
    );

    for _ in 0..10_000 {
        render_path.advance_convergence_at_frame_boundary(None)?;
        if controller
            .convergence_status()?
            .is_some_and(|status| status.visible_revision == VoxelSceneRevision::new(3))
        {
            break;
        }
        thread::yield_now();
    }
    assert_eq!(
        controller.convergence_status()?,
        Some(RasterConvergenceStatus {
            required_revision: VoxelSceneRevision::new(3),
            visible_revision: VoxelSceneRevision::new(3),
            affected_region_count: 2,
            unaffected_region_count: 0,
        })
    );
    Ok(())
}

#[test]
fn dropping_convergence_releases_a_held_cpu_barrier_and_joins_the_worker()
-> Result<(), Box<dyn std::error::Error>> {
    let frontend = frontend(1, 4)?;
    let mut render_path = render_path(&frontend)?;
    let controller = render_path.enable_lifecycle_control();
    controller.hold_next_cpu_generation_after_regions(1)?;
    controller.submit(changed(&frontend, 0)?)?;
    render_path.advance_convergence_at_frame_boundary(None)?;
    for _ in 0..10_000 {
        if controller
            .cpu_barrier_observation()?
            .is_some_and(|observation| observation.reached_revision.is_some())
        {
            break;
        }
        thread::yield_now();
    }
    drop(render_path);
    assert_eq!(
        controller.cpu_barrier_observation()?,
        Some(RasterConvergenceCpuBarrierObservation {
            reached_revision: Some(VoxelSceneRevision::new(2)),
            scheduled_region_count: 1,
            finished: true,
            cancelled: true,
        })
    );
    Ok(())
}

fn fake_resources(identity: RasterRegionIdentity, raw_identity: u64) -> RasterRegionGpuResources {
    RasterRegionGpuResources {
        material_buffer_bytes: 0,
        material_buffer: vk::Buffer::null(),
        material_memory: vk::DeviceMemory::null(),
        material_layout: vk::DescriptorSetLayout::null(),
        material_pool: vk::DescriptorPool::null(),
        material_set: vk::DescriptorSet::null(),
        transform_constants: [0; 8],
        identity,
        vertex_buffer: vk::Buffer::from_raw(raw_identity),
        vertex_memory: vk::DeviceMemory::from_raw(raw_identity + 1_000),
        index_buffer: vk::Buffer::from_raw(raw_identity + 2_000),
        index_memory: vk::DeviceMemory::from_raw(raw_identity + 3_000),
        index_count: 6,
        vertex_buffer_bytes: 0,
        index_buffer_bytes: 0,
    }
}

#[test]
fn lifecycle_controller_reports_peak_live_raster_gpu_bytes_and_buffers()
-> Result<(), Box<dyn std::error::Error>> {
    let frontend = frontend(1, 1)?;
    let identity = render_path(&frontend)?
        .installed_regions()
        .first()
        .ok_or("missing installed Raster Region")?
        .identity()
        .clone();
    let mut render_path = RasterRenderPath::new();
    let controller = render_path.enable_lifecycle_control();
    let mut resources = fake_resources(identity, 1);
    resources.vertex_buffer_bytes = 400;
    resources.index_buffer_bytes = 200;

    resources.material_buffer = vk::Buffer::from_raw(5000);
    resources.material_buffer_bytes = 32;
    render_path.observe_live_gpu_resources([&resources])?;

    assert_eq!(
        controller.gpu_resource_peak()?,
        RasterGpuResourcePeak {
            bytes: 632,
            resources: 3,
        }
    );
    Ok(())
}

#[test]
fn lifecycle_characterization_starts_from_the_installed_resources_and_resets_samples()
-> Result<(), Box<dyn std::error::Error>> {
    let frontend = frontend(1, 1)?;
    let identity = render_path(&frontend)?
        .installed_regions()
        .first()
        .ok_or("canonical artifact has no Raster Regions")?
        .identity()
        .clone();
    let mut render_path = RasterRenderPath::new();
    let controller = render_path.enable_lifecycle_control();
    let mut resources = fake_resources(identity, 1);
    resources.vertex_buffer_bytes = 400;
    resources.index_buffer_bytes = 200;
    render_path.observe_installed_gpu_resources([&resources])?;

    controller.begin_characterization()?;
    let first = controller
        .characterization()?
        .ok_or("characterization did not start")?;
    assert_eq!(
        first.installed,
        RasterGpuResourceUsage {
            bytes: 600,
            resources: 2,
        }
    );
    assert_eq!(first.peak, first.installed);

    controller.begin_characterization()?;
    let reset = controller
        .characterization()?
        .ok_or("characterization did not restart")?;
    assert_eq!(reset.phases, RasterConvergencePhaseTimings::default());
    assert_eq!(reset.work, RasterRegionWorkDisposition::default());
    assert!(reset.cancellation_observations.is_empty());
    assert!(reset.safe_retirements.is_empty());
    Ok(())
}

#[test]
fn submission_bookkeeping_ends_at_the_queued_wait_boundary()
-> Result<(), Box<dyn std::error::Error>> {
    let frontend = frontend(1, 1)?;
    let mut render_path = render_path(&frontend)?;
    let controller = render_path.enable_lifecycle_control();
    controller.begin_characterization()?;
    let observation_started_at = Instant::now();

    controller.submit(changed(&frontend, 0)?)?;

    let state = controller
        .state
        .lock()
        .map_err(|_| "control state poisoned")?;
    let queued_at = state
        .pending_outcomes
        .front()
        .ok_or("submitted outcome was not queued")?
        .queued_at;
    let submission_bookkeeping = state
        .characterization
        .as_ref()
        .ok_or("characterization did not start")?
        .phases
        .submission_bookkeeping_milliseconds;
    assert!(queued_at >= observation_started_at);
    assert!(
        queued_at
            .saturating_duration_since(observation_started_at)
            .as_secs_f64()
            * 1_000.0
            >= submission_bookkeeping
    );
    Ok(())
}

#[test]
fn fixed_candidate_extents_pass_canonical_semantic_localization_and_failure_retry_gates()
-> Result<(), Box<dyn std::error::Error>> {
    for extent in [16, 32, 64] {
        let region_extent = VoxelExtent::new(extent, extent, extent);
        let frontend = canonical_frontend()?;
        let initial_view = frontend.scene_view()?;
        let mut render_path = RasterRenderPath::new();
        render_path.install_artifact(derive_raster_regions(&initial_view, region_extent)?);
        for coordinate_x in [0, 40, 80] {
            let outcome = canonical_changed(&frontend, coordinate_x)?;
            let VoxelEditOutcome::Changed { view, change_set } = &outcome else {
                return Err("canonical qualification command did not change a Voxel Value".into());
            };
            render_path.apply_adjacent_change(view, change_set)?;
        }
        let final_view = frontend.scene_view()?;
        let localized_faces = render_path
            .installed_artifact()
            .ok_or("localized candidate has no installed artifact")?
            .semantic_faces()
            .cloned()
            .collect::<HashSet<_>>();
        let complete_region_faces = derive_raster_regions(&final_view, region_extent)?
            .semantic_faces()
            .cloned()
            .collect::<HashSet<_>>();
        let semantic_reference =
            derive_raster_artifact(&final_view, &VoxelVolumeId::new("canonical-volume"))?
                .semantic_faces()
                .cloned()
                .collect::<HashSet<_>>();
        assert_eq!(localized_faces, complete_region_faces);
        assert_eq!(localized_faces, semantic_reference);

        let retry_frontend = canonical_frontend()?;
        let mut retry_render_path = RasterRenderPath::new();
        retry_render_path.install_artifact(derive_raster_regions(
            &retry_frontend.scene_view()?,
            region_extent,
        )?);
        retry_render_path.region_resources = retry_render_path
            .installed_regions()
            .iter()
            .enumerate()
            .map(|(index, installation)| {
                Ok(fake_resources(
                    installation.identity().clone(),
                    u64::try_from(index)? + 10,
                ))
            })
            .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;
        let visible_installations = retry_render_path.installed_regions().to_vec();
        let mut convergence = RasterConvergence::from_visible(&retry_render_path)?;
        convergence.accept(canonical_changed(&retry_frontend, 0)?)?;
        wait_until_canonical_candidate_is_ready(&mut convergence)?;
        assert!(matches!(
            convergence.upload_ready_with_test_resources(&retry_render_path, &mut |_| Err(
                RasterConvergenceError::ResourceBookkeepingAllocation
            ),)?,
            RasterConvergenceUpload::NoReadyPreparation
        ));
        assert_eq!(retry_render_path.installed_regions(), visible_installations);
        assert_eq!(
            convergence.request_retry()?,
            RasterConvergenceRetry::Requested {
                revision: VoxelSceneRevision::new(2),
            }
        );
        wait_until_canonical_candidate_is_ready(&mut convergence)?;
        convergence.upload_ready_with_test_resources(&retry_render_path, &mut |region| {
            Ok(fake_resources(region.identity().clone(), 100_000))
        })?;
        let RasterConvergenceCommit::Committed { retirement } =
            convergence.commit_at_frame_boundary(&mut retry_render_path)?
        else {
            return Err("retried canonical candidate did not commit".into());
        };
        retirement.release_with(drop);
        assert_eq!(
            retry_render_path.installed_source_revision(),
            Some(VoxelSceneRevision::new(2))
        );
    }
    Ok(())
}

#[derive(Default)]
struct DeterministicResourceLifecycle {
    created: usize,
    retired: usize,
    live: HashSet<u64>,
    next_identity: u64,
}

impl DeterministicResourceLifecycle {
    fn with_next_identity(next_identity: u64) -> Self {
        Self {
            next_identity,
            ..Self::default()
        }
    }

    fn create(&mut self, identity: RasterRegionIdentity) -> RasterRegionGpuResources {
        let raw_identity = self.next_identity;
        self.next_identity += 1;
        self.created += 1;
        assert!(self.live.insert(raw_identity));
        fake_resources(identity, raw_identity)
    }

    fn retire(&mut self, resources: RasterRegionGpuResources) {
        self.retired += 1;
        assert!(self.live.remove(&resources.vertex_buffer.as_raw()));
    }

    fn assert_balanced(&self) {
        assert_eq!(self.created, self.retired);
        assert!(self.live.is_empty());
    }
}

#[test]
fn superseded_uploaded_resources_remain_live_until_fence_safe_retirement()
-> Result<(), Box<dyn std::error::Error>> {
    let frontend = frontend(190, 2)?;
    let mut render_path = render_path(&frontend)?;
    let mut lifecycle = DeterministicResourceLifecycle::with_next_identity(10);
    render_path.region_resources = render_path
        .installed_regions()
        .iter()
        .map(|installation| lifecycle.create(installation.identity().clone()))
        .collect();
    let mut convergence = RasterConvergence::from_visible(&render_path)?;

    convergence.accept(changed(&frontend, 0)?)?;
    wait_until_ready_without_draining(&mut convergence)?;
    convergence.upload_ready_with_test_resources(&render_path, &mut |region| {
        Ok(lifecycle.create(region.identity().clone()))
    })?;
    convergence.accept(changed(&frontend, 1)?)?;
    let RasterConvergenceCommit::Rejected { retirement, .. } =
        convergence.commit_at_frame_boundary(&mut render_path)?
    else {
        return Err("superseded uploaded candidate was not rejected".into());
    };
    let superseded_resource_count = retirement.resource_count();
    assert_eq!(lifecycle.live.len(), lifecycle.created);
    assert_eq!(lifecycle.retired, 0);
    retirement.release_with(|resources| lifecycle.retire(resources));
    assert_eq!(lifecycle.retired, superseded_resource_count);

    wait_until_ready_without_draining(&mut convergence)?;
    convergence.upload_ready_with_test_resources(&render_path, &mut |region| {
        Ok(lifecycle.create(region.identity().clone()))
    })?;
    let RasterConvergenceCommit::Committed { retirement } =
        convergence.commit_at_frame_boundary(&mut render_path)?
    else {
        return Err("newest uploaded candidate was not committed".into());
    };
    retirement.release_with(|resources| lifecycle.retire(resources));
    for resources in std::mem::take(&mut render_path.region_resources) {
        lifecycle.retire(resources);
    }
    convergence
        .shutdown()
        .retirement
        .release_with(|resources| lifecycle.retire(resources));
    lifecycle.assert_balanced();
    Ok(())
}

#[test]
fn residency_crossing_reuses_gpu_resources_and_retires_outgoing_volumes_at_the_boundary()
-> Result<(), Box<dyn std::error::Error>> {
    let frontend = Arc::new(VoxelFrontend::with_residency_limits(
        voxel_frontend::VoxelResidencyLimits::new(9)?,
    ));
    let scene = VoxelSceneId::new("gpu-residency");
    let view = frontend.publish(DenseVoxelScene::new(
        scene,
        VoxelSceneRevision::new(1),
        vec![VoxelMaterial::new(VoxelMaterialId::new("stone"), [1.0; 4])],
        (0..3)
            .map(|index| {
                DenseVoxelVolume::new(
                    VoxelVolumeMetadata::new(
                        VoxelVolumeId::new(index.to_string()),
                        VoxelExtent::new(1, 1, 1),
                        [index as f32, 0.0, 0.0],
                        1.0,
                    ),
                    vec![DenseVoxelBatch::new(
                        VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), VoxelExtent::new(1, 1, 1)),
                        vec![VoxelValue::Occupied(VoxelMaterialId::new("stone"))],
                    )],
                )
            })
            .collect(),
    ))?;
    let old = view.residency_selection(
        voxel_frontend::VoxelResidencySelectionId::new(1),
        [VoxelVolumeId::new("0"), VoxelVolumeId::new("1")],
    )?;
    let newest = view.residency_selection(
        voxel_frontend::VoxelResidencySelectionId::new(2),
        [VoxelVolumeId::new("1"), VoxelVolumeId::new("2")],
    )?;
    let mut path = RasterRenderPath::new();
    path.install_artifact(derive_raster_residency(
        frontend,
        &view,
        old,
        VoxelExtent::new(1, 1, 1),
    )?);
    let mut lifecycle = DeterministicResourceLifecycle::with_next_identity(10);
    path.region_resources = path
        .installed_regions()
        .iter()
        .map(|installation| lifecycle.create(installation.identity().clone()))
        .collect();
    let shared_buffer = path
        .region_resources
        .iter()
        .find(|resources| resources.identity.volume_identity() == &VoxelVolumeId::new("1"))
        .ok_or("missing shared volume")?
        .vertex_buffer;
    let mut convergence = RasterConvergence::from_visible(&path)?;
    convergence.accept_residency_selection(newest.clone())?;
    wait_until_ready_without_draining(&mut convergence)?;
    convergence.upload_ready_with_test_resources(&path, &mut |region| {
        Ok(lifecycle.create(region.identity().clone()))
    })?;
    assert_eq!(lifecycle.created, 3);
    assert_eq!(lifecycle.retired, 0);
    let RasterConvergenceCommit::Committed { retirement } =
        convergence.commit_at_frame_boundary(&mut path)?
    else {
        return Err("selection did not commit".into());
    };
    assert_eq!(path.installed_residency(), Some(&newest));
    assert_eq!(path.region_resources.len(), 2);
    assert_eq!(retirement.resource_count(), 1);
    assert_eq!(lifecycle.live.len(), 3);
    assert_eq!(
        path.region_resources
            .iter()
            .find(|resources| resources.identity.volume_identity() == &VoxelVolumeId::new("1"))
            .ok_or("shared volume disappeared")?
            .vertex_buffer,
        shared_buffer
    );
    retirement.release_with(|resources| lifecycle.retire(resources));
    assert_eq!(lifecycle.live.len(), 2);
    convergence
        .shutdown()
        .retirement
        .release_with(|resources| lifecycle.retire(resources));
    for resources in std::mem::take(&mut path.region_resources) {
        lifecycle.retire(resources);
    }
    path.release_residency_artifact();
    let cache = convergence
        .residency_target
        .as_ref()
        .ok_or("missing cache")?
        .cache
        .lock()
        .map_err(|_| "cache poisoned")?;
    assert_eq!(cache.retained_copies(), 0);
    assert!(convergence.worker_pool.is_none());
    lifecycle.assert_balanced();
    Ok(())
}

#[test]
fn shutdown_joins_active_work_and_disposes_a_hidden_uploaded_candidate()
-> Result<(), Box<dyn std::error::Error>> {
    let frontend = frontend(200, 2)?;
    let mut render_path = render_path(&frontend)?;
    let mut lifecycle = DeterministicResourceLifecycle::with_next_identity(40);
    render_path.region_resources = render_path
        .installed_regions()
        .iter()
        .map(|installation| lifecycle.create(installation.identity().clone()))
        .collect();
    let mut convergence = RasterConvergence::from_visible(&render_path)?;
    convergence.accept(changed(&frontend, 0)?)?;
    wait_until_ready_without_draining(&mut convergence)?;
    convergence.upload_ready_with_test_resources(&render_path, &mut |region| {
        Ok(lifecycle.create(region.identity().clone()))
    })?;
    convergence.accept(changed(&frontend, 1)?)?;

    convergence
        .shutdown()
        .retirement
        .release_with(|resources| lifecycle.retire(resources));
    assert!(convergence.active.is_none());
    assert!(convergence.pending.is_none());
    assert!(convergence.paused.is_none());
    assert!(convergence.hidden_candidate.is_none());
    for resources in std::mem::take(&mut render_path.region_resources) {
        lifecycle.retire(resources);
    }
    lifecycle.assert_balanced();
    Ok(())
}

#[test]
fn presentation_release_restarts_the_held_candidate_target()
-> Result<(), Box<dyn std::error::Error>> {
    let frontend = frontend(210, 1)?;
    let mut render_path = render_path(&frontend)?;
    render_path.region_resources = render_path
        .installed_regions()
        .iter()
        .map(|installation| fake_resources(installation.identity().clone(), 70))
        .collect();
    let mut convergence = RasterConvergence::from_visible(&render_path)?;
    convergence.accept(changed(&frontend, 0)?)?;
    wait_until_ready_without_draining(&mut convergence)?;
    convergence.upload_ready_with_test_resources(&render_path, &mut |region| {
        Ok(fake_resources(region.identity().clone(), 80))
    })?;

    let release = convergence.take_hidden_resources_for_release();
    assert_eq!(release.retirement.resource_count(), 1);
    assert!(release.restart_error.is_none());
    assert!(convergence.hidden_candidate.is_none());
    assert!(convergence.active.is_some());
    assert!(convergence.pending.is_none());
    Ok(())
}

#[test]
fn presentation_release_returns_hidden_retirement_when_restart_fails()
-> Result<(), Box<dyn std::error::Error>> {
    let frontend = frontend(220, 1)?;
    let mut render_path = render_path(&frontend)?;
    render_path.region_resources = render_path
        .installed_regions()
        .iter()
        .map(|installation| fake_resources(installation.identity().clone(), 90))
        .collect();
    let mut convergence = RasterConvergence::from_visible(&render_path)?;
    convergence.accept(changed(&frontend, 0)?)?;
    wait_until_ready_without_draining(&mut convergence)?;
    convergence.upload_ready_with_test_resources(&render_path, &mut |region| {
        Ok(fake_resources(region.identity().clone(), 100))
    })?;

    let release = convergence.take_hidden_resources_for_release_with_restart(|_| {
        Err(RasterConvergenceError::ResourceBookkeepingAllocation)
    });
    assert_eq!(release.retirement.resource_count(), 1);
    assert!(matches!(
        release.restart_error,
        Some(RasterConvergenceError::ResourceBookkeepingAllocation)
    ));
    assert!(convergence.pending.is_some());
    assert!(convergence.hidden_candidate.is_none());
    Ok(())
}

#[test]
fn lifecycle_cleanup_preserves_the_operational_error_when_control_reporting_also_fails() {
    let error = finish_lifecycle_operation(
        "shutdown",
        Some(RasterConvergenceError::ResourceBookkeepingAllocation),
        Some(RasterLifecycleControlError),
    )
    .expect_err("the operational lifecycle error should reach the caller");
    assert!(error.to_string().contains("resource bookkeeping"));
}

#[test]
fn private_target_bookkeeping_accumulates_adjacent_work_and_marks_discontinuities()
-> Result<(), Box<dyn std::error::Error>> {
    let adjacent_frontend = frontend(10, 4)?;
    let discontinuous_frontend = frontend(30, 4)?;
    let mut convergence = convergence(&adjacent_frontend)?;

    convergence.accept(changed(&adjacent_frontend, 0)?)?;
    convergence.accept(changed(&adjacent_frontend, 3)?)?;
    let localized = convergence
        .pending
        .as_ref()
        .ok_or("missing newest pending target")?;
    let RasterPreparationTargetScope::Localized(affected) = &localized.scope else {
        return Err("adjacent outcomes did not retain localized work".into());
    };
    assert!(affected.len() >= 2);

    convergence.accept(changed(&discontinuous_frontend, 1)?)?;
    assert!(matches!(
        convergence
            .pending
            .as_ref()
            .ok_or("missing discontinuous pending target")?
            .scope,
        RasterPreparationTargetScope::FullRebuild
    ));
    Ok(())
}

#[test]
fn failed_preparation_clears_dead_active_state_and_retry_progresses()
-> Result<(), Box<dyn std::error::Error>> {
    let frontend = frontend(40, 1)?;
    let mut convergence = convergence(&frontend)?;
    convergence.accept(changed(&frontend, 0)?)?;
    wait_until_ready(&mut convergence)?;

    let active = convergence
        .active
        .as_mut()
        .ok_or("missing ready preparation")?;
    let (completion_sender, completion_receiver) = mpsc::sync_channel(1);
    completion_sender.send(RasterPreparationCompletion::Completed(Err(
        RasterDerivationFailure {
            region_identity: None,
            source: metadata_dimensions_error(VoxelSceneRevision::new(41)),
        },
    )))?;
    active.status = RasterActivePreparationStatus::Running;
    active.completion_receiver = completion_receiver;
    active.worker = Some(
        convergence
            .worker_pool
            .as_ref()
            .ok_or("missing pool")?
            .execute(|| {})?,
    );

    let events = convergence.drain_events()?;
    assert!(events.iter().any(|event| matches!(
        event,
        RasterConvergenceEvent::Failure { failure }
            if failure.scene_identity() == &VoxelSceneId::new("convergence-unit")
                && failure.failed_revision() == VoxelSceneRevision::new(41)
                && failure.phase() == RasterConvergenceFailurePhase::Derivation
                && failure.region_identity().is_none()
                && failure.source().contains("dimensions")
    )));
    assert!(convergence.active.is_none());
    assert!(convergence.pending.is_none());
    assert_eq!(
        convergence.request_retry()?,
        RasterConvergenceRetry::Requested {
            revision: VoxelSceneRevision::new(41),
        }
    );
    wait_until_ready(&mut convergence)?;
    Ok(())
}

#[test]
fn terminated_preparation_clears_dead_active_state_and_retry_progresses()
-> Result<(), Box<dyn std::error::Error>> {
    let frontend = frontend(50, 1)?;
    let mut convergence = convergence(&frontend)?;
    convergence.accept(changed(&frontend, 0)?)?;
    wait_until_ready(&mut convergence)?;

    let active = convergence
        .active
        .as_mut()
        .ok_or("missing ready preparation")?;
    let (completion_sender, completion_receiver) = mpsc::channel();
    drop(completion_sender);
    active.status = RasterActivePreparationStatus::Running;
    active.completion_receiver = completion_receiver;

    assert!(matches!(
        convergence.drain_events(),
        Err(RasterConvergenceError::PreparationTerminated { .. })
    ));
    assert!(convergence.active.is_none());
    convergence.request_retry()?;
    wait_until_ready(&mut convergence)?;
    Ok(())
}

#[test]
fn frame_boundary_hook_commits_all_affected_entries_and_visible_revision_together()
-> Result<(), Box<dyn std::error::Error>> {
    let frontend = frontend(100, 3)?;
    let mut render_path = render_path(&frontend)?;
    let before = render_path.installed_regions().to_vec();
    render_path.begin_convergence()?;
    render_path.accept_edit_outcome(changed(&frontend, 0)?)?;
    wait_until_ready_without_draining(
        render_path
            .convergence
            .as_mut()
            .ok_or("missing convergence")?,
    )?;

    let mut convergence = render_path
        .convergence
        .take()
        .ok_or("missing convergence")?;
    assert!(matches!(
        convergence.upload_ready_with_optional_device(None, &render_path)?,
        RasterConvergenceUpload::Uploaded { revision }
            if revision == VoxelSceneRevision::new(101)
    ));
    render_path.convergence = Some(convergence);
    assert_eq!(
        render_path.visible_revision(),
        Some(VoxelSceneRevision::new(100))
    );
    assert_eq!(
        render_path.installed_source_revision(),
        Some(VoxelSceneRevision::new(100))
    );

    render_path.advance_convergence_at_frame_boundary(None)?;
    assert_eq!(
        render_path.visible_revision(),
        Some(VoxelSceneRevision::new(101))
    );
    assert_eq!(
        render_path.installed_source_revision(),
        Some(VoxelSceneRevision::new(101))
    );
    assert!(
        render_path
            .installed_artifact()
            .ok_or("missing committed artifact")?
            .regions()
            .iter()
            .filter(|region| region.identity().core_origin() != VoxelCoordinate::new(2, 0, 0))
            .all(|region| region.source_revision() == VoxelSceneRevision::new(101))
    );
    for installation in render_path.installed_regions() {
        let prior = before
            .iter()
            .find(|prior| prior.identity() == installation.identity())
            .ok_or("missing prior installation")?;
        if installation.identity().core_origin() == VoxelCoordinate::new(2, 0, 0) {
            assert_eq!(installation, prior);
        } else {
            assert_eq!(
                installation.installation_generation(),
                prior
                    .installation_generation()
                    .checked_successor()
                    .ok_or("test installation generation overflow")?
            );
        }
    }
    Ok(())
}

#[test]
fn configured_candidate_swaps_affected_resources_and_preserves_unaffected_resources()
-> Result<(), Box<dyn std::error::Error>> {
    let frontend = frontend(110, 3)?;
    let mut render_path = render_path(&frontend)?;
    render_path.region_resources = render_path
        .installed_regions()
        .iter()
        .enumerate()
        .map(|(index, installation)| {
            Ok(fake_resources(
                installation.identity().clone(),
                u64::try_from(index)? + 10,
            ))
        })
        .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;
    let unaffected_identity = RasterRegionIdentity {
        volume_identity: VoxelVolumeId::new("terrain"),
        core_origin: VoxelCoordinate::new(2, 0, 0),
    };
    let unaffected_buffer = render_path
        .region_resources
        .iter()
        .find(|resources| resources.identity == unaffected_identity)
        .ok_or("missing unaffected resource")?
        .vertex_buffer;
    let before_installations = render_path.installed_regions().to_vec();
    let mut convergence = RasterConvergence::from_visible(&render_path)?;
    convergence.accept(changed(&frontend, 0)?)?;
    wait_until_ready_without_draining(&mut convergence)?;
    let mut next_resource_identity = 100_u64;
    convergence.upload_ready_with_test_resources(&render_path, &mut |region| {
        let resources = fake_resources(region.identity().clone(), next_resource_identity);
        next_resource_identity += 1;
        Ok(resources)
    })?;
    assert!(matches!(
        convergence.upload_ready_with_test_resources(&render_path, &mut |_| {
            Err(RasterConvergenceError::ResourceBookkeepingAllocation)
        })?,
        RasterConvergenceUpload::CandidateAlreadyRetained { .. }
    ));
    assert_eq!(
        render_path.installed_source_revision(),
        Some(VoxelSceneRevision::new(110))
    );

    let RasterConvergenceCommit::Committed { retirement, .. } =
        convergence.commit_at_frame_boundary(&mut render_path)?
    else {
        return Err("configured candidate was not committed".into());
    };
    assert_eq!(retirement.resource_count(), 2);
    assert_eq!(
        render_path
            .region_resources
            .iter()
            .find(|resources| resources.identity == unaffected_identity)
            .ok_or("missing retained unaffected resource")?
            .vertex_buffer,
        unaffected_buffer
    );
    let unaffected_installation = render_path
        .installed_regions()
        .iter()
        .find(|installation| installation.identity() == &unaffected_identity)
        .ok_or("missing unaffected installation")?;
    let prior_unaffected = before_installations
        .iter()
        .find(|installation| installation.identity() == &unaffected_identity)
        .ok_or("missing prior unaffected installation")?;
    assert_eq!(unaffected_installation, prior_unaffected);
    assert!(render_path.region_resources.iter().all(|resources| {
        resources.identity == unaffected_identity || resources.vertex_buffer.as_raw() >= 100
    }));
    Ok(())
}

#[test]
fn upload_failure_pauses_the_required_target_and_preserves_the_visible_installation()
-> Result<(), Box<dyn std::error::Error>> {
    let frontend = frontend(150, 1)?;
    let mut render_path = render_path(&frontend)?;
    render_path.region_resources = render_path
        .installed_regions()
        .iter()
        .map(|installation| fake_resources(installation.identity().clone(), 10))
        .collect();
    let visible_installations = render_path.installed_regions().to_vec();
    let mut convergence = RasterConvergence::from_visible(&render_path)?;
    convergence.accept(changed(&frontend, 0)?)?;
    wait_until_ready_without_draining(&mut convergence)?;

    assert!(matches!(
        convergence.upload_ready_with_test_resources(&render_path, &mut |_| {
            Err(RasterConvergenceError::ResourceBookkeepingAllocation)
        })?,
        RasterConvergenceUpload::NoReadyPreparation
    ));
    assert_eq!(render_path.installed_regions(), visible_installations);
    assert!(convergence.active.is_none());
    assert!(convergence.pending.is_none());
    assert!(convergence.events.retained.iter().any(|event| matches!(
        event,
        RasterConvergenceEvent::Failure { failure }
            if failure.scene_identity() == &VoxelSceneId::new("convergence-unit")
                && failure.failed_revision() == VoxelSceneRevision::new(151)
                && failure.phase() == RasterConvergenceFailurePhase::Upload
                && failure.region_identity().is_some()
                && failure.source().contains("resource bookkeeping")
    )));
    assert_eq!(
        convergence.request_retry()?,
        RasterConvergenceRetry::Requested {
            revision: VoxelSceneRevision::new(151)
        }
    );
    wait_until_ready_without_draining(&mut convergence)?;
    convergence.upload_ready_with_test_resources(&render_path, &mut |region| {
        Ok(fake_resources(region.identity().clone(), 60))
    })?;
    let RasterConvergenceCommit::Committed { retirement } =
        convergence.commit_at_frame_boundary(&mut render_path)?
    else {
        return Err("retried complete view was not committed".into());
    };
    assert_eq!(retirement.resource_count(), 1);
    assert_eq!(
        render_path.installed_source_revision(),
        Some(VoxelSceneRevision::new(151))
    );
    Ok(())
}

#[test]
fn installation_generation_overflow_uses_the_contextual_upload_failure_path()
-> Result<(), Box<dyn std::error::Error>> {
    let frontend = frontend(155, 1)?;
    let mut render_path = render_path(&frontend)?;
    let installation = render_path
        .installed_regions
        .get_mut(0)
        .ok_or("missing visible installation")?;
    installation.installation_generation = RasterRegionInstallationGeneration::new(u64::MAX);
    let visible_installations = render_path.installed_regions().to_vec();
    let mut convergence = RasterConvergence::from_visible(&render_path)?;
    convergence.accept(changed(&frontend, 0)?)?;
    wait_until_ready_without_draining(&mut convergence)?;

    assert!(matches!(
        convergence.upload_ready_with_optional_device(None, &render_path)?,
        RasterConvergenceUpload::NoReadyPreparation
    ));
    assert_eq!(render_path.installed_regions(), visible_installations);
    assert!(convergence.active.is_none());
    assert!(convergence.paused.is_some());
    assert!(convergence.events.retained.iter().any(|event| matches!(
        event,
        RasterConvergenceEvent::Failure { failure }
            if failure.failed_revision() == VoxelSceneRevision::new(156)
                && failure.phase() == RasterConvergenceFailurePhase::Upload
                && failure.region_identity().is_some()
                && failure.source().contains("generation overflow")
    )));
    Ok(())
}

#[test]
fn missing_configured_region_uses_the_contextual_upload_failure_path()
-> Result<(), Box<dyn std::error::Error>> {
    let frontend = frontend(157, 2)?;
    let mut render_path = render_path(&frontend)?;
    let retained_region = render_path
        .installed_regions()
        .first()
        .ok_or("missing first visible installation")?;
    render_path.region_resources = vec![fake_resources(retained_region.identity().clone(), 40)];
    let mut convergence = RasterConvergence::from_visible(&render_path)?;
    convergence.accept(changed(&frontend, 1)?)?;
    wait_until_ready_without_draining(&mut convergence)?;

    assert!(matches!(
        convergence.upload_ready_with_test_resources(&render_path, &mut |region| {
            Ok(fake_resources(region.identity().clone(), 50))
        })?,
        RasterConvergenceUpload::NoReadyPreparation
    ));
    assert!(convergence.active.is_none());
    assert!(convergence.paused.is_some());
    assert!(convergence.events.retained.iter().any(|event| matches!(
        event,
        RasterConvergenceEvent::Failure { failure }
            if failure.failed_revision() == VoxelSceneRevision::new(158)
                && failure.phase() == RasterConvergenceFailurePhase::Upload
                && failure.region_identity().is_some()
                && failure.source().contains("configured GPU resources are missing")
    )));
    Ok(())
}

#[test]
fn commit_failure_discards_the_hidden_candidate_and_pauses_the_required_target()
-> Result<(), Box<dyn std::error::Error>> {
    let frontend = frontend(160, 1)?;
    let mut render_path = render_path(&frontend)?;
    render_path.region_resources = render_path
        .installed_regions()
        .iter()
        .map(|installation| fake_resources(installation.identity().clone(), 20))
        .collect();
    let visible_installations = render_path.installed_regions().to_vec();
    let mut convergence = RasterConvergence::from_visible(&render_path)?;
    convergence.accept(changed(&frontend, 0)?)?;
    wait_until_ready_without_draining(&mut convergence)?;
    convergence.upload_ready_with_test_resources(&render_path, &mut |region| {
        Ok(fake_resources(region.identity().clone(), 30))
    })?;
    convergence
        .hidden_candidate
        .as_mut()
        .ok_or("missing hidden candidate")?
        .visible_installations
        .clear();

    let RasterConvergenceCommit::Failed { retirement } =
        convergence.commit_at_frame_boundary(&mut render_path)?
    else {
        return Err("commit failure did not return candidate resources for retirement".into());
    };
    assert_eq!(retirement.resource_count(), 1);
    assert_eq!(render_path.installed_regions(), visible_installations);
    assert!(convergence.hidden_candidate.is_none());
    assert!(convergence.events.retained.iter().any(|event| matches!(
        event,
        RasterConvergenceEvent::Failure { failure }
            if failure.failed_revision() == VoxelSceneRevision::new(161)
                && failure.phase() == RasterConvergenceFailurePhase::Commit
                && failure.source().contains("changed before candidate commit")
    )));
    assert_eq!(
        convergence.request_retry()?,
        RasterConvergenceRetry::Requested {
            revision: VoxelSceneRevision::new(161)
        }
    );
    Ok(())
}

#[test]
fn late_older_derivation_failure_is_observable_without_pausing_newer_convergence()
-> Result<(), Box<dyn std::error::Error>> {
    let frontend = frontend(170, 2)?;
    let mut convergence = convergence(&frontend)?;
    convergence.accept(changed(&frontend, 0)?)?;
    wait_until_ready_without_draining(&mut convergence)?;

    let active = convergence
        .active
        .as_mut()
        .ok_or("missing older preparation")?;
    let (completion_sender, completion_receiver) = mpsc::sync_channel(1);
    completion_sender.send(RasterPreparationCompletion::Completed(Err(
        RasterDerivationFailure {
            region_identity: Some(RasterRegionIdentity {
                volume_identity: VoxelVolumeId::new("terrain"),
                core_origin: VoxelCoordinate::new(0, 0, 0),
            }),
            source: metadata_dimensions_error(VoxelSceneRevision::new(171)),
        },
    )))?;
    active.status = RasterActivePreparationStatus::Running;
    active.completion_receiver = completion_receiver;
    active.worker = Some(
        convergence
            .worker_pool
            .as_ref()
            .ok_or("missing pool")?
            .execute(|| {})?,
    );
    convergence.accept(changed(&frontend, 1)?)?;

    let events = convergence.drain_events()?;
    assert!(events.iter().any(|event| matches!(
        event,
        RasterConvergenceEvent::Failure { failure }
            if failure.failed_revision() == VoxelSceneRevision::new(171)
                && failure.phase() == RasterConvergenceFailurePhase::Derivation
                && failure.region_identity().is_some()
    )));
    wait_until_ready_without_draining(&mut convergence)?;
    assert_eq!(
        convergence.required_revision(),
        VoxelSceneRevision::new(172)
    );
    assert!(convergence.paused.is_none());
    Ok(())
}

#[test]
fn newer_changed_outcome_resumes_after_the_older_requirement_was_paused()
-> Result<(), Box<dyn std::error::Error>> {
    let frontend = frontend(180, 2)?;
    let mut convergence = convergence(&frontend)?;
    convergence.accept(changed(&frontend, 0)?)?;
    wait_until_ready_without_draining(&mut convergence)?;
    let active = convergence
        .active
        .as_mut()
        .ok_or("missing older preparation")?;
    let (completion_sender, completion_receiver) = mpsc::sync_channel(1);
    completion_sender.send(RasterPreparationCompletion::Completed(Err(
        RasterDerivationFailure {
            region_identity: None,
            source: metadata_dimensions_error(VoxelSceneRevision::new(181)),
        },
    )))?;
    active.status = RasterActivePreparationStatus::Running;
    active.completion_receiver = completion_receiver;
    active.worker = Some(
        convergence
            .worker_pool
            .as_ref()
            .ok_or("missing pool")?
            .execute(|| {})?,
    );
    convergence.drain_events()?;
    assert!(convergence.paused.is_some());

    convergence.accept(changed(&frontend, 1)?)?;
    wait_until_ready_without_draining(&mut convergence)?;
    assert_eq!(
        convergence.required_revision(),
        VoxelSceneRevision::new(182)
    );
    assert!(convergence.paused.is_none());
    Ok(())
}

#[test]
fn superseded_candidate_is_rejected_and_event_retention_stays_bounded()
-> Result<(), Box<dyn std::error::Error>> {
    let frontend = frontend(200, 24)?;
    let mut render_path = render_path(&frontend)?;
    let mut convergence = RasterConvergence::from_visible(&render_path)?;

    convergence.accept(changed(&frontend, 0)?)?;
    wait_until_ready_without_draining(&mut convergence)?;
    convergence.upload_ready_with_optional_device(None, &render_path)?;
    convergence.accept(changed(&frontend, 1)?)?;
    assert!(matches!(
        convergence.commit_at_frame_boundary(&mut render_path)?,
        RasterConvergenceCommit::Rejected { .. }
    ));
    assert!(convergence.events.retained.iter().any(|event| matches!(
        event,
        RasterConvergenceEvent::CandidateRejected { revision, .. }
            if *revision == VoxelSceneRevision::new(201)
    )));
    assert_eq!(convergence.visible_revision(), VoxelSceneRevision::new(200));

    for coordinate in 2..24 {
        convergence.accept(changed(&frontend, coordinate)?)?;
        wait_until_ready_without_draining(&mut convergence)?;
        convergence.upload_ready_with_optional_device(None, &render_path)?;
        let commit = convergence.commit_at_frame_boundary(&mut render_path)?;
        if !matches!(commit, RasterConvergenceCommit::Committed { .. }) {
            return Err("newest candidate was not committed".into());
        }
    }
    assert_eq!(
        convergence.events.retained.len(),
        RASTER_CONVERGENCE_EVENT_CAPACITY
    );
    let events = convergence.drain_events()?;
    assert!(matches!(
        events.first(),
        Some(RasterConvergenceEvent::EventsCompacted { discarded }) if *discarded > 0
    ));
    assert!(events.iter().any(|event| matches!(
        event,
        RasterConvergenceEvent::CandidateRejectionsCompacted {
            first_revision,
            last_revision,
            discarded: 1,
            disposition: RasterPreparationDisposition::SupersededAfterUpload,
        } if *first_revision == VoxelSceneRevision::new(201)
            && *last_revision == VoxelSceneRevision::new(201)
    )));
    assert!(
        events
            .iter()
            .any(|event| matches!(event, RasterConvergenceEvent::CandidateCommitted { .. }))
    );
    Ok(())
}

#[test]
fn newer_generation_rejects_a_same_revision_candidate() -> Result<(), Box<dyn std::error::Error>> {
    let frontend = frontend(300, 1)?;
    let mut render_path = render_path(&frontend)?;
    let mut convergence = RasterConvergence::from_visible(&render_path)?;
    convergence.accept(changed(&frontend, 0)?)?;
    wait_until_ready_without_draining(&mut convergence)?;
    convergence.upload_ready_with_optional_device(None, &render_path)?;
    assert_eq!(
        convergence.request_retry()?,
        RasterConvergenceRetry::Requested {
            revision: VoxelSceneRevision::new(301),
        }
    );

    assert!(matches!(
        convergence.commit_at_frame_boundary(&mut render_path)?,
        RasterConvergenceCommit::Rejected { .. }
    ));
    assert_eq!(convergence.visible_revision(), VoxelSceneRevision::new(300));
    assert_eq!(
        render_path.installed_source_revision(),
        Some(VoxelSceneRevision::new(300))
    );
    Ok(())
}

#[test]
fn convergence_reuses_pool_across_generations_and_shutdown_releases_it()
-> Result<(), Box<dyn std::error::Error>> {
    let frontend = frontend(500, 12)?;
    let mut render_path = render_path(&frontend)?;
    let mut convergence = RasterConvergence::from_visible(&render_path)?;
    convergence.accept(changed(&frontend, 0)?)?;
    wait_until_ready_without_draining(&mut convergence)?;
    let pool = std::sync::Arc::downgrade(convergence.worker_pool.as_ref().ok_or("missing pool")?);
    for coordinate in [3, 6, 9] {
        convergence.accept(changed(&frontend, coordinate)?)?;
        wait_until_ready_without_draining(&mut convergence)?;
        assert!(std::ptr::eq(
            pool.as_ptr(),
            std::sync::Arc::as_ptr(
                convergence
                    .worker_pool
                    .as_ref()
                    .ok_or("missing reused pool")?
            )
        ));
    }
    convergence.upload_ready_with_optional_device(None, &render_path)?;
    let RasterConvergenceCommit::Committed { retirement } =
        convergence.commit_at_frame_boundary(&mut render_path)?
    else {
        return Err("candidate did not commit".into());
    };
    retirement.release_with(drop);
    let expected = derive_raster_regions(&frontend.scene_view()?, VoxelExtent::new(1, 1, 1))?;
    let installed = render_path
        .installed_artifact()
        .ok_or("missing installed artifact")?;
    assert_eq!(installed.regions().len(), expected.regions().len());
    for (actual, expected) in installed.regions().iter().zip(expected.regions()) {
        assert_eq!(actual.identity(), expected.identity());
        assert_eq!(actual.vertices(), expected.vertices());
        assert_eq!(actual.indices(), expected.indices());
        assert_eq!(actual.semantic_faces(), expected.semantic_faces());
    }
    let shutdown = convergence.shutdown();
    assert!(shutdown.worker_error.is_none());
    assert!(convergence.worker_pool.is_none());
    assert!(pool.upgrade().is_none());
    Ok(())
}

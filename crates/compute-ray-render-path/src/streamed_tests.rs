use super::*;
use semantic_ray_oracle::{SemanticRay, SemanticRayResult};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};
use voxel_frontend::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn wait_for_preparation_barrier(control: &ComputeConvergenceController) -> TestResult {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !control
        .preparation_barrier_observation()?
        .is_some_and(|observation| observation.reached_revision().is_some())
    {
        if Instant::now() >= deadline {
            return Err("preparation barrier not reached".into());
        }
        std::thread::yield_now();
    }
    Ok(())
}

#[derive(Default)]
struct GenerationGate {
    state: Mutex<(bool, bool)>,
    changed: Condvar,
}

impl GenerationGate {
    fn hold(&self) -> Result<(), VoxelSourceError> {
        let mut state = self
            .state
            .lock()
            .map_err(|error| VoxelSourceError::Generation {
                message: error.to_string(),
            })?;
        state.0 = true;
        self.changed.notify_all();
        while !state.1 {
            state = self
                .changed
                .wait(state)
                .map_err(|error| VoxelSourceError::Generation {
                    message: error.to_string(),
                })?;
        }
        Ok(())
    }

    fn wait(&self) -> TestResult {
        let state = self
            .state
            .lock()
            .map_err(|_| "generation gate unavailable")?;
        let (state, _) = self
            .changed
            .wait_timeout_while(state, Duration::from_secs(10), |state| !state.0)
            .map_err(|_| "generation gate unavailable")?;
        if !state.0 {
            return Err("generation did not reach gate".into());
        }
        Ok(())
    }

    fn release(&self) -> TestResult {
        self.state
            .lock()
            .map_err(|_| "generation gate unavailable")?
            .1 = true;
        self.changed.notify_all();
        Ok(())
    }
}

struct Recipe {
    scene: VoxelSceneId,
    metadata: VoxelVolumeMetadata,
    generations: Arc<AtomicUsize>,
    gate: Option<Arc<GenerationGate>>,
}

impl VoxelVolumeSource for Recipe {
    fn scene_id(&self) -> &VoxelSceneId {
        &self.scene
    }
    fn requires_materials(&self) -> bool {
        true
    }
    fn value(&self, _: VoxelCoordinate) -> Result<VoxelValue, VoxelSourceError> {
        Ok(VoxelValue::Occupied(VoxelMaterialId::new("stone")))
    }
    fn materialize(&self) -> Result<SparseVoxelVolume, VoxelSourceError> {
        self.generations.fetch_add(1, Ordering::SeqCst);
        if let Some(gate) = &self.gate {
            gate.hold()?;
        }
        Ok(SparseVoxelVolume::new(
            self.metadata.clone(),
            SparseVoxelBackground::Empty,
            vec![SparseVoxelBatch::Fill(VoxelRegionFill::new(
                VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), self.metadata.extent()),
                VoxelValue::Occupied(VoxelMaterialId::new("stone")),
            ))],
        )
        .with_storage_tier(StorageTier::SparsePages))
    }
}

struct Fixture {
    frontend: Arc<VoxelFrontend>,
    adapter: ComputeRayRenderPathAdapter,
    generations: Vec<Arc<AtomicUsize>>,
}

fn volume(index: usize) -> VoxelVolumeId {
    VoxelVolumeId::new(format!("{index:02}"))
}

impl Fixture {
    fn new(gate: Option<Arc<GenerationGate>>) -> Result<Self, Box<dyn std::error::Error>> {
        Self::with_limits(gate, 9, 20)
    }

    fn with_limits(
        gate: Option<Arc<GenerationGate>>,
        maximum_selection_volumes: usize,
        count: usize,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let frontend = Arc::new(VoxelFrontend::with_residency_limits(
            VoxelResidencyLimits::new(maximum_selection_volumes)?,
        ));
        let scene = VoxelSceneId::new("streamed-compute");
        let generations: Vec<_> = (0..count).map(|_| Arc::new(AtomicUsize::new(0))).collect();
        let volumes = generations
            .iter()
            .enumerate()
            .map(|(index, generations)| {
                let metadata = VoxelVolumeMetadata::new(
                    volume(index),
                    VoxelExtent::new(8, 8, 8),
                    [index as f32 * 8.0, 0.0, 0.0],
                    1.0,
                );
                StreamedVoxelVolume::new(
                    metadata.clone(),
                    Arc::new(Recipe {
                        scene: scene.clone(),
                        metadata,
                        generations: generations.clone(),
                        gate: if index == 9 { gate.clone() } else { None },
                    }),
                )
            })
            .collect();
        let view = frontend.publish_streamed(StreamedVoxelScene::new(
            scene,
            VoxelSceneRevision::new(1),
            vec![VoxelMaterial::new(VoxelMaterialId::new("stone"), [1.0; 4])],
            volumes,
        ))?;
        let selection = view.residency_selection(
            VoxelResidencySelectionId::new(1),
            (0..maximum_selection_volumes).map(volume),
        )?;
        let adapter = ComputeRayRenderPathAdapter::new_streamed(
            frontend.clone(),
            selection,
            CameraState::new([2.0; 3], [0.0; 3], [0.0, 1.0, 0.0], 50.0, 0.1, 100.0)?,
            CameraStateRevision::new(1),
            ComputeRepresentation::Brickmap {
                budget_bytes: 1_000_000,
            },
        )?;
        Ok(Self {
            frontend,
            adapter,
            generations,
        })
    }

    fn selection(
        &self,
        identity: u64,
        indices: std::ops::Range<usize>,
    ) -> Result<VoxelResidencySelection, VoxelFrontendError> {
        self.frontend.scene_view()?.residency_selection(
            VoxelResidencySelectionId::new(identity),
            indices.map(volume),
        )
    }

    fn ready(&mut self) -> TestResult {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            self.adapter
                .render_path
                .convergence
                .retain_ready_candidate();
            if self.adapter.convergence_status().hidden().is_some() {
                return Ok(());
            }
            std::thread::yield_now();
        }
        Err(format!(
            "candidate did not become ready: {:?}",
            self.adapter.convergence_status()
        )
        .into())
    }

    fn install(&mut self) -> TestResult {
        self.ready()?;
        let visible = self.adapter.convergence_status().visible_revision();
        drop(
            self.adapter
                .render_path
                .convergence
                .install_hidden(visible)?,
        );
        Ok(())
    }
}

#[test]
fn streamed_selections_install_complete_bundles_from_shared_copies() -> TestResult {
    let mut fixture = Fixture::new(None)?;
    let initial = fixture.adapter.scene_bundle().clone();
    let selection = fixture.selection(2, 9..18)?;
    let view = fixture.frontend.scene_view()?;
    let shared = fixture.frontend.materialize_residency(&selection, &view)?;
    fixture
        .adapter
        .submit_residency_selection(selection.clone())
        .map_err(|error| -> Box<dyn std::error::Error> { error })?;
    fixture.ready()?;
    assert_eq!(
        fixture.adapter.stamp().installed_selection(),
        initial
            .residency_selection()
            .map(VoxelResidencySelection::identity)
    );
    assert_eq!(fixture.frontend.materialization_cache_stats()?.copies, 18);
    assert_eq!(
        fixture
            .adapter
            .render_path
            .owned_resource_counts()?
            .residency_copies(),
        18
    );
    assert!(
        fixture
            .generations
            .iter()
            .take(18)
            .all(|count| count.load(Ordering::SeqCst) == 1)
    );
    fixture.install()?;
    assert_eq!(
        fixture
            .adapter
            .scene_bundle()
            .volume_headers()
            .iter()
            .map(|header| header.identity().clone())
            .collect::<Vec<_>>(),
        selection.volumes()
    );
    assert_eq!(
        fixture.adapter.stamp().installed_selection(),
        Some(selection.identity())
    );
    assert_eq!(
        fixture
            .adapter
            .scene_bundle()
            .brickmap_patch_observations()
            .uploaded_bytes,
        0
    );
    assert_eq!(
        initial
            .volume_headers()
            .first()
            .ok_or("missing volume")?
            .identity(),
        &volume(0)
    );
    drop(initial);
    drop(shared);
    assert_eq!(fixture.frontend.materialization_cache_stats()?.copies, 9);
    assert_eq!(
        fixture
            .adapter
            .render_path
            .owned_resource_counts()?
            .residency_copies(),
        9
    );
    Ok(())
}

#[test]
fn streamed_combined_targets_supersede_preparing_and_uploaded_selections() -> TestResult {
    let mut fixture = Fixture::new(None)?;
    let control = fixture.adapter.enable_convergence_control();
    control.hold_next_preparation_after_blocks(1)?;
    fixture
        .adapter
        .submit_residency_selection(fixture.selection(2, 9..18)?)
        .map_err(|error| -> Box<dyn std::error::Error> { error })?;
    wait_for_preparation_barrier(&control)?;
    let outcome = fixture.frontend.edit(VoxelEditCommand::new(
        volume(18),
        VoxelCoordinate::new(0, 0, 0),
        VoxelValue::Empty,
    ))?;
    fixture.adapter.accept_edit_outcome(outcome)?;
    let newest = fixture.selection(3, 11..20)?;
    fixture
        .adapter
        .submit_residency_selection(newest.clone())
        .map_err(|error| -> Box<dyn std::error::Error> { error })?;
    assert_eq!(fixture.adapter.convergence_status().worker_count(), 1);
    control.release_preparation_barrier()?;
    fixture.ready()?;
    fixture
        .adapter
        .render_path
        .convergence
        .mark_hidden_uploaded();
    let newer = fixture.selection(4, 10..19)?;
    fixture
        .adapter
        .submit_residency_selection(newer.clone())
        .map_err(|error| -> Box<dyn std::error::Error> { error })?;
    fixture.install()?;
    assert_eq!(
        fixture.adapter.stamp().visible_revision(),
        VoxelSceneRevision::new(2)
    );
    assert_eq!(
        fixture.adapter.stamp().installed_selection(),
        Some(newer.identity())
    );
    let ray = SemanticRay::new([144.5, 0.5, 0.5], [1.0, 0.0, 0.0], 0.0, 0.4)?;
    assert!(matches!(
        fixture.adapter.scene_bundle().observe(&ray).result(),
        SemanticRayResult::Miss
    ));
    let installed: Vec<_> = fixture
        .adapter
        .drain_convergence_events()
        .into_iter()
        .filter_map(|event| {
            if let ComputeConvergenceEvent::CandidateInstalled { stamp } = event {
                Some(stamp)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(installed.len(), 1);
    assert_eq!(
        installed
            .first()
            .ok_or("missing install")?
            .residency_selection(),
        Some(newer.identity())
    );
    assert!(fixture.frontend.materialization_cache_stats()?.peak_copies <= 18);
    Ok(())
}

#[test]
fn streamed_materialization_cancellation_finishes_only_its_current_volume() -> TestResult {
    let gate = Arc::new(GenerationGate::default());
    let mut fixture = Fixture::new(Some(gate.clone()))?;
    fixture
        .adapter
        .submit_residency_selection(fixture.selection(2, 9..18)?)
        .map_err(|error| -> Box<dyn std::error::Error> { error })?;
    gate.wait()?;
    let newest = fixture.selection(3, 11..20)?;
    fixture
        .adapter
        .submit_residency_selection(newest.clone())
        .map_err(|error| -> Box<dyn std::error::Error> { error })?;
    gate.release()?;
    fixture.install()?;
    assert_eq!(
        fixture
            .generations
            .get(9)
            .ok_or("missing counter")?
            .load(Ordering::SeqCst),
        1
    );
    assert_eq!(
        fixture
            .generations
            .get(10)
            .ok_or("missing counter")?
            .load(Ordering::SeqCst),
        0
    );
    assert_eq!(
        fixture.adapter.stamp().installed_selection(),
        Some(newest.identity())
    );
    assert_eq!(fixture.frontend.materialization_cache_stats()?.copies, 9);
    Ok(())
}

#[test]
fn streamed_failures_preserve_presenting_bundle_and_retry_selection_only_work() -> TestResult {
    for phase in [
        ComputeConvergenceFailurePhase::Preparation,
        ComputeConvergenceFailurePhase::Upload,
        ComputeConvergenceFailurePhase::Installation,
    ] {
        let mut fixture = Fixture::new(None)?;
        let initial_words = fixture.adapter.scene_bundle().storage_words();
        let initial_stamp = fixture.adapter.stamp();
        let control = fixture.adapter.enable_convergence_control();
        control.inject_next_failure(phase)?;
        let selection = fixture.selection(2, 9..18)?;
        fixture
            .adapter
            .submit_residency_selection(selection.clone())
            .map_err(|error| -> Box<dyn std::error::Error> { error })?;
        if phase == ComputeConvergenceFailurePhase::Preparation {
            let deadline = Instant::now() + Duration::from_secs(10);
            while fixture.adapter.convergence_status().paused().is_none() {
                fixture.adapter.drain_convergence_events();
                if Instant::now() >= deadline {
                    return Err("preparation did not fail".into());
                }
                std::thread::yield_now();
            }
        } else {
            fixture.ready()?;
            assert!(
                fixture
                    .adapter
                    .render_path
                    .convergence
                    .fail_hidden_if_injected(phase)?
            );
        }
        assert_eq!(
            fixture.adapter.scene_bundle().storage_words(),
            initial_words
        );
        assert_eq!(
            fixture.adapter.stamp().visible_revision(),
            initial_stamp.visible_revision()
        );
        assert_eq!(
            fixture.adapter.stamp().installed_selection(),
            initial_stamp.installed_selection()
        );
        assert_eq!(fixture.frontend.materialization_cache_stats()?.copies, 9);
        assert!(matches!(
            fixture.adapter.request_convergence_retry()?,
            ComputeConvergenceRetry::Requested { .. }
        ));
        fixture.install()?;
        assert_eq!(
            fixture.adapter.stamp().installed_selection(),
            Some(selection.identity())
        );
        assert_eq!(fixture.frontend.materialization_cache_stats()?.copies, 9);
    }
    Ok(())
}

#[test]
fn streamed_retirement_releases_preparing_and_installed_copies_and_workers() -> TestResult {
    let mut fixture = Fixture::new(None)?;
    let control = fixture.adapter.enable_convergence_control();
    control.hold_next_preparation_after_blocks(1)?;
    fixture
        .adapter
        .submit_residency_selection(fixture.selection(2, 9..18)?)
        .map_err(|error| -> Box<dyn std::error::Error> { error })?;
    wait_for_preparation_barrier(&control)?;
    assert_eq!(fixture.frontend.materialization_cache_stats()?.copies, 18);
    fixture.adapter.render_path.convergence.shutdown()?;
    assert_eq!(fixture.frontend.materialization_cache_stats()?.copies, 0);
    assert_eq!(fixture.adapter.convergence_status().worker_count(), 0);
    assert_eq!(
        fixture
            .adapter
            .render_path
            .owned_resource_counts()?
            .residency_copies(),
        0
    );
    assert!(
        fixture
            .adapter
            .render_path
            .owned_resource_counts()?
            .is_zero()
    );
    Ok(())
}

#[test]
fn streamed_revision_changes_within_a_selection_rebuild_the_complete_bundle() -> TestResult {
    let mut fixture = Fixture::new(None)?;
    let old_words = fixture.adapter.scene_bundle().storage_words();
    let outcome = fixture.frontend.edit(VoxelEditCommand::new(
        volume(0),
        VoxelCoordinate::new(0, 0, 0),
        VoxelValue::Empty,
    ))?;
    fixture.adapter.accept_edit_outcome(outcome)?;
    fixture.ready()?;
    assert_eq!(fixture.adapter.scene_bundle().storage_words(), old_words);
    fixture.install()?;
    assert_eq!(
        fixture.adapter.stamp().visible_revision(),
        VoxelSceneRevision::new(2)
    );
    assert_eq!(
        fixture.adapter.stamp().installed_selection(),
        Some(VoxelResidencySelectionId::new(1))
    );
    assert_ne!(fixture.adapter.scene_bundle().storage_words(), old_words);
    assert_eq!(
        fixture
            .adapter
            .scene_bundle()
            .brickmap_patch_observations()
            .uploaded_bytes,
        0
    );
    assert_eq!(fixture.frontend.materialization_cache_stats()?.copies, 9);
    assert_eq!(
        fixture
            .generations
            .first()
            .ok_or("missing counter")?
            .load(Ordering::SeqCst),
        2
    );
    assert!(
        fixture
            .generations
            .iter()
            .skip(1)
            .take(8)
            .all(|count| count.load(Ordering::SeqCst) == 1)
    );
    Ok(())
}

#[test]
fn streamed_selection_rejections_preserve_requirements_and_the_nine_volume_bound() -> TestResult {
    let mut fixture = Fixture::new(None)?;
    let initial = fixture.adapter.convergence_status();
    assert!(matches!(
        fixture
            .adapter
            .render_path
            .convergence
            .accept_residency_selection(fixture.selection(1, 9..18)?),
        Err(ComputeConvergenceError::Residency(
            VoxelFrontendError::ResidencyIdentityConflict
        ))
    ));
    assert!(matches!(
        fixture
            .adapter
            .render_path
            .convergence
            .accept_residency_selection(fixture.selection(2, 9..19)?),
        Err(ComputeConvergenceError::ResidencyVolumeLimit { maximum: 9 })
    ));
    fixture
        .adapter
        .render_path
        .convergence
        .accept_residency_selection(fixture.selection(0, 9..18)?)?;
    assert_eq!(fixture.adapter.convergence_status(), initial);
    assert_eq!(
        fixture.frontend.materialization_cache_stats()?.peak_copies,
        9
    );
    assert!(
        fixture
            .generations
            .iter()
            .skip(9)
            .all(|count| count.load(Ordering::SeqCst) == 0)
    );
    assert!(matches!(
        ComputeRayRenderPathAdapter::new_streamed(
            fixture.frontend.clone(),
            fixture.selection(2, 0..10)?,
            fixture.adapter.camera_state(),
            CameraStateRevision::new(1),
            ComputeRepresentation::Brickmap {
                budget_bytes: 1_000_000
            },
        ),
        Err(ComputeSceneBuildError::ResidencyVolumeLimit { maximum: 9 })
    ));
    assert_eq!(
        fixture.frontend.materialization_cache_stats()?.peak_copies,
        9
    );
    Ok(())
}

#[test]
fn a_configured_maximum_selection_size_bounds_brickmap_selections() -> TestResult {
    let maximum = 25;
    let mut fixture = Fixture::with_limits(None, maximum, maximum + 2)?;
    assert_eq!(
        fixture
            .adapter
            .scene_bundle()
            .residency_selection()
            .map(|selection| selection.volumes().len()),
        Some(maximum)
    );
    assert!(matches!(
        fixture
            .adapter
            .render_path
            .convergence
            .accept_residency_selection(fixture.selection(2, 0..maximum + 1)?),
        Err(ComputeConvergenceError::ResidencyVolumeLimit { maximum: 25 })
    ));
    fixture
        .adapter
        .render_path
        .convergence
        .accept_residency_selection(fixture.selection(3, 2..maximum + 2)?)?;
    assert!(matches!(
        ComputeRayRenderPathAdapter::new_streamed(
            fixture.frontend.clone(),
            fixture.selection(4, 0..maximum + 1)?,
            fixture.adapter.camera_state(),
            CameraStateRevision::new(1),
            ComputeRepresentation::Brickmap {
                budget_bytes: 1_000_000
            },
        ),
        Err(ComputeSceneBuildError::ResidencyVolumeLimit { maximum: 25 })
    ));
    Ok(())
}

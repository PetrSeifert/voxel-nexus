use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use voxel_frontend::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;
type GenerationHook =
    Arc<dyn Fn(&VoxelVolumeMetadata) -> Result<(), VoxelSourceError> + Send + Sync>;

struct Recipe {
    scene: VoxelSceneId,
    metadata: VoxelVolumeMetadata,
    frontend: Weak<VoxelFrontend>,
    generations: Arc<AtomicUsize>,
    observations: Arc<Mutex<Vec<MaterializationCacheStats>>>,
    hook: Option<GenerationHook>,
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
        let observe = || -> Result<(), Box<dyn std::error::Error>> {
            let frontend = self.frontend.upgrade().ok_or("frontend dropped")?;
            self.observations
                .lock()
                .map_err(|_| "observations poisoned")?
                .push(frontend.materialization_cache_stats()?);
            Ok(())
        };
        observe().map_err(|error| VoxelSourceError::Generation {
            message: error.to_string(),
        })?;
        self.generations.fetch_add(1, Ordering::SeqCst);
        if let Some(hook) = &self.hook {
            hook(&self.metadata)?;
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
    view: VoxelSceneView,
    generations: Vec<Arc<AtomicUsize>>,
    observations: Arc<Mutex<Vec<MaterializationCacheStats>>>,
}

impl Fixture {
    fn new(count: usize) -> Result<Self, Box<dyn std::error::Error>> {
        Self::with_hook(count, None)
    }

    fn with_hook(
        count: usize,
        hook: Option<GenerationHook>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let frontend = Arc::new(VoxelFrontend::new());
        let scene = VoxelSceneId::new("streamed");
        let generations: Vec<_> = (0..count).map(|_| Arc::new(AtomicUsize::new(0))).collect();
        let observations = Arc::new(Mutex::new(vec![]));
        let volumes = generations
            .iter()
            .enumerate()
            .map(|(index, counter)| {
                let metadata = VoxelVolumeMetadata::new(
                    Self::volume(index),
                    VoxelExtent::new(2, 2, 2),
                    [index as f32 * 2.0, 0.0, 0.0],
                    1.0,
                );
                StreamedVoxelVolume::new(
                    metadata.clone(),
                    Arc::new(Recipe {
                        scene: scene.clone(),
                        metadata,
                        frontend: Arc::downgrade(&frontend),
                        generations: counter.clone(),
                        observations: observations.clone(),
                        hook: hook.clone(),
                    }),
                )
            })
            .collect();
        let view = frontend.publish_streamed(StreamedVoxelScene::new(
            scene,
            VoxelSceneRevision::new(7),
            vec![VoxelMaterial::new(VoxelMaterialId::new("stone"), [1.0; 4])],
            volumes,
        ))?;
        Ok(Self {
            frontend,
            view,
            generations,
            observations,
        })
    }

    fn volume(index: usize) -> VoxelVolumeId {
        VoxelVolumeId::new(format!("volume-{index:02}"))
    }

    fn selection(
        &self,
        identity: u64,
        indices: impl IntoIterator<Item = usize>,
    ) -> Result<VoxelResidencySelection, VoxelFrontendError> {
        self.view.residency_selection(
            VoxelResidencySelectionId::new(identity),
            indices.into_iter().map(Self::volume),
        )
    }
}

#[test]
fn edits_preserve_residency_and_share_only_matching_content_versions() -> TestResult {
    let fixture = Fixture::new(3)?;
    let frontend = &fixture.frontend;
    let selected = fixture.selection(1, 0..2)?;
    frontend.require_residency(selected.clone())?;
    assert!(frontend.establish_residency()?);
    let before = frontend.materialization_cache_stats()?;
    let edited = frontend.edit(VoxelEditCommand::from_edits(
        [0, 2]
            .map(|index| {
                VoxelEdit::new(
                    Fixture::volume(index),
                    VoxelCoordinate::new(0, 0, 0),
                    VoxelValue::Empty,
                )
            })
            .into(),
    ))?;
    assert_eq!(edited.view().revision(), VoxelSceneRevision::new(8));
    assert_eq!(frontend.materialization_cache_stats()?, before);
    assert_eq!(frontend.required_residency()?, Some(selected.clone()));
    assert_eq!(frontend.installed_residency()?, Some(selected.clone()));
    assert_eq!(
        fixture
            .generations
            .iter()
            .map(|counter| counter.load(Ordering::SeqCst))
            .collect::<Vec<_>>(),
        vec![1, 1, 0]
    );

    let copies = frontend.materialize_residency(&selected, edited.view())?;
    assert_eq!(frontend.materialization_cache_stats()?.copies, 3);
    assert_eq!(
        fixture
            .generations
            .iter()
            .map(|counter| counter.load(Ordering::SeqCst))
            .collect::<Vec<_>>(),
        vec![2, 1, 0]
    );
    let sample = |view: &VoxelSceneView, index| -> Result<VoxelValue, Box<dyn std::error::Error>> {
        let samples = view.read_region(
            &Fixture::volume(index),
            VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), VoxelExtent::new(1, 1, 1)),
        )?;
        Ok(samples.first().ok_or("missing sample")?.value().clone())
    };
    assert_eq!(sample(copies.scene_view(), 0)?, VoxelValue::Empty);
    assert_eq!(
        sample(&fixture.view, 0)?,
        VoxelValue::Occupied(VoxelMaterialId::new("stone"))
    );
    assert_eq!(sample(edited.view(), 2)?, VoxelValue::Empty);
    assert_eq!(frontend.materialization_cache_stats()?.copies, 3);

    let newest = edited.view().residency_selection(
        VoxelResidencySelectionId::new(2),
        [Fixture::volume(0), Fixture::volume(1)],
    )?;
    frontend.require_residency(newest.clone())?;
    assert!(frontend.establish_residency()?);
    assert_eq!(frontend.installed_residency()?, Some(newest));
    assert_eq!(frontend.materialization_cache_stats()?.copies, 2);
    assert_eq!(
        sample(&fixture.view, 0)?,
        VoxelValue::Occupied(VoxelMaterialId::new("stone"))
    );
    assert_eq!(sample(copies.scene_view(), 0)?, VoxelValue::Empty);
    assert_eq!(frontend.materialization_cache_stats()?.copies, 2);
    Ok(())
}

#[test]
fn supersession_finishes_only_the_current_volume_and_releases_before_new_admission() -> TestResult {
    use std::sync::mpsc;
    use std::time::Duration;

    let (started_sender, started_receiver) = mpsc::channel();
    let (release_sender, release_receiver) = mpsc::channel();
    let release_receiver = Mutex::new(release_receiver);
    let hook: GenerationHook = Arc::new(move |metadata| {
        if metadata.identity() == &Fixture::volume(10)
            || metadata.identity() == &Fixture::volume(18)
        {
            let wait = || -> TestResult {
                started_sender.send(metadata.identity().clone())?;
                release_receiver
                    .lock()
                    .map_err(|_| "release channel poisoned")?
                    .recv_timeout(Duration::from_secs(5))?;
                Ok(())
            };
            wait().map_err(|error| VoxelSourceError::Generation {
                message: error.to_string(),
            })?;
        }
        Ok(())
    });
    let fixture = Fixture::with_hook(27, Some(hook))?;
    let old = fixture.selection(1, 0..9)?;
    fixture.frontend.require_residency(old.clone())?;
    fixture.frontend.establish_residency()?;
    fixture
        .frontend
        .require_residency(fixture.selection(2, 9..18)?)?;
    std::thread::scope(|scope| -> TestResult {
        let worker = scope.spawn(|| fixture.frontend.establish_residency());
        assert_eq!(
            started_receiver.recv_timeout(Duration::from_secs(5))?,
            Fixture::volume(10)
        );
        assert_eq!(fixture.frontend.installed_residency()?, Some(old.clone()));
        assert!(!fixture.frontend.establish_residency()?);
        fixture
            .frontend
            .require_residency(fixture.selection(3, 18..27)?)?;
        assert!(matches!(
            fixture
                .frontend
                .materialize_residency(&fixture.selection(3, 18..27)?, &fixture.view),
            Err(VoxelFrontendError::MaterializationInProgress)
        ));
        let existing = fixture
            .frontend
            .materialize_residency(&old, &fixture.view)?;
        assert_eq!(existing.selection(), &old);
        assert_eq!(
            fixture
                .generations
                .get(18)
                .ok_or("missing newest counter")?
                .load(Ordering::SeqCst),
            0
        );
        release_sender.send(())?;
        assert_eq!(
            started_receiver.recv_timeout(Duration::from_secs(5))?,
            Fixture::volume(18)
        );
        assert_eq!(fixture.frontend.installed_residency()?, Some(old));
        // Nine installed copies plus the newest worker's reservation, with no superseded copies.
        assert_eq!(fixture.frontend.materialization_cache_stats()?.copies, 10);
        assert_eq!(fixture.view.storage_bytes(&Fixture::volume(9))?, 0);
        assert_eq!(fixture.view.storage_bytes(&Fixture::volume(10))?, 0);
        release_sender.send(())?;
        assert!(worker.join().map_err(|_| "residency worker panicked")??);
        Ok(())
    })?;
    assert_eq!(
        fixture.frontend.installed_residency()?,
        Some(fixture.selection(3, 18..27)?)
    );
    assert!(
        fixture
            .generations
            .iter()
            .skip(11)
            .take(7)
            .all(|counter| counter.load(Ordering::SeqCst) == 0)
    );
    assert_eq!(fixture.frontend.materialization_cache_stats()?.copies, 9);
    Ok(())
}

#[test]
fn selection_identity_is_independent_and_invalid_membership_is_rejected() -> TestResult {
    let frontend = VoxelFrontend::new();
    let identity = VoxelVolumeId::new("volume");
    let view = frontend.publish_sparse(SparseVoxelScene::new(
        VoxelSceneId::new("scene"),
        VoxelSceneRevision::new(100),
        vec![],
        vec![SparseVoxelVolume::new(
            VoxelVolumeMetadata::new(identity.clone(), VoxelExtent::new(1, 1, 1), [0.0; 3], 1.0),
            SparseVoxelBackground::Empty,
            vec![],
        )],
    ))?;
    let first = view.residency_selection(VoxelResidencySelectionId::new(1), [identity.clone()])?;
    let second = view.residency_selection(
        VoxelResidencySelectionId::new(2),
        [identity.clone(), identity],
    )?;
    assert!(second.identity() > first.identity());
    assert_eq!(second.volumes().len(), 1);
    assert!(matches!(
        view.residency_selection(VoxelResidencySelectionId::new(3), []),
        Err(VoxelFrontendError::EmptyResidencySelection)
    ));
    assert!(matches!(
        view.residency_selection(
            VoxelResidencySelectionId::new(3),
            [VoxelVolumeId::new("unknown")]
        ),
        Err(VoxelFrontendError::UnknownVolumeIdentity { .. })
    ));
    assert!(frontend.require_residency(first.clone())?);
    assert!(frontend.require_residency(second.clone())?);
    assert!(!frontend.require_residency(first)?);
    assert_eq!(frontend.required_residency()?, Some(second.clone()));
    assert_eq!(frontend.installed_residency()?, None);
    assert!(frontend.establish_residency()?);
    assert_eq!(frontend.installed_residency()?, Some(second));
    assert_eq!(
        frontend.scene_view()?.revision(),
        VoxelSceneRevision::new(100)
    );
    Ok(())
}

#[test]
fn disjoint_selections_and_query_reserve_exactly_nineteen_copies_including_workers() -> TestResult {
    let fixture = Fixture::new(21)?;
    let frontend = &fixture.frontend;
    let old = fixture.selection(1, 0..9)?;
    frontend.require_residency(old.clone())?;
    frontend.establish_residency()?;
    let reader = frontend.materialize_residency(&old, &fixture.view)?;
    assert_eq!(frontend.materialization_cache_stats()?.copies, 9);
    let query = fixture.view.enumerate_cells(&Fixture::volume(18), 1, 1)?;
    frontend.require_residency(fixture.selection(2, 9..18)?)?;
    frontend.establish_residency()?;
    let stats = frontend.materialization_cache_stats()?;
    assert_eq!(stats.copies, 19);
    assert_eq!(stats.peak_copies, 19);
    assert!(matches!(
        fixture.view.enumerate_cells(&Fixture::volume(19), 1, 1),
        Err(VoxelFrontendError::QueryOnlyCopyBusy)
    ));
    let overflow = fixture.selection(3, [20])?;
    assert!(matches!(
        frontend.materialize_residency(&overflow, &fixture.view),
        Err(VoxelFrontendError::MaterializationCacheExhausted)
    ));
    assert_eq!(
        fixture
            .generations
            .get(20)
            .ok_or("missing counter")?
            .load(Ordering::SeqCst),
        0
    );
    let observations = fixture
        .observations
        .lock()
        .map_err(|_| "observations poisoned")?;
    assert_eq!(
        observations.last().ok_or("no generation samples")?.copies,
        19
    );
    assert!(observations.iter().all(|sample| sample.copies <= 19));
    drop(observations);
    drop(query);
    assert_eq!(frontend.materialization_cache_stats()?.copies, 18);
    drop(reader);
    assert_eq!(frontend.materialization_cache_stats()?.copies, 9);
    assert_eq!(fixture.view.storage_bytes(&Fixture::volume(0))?, 0);
    Ok(())
}

#[test]
fn a_query_transfers_to_residency_without_generation_and_releases_on_last_drop() -> TestResult {
    let fixture = Fixture::new(3)?;
    let first = Fixture::volume(0);
    let query = fixture.view.enumerate_cells(&first, 1, 1)?;
    let selection = fixture.selection(1, [0])?;
    assert_eq!(
        fixture
            .frontend
            .materialization_cache_stats()?
            .query_only_copies,
        1
    );
    fixture.frontend.require_residency(selection.clone())?;
    fixture.frontend.establish_residency()?;
    let copies = fixture
        .frontend
        .materialize_residency(&selection, &fixture.view)?;
    let reader = copies.clone();
    assert_eq!(
        fixture
            .generations
            .first()
            .ok_or("missing generation counter")?
            .load(Ordering::SeqCst),
        1
    );
    assert_eq!(
        fixture
            .frontend
            .materialization_cache_stats()?
            .query_only_copies,
        0
    );
    assert_eq!(copies.scene_view().revision(), VoxelSceneRevision::new(7));
    assert_eq!(copies.selection(), &selection);
    assert!(copies.storage_bytes() > 0);
    let other_query = fixture.view.enumerate_cells(&Fixture::volume(2), 1, 1)?;
    fixture
        .frontend
        .require_residency(fixture.selection(2, [1])?)?;
    fixture.frontend.establish_residency()?;
    drop(copies);
    drop(reader);
    assert!(fixture.view.storage_bytes(&first)? > 0);
    drop(query);
    assert_eq!(fixture.view.storage_bytes(&first)?, 0);
    assert_eq!(fixture.frontend.materialization_cache_stats()?.copies, 2);
    drop(other_query);
    assert_eq!(fixture.frontend.materialization_cache_stats()?.copies, 1);
    let view = fixture.view.clone();
    drop(fixture);
    // Historical views retain recipes and cache access, without retaining installed payloads.
    assert_eq!(view.storage_bytes(&Fixture::volume(1))?, 0);
    Ok(())
}

#[test]
fn repeated_travel_returns_to_the_same_allocation_plateau_and_reads_keep_their_meaning()
-> TestResult {
    let fixture = Fixture::new(28)?;
    let mut plateaus = Vec::new();
    for lap in 0..3 {
        let mut samples = Vec::new();
        for (step, volumes) in [(0, 0..9), (1, 9..18), (2, 18..27), (3, 0..9)] {
            let selection = fixture.selection(lap * 4 + step + 1, volumes)?;
            fixture.frontend.require_residency(selection.clone())?;
            fixture.frontend.establish_residency()?;
            assert_eq!(fixture.frontend.installed_residency()?, Some(selection));
            for identity in [Fixture::volume(0), Fixture::volume(15), Fixture::volume(27)] {
                assert_eq!(
                    fixture.view.region_content(
                        &identity,
                        VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), VoxelExtent::new(2, 2, 2)),
                    )?,
                    VoxelRegionContent::Uniform(VoxelValue::Occupied(VoxelMaterialId::new(
                        "stone"
                    )))
                );
            }
            let stats = fixture.frontend.materialization_cache_stats()?;
            assert_eq!(stats.copies, 9);
            assert_eq!(stats.generating, 0);
            assert_eq!(stats.query_only_copies, 0);
            assert!(stats.peak_copies <= 19);
            samples.push((stats.copies, stats.storage_bytes));
            assert_eq!(
                fixture.frontend.scene_view()?.revision(),
                VoxelSceneRevision::new(7)
            );
        }
        plateaus.push(samples);
    }
    assert!(plateaus.windows(2).all(|laps| laps.first() == laps.get(1)));
    Ok(())
}

#[test]
fn generation_failure_preserves_the_installed_selection_and_can_be_retried() -> TestResult {
    let failures = Arc::new(AtomicUsize::new(0));
    let hook: GenerationHook = Arc::new(move |metadata| {
        if metadata.identity() == &Fixture::volume(2)
            && failures.fetch_add(1, Ordering::SeqCst) == 0
        {
            return Err(VoxelSourceError::Generation {
                message: "source unavailable".into(),
            });
        }
        Ok(())
    });
    let fixture = Fixture::with_hook(4, Some(hook))?;
    let old = fixture.selection(1, [0])?;
    fixture.frontend.require_residency(old.clone())?;
    fixture.frontend.establish_residency()?;
    let newest = fixture.selection(2, 1..4)?;
    fixture.frontend.require_residency(newest.clone())?;
    assert!(matches!(
        fixture.frontend.establish_residency(),
        Err(VoxelFrontendError::VolumeGeneration { .. })
    ));
    assert_eq!(fixture.frontend.installed_residency()?, Some(old));
    assert_eq!(fixture.frontend.required_residency()?, Some(newest.clone()));
    assert_eq!(fixture.frontend.materialization_cache_stats()?.copies, 1);
    assert!(fixture.frontend.establish_residency()?);
    assert_eq!(fixture.frontend.installed_residency()?, Some(newest));
    assert_eq!(fixture.frontend.materialization_cache_stats()?.copies, 3);
    Ok(())
}

#[test]
fn consumers_cannot_materialize_through_another_frontends_cache() -> TestResult {
    let owner = Fixture::new(1)?;
    let other = Fixture::new(1)?;
    let selection = owner.selection(1, [0])?;
    assert!(matches!(
        owner
            .frontend
            .materialize_residency(&selection, &other.view),
        Err(VoxelFrontendError::ResidencySceneMismatch)
    ));
    assert_eq!(other.frontend.materialization_cache_stats()?.copies, 0);
    Ok(())
}

#[test]
fn consumer_generation_stops_on_supersession_and_releases_even_its_final_volume() -> TestResult {
    use std::sync::mpsc;
    use std::time::Duration;

    for selection_size in [2, 3] {
        let (started_sender, started_receiver) = mpsc::channel();
        let (release_sender, release_receiver) = mpsc::channel();
        let release_receiver = Mutex::new(release_receiver);
        let hook: GenerationHook = Arc::new(move |metadata| {
            if metadata.identity() == &Fixture::volume(1) {
                let wait = || -> TestResult {
                    started_sender.send(())?;
                    release_receiver
                        .lock()
                        .map_err(|_| "release channel poisoned")?
                        .recv_timeout(Duration::from_secs(5))?;
                    Ok(())
                };
                wait().map_err(|error| VoxelSourceError::Generation {
                    message: error.to_string(),
                })?;
            }
            Ok(())
        });
        let fixture = Fixture::with_hook(5, Some(hook))?;
        let superseded = fixture.selection(1, 0..selection_size)?;
        fixture.frontend.require_residency(superseded.clone())?;
        std::thread::scope(|scope| -> TestResult {
            let reader = scope.spawn(|| {
                fixture
                    .frontend
                    .materialize_residency(&superseded, &fixture.view)
            });
            started_receiver.recv_timeout(Duration::from_secs(5))?;
            fixture
                .frontend
                .require_residency(fixture.selection(2, 3..5)?)?;
            assert!(!fixture.frontend.establish_residency()?);
            release_sender.send(())?;
            assert!(matches!(
                reader.join().map_err(|_| "consumer panicked")?,
                Err(VoxelFrontendError::ResidencySuperseded)
            ));
            Ok(())
        })?;
        assert_eq!(fixture.frontend.materialization_cache_stats()?.copies, 0);
        assert_eq!(
            fixture
                .generations
                .get(2)
                .ok_or("missing superseded counter")?
                .load(Ordering::SeqCst),
            0
        );
        fixture.frontend.establish_residency()?;
        assert_eq!(
            fixture.frontend.installed_residency()?,
            Some(fixture.selection(2, 3..5)?)
        );
    }
    Ok(())
}

#[test]
fn failed_partial_consumer_handouts_do_not_transfer_the_query_slot() -> TestResult {
    use std::sync::mpsc;
    use std::time::Duration;

    let (started_sender, started_receiver) = mpsc::channel();
    let (release_sender, release_receiver) = mpsc::channel();
    let release_receiver = Mutex::new(release_receiver);
    let hook: GenerationHook = Arc::new(move |metadata| {
        if metadata.identity() == &Fixture::volume(1) {
            return Err(VoxelSourceError::Generation {
                message: "source unavailable".into(),
            });
        }
        if metadata.identity() == &Fixture::volume(2) {
            let wait = || -> TestResult {
                started_sender.send(())?;
                release_receiver
                    .lock()
                    .map_err(|_| "release channel poisoned")?
                    .recv_timeout(Duration::from_secs(5))?;
                Ok(())
            };
            wait().map_err(|error| VoxelSourceError::Generation {
                message: error.to_string(),
            })?;
        }
        Ok(())
    });
    let fixture = Fixture::with_hook(4, Some(hook))?;
    let query = fixture.view.enumerate_cells(&Fixture::volume(0), 1, 1)?;
    let candidate = fixture.selection(1, 0..2)?;
    assert!(matches!(
        fixture
            .frontend
            .materialize_residency(&candidate, &fixture.view),
        Err(VoxelFrontendError::VolumeGeneration { .. })
    ));
    assert_eq!(
        fixture
            .frontend
            .materialization_cache_stats()?
            .query_only_copies,
        1
    );
    fixture
        .frontend
        .require_residency(fixture.selection(2, [2])?)?;
    std::thread::scope(|scope| -> TestResult {
        let worker = scope.spawn(|| fixture.frontend.establish_residency());
        started_receiver.recv_timeout(Duration::from_secs(5))?;
        let handout = fixture
            .frontend
            .materialize_residency(&candidate, &fixture.view);
        let second_query = fixture.view.enumerate_cells(&Fixture::volume(3), 1, 1);
        release_sender.send(())?;
        assert!(worker.join().map_err(|_| "worker panicked")??);
        assert!(matches!(
            handout,
            Err(VoxelFrontendError::MaterializationInProgress)
        ));
        assert!(matches!(
            second_query,
            Err(VoxelFrontendError::QueryOnlyCopyBusy)
        ));
        Ok(())
    })?;
    assert_eq!(
        fixture
            .generations
            .get(3)
            .ok_or("missing query counter")?
            .load(Ordering::SeqCst),
        0
    );
    drop(query);
    Ok(())
}

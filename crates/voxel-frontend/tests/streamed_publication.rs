use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use voxel_frontend::*;

struct Recipe {
    scene: VoxelSceneId,
    metadata: VoxelVolumeMetadata,
    generations: Arc<AtomicUsize>,
    batches: Option<Vec<SparseVoxelBatch>>,
}

impl VoxelVolumeSource for Recipe {
    fn scene_id(&self) -> &VoxelSceneId {
        &self.scene
    }

    fn requires_materials(&self) -> bool {
        true
    }

    fn value(&self, _: VoxelCoordinate) -> Result<VoxelValue, VoxelSourceError> {
        if self.batches.is_some() {
            return Err(VoxelSourceError::Generation {
                message: "this publication-only recipe has no coordinate evaluator".into(),
            });
        }
        Ok(VoxelValue::Occupied(VoxelMaterialId::new("stone")))
    }

    fn materialize(&self) -> Result<SparseVoxelVolume, VoxelSourceError> {
        self.generations.fetch_add(1, Ordering::SeqCst);
        Ok(SparseVoxelVolume::new(
            self.metadata.clone(),
            SparseVoxelBackground::Empty,
            self.batches.clone().unwrap_or_else(|| {
                vec![SparseVoxelBatch::Fill(VoxelRegionFill::new(
                    VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), self.metadata.extent()),
                    VoxelValue::Occupied(VoxelMaterialId::new("stone")),
                ))]
            }),
        )
        .with_storage_tier(StorageTier::SparsePages))
    }
}

#[test]
fn publication_keeps_only_metadata_until_a_whole_scene_read()
-> Result<(), Box<dyn std::error::Error>> {
    let frontend = VoxelFrontend::new();
    let scene = VoxelSceneId::new("streamed");
    let revision = VoxelSceneRevision::new(7);
    let generations = Arc::new(AtomicUsize::new(0));
    let metadata: Vec<_> = (0..32)
        .map(|index| {
            VoxelVolumeMetadata::new(
                VoxelVolumeId::new(format!("volume-{index}")),
                VoxelExtent::new(3, 2, 1),
                [index as f32 * 3.0, 0.0, 0.0],
                1.0,
            )
        })
        .collect();
    let volumes = metadata
        .iter()
        .map(|metadata| {
            StreamedVoxelVolume::new(
                metadata.clone(),
                Arc::new(Recipe {
                    scene: scene.clone(),
                    metadata: metadata.clone(),
                    generations: generations.clone(),
                    batches: None,
                }),
            )
        })
        .collect();
    let view = frontend.publish_streamed(StreamedVoxelScene::new(
        scene.clone(),
        revision,
        vec![VoxelMaterial::new(VoxelMaterialId::new("stone"), [1.0; 4])],
        volumes,
    ))?;
    assert_eq!(generations.load(Ordering::SeqCst), 0);
    assert_eq!(view.scene_id(), &scene);
    assert_eq!(view.volumes(), metadata);
    for volume in view.volumes() {
        assert_eq!(view.volume_content_version(volume.identity())?, revision);
        assert_eq!(view.storage_bytes(volume.identity())?, 0);
        let samples = view.read_region(
            volume.identity(),
            VoxelRegion::new(VoxelCoordinate::new(-1, 0, 0), VoxelExtent::new(5, 1, 1)),
        )?;
        let values: Vec<_> = samples
            .iter()
            .map(|sample| sample.value().clone())
            .collect();
        assert_eq!(
            values,
            vec![
                VoxelValue::Empty,
                VoxelValue::Occupied(VoxelMaterialId::new("stone")),
                VoxelValue::Occupied(VoxelMaterialId::new("stone")),
                VoxelValue::Occupied(VoxelMaterialId::new("stone")),
                VoxelValue::Empty,
            ]
        );
    }
    assert_eq!(generations.load(Ordering::SeqCst), 32);
    Ok(())
}

fn palette() -> Vec<VoxelMaterial> {
    ["stone", "grass"]
        .map(|name| VoxelMaterial::new(VoxelMaterialId::new(name), [1.0; 4]))
        .into()
}

fn metadata(name: &str) -> VoxelVolumeMetadata {
    VoxelVolumeMetadata::new(
        VoxelVolumeId::new(name),
        VoxelExtent::new(19, 5, 3),
        [0.0; 3],
        1.0,
    )
}

fn recipe(metadata: VoxelVolumeMetadata, batches: Option<Vec<SparseVoxelBatch>>) -> Recipe {
    Recipe {
        scene: VoxelSceneId::new("streamed"),
        metadata,
        generations: Arc::new(AtomicUsize::new(0)),
        batches,
    }
}

fn streamed(volumes: Vec<StreamedVoxelVolume>) -> StreamedVoxelScene {
    StreamedVoxelScene::new(
        VoxelSceneId::new("streamed"),
        VoxelSceneRevision::new(7),
        palette(),
        volumes,
    )
}

fn cells(
    view: &VoxelSceneView,
    volume: &VoxelVolumeId,
    edge: u32,
) -> Result<Vec<VoxelCell>, VoxelFrontendError> {
    let mut cells = view
        .enumerate_cells(volume, edge, 2)?
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    cells.sort_by_key(|cell| cell.coordinate().components());
    Ok(cells)
}

#[test]
fn every_read_api_equals_fully_materialized_sparse_meaning()
-> Result<(), Box<dyn std::error::Error>> {
    let metadata = [metadata("first"), metadata("second")];
    let batches = vec![
        SparseVoxelBatch::Fill(VoxelRegionFill::new(
            VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), VoxelExtent::new(16, 2, 3)),
            VoxelValue::Occupied(VoxelMaterialId::new("stone")),
        )),
        SparseVoxelBatch::Detail(DenseVoxelBatch::new(
            VoxelRegion::new(VoxelCoordinate::new(16, 2, 1), VoxelExtent::new(3, 2, 1)),
            vec![
                VoxelValue::Empty,
                VoxelValue::Occupied(VoxelMaterialId::new("grass")),
                VoxelValue::Occupied(VoxelMaterialId::new("stone")),
                VoxelValue::Occupied(VoxelMaterialId::new("grass")),
                VoxelValue::Empty,
                VoxelValue::Occupied(VoxelMaterialId::new("grass")),
            ],
        )),
    ];
    let streamed = VoxelFrontend::new().publish_streamed(streamed(
        metadata
            .iter()
            .map(|metadata| {
                StreamedVoxelVolume::new(
                    metadata.clone(),
                    Arc::new(recipe(metadata.clone(), Some(batches.clone()))),
                )
            })
            .collect(),
    ))?;
    let materialized = VoxelFrontend::new().publish_sparse(
        SparseVoxelScene::new(
            VoxelSceneId::new("streamed"),
            VoxelSceneRevision::new(7),
            palette(),
            metadata
                .iter()
                .map(|metadata| {
                    SparseVoxelVolume::new(
                        metadata.clone(),
                        SparseVoxelBackground::Empty,
                        batches.clone(),
                    )
                })
                .collect(),
        )
        .with_storage_tier(StorageTier::SparsePages),
    )?;
    let regions = [
        VoxelRegion::new(VoxelCoordinate::new(-2, -1, -1), VoxelExtent::new(23, 7, 5)),
        VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), VoxelExtent::new(16, 2, 3)),
        VoxelRegion::new(VoxelCoordinate::new(16, 2, 1), VoxelExtent::new(3, 2, 1)),
        VoxelRegion::new(VoxelCoordinate::new(0, 4, 0), VoxelExtent::new(19, 1, 3)),
        VoxelRegion::new(VoxelCoordinate::new(19, 0, 0), VoxelExtent::new(2, 5, 3)),
    ];
    for _ in 0..2 {
        for volume in &metadata {
            for region in regions {
                assert_eq!(
                    streamed.read_region(volume.identity(), region)?,
                    materialized.read_region(volume.identity(), region)?
                );
                assert_eq!(
                    streamed.region_content(volume.identity(), region)?,
                    materialized.region_content(volume.identity(), region)?
                );
                let [width, height, depth] = region.extent().dimensions();
                let mut actual = vec![VoxelValue::Empty; (width * height * depth) as usize];
                let mut expected = actual.clone();
                streamed.read_region_into(volume.identity(), region, &mut actual)?;
                materialized.read_region_into(volume.identity(), region, &mut expected)?;
                assert_eq!(actual, expected);
            }
            for edge in [1, 2, 4, 16, 32] {
                assert_eq!(
                    cells(&streamed, volume.identity(), edge)?,
                    cells(&materialized, volume.identity(), edge)?
                );
            }
        }
    }
    Ok(())
}

#[test]
fn a_live_query_copy_is_shared_and_a_second_copy_is_rejected()
-> Result<(), Box<dyn std::error::Error>> {
    let frontend = VoxelFrontend::new();
    let first = Arc::new(recipe(metadata("first"), None));
    let second = Arc::new(recipe(metadata("second"), None));
    let view = frontend.publish_streamed(streamed(vec![
        StreamedVoxelVolume::new(first.metadata.clone(), first.clone()),
        StreamedVoxelVolume::new(second.metadata.clone(), second.clone()),
    ]))?;
    let first_identity = first.metadata.identity();
    let second_identity = second.metadata.identity();
    let mut enumeration = view.enumerate_cells(first_identity, 1, 2)?;
    let same_copy = frontend
        .scene_view()?
        .enumerate_cells(first_identity, 4, 2)?;
    assert_eq!(first.generations.load(Ordering::SeqCst), 1);
    assert!(matches!(
        view.enumerate_cells(second_identity, 1, 2),
        Err(VoxelFrontendError::QueryOnlyCopyBusy)
    ));
    assert_eq!(second.generations.load(Ordering::SeqCst), 0);
    let outside = VoxelRegion::new(VoxelCoordinate::new(-2, 0, 0), VoxelExtent::new(2, 1, 1));
    assert_eq!(
        view.region_content(second_identity, outside)?,
        VoxelRegionContent::Uniform(VoxelValue::Empty)
    );
    assert!(
        view.read_region(second_identity, outside)?
            .iter()
            .all(|sample| sample.value() == &VoxelValue::Empty)
    );
    let mut values = vec![VoxelValue::Occupied(VoxelMaterialId::new("stone")); 2];
    view.read_region_into(second_identity, outside, &mut values)?;
    assert_eq!(values, vec![VoxelValue::Empty; 2]);
    drop(same_copy);
    for batch in enumeration.by_ref() {
        batch?;
    }
    // Exhausting an enumeration releases its copy even if the iterator itself stays alive.
    let _second_copy = view.enumerate_cells(second_identity, 2, 2)?;
    assert_eq!(second.generations.load(Ordering::SeqCst), 1);
    assert_eq!(view.storage_bytes(first_identity)?, 0);
    Ok(())
}

struct ContentSource {
    scene: VoxelSceneId,
    requires_materials: bool,
    generate: Box<dyn Fn() -> Result<SparseVoxelVolume, VoxelSourceError> + Send + Sync>,
    evaluate: Box<dyn Fn(VoxelCoordinate) -> Result<VoxelValue, VoxelSourceError> + Send + Sync>,
}

impl VoxelVolumeSource for ContentSource {
    fn scene_id(&self) -> &VoxelSceneId {
        &self.scene
    }

    fn requires_materials(&self) -> bool {
        self.requires_materials
    }

    fn value(&self, coordinate: VoxelCoordinate) -> Result<VoxelValue, VoxelSourceError> {
        (self.evaluate)(coordinate)
    }

    fn materialize(&self) -> Result<SparseVoxelVolume, VoxelSourceError> {
        (self.generate)()
    }
}

fn source_volume(
    metadata: VoxelVolumeMetadata,
    generate: impl Fn() -> Result<SparseVoxelVolume, VoxelSourceError> + Send + Sync + 'static,
) -> StreamedVoxelVolume {
    source_volume_with_value(
        metadata,
        |_| {
            Err(VoxelSourceError::Generation {
                message: "coordinate evaluation failed".into(),
            })
        },
        generate,
    )
}

fn source_volume_with_value(
    metadata: VoxelVolumeMetadata,
    evaluate: impl Fn(VoxelCoordinate) -> Result<VoxelValue, VoxelSourceError> + Send + Sync + 'static,
    generate: impl Fn() -> Result<SparseVoxelVolume, VoxelSourceError> + Send + Sync + 'static,
) -> StreamedVoxelVolume {
    StreamedVoxelVolume::new(
        metadata,
        Arc::new(ContentSource {
            scene: VoxelSceneId::new("streamed"),
            requires_materials: false,
            generate: Box::new(generate),
            evaluate: Box::new(evaluate),
        }),
    )
}

#[test]
fn publication_rejects_each_invalid_input_without_generating()
-> Result<(), Box<dyn std::error::Error>> {
    let source = Arc::new(recipe(metadata("volume"), None));
    let volume = StreamedVoxelVolume::new(source.metadata.clone(), source.clone());
    assert!(matches!(
        VoxelFrontend::new().publish_streamed(streamed(vec![volume.clone(), volume.clone()])),
        Err(VoxelFrontendError::DuplicateVolumeIdentity { .. })
    ));
    assert!(matches!(
        VoxelFrontend::new()
            .publish_streamed(streamed(vec![volume.clone()]).with_storage_tier(StorageTier::Dense)),
        Err(VoxelFrontendError::StreamedStorageTier { .. })
    ));
    assert!(matches!(
        VoxelFrontend::new().publish_streamed(StreamedVoxelScene::new(
            VoxelSceneId::new("streamed"),
            VoxelSceneRevision::new(7),
            vec![],
            vec![volume.clone()]
        )),
        Err(VoxelFrontendError::EmptySourceMaterialPalette { .. })
    ));
    assert!(matches!(
        VoxelFrontend::new().publish_streamed(StreamedVoxelScene::new(
            VoxelSceneId::new("another-scene"),
            VoxelSceneRevision::new(7),
            palette(),
            vec![volume.clone()]
        )),
        Err(VoxelFrontendError::SourceSceneMismatch { .. })
    ));
    for (origin, voxel_size) in [
        ([f32::NAN, 0.0, 0.0], 1.0),
        ([0.0; 3], 0.0),
        ([0.0; 3], f32::INFINITY),
    ] {
        let invalid = VoxelVolumeMetadata::new(
            VoxelVolumeId::new("volume"),
            source.metadata.extent(),
            origin,
            voxel_size,
        );
        assert!(matches!(
            VoxelFrontend::new().publish_streamed(streamed(vec![StreamedVoxelVolume::new(
                invalid,
                source.clone()
            )])),
            Err(VoxelFrontendError::InvalidVolumeMetadata { .. })
        ));
    }
    assert!(matches!(
        VoxelFrontend::new().publish_streamed(streamed(vec![StreamedVoxelVolume::new(
            VoxelVolumeMetadata::new(
                VoxelVolumeId::new("volume"),
                VoxelExtent::new(0, 1, 1),
                [0.0; 3],
                1.0
            ),
            source.clone()
        )])),
        Err(VoxelFrontendError::EmptyVolumeExtent { .. })
    ));
    assert!(matches!(
        VoxelFrontend::new().publish_streamed(streamed(vec![StreamedVoxelVolume::new(
            VoxelVolumeMetadata::new(
                VoxelVolumeId::new("volume"),
                VoxelExtent::new(u32::MAX, 1, 1),
                [0.0; 3],
                1.0
            ),
            source.clone()
        )])),
        Err(VoxelFrontendError::VolumeTooLarge { .. })
    ));
    assert_eq!(source.generations.load(Ordering::SeqCst), 0);
    let frontend = VoxelFrontend::new();
    frontend.publish_streamed(streamed(vec![volume.clone()]))?;
    assert!(matches!(
        frontend.publish_streamed(streamed(vec![volume])),
        Err(VoxelFrontendError::SceneAlreadyPublished)
    ));
    assert_eq!(frontend.scene_view()?.volumes().len(), 1);
    Ok(())
}

#[test]
fn an_empty_source_needs_no_material_palette() -> Result<(), Box<dyn std::error::Error>> {
    let metadata = metadata("empty");
    let output_metadata = metadata.clone();
    let view = VoxelFrontend::new().publish_streamed(StreamedVoxelScene::new(
        VoxelSceneId::new("streamed"),
        VoxelSceneRevision::new(7),
        vec![],
        vec![source_volume(metadata.clone(), move || {
            Ok(SparseVoxelVolume::new(
                output_metadata.clone(),
                SparseVoxelBackground::Empty,
                vec![],
            )
            .with_storage_tier(StorageTier::SparsePages))
        })],
    ))?;
    assert_eq!(
        view.region_content(
            metadata.identity(),
            VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), metadata.extent())
        )?,
        VoxelRegionContent::Uniform(VoxelValue::Empty)
    );
    assert!(cells(&view, metadata.identity(), 4)?.is_empty());
    Ok(())
}

#[test]
fn invalid_source_output_never_becomes_scene_meaning() -> Result<(), Box<dyn std::error::Error>> {
    let metadata = metadata("volume");
    let region = VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), metadata.extent());
    for malformed in [
        "material", "identity", "extent", "origin", "size", "tier", "bounds", "count", "overlap",
    ] {
        let output_metadata = match malformed {
            "identity" => self::metadata("another-volume"),
            "extent" => VoxelVolumeMetadata::new(
                metadata.identity().clone(),
                VoxelExtent::new(18, 5, 3),
                [0.0; 3],
                1.0,
            ),
            "origin" => VoxelVolumeMetadata::new(
                metadata.identity().clone(),
                metadata.extent(),
                [1.0; 3],
                1.0,
            ),
            "size" => VoxelVolumeMetadata::new(
                metadata.identity().clone(),
                metadata.extent(),
                [0.0; 3],
                2.0,
            ),
            _ => metadata.clone(),
        };
        let batches = match malformed {
            "material" => vec![SparseVoxelBatch::Fill(VoxelRegionFill::new(
                region,
                VoxelValue::Occupied(VoxelMaterialId::new("undeclared")),
            ))],
            "bounds" => vec![SparseVoxelBatch::Fill(VoxelRegionFill::new(
                VoxelRegion::new(VoxelCoordinate::new(19, 0, 0), VoxelExtent::new(1, 1, 1)),
                VoxelValue::Empty,
            ))],
            "count" => vec![SparseVoxelBatch::Detail(DenseVoxelBatch::new(
                region,
                vec![],
            ))],
            "overlap" => {
                vec![SparseVoxelBatch::Fill(VoxelRegionFill::new(region, VoxelValue::Empty)); 2]
            }
            _ => vec![],
        };
        let attempts = Arc::new(AtomicUsize::new(0));
        let source_attempts = attempts.clone();
        let volume = source_volume(metadata.clone(), move || {
            source_attempts.fetch_add(1, Ordering::SeqCst);
            Ok(SparseVoxelVolume::new(
                output_metadata.clone(),
                SparseVoxelBackground::Empty,
                batches.clone(),
            )
            .with_storage_tier(if malformed == "tier" {
                StorageTier::Dense
            } else {
                StorageTier::SparsePages
            }))
        });
        let view = VoxelFrontend::new().publish_streamed(streamed(vec![volume]))?;
        for _ in 0..2 {
            let mut values = vec![VoxelValue::Occupied(VoxelMaterialId::new("grass")); 19 * 5 * 3];
            let unchanged = values.clone();
            let error = view.read_region_into(metadata.identity(), region, &mut values);
            assert_eq!(values, unchanged);
            let correct_error = match malformed {
                "material" => matches!(
                    error,
                    Err(VoxelFrontendError::UnknownMaterialReference { .. })
                ),
                "tier" => matches!(error, Err(VoxelFrontendError::StreamedStorageTier { .. })),
                "bounds" => matches!(error, Err(VoxelFrontendError::BatchOutsideVolume { .. })),
                "count" => matches!(error, Err(VoxelFrontendError::BatchValueCount { .. })),
                "overlap" => matches!(error, Err(VoxelFrontendError::OverlappingBatches { .. })),
                _ => matches!(
                    error,
                    Err(VoxelFrontendError::SourceMetadataMismatch { .. })
                ),
            };
            assert!(correct_error, "{malformed}");
            assert_eq!(view.storage_bytes(metadata.identity())?, 0);
        }
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }
    Ok(())
}

#[test]
fn generation_and_cache_exhaustion_errors_release_admission_for_retry()
-> Result<(), Box<dyn std::error::Error>> {
    for allocation_failure in [false, true] {
        let metadata = metadata("volume");
        let output_metadata = metadata.clone();
        let region = VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), metadata.extent());
        let attempts = Arc::new(AtomicUsize::new(0));
        let source_attempts = attempts.clone();
        let volume = source_volume(metadata.clone(), move || {
            if source_attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                return Err(if allocation_failure {
                    VoxelSourceError::Allocation
                } else {
                    VoxelSourceError::Generation {
                        message: "content source unavailable".into(),
                    }
                });
            }
            Ok(SparseVoxelVolume::new(
                output_metadata.clone(),
                SparseVoxelBackground::Empty,
                vec![],
            )
            .with_storage_tier(StorageTier::SparsePages))
        });
        let view = VoxelFrontend::new().publish_streamed(streamed(vec![volume]))?;
        let failure = view.region_content(metadata.identity(), region);
        if allocation_failure {
            assert!(matches!(
                failure,
                Err(VoxelFrontendError::MaterializationCacheExhausted)
            ));
        } else {
            assert!(matches!(
                failure,
                Err(VoxelFrontendError::VolumeGeneration {
                    source: VoxelSourceError::Generation { .. },
                    ..
                })
            ));
        }
        assert_eq!(view.storage_bytes(metadata.identity())?, 0);
        assert_eq!(
            view.region_content(metadata.identity(), region)?,
            VoxelRegionContent::Uniform(VoxelValue::Empty)
        );
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }
    Ok(())
}

#[test]
fn invalid_read_requests_do_not_materialize_content() -> Result<(), Box<dyn std::error::Error>> {
    let recipe = Arc::new(recipe(metadata("volume"), None));
    let frontend = VoxelFrontend::new();
    let view = frontend.publish_streamed(streamed(vec![StreamedVoxelVolume::new(
        recipe.metadata.clone(),
        recipe.clone(),
    )]))?;
    let identity = recipe.metadata.identity();
    let region = VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), recipe.metadata.extent());
    let mut values = vec![VoxelValue::Occupied(VoxelMaterialId::new("grass"))];
    let unchanged = values.clone();
    assert!(matches!(
        view.read_region_into(identity, region, &mut values),
        Err(VoxelFrontendError::RegionBufferSize { .. })
    ));
    assert_eq!(values, unchanged);
    assert!(matches!(
        view.read_region(
            identity,
            VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), VoxelExtent::new(0, 1, 1))
        ),
        Err(VoxelFrontendError::EmptyRegionRequest { .. })
    ));
    assert!(matches!(
        view.region_content(
            identity,
            VoxelRegion::new(
                VoxelCoordinate::new(i32::MAX, 0, 0),
                VoxelExtent::new(2, 1, 1)
            )
        ),
        Err(VoxelFrontendError::InvalidRegionBounds { .. })
    ));
    assert!(matches!(
        view.enumerate_cells(identity, 3, 1),
        Err(VoxelFrontendError::InvalidCellEdge { .. })
    ));
    assert!(matches!(
        view.enumerate_cells(identity, 1, 0),
        Err(VoxelFrontendError::ZeroCellBatchCapacity { .. })
    ));
    let unknown = VoxelVolumeId::new("unknown");
    assert!(matches!(
        view.read_region(&unknown, region),
        Err(VoxelFrontendError::UnknownVolumeIdentity { .. })
    ));
    assert!(matches!(
        view.volume_content_version(&unknown),
        Err(VoxelFrontendError::UnknownVolumeIdentity { .. })
    ));
    assert!(matches!(
        frontend.edit(VoxelEditCommand::new(
            identity.clone(),
            VoxelCoordinate::new(0, 0, 0),
            VoxelValue::Occupied(VoxelMaterialId::new("stone"))
        )),
        Ok(VoxelEditOutcome::Unchanged(_))
    ));
    assert_eq!(
        frontend.scene_view()?.revision(),
        VoxelSceneRevision::new(7)
    );
    assert_eq!(recipe.generations.load(Ordering::SeqCst), 0);
    Ok(())
}

#[test]
fn concurrent_generation_reserves_the_query_slot_before_calling_the_source()
-> Result<(), Box<dyn std::error::Error>> {
    use std::sync::{Mutex, mpsc};
    use std::time::Duration;

    let first_metadata = metadata("first");
    let output_metadata = first_metadata.clone();
    let second = Arc::new(recipe(metadata("second"), None));
    let (started_sender, started_receiver) = mpsc::channel();
    let (release_sender, release_receiver) = mpsc::channel();
    let release_receiver = Mutex::new(release_receiver);
    let first = source_volume(first_metadata.clone(), move || {
        started_sender
            .send(())
            .map_err(|error| VoxelSourceError::Generation {
                message: error.to_string(),
            })?;
        release_receiver
            .lock()
            .map_err(|error| VoxelSourceError::Generation {
                message: error.to_string(),
            })?
            .recv_timeout(Duration::from_secs(5))
            .map_err(|error| VoxelSourceError::Generation {
                message: error.to_string(),
            })?;
        Ok(SparseVoxelVolume::new(
            output_metadata.clone(),
            SparseVoxelBackground::Empty,
            vec![],
        )
        .with_storage_tier(StorageTier::SparsePages))
    });
    let view = VoxelFrontend::new().publish_streamed(streamed(vec![
        first,
        StreamedVoxelVolume::new(second.metadata.clone(), second.clone()),
    ]))?;
    let first_region = VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), first_metadata.extent());
    std::thread::scope(|scope| -> Result<(), Box<dyn std::error::Error>> {
        let worker = scope.spawn(|| view.region_content(first_metadata.identity(), first_region));
        started_receiver.recv_timeout(Duration::from_secs(5))?;
        let concurrent = view.enumerate_cells(second.metadata.identity(), 1, 1);
        let same_key = view.region_content(first_metadata.identity(), first_region);
        release_sender.send(())?;
        let completed = worker.join().map_err(|_| "query worker panicked")??;
        assert!(matches!(
            concurrent,
            Err(VoxelFrontendError::QueryOnlyCopyBusy)
        ));
        assert!(matches!(
            same_key,
            Err(VoxelFrontendError::QueryOnlyCopyBusy)
        ));
        assert_eq!(second.generations.load(Ordering::SeqCst), 0);
        assert_eq!(completed, VoxelRegionContent::Uniform(VoxelValue::Empty));
        Ok(())
    })?;
    assert!(!cells(&view, second.metadata.identity(), 4)?.is_empty());
    Ok(())
}

#[test]
fn generated_terrain_reproduces_the_frozen_cpu_prototype_fingerprint_after_eviction()
-> Result<(), Box<dyn std::error::Error>> {
    let generations = Arc::new(AtomicUsize::new(0));
    let metadata: Vec<_> = ["first", "second"]
        .into_iter()
        .map(|name| {
            VoxelVolumeMetadata::new(
                VoxelVolumeId::new(name),
                VoxelExtent::new(64, 64, 64),
                [0.0; 3],
                1.0,
            )
        })
        .collect();
    let volumes = metadata
        .iter()
        .map(|metadata| {
            let output_metadata = metadata.clone();
            let generations = generations.clone();
            source_volume_with_value(
                metadata.clone(),
                |coordinate| {
                    let [x, y, z] = coordinate.components();
                    let height = 24 + (x / 8 + z / 8) % 9;
                    Ok(
                        if y > height
                            || ((16..24).contains(&x)
                                && (8..16).contains(&y)
                                && (16..24).contains(&z))
                        {
                            VoxelValue::Empty
                        } else if y == height {
                            VoxelValue::Occupied(VoxelMaterialId::new("grass"))
                        } else {
                            VoxelValue::Occupied(VoxelMaterialId::new("stone"))
                        },
                    )
                },
                move || {
                    generations.fetch_add(1, Ordering::SeqCst);
                    let mut batches = Vec::new();
                    // The frozen prototype has 8-voxel terraces and one enclosed empty cavity.
                    for z in 0..64 {
                        for x in 0..64 {
                            let height = 24 + (x / 8 + z / 8) % 9;
                            let intervals = if (16..24).contains(&x) && (16..24).contains(&z) {
                                vec![(0, 8), (16, height)]
                            } else {
                                vec![(0, height)]
                            };
                            for (bottom, top) in intervals {
                                batches.push(SparseVoxelBatch::Fill(VoxelRegionFill::new(
                                    VoxelRegion::new(
                                        VoxelCoordinate::new(x, bottom, z),
                                        VoxelExtent::new(1, (top - bottom) as u32, 1),
                                    ),
                                    VoxelValue::Occupied(VoxelMaterialId::new("stone")),
                                )));
                            }
                            batches.push(SparseVoxelBatch::Fill(VoxelRegionFill::new(
                                VoxelRegion::new(
                                    VoxelCoordinate::new(x, height, z),
                                    VoxelExtent::new(1, 1, 1),
                                ),
                                VoxelValue::Occupied(VoxelMaterialId::new("grass")),
                            )));
                        }
                    }
                    Ok(SparseVoxelVolume::new(
                        output_metadata.clone(),
                        SparseVoxelBackground::Empty,
                        batches,
                    )
                    .with_storage_tier(StorageTier::SparsePages))
                },
            )
        })
        .collect();
    let frontend = VoxelFrontend::new();
    let view = frontend.publish_streamed(streamed(volumes))?;
    let mut values = vec![VoxelValue::Empty; 64 * 64 * 64];
    for identity in [
        VoxelVolumeId::new("first"),
        VoxelVolumeId::new("second"),
        VoxelVolumeId::new("first"),
    ] {
        view.read_region_into(
            &identity,
            VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), VoxelExtent::new(64, 64, 64)),
            &mut values,
        )?;
        assert_eq!(fixture_fingerprint(&values)?, 0x9660_d930_8d69_ede5);
        assert_eq!(
            view.volume_content_version(&identity)?,
            VoxelSceneRevision::new(7)
        );
    }
    assert_eq!(generations.load(Ordering::SeqCst), 3);
    let volume = VoxelVolumeId::new("second");
    let region = VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), VoxelExtent::new(64, 64, 64));
    let edit_coordinates = [[4, 40, 4], [32, 20, 32], [20, 12, 20]];
    let command = |values: [VoxelValue; 3]| {
        VoxelEditCommand::from_edits(
            edit_coordinates
                .into_iter()
                .zip(values)
                .map(|([x, y, z], value)| {
                    VoxelEdit::new(volume.clone(), VoxelCoordinate::new(x, y, z), value)
                })
                .collect(),
        )
    };
    let edited = frontend
        .edit(command([
            VoxelValue::Occupied(VoxelMaterialId::new("stone")),
            VoxelValue::Empty,
            VoxelValue::Occupied(VoxelMaterialId::new("grass")),
        ]))?
        .view()
        .clone();
    assert_eq!(generations.load(Ordering::SeqCst), 3);
    let restored = frontend
        .edit(command([
            VoxelValue::Empty,
            VoxelValue::Occupied(VoxelMaterialId::new("stone")),
            VoxelValue::Empty,
        ]))?
        .view()
        .clone();
    assert_eq!(generations.load(Ordering::SeqCst), 3);
    for (historical, expected) in [
        (&edited, 0x478e_e4a9_737e_1ad7),
        (&view, 0x9660_d930_8d69_ede5),
        (&restored, 0x9660_d930_8d69_ede5),
        (&edited, 0x478e_e4a9_737e_1ad7),
    ] {
        historical.read_region_into(&volume, region, &mut values)?;
        assert_eq!(fixture_fingerprint(&values)?, expected);
    }
    assert_eq!(
        restored.volume_content_version(&volume)?,
        VoxelSceneRevision::new(9)
    );
    Ok(())
}

fn fixture_fingerprint(values: &[VoxelValue]) -> Result<u64, &'static str> {
    values
        .iter()
        .try_fold(0xcbf2_9ce4_8422_2325_u64, |hash, value| {
            let code = match value {
                VoxelValue::Empty => 0,
                VoxelValue::Occupied(material) if material == &VoxelMaterialId::new("stone") => 1,
                VoxelValue::Occupied(material) if material == &VoxelMaterialId::new("grass") => 2,
                _ => return Err("unexpected material in frozen fixture"),
            };
            Ok((hash ^ code).wrapping_mul(0x100_0000_01b3))
        })
}

#[test]
fn source_coordinate_failures_reject_the_entire_command_without_generating()
-> Result<(), Box<dyn std::error::Error>> {
    for invalid_material in [false, true] {
        let first_metadata = metadata("first");
        let second_metadata = metadata("second");
        let generations = Arc::new(AtomicUsize::new(0));
        let generation_counter = generations.clone();
        let output_metadata = second_metadata.clone();
        let frontend = VoxelFrontend::new();
        let original = frontend.publish_streamed(streamed(vec![
            StreamedVoxelVolume::new(
                first_metadata.clone(),
                Arc::new(recipe(first_metadata, None)),
            ),
            source_volume_with_value(
                second_metadata,
                move |_| {
                    if invalid_material {
                        Ok(VoxelValue::Occupied(VoxelMaterialId::new("undeclared")))
                    } else {
                        Err(VoxelSourceError::Generation {
                            message: "coordinate evaluation failed".into(),
                        })
                    }
                },
                move || {
                    generation_counter.fetch_add(1, Ordering::SeqCst);
                    Ok(SparseVoxelVolume::new(
                        output_metadata.clone(),
                        SparseVoxelBackground::Empty,
                        vec![],
                    )
                    .with_storage_tier(StorageTier::SparsePages))
                },
            ),
        ]))?;
        let before = frontend.streamed_edit_statistics()?;
        let failure = frontend.edit(VoxelEditCommand::from_edits(
            ["first", "second"]
                .map(|name| {
                    VoxelEdit::new(
                        VoxelVolumeId::new(name),
                        VoxelCoordinate::new(0, 0, 0),
                        VoxelValue::Empty,
                    )
                })
                .into(),
        ));
        if invalid_material {
            assert!(matches!(
                failure,
                Err(VoxelFrontendError::UnknownMaterialReference { .. })
            ));
        } else {
            assert!(matches!(
                failure,
                Err(VoxelFrontendError::VolumeGeneration { .. })
            ));
        }
        assert_eq!(frontend.scene_view()?.revision(), original.revision());
        assert_eq!(frontend.streamed_edit_statistics()?, before);
        assert_eq!(generations.load(Ordering::SeqCst), 0);
    }
    Ok(())
}

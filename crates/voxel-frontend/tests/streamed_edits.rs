use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use voxel_frontend::*;

struct Recipe {
    scene: VoxelSceneId,
    metadata: VoxelVolumeMetadata,
    generations: AtomicUsize,
    evaluations: AtomicUsize,
}

impl VoxelVolumeSource for Recipe {
    fn scene_id(&self) -> &VoxelSceneId {
        &self.scene
    }

    fn requires_materials(&self) -> bool {
        true
    }

    fn value(&self, _: VoxelCoordinate) -> Result<VoxelValue, VoxelSourceError> {
        self.evaluations.fetch_add(1, Ordering::SeqCst);
        Ok(stone())
    }

    fn materialize(&self) -> Result<SparseVoxelVolume, VoxelSourceError> {
        self.generations.fetch_add(1, Ordering::SeqCst);
        Ok(SparseVoxelVolume::new(
            self.metadata.clone(),
            SparseVoxelBackground::Empty,
            vec![SparseVoxelBatch::Fill(VoxelRegionFill::new(
                VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), self.metadata.extent()),
                stone(),
            ))],
        )
        .with_storage_tier(StorageTier::SparsePages))
    }
}

fn stone() -> VoxelValue {
    VoxelValue::Occupied(VoxelMaterialId::new("stone"))
}

fn frontend(revision: u64) -> Result<(VoxelFrontend, Vec<Arc<Recipe>>), VoxelFrontendError> {
    frontend_with_extent(revision, VoxelExtent::new(64, 64, 64))
}

fn frontend_with_extent(
    revision: u64,
    extent: VoxelExtent,
) -> Result<(VoxelFrontend, Vec<Arc<Recipe>>), VoxelFrontendError> {
    let frontend = streamed_frontend();
    let sources: Vec<_> = ["first", "second", "third"]
        .map(|name| {
            Arc::new(Recipe {
                scene: VoxelSceneId::new("streamed-edits"),
                metadata: VoxelVolumeMetadata::new(VoxelVolumeId::new(name), extent, [0.0; 3], 1.0),
                generations: AtomicUsize::new(0),
                evaluations: AtomicUsize::new(0),
            })
        })
        .into();
    frontend.publish_streamed(StreamedVoxelScene::new(
        VoxelSceneId::new("streamed-edits"),
        VoxelSceneRevision::new(revision),
        vec![VoxelMaterial::new(VoxelMaterialId::new("stone"), [1.0; 4])],
        sources
            .iter()
            .map(|source| StreamedVoxelVolume::new(source.metadata.clone(), source.clone()))
            .collect(),
    ))?;
    Ok((frontend, sources))
}

fn edit(volume: &str, x: i32, value: VoxelValue) -> VoxelEdit {
    VoxelEdit::new(
        VoxelVolumeId::new(volume),
        VoxelCoordinate::new(x, 0, 0),
        value,
    )
}

#[test]
fn non_resident_command_publishes_once_without_generating() -> Result<(), Box<dyn std::error::Error>>
{
    let (frontend, sources) = frontend(7)?;
    let outcome = frontend.edit(VoxelEditCommand::from_edits(vec![
        edit("first", 1, VoxelValue::Empty),
        edit("first", 1, stone()),
        edit("first", 2, VoxelValue::Empty),
        edit("second", 3, VoxelValue::Empty),
    ]))?;
    let changes = outcome.change_set().ok_or("missing changes")?;
    assert_eq!(changes.predecessor_revision(), VoxelSceneRevision::new(7));
    assert_eq!(changes.successor_revision(), VoxelSceneRevision::new(8));
    assert_eq!(changes.changed_regions().len(), 2);
    assert_eq!(
        frontend.scene_view()?.revision(),
        VoxelSceneRevision::new(8)
    );
    for source in &sources {
        assert_eq!(source.generations.load(Ordering::SeqCst), 0);
        assert_eq!(outcome.view().storage_bytes(source.metadata.identity())?, 0);
    }
    assert_eq!(
        sources
            .first()
            .ok_or("missing source")?
            .evaluations
            .load(Ordering::SeqCst),
        2
    );
    assert_eq!(
        sources
            .get(1)
            .ok_or("missing source")?
            .evaluations
            .load(Ordering::SeqCst),
        1
    );
    assert_eq!(
        sources
            .get(2)
            .ok_or("missing source")?
            .evaluations
            .load(Ordering::SeqCst),
        0
    );
    Ok(())
}

#[test]
fn non_resident_coordinates_need_no_dense_linear_address() -> Result<(), Box<dyn std::error::Error>>
{
    let edge = 1 << 24;
    let (frontend, sources) = frontend_with_extent(7, VoxelExtent::new(edge, edge, edge))?;
    let coordinate = VoxelCoordinate::new(edge as i32 - 1, edge as i32 - 1, edge as i32 - 1);
    let outcome = frontend.edit(VoxelEditCommand::new(
        VoxelVolumeId::new("first"),
        coordinate,
        VoxelValue::Empty,
    ))?;
    assert_eq!(outcome.view().revision(), VoxelSceneRevision::new(8));
    assert_eq!(
        outcome
            .change_set()
            .ok_or("missing changes")?
            .changed_regions()
            .first()
            .ok_or("missing region")?
            .region(),
        VoxelRegion::new(coordinate, VoxelExtent::new(1, 1, 1))
    );
    for source in sources {
        assert_eq!(source.generations.load(Ordering::SeqCst), 0);
    }
    Ok(())
}

fn sample(view: &VoxelSceneView, volume: &str, x: i32) -> Result<VoxelValue, VoxelFrontendError> {
    let samples = view.read_region(
        &VoxelVolumeId::new(volume),
        VoxelRegion::new(VoxelCoordinate::new(x, 0, 0), VoxelExtent::new(1, 1, 1)),
    )?;
    Ok(samples
        .first()
        .expect("one-coordinate reads return one sample")
        .value()
        .clone())
}

#[test]
fn historical_views_rebuild_after_eviction_and_release_unreachable_edits()
-> Result<(), Box<dyn std::error::Error>> {
    let (frontend, sources) = frontend(7)?;
    let generated = frontend.scene_view()?;
    let edited = frontend
        .edit(VoxelEditCommand::from_edits(vec![edit(
            "first",
            2,
            VoxelValue::Empty,
        )]))?
        .view()
        .clone();
    assert_eq!(sample(&edited, "first", 2)?, VoxelValue::Empty);
    sample(&edited, "second", 0)?;
    assert_eq!(edited.storage_bytes(&VoxelVolumeId::new("first"))?, 0);

    let restored = frontend
        .edit(VoxelEditCommand::from_edits(vec![edit(
            "first",
            2,
            stone(),
        )]))?
        .view()
        .clone();
    assert_eq!(
        restored.volume_content_version(&VoxelVolumeId::new("first"))?,
        VoxelSceneRevision::new(9)
    );
    assert_eq!(sample(&restored, "first", 2)?, stone());
    assert_eq!(sample(&edited, "first", 2)?, VoxelValue::Empty);
    assert_eq!(sample(&generated, "first", 2)?, stone());
    assert_eq!(sample(&restored, "first", 2)?, stone());
    let retained = frontend
        .streamed_edit_statistics()?
        .ok_or("missing streamed statistics")?;
    assert_eq!(retained.current_coordinates, 0);
    assert_eq!(retained.retained_entries, 2);
    assert_eq!(retained.live_versions, 3);

    drop(edited);
    drop(generated);
    let compacted = frontend
        .streamed_edit_statistics()?
        .ok_or("missing streamed statistics")?;
    assert_eq!(compacted.retained_entries, 0);
    assert_eq!(compacted.live_versions, 1);
    assert!(compacted.storage_bytes < retained.storage_bytes);
    assert!(
        sources
            .first()
            .ok_or("missing source")?
            .generations
            .load(Ordering::SeqCst)
            >= 5
    );
    Ok(())
}

#[test]
fn no_op_and_invalid_commands_leave_revision_versions_and_history_unchanged()
-> Result<(), Box<dyn std::error::Error>> {
    let (frontend, sources) = frontend(7)?;
    let before = frontend.streamed_edit_statistics()?;
    for command in [
        VoxelEditCommand::from_edits(vec![]),
        VoxelEditCommand::from_edits(vec![edit("first", 0, stone())]),
        VoxelEditCommand::from_edits(vec![
            edit("first", 0, VoxelValue::Empty),
            edit("first", 0, stone()),
        ]),
    ] {
        let outcome = frontend.edit(command)?;
        assert!(outcome.change_set().is_none());
        assert_eq!(outcome.view().revision(), VoxelSceneRevision::new(7));
    }
    for invalid in [
        edit("unknown", 0, VoxelValue::Empty),
        edit("first", -1, VoxelValue::Empty),
        edit("first", 64, VoxelValue::Empty),
        edit(
            "first",
            0,
            VoxelValue::Occupied(VoxelMaterialId::new("unknown")),
        ),
    ] {
        let command = VoxelEditCommand::from_edits(vec![
            edit("second", 0, VoxelValue::Empty),
            invalid,
            edit("first", 0, stone()),
        ]);
        assert!(frontend.edit(command).is_err());
        assert_eq!(
            frontend.scene_view()?.revision(),
            VoxelSceneRevision::new(7)
        );
        assert_eq!(frontend.streamed_edit_statistics()?, before);
    }
    let outcome = frontend.edit(VoxelEditCommand::from_edits(vec![edit(
        "first",
        0,
        VoxelValue::Empty,
    )]))?;
    let after = frontend.streamed_edit_statistics()?;
    assert!(
        frontend
            .edit(VoxelEditCommand::from_edits(vec![edit(
                "first",
                0,
                VoxelValue::Empty
            )]))?
            .change_set()
            .is_none()
    );
    assert_eq!(frontend.streamed_edit_statistics()?, after);
    assert_eq!(sample(outcome.view(), "first", 0)?, VoxelValue::Empty);
    assert_eq!(
        sources
            .get(1)
            .ok_or("missing source")?
            .generations
            .load(Ordering::SeqCst),
        0
    );
    Ok(())
}

#[test]
fn editing_with_a_held_query_copy_preserves_unrelated_cache_keys()
-> Result<(), Box<dyn std::error::Error>> {
    let (frontend, sources) = frontend(7)?;
    let original = frontend.scene_view()?;
    let first = VoxelVolumeId::new("first");
    let second = VoxelVolumeId::new("second");
    let mut held = original.enumerate_cells(&second, 64, 1)?;
    let edited = frontend.edit(VoxelEditCommand::from_edits(vec![edit(
        "first",
        0,
        VoxelValue::Empty,
    )]))?;
    assert_eq!(
        edited.view().volume_content_version(&first)?,
        VoxelSceneRevision::new(8)
    );
    assert_eq!(
        edited.view().volume_content_version(&second)?,
        VoxelSceneRevision::new(7)
    );
    assert_eq!(sample(edited.view(), "second", 0)?, stone());
    assert_eq!(
        sources
            .get(1)
            .ok_or("missing source")?
            .generations
            .load(Ordering::SeqCst),
        1
    );
    assert_eq!(
        sources
            .first()
            .ok_or("missing source")?
            .generations
            .load(Ordering::SeqCst),
        0
    );
    assert!(held.next().transpose()?.is_some());
    drop(held);
    assert_eq!(sample(edited.view(), "first", 0)?, VoxelValue::Empty);
    assert_eq!(sample(&original, "first", 0)?, stone());
    Ok(())
}

#[test]
fn dropped_intermediate_views_do_not_accumulate_history_or_allocation()
-> Result<(), Box<dyn std::error::Error>> {
    let (frontend, _) = frontend(7)?;
    let generated = frontend.scene_view()?;
    let first_edit = frontend
        .edit(VoxelEditCommand::from_edits(vec![edit(
            "first",
            0,
            VoxelValue::Empty,
        )]))?
        .view()
        .clone();
    let mut edited_plateau = None;
    let mut restored_plateau = None;
    for _ in 0..64 {
        drop(frontend.edit(VoxelEditCommand::from_edits(vec![edit(
            "first",
            0,
            stone(),
        )]))?);
        let restored = frontend
            .streamed_edit_statistics()?
            .ok_or("missing statistics")?;
        assert_eq!(restored.current_coordinates, 0);
        assert_eq!(restored.retained_entries, 2);
        assert_eq!(restored.live_versions, 3);
        assert_eq!(*restored_plateau.get_or_insert(restored), restored);
        drop(frontend.edit(VoxelEditCommand::from_edits(vec![edit(
            "first",
            0,
            VoxelValue::Empty,
        )]))?);
        let edited = frontend
            .streamed_edit_statistics()?
            .ok_or("missing statistics")?;
        assert_eq!(edited.current_coordinates, 1);
        assert_eq!(edited.retained_entries, 2);
        assert_eq!(*edited_plateau.get_or_insert(edited), edited);
    }
    assert_eq!(sample(&generated, "first", 0)?, stone());
    assert_eq!(sample(&first_edit, "first", 0)?, VoxelValue::Empty);
    drop(generated);
    drop(first_edit);
    let compacted = frontend
        .streamed_edit_statistics()?
        .ok_or("missing statistics")?;
    assert_eq!(compacted.live_versions, 1);
    assert_eq!(compacted.retained_entries, 1);
    drop(frontend.edit(VoxelEditCommand::from_edits(vec![edit(
        "first",
        0,
        stone(),
    )]))?);
    let empty = frontend
        .streamed_edit_statistics()?
        .ok_or("missing statistics")?;
    assert_eq!(empty.retained_entries, 0);
    for x in 0..64 {
        drop(frontend.edit(VoxelEditCommand::from_edits(vec![edit(
            "first",
            x,
            VoxelValue::Empty,
        )]))?);
        drop(frontend.edit(VoxelEditCommand::from_edits(vec![edit(
            "first",
            x,
            stone(),
        )]))?);
        assert_eq!(frontend.streamed_edit_statistics()?, Some(empty));
    }
    Ok(())
}

#[test]
fn revision_exhaustion_rejects_changes_but_allows_non_resident_no_ops()
-> Result<(), Box<dyn std::error::Error>> {
    let (frontend, _) = frontend(u64::MAX)?;
    let before = frontend.streamed_edit_statistics()?;
    assert!(matches!(
        frontend.edit(VoxelEditCommand::from_edits(vec![edit(
            "first",
            0,
            VoxelValue::Empty
        )])),
        Err(VoxelFrontendError::RevisionOverflow { .. })
    ));
    assert_eq!(frontend.streamed_edit_statistics()?, before);
    assert!(
        frontend
            .edit(VoxelEditCommand::from_edits(vec![edit(
                "first",
                0,
                stone()
            )]))?
            .change_set()
            .is_none()
    );
    assert_eq!(sample(&frontend.scene_view()?, "first", 0)?, stone());
    Ok(())
}

#[test]
fn a_restored_historical_view_needs_no_leading_restore_marker()
-> Result<(), Box<dyn std::error::Error>> {
    let (frontend, _) = frontend(7)?;
    let edited = frontend
        .edit(VoxelEditCommand::from_edits(vec![edit(
            "first",
            0,
            VoxelValue::Empty,
        )]))?
        .view()
        .clone();
    let restored = frontend
        .edit(VoxelEditCommand::from_edits(vec![edit(
            "first",
            0,
            stone(),
        )]))?
        .view()
        .clone();
    let reedited = frontend
        .edit(VoxelEditCommand::from_edits(vec![edit(
            "first",
            0,
            VoxelValue::Empty,
        )]))?
        .view()
        .clone();
    drop(edited);
    let statistics = frontend
        .streamed_edit_statistics()?
        .ok_or("missing statistics")?;
    assert_eq!(statistics.retained_entries, 1);
    assert_eq!(sample(&restored, "first", 0)?, stone());
    assert_eq!(sample(&reedited, "first", 0)?, VoxelValue::Empty);
    Ok(())
}

#[test]
fn historical_reconstruction_information_is_accounted_and_reclaimed()
-> Result<(), Box<dyn std::error::Error>> {
    let (frontend, _) = frontend(7)?;
    let baseline = frontend
        .streamed_edit_statistics()?
        .ok_or("missing statistics")?;
    let mut historical = Vec::new();
    for _ in 0..16 {
        historical.push(frontend.scene_view()?);
        drop(frontend.edit(VoxelEditCommand::from_edits(vec![edit(
            "first",
            0,
            VoxelValue::Empty,
        )]))?);
        drop(frontend.edit(VoxelEditCommand::from_edits(vec![edit(
            "first",
            0,
            stone(),
        )]))?);
        let retained = frontend
            .streamed_edit_statistics()?
            .ok_or("missing statistics")?;
        assert_eq!(retained.retained_entries, 0);
        assert_eq!(retained.live_versions, historical.len() + 1);
        let minimum_reconstruction_bytes =
            historical.len() * (size_of::<VoxelSceneRevision>() + size_of::<usize>());
        assert!(retained.storage_bytes >= baseline.storage_bytes + minimum_reconstruction_bytes);
    }
    drop(historical);
    assert_eq!(frontend.streamed_edit_statistics()?, Some(baseline));
    Ok(())
}

#[test]
fn edited_and_historical_read_apis_equal_an_unstreamed_scene()
-> Result<(), Box<dyn std::error::Error>> {
    let (streamed, sources) = frontend(7)?;
    let materialized = VoxelFrontend::new();
    let original = materialized.publish_sparse(SparseVoxelScene::new(
        VoxelSceneId::new("streamed-edits"),
        VoxelSceneRevision::new(7),
        vec![VoxelMaterial::new(VoxelMaterialId::new("stone"), [1.0; 4])],
        sources
            .iter()
            .map(|source| source.materialize())
            .collect::<Result<Vec<_>, _>>()?,
    ))?;
    let original_streamed = streamed.scene_view()?;
    let command = VoxelEditCommand::from_edits(vec![
        edit("first", 0, VoxelValue::Empty),
        edit("first", 17, VoxelValue::Empty),
        edit("second", 63, VoxelValue::Empty),
    ]);
    let edited = materialized.edit(command.clone())?.view().clone();
    let edited_streamed = streamed.edit(command)?.view().clone();
    let command = VoxelEditCommand::from_edits(vec![
        edit("first", 0, stone()),
        edit("first", 16, VoxelValue::Empty),
    ]);
    let latest = materialized.edit(command.clone())?.view().clone();
    let latest_streamed = streamed.edit(command)?.view().clone();
    for (expected, actual) in [
        (&latest, &latest_streamed),
        (&original, &original_streamed),
        (&edited, &edited_streamed),
    ] {
        for name in ["first", "second", "third"] {
            let identity = VoxelVolumeId::new(name);
            for region in [
                VoxelRegion::new(VoxelCoordinate::new(-1, 0, 0), VoxelExtent::new(66, 1, 1)),
                VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), VoxelExtent::new(1, 1, 1)),
                VoxelRegion::new(VoxelCoordinate::new(16, 0, 0), VoxelExtent::new(2, 1, 1)),
            ] {
                assert_eq!(
                    actual.read_region(&identity, region)?,
                    expected.read_region(&identity, region)?
                );
                assert_eq!(
                    actual.region_content(&identity, region)?,
                    expected.region_content(&identity, region)?
                );
                let [width, height, depth] = region.extent().dimensions();
                let mut actual_values = vec![VoxelValue::Empty; (width * height * depth) as usize];
                let mut expected_values = actual_values.clone();
                actual.read_region_into(&identity, region, &mut actual_values)?;
                expected.read_region_into(&identity, region, &mut expected_values)?;
                assert_eq!(actual_values, expected_values);
            }
            let enumerate = |view: &VoxelSceneView| -> Result<Vec<VoxelCell>, VoxelFrontendError> {
                let mut cells: Vec<_> = view
                    .enumerate_cells(&identity, 16, 2)?
                    .collect::<Result<Vec<_>, _>>()?
                    .into_iter()
                    .flatten()
                    .collect();
                cells.sort_by_key(|cell| cell.coordinate().components());
                Ok(cells)
            };
            assert_eq!(enumerate(actual)?, enumerate(expected)?);
        }
    }
    Ok(())
}

fn streamed_frontend() -> VoxelFrontend {
    VoxelFrontend::with_residency_limits(
        VoxelResidencyLimits::new(9).expect("nine is a valid maximum selection size"),
    )
}

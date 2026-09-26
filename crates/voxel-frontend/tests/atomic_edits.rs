use voxel_frontend::*;

fn frontend(revision: u64) -> Result<VoxelFrontend, VoxelFrontendError> {
    let frontend = VoxelFrontend::new();
    let extent = VoxelExtent::new(513, 1, 1);
    frontend.publish(DenseVoxelScene::new(
        VoxelSceneId::new("atomic"),
        VoxelSceneRevision::new(revision),
        vec![VoxelMaterial::new(VoxelMaterialId::new("stone"), [1.0; 4])],
        [
            ("dense", StorageTier::Dense),
            ("sparse", StorageTier::SparsePages),
        ]
        .into_iter()
        .map(|(identity, tier)| {
            DenseVoxelVolume::new(
                VoxelVolumeMetadata::new(VoxelVolumeId::new(identity), extent, [0.0; 3], 1.0),
                vec![DenseVoxelBatch::new(
                    VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), extent),
                    vec![VoxelValue::Empty; 513],
                )],
            )
            .with_storage_tier(tier)
        })
        .collect(),
    ))?;
    Ok(frontend)
}

fn edit(volume: &str, coordinate: i32, occupied: bool) -> VoxelEdit {
    VoxelEdit::new(
        VoxelVolumeId::new(volume),
        VoxelCoordinate::new(coordinate, 0, 0),
        if occupied {
            VoxelValue::Occupied(VoxelMaterialId::new("stone"))
        } else {
            VoxelValue::Empty
        },
    )
}

#[test]
fn multi_volume_command_publishes_one_complete_successor_and_retains_old_views()
-> Result<(), Box<dyn std::error::Error>> {
    let frontend = frontend(7)?;
    let before = frontend.scene_view()?;
    let edits: Vec<_> = ["dense", "sparse"]
        .into_iter()
        .flat_map(|volume| [0, 255, 256, 512].map(|coordinate| edit(volume, coordinate, true)))
        .collect();
    let outcome = frontend.edit(VoxelEditCommand::from_edits(edits.clone()))?;
    let changes = outcome.change_set().ok_or("missing changes")?;
    assert_eq!(changes.predecessor_revision(), VoxelSceneRevision::new(7));
    assert_eq!(changes.successor_revision(), VoxelSceneRevision::new(8));
    assert_eq!(changes.changed_regions().len(), edits.len());
    for edit in edits {
        let region = VoxelRegion::new(edit.coordinate(), VoxelExtent::new(1, 1, 1));
        assert!(
            changes
                .changed_regions()
                .iter()
                .any(|change| change.volume_identity() == edit.volume_identity()
                    && change.region() == region)
        );
        assert_eq!(
            before
                .read_region(edit.volume_identity(), region)?
                .first()
                .map(VoxelSample::value),
            Some(&VoxelValue::Empty)
        );
        assert_eq!(
            outcome
                .view()
                .read_region(edit.volume_identity(), region)?
                .first()
                .map(VoxelSample::value),
            Some(edit.value())
        );
    }
    Ok(())
}

#[test]
fn invalid_entries_reject_every_volume_even_when_overwritten()
-> Result<(), Box<dyn std::error::Error>> {
    for invalid in [
        edit("missing", 0, true),
        edit("sparse", 513, true),
        VoxelEdit::new(
            VoxelVolumeId::new("sparse"),
            VoxelCoordinate::new(0, 0, 0),
            VoxelValue::Occupied(VoxelMaterialId::new("missing")),
        ),
    ] {
        let frontend = frontend(7)?;
        let result = frontend.edit(VoxelEditCommand::from_edits(vec![
            edit("dense", 0, true),
            invalid,
            edit("sparse", 0, false),
        ]));
        assert!(result.is_err());
        let view = frontend.scene_view()?;
        assert_eq!(view.revision(), VoxelSceneRevision::new(7));
        for volume in ["dense", "sparse"] {
            assert_eq!(
                view.region_content(
                    &VoxelVolumeId::new(volume),
                    VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), VoxelExtent::new(513, 1, 1))
                )?,
                VoxelRegionContent::Uniform(VoxelValue::Empty)
            );
        }
    }
    Ok(())
}

#[test]
fn empty_and_net_unchanged_commands_do_not_overflow_but_changed_commands_do()
-> Result<(), Box<dyn std::error::Error>> {
    let frontend = frontend(u64::MAX)?;
    for edits in [
        vec![],
        vec![edit("dense", 0, false)],
        vec![edit("dense", 0, true), edit("dense", 0, false)],
    ] {
        let outcome = frontend.edit(VoxelEditCommand::from_edits(edits))?;
        assert!(outcome.change_set().is_none());
        assert_eq!(outcome.view().revision(), VoxelSceneRevision::new(u64::MAX));
    }
    assert!(matches!(
        frontend.edit(VoxelEditCommand::from_edits(vec![
            edit("dense", 0, true),
            edit("sparse", 512, true)
        ])),
        Err(VoxelFrontendError::RevisionOverflow { .. })
    ));
    assert_eq!(
        frontend.scene_view()?.region_content(
            &VoxelVolumeId::new("dense"),
            VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), VoxelExtent::new(513, 1, 1))
        )?,
        VoxelRegionContent::Uniform(VoxelValue::Empty)
    );
    Ok(())
}

#[test]
fn repeated_coordinates_use_last_value_and_report_only_final_changes()
-> Result<(), Box<dyn std::error::Error>> {
    let frontend = frontend(0)?;
    let outcome = frontend.edit(VoxelEditCommand::from_edits(vec![
        edit("dense", 0, true),
        edit("dense", 0, false),
        edit("sparse", 512, false),
        edit("sparse", 512, true),
    ]))?;
    let changes = outcome.change_set().ok_or("missing changes")?;
    assert_eq!(changes.changed_regions().len(), 1);
    let change = changes.changed_regions().first().ok_or("missing region")?;
    assert_eq!(change.volume_identity(), &VoxelVolumeId::new("sparse"));
    assert_eq!(change.region().origin(), VoxelCoordinate::new(512, 0, 0));
    Ok(())
}

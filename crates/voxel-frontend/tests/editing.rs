use voxel_frontend::{
    DenseVoxelBatch, DenseVoxelScene, DenseVoxelVolume, VoxelCoordinate, VoxelEditCommand,
    VoxelExtent, VoxelFrontend, VoxelFrontendError, VoxelMaterial, VoxelMaterialId, VoxelRegion,
    VoxelSceneId, VoxelSceneRevision, VoxelValue, VoxelVolumeId, VoxelVolumeMetadata,
};

fn frontend_at_revision(revision: u64) -> Result<VoxelFrontend, Box<dyn std::error::Error>> {
    let extent = VoxelExtent::new(2, 1, 1);
    let frontend = VoxelFrontend::new();
    frontend.publish(DenseVoxelScene::new(
        VoxelSceneId::new("edit-scene"),
        VoxelSceneRevision::new(revision),
        vec![
            VoxelMaterial::new(VoxelMaterialId::new("stone"), [0.2, 0.3, 0.4, 1.0]),
            VoxelMaterial::new(VoxelMaterialId::new("moss"), [0.1, 0.5, 0.2, 1.0]),
        ],
        vec![DenseVoxelVolume::new(
            VoxelVolumeMetadata::new(VoxelVolumeId::new("terrain"), extent, [0.0, 0.0, 0.0], 1.0),
            vec![DenseVoxelBatch::new(
                VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), extent),
                vec![VoxelValue::Empty, VoxelValue::Empty],
            )],
        )],
    ))?;
    Ok(frontend)
}

fn expected_error(
    result: Result<voxel_frontend::VoxelEditOutcome, VoxelFrontendError>,
) -> Result<VoxelFrontendError, Box<dyn std::error::Error>> {
    match result {
        Ok(_) => Err("invalid edit should return an error".into()),
        Err(error) => Ok(error),
    }
}

#[test]
fn invalid_edits_preserve_the_current_publication_with_contextual_errors()
-> Result<(), Box<dyn std::error::Error>> {
    let frontend = frontend_at_revision(u64::MAX)?;

    let unknown_volume_error = expected_error(frontend.edit(VoxelEditCommand::new(
        VoxelVolumeId::new("missing"),
        VoxelCoordinate::new(0, 0, 0),
        VoxelValue::Empty,
    )))?;
    assert!(unknown_volume_error.to_string().contains("missing"));

    let out_of_bounds_error = expected_error(frontend.edit(VoxelEditCommand::new(
        VoxelVolumeId::new("terrain"),
        VoxelCoordinate::new(2, 0, 0),
        VoxelValue::Empty,
    )))?;
    assert!(out_of_bounds_error.to_string().contains("terrain"));
    assert!(out_of_bounds_error.to_string().contains("VoxelCoordinate"));

    let unknown_material_error = expected_error(frontend.edit(VoxelEditCommand::new(
        VoxelVolumeId::new("terrain"),
        VoxelCoordinate::new(0, 0, 0),
        VoxelValue::Occupied(VoxelMaterialId::new("missing-material")),
    )))?;
    assert!(unknown_material_error.to_string().contains("terrain"));
    assert!(
        unknown_material_error
            .to_string()
            .contains("missing-material")
    );

    let overflow_error = expected_error(frontend.edit(VoxelEditCommand::new(
        VoxelVolumeId::new("terrain"),
        VoxelCoordinate::new(0, 0, 0),
        VoxelValue::Occupied(VoxelMaterialId::new("stone")),
    )))?;
    assert!(overflow_error.to_string().contains("edit-scene"));
    assert!(overflow_error.to_string().contains(&u64::MAX.to_string()));

    let current = frontend.scene_view()?;
    assert_eq!(current.revision(), VoxelSceneRevision::new(u64::MAX));
    assert_eq!(
        current
            .read_region(
                &VoxelVolumeId::new("terrain"),
                VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), VoxelExtent::new(1, 1, 1),),
            )?
            .first()
            .map(|sample| sample.value()),
        Some(&VoxelValue::Empty)
    );

    Ok(())
}

#[test]
fn semantically_unchanged_edit_returns_the_current_view_without_a_change_set()
-> Result<(), Box<dyn std::error::Error>> {
    let frontend = frontend_at_revision(12)?;

    let outcome = frontend.edit(VoxelEditCommand::new(
        VoxelVolumeId::new("terrain"),
        VoxelCoordinate::new(0, 0, 0),
        VoxelValue::Empty,
    ))?;

    assert!(matches!(
        &outcome,
        voxel_frontend::VoxelEditOutcome::Unchanged(_)
    ));
    assert_eq!(outcome.view().revision(), VoxelSceneRevision::new(12));
    assert!(outcome.change_set().is_none());
    assert_eq!(
        frontend.scene_view()?.revision(),
        VoxelSceneRevision::new(12)
    );

    Ok(())
}

#[test]
fn changed_edit_publishes_the_adjacent_view_and_complete_logical_change_set()
-> Result<(), Box<dyn std::error::Error>> {
    let frontend = frontend_at_revision(12)?;
    let retained_predecessor = frontend.scene_view()?;
    let edited_coordinate = VoxelCoordinate::new(1, 0, 0);

    let outcome = frontend.edit(VoxelEditCommand::new(
        VoxelVolumeId::new("terrain"),
        edited_coordinate,
        VoxelValue::Occupied(VoxelMaterialId::new("stone")),
    ))?;

    assert!(matches!(
        &outcome,
        voxel_frontend::VoxelEditOutcome::Changed { .. }
    ));
    assert_eq!(outcome.view().scene_id(), &VoxelSceneId::new("edit-scene"));
    assert_eq!(outcome.view().revision(), VoxelSceneRevision::new(13));
    let change_set = outcome
        .change_set()
        .ok_or("changed edit should return a Voxel Change Set")?;
    assert_eq!(
        change_set.scene_identity(),
        &VoxelSceneId::new("edit-scene")
    );
    assert_eq!(
        change_set.predecessor_revision(),
        VoxelSceneRevision::new(12)
    );
    assert_eq!(change_set.successor_revision(), VoxelSceneRevision::new(13));
    assert_eq!(change_set.changed_regions().len(), 1);
    let changed_region = change_set
        .changed_regions()
        .first()
        .ok_or("changed edit should identify its affected region")?;
    assert_eq!(
        changed_region.volume_identity(),
        &VoxelVolumeId::new("terrain")
    );
    assert_eq!(changed_region.region().origin(), edited_coordinate);
    assert_eq!(changed_region.region().extent(), VoxelExtent::new(1, 1, 1));

    let requested_region = VoxelRegion::new(edited_coordinate, VoxelExtent::new(1, 1, 1));
    assert_eq!(
        retained_predecessor
            .read_region(&VoxelVolumeId::new("terrain"), requested_region)?
            .first()
            .map(|sample| sample.value()),
        Some(&VoxelValue::Empty)
    );
    assert_eq!(
        outcome
            .view()
            .read_region(&VoxelVolumeId::new("terrain"), requested_region)?
            .first()
            .map(|sample| sample.value()),
        Some(&VoxelValue::Occupied(VoxelMaterialId::new("stone")))
    );
    assert_eq!(
        frontend.scene_view()?.revision(),
        VoxelSceneRevision::new(13)
    );

    Ok(())
}

#[test]
fn concurrent_commands_are_classified_in_serialized_publication_order()
-> Result<(), Box<dyn std::error::Error>> {
    let frontend = frontend_at_revision(12)?;
    let coordinate = VoxelCoordinate::new(0, 0, 0);
    let (stone_outcome, moss_outcome) = std::thread::scope(|scope| {
        let stone = scope.spawn(|| {
            frontend.edit(VoxelEditCommand::new(
                VoxelVolumeId::new("terrain"),
                coordinate,
                VoxelValue::Occupied(VoxelMaterialId::new("stone")),
            ))
        });
        let moss = scope.spawn(|| {
            frontend.edit(VoxelEditCommand::new(
                VoxelVolumeId::new("terrain"),
                coordinate,
                VoxelValue::Occupied(VoxelMaterialId::new("moss")),
            ))
        });
        let stone_outcome = stone.join().map_err(|_| "stone edit thread panicked")??;
        let moss_outcome = moss.join().map_err(|_| "moss edit thread panicked")??;
        Ok::<_, Box<dyn std::error::Error>>((stone_outcome, moss_outcome))
    })?;

    let revisions = [
        stone_outcome.view().revision(),
        moss_outcome.view().revision(),
    ];
    assert!(revisions.contains(&VoxelSceneRevision::new(13)));
    assert!(revisions.contains(&VoxelSceneRevision::new(14)));
    for outcome in [&stone_outcome, &moss_outcome] {
        let change_set = outcome
            .change_set()
            .ok_or("concurrent value-changing command should return a change set")?;
        assert_eq!(change_set.successor_revision(), outcome.view().revision());
        let expected_predecessor = if change_set.successor_revision() == VoxelSceneRevision::new(13)
        {
            VoxelSceneRevision::new(12)
        } else {
            VoxelSceneRevision::new(13)
        };
        assert_eq!(change_set.predecessor_revision(), expected_predecessor);
    }

    let requested_region = VoxelRegion::new(coordinate, VoxelExtent::new(1, 1, 1));
    assert_eq!(
        stone_outcome
            .view()
            .read_region(&VoxelVolumeId::new("terrain"), requested_region)?
            .first()
            .map(|sample| sample.value()),
        Some(&VoxelValue::Occupied(VoxelMaterialId::new("stone")))
    );
    assert_eq!(
        moss_outcome
            .view()
            .read_region(&VoxelVolumeId::new("terrain"), requested_region)?
            .first()
            .map(|sample| sample.value()),
        Some(&VoxelValue::Occupied(VoxelMaterialId::new("moss")))
    );
    assert_eq!(
        frontend.scene_view()?.revision(),
        VoxelSceneRevision::new(14)
    );

    Ok(())
}

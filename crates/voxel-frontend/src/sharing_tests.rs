use super::*;

#[test]
fn revisions_share_all_but_the_edited_page() -> Result<(), Box<dyn std::error::Error>> {
    let extent = VoxelExtent::new(513, 1, 1);
    let material = VoxelMaterialId::new("stone");
    let frontend = VoxelFrontend::new();
    let initial = frontend.publish(DenseVoxelScene::new(
        VoxelSceneId::new("sharing"),
        VoxelSceneRevision::new(0),
        vec![VoxelMaterial::new(material.clone(), [1.0; 4])],
        ["edited", "unrelated"]
            .into_iter()
            .map(|identity| {
                DenseVoxelVolume::new(
                    VoxelVolumeMetadata::new(VoxelVolumeId::new(identity), extent, [0.0; 3], 1.0),
                    vec![DenseVoxelBatch::new(
                        VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), extent),
                        vec![VoxelValue::Empty; 513],
                    )],
                )
            })
            .collect(),
    ))?;
    let edited = VoxelVolumeId::new("edited");
    let unrelated = VoxelVolumeId::new("unrelated");
    let mut retained = vec![initial];
    // Boundary and partial-page edits catch incorrect index splitting; revisiting
    // the first page catches accidental mutation of an older retained revision.
    for coordinate in [255, 256, 512, 0] {
        let predecessor = retained.last().ok_or("missing predecessor")?;
        let outcome = frontend.edit(VoxelEditCommand::new(
            edited.clone(),
            VoxelCoordinate::new(coordinate, 0, 0),
            VoxelValue::Occupied(material.clone()),
        ))?;
        let successor = outcome.view();
        let previous_storage = predecessor
            .published
            .volumes
            .get(&edited)
            .ok_or("missing volume")?;
        let next_storage = successor
            .published
            .volumes
            .get(&edited)
            .ok_or("missing volume")?;
        let edited_page = usize::try_from(coordinate)? / DenseStorage::PAGE_VALUES;
        assert_eq!(previous_storage.pages.len(), next_storage.pages.len());
        let mut copied_values = 0;
        for (page_index, (previous, next)) in previous_storage
            .pages
            .iter()
            .zip(next_storage.pages.iter())
            .enumerate()
        {
            assert_eq!(Arc::ptr_eq(previous, next), page_index != edited_page);
            if !Arc::ptr_eq(previous, next) {
                copied_values += previous.len();
            }
        }
        assert_eq!(copied_values, if coordinate == 512 { 1 } else { 256 });
        assert!(Arc::ptr_eq(
            &predecessor
                .published
                .volumes
                .get(&unrelated)
                .ok_or("missing volume")?
                .pages,
            &successor
                .published
                .volumes
                .get(&unrelated)
                .ok_or("missing volume")?
                .pages,
        ));
        assert!(Arc::ptr_eq(
            &predecessor.published.materials,
            &successor.published.materials
        ));
        assert!(Arc::ptr_eq(
            &predecessor.published.volume_metadata,
            &successor.published.volume_metadata
        ));
        assert_eq!(
            previous_storage.value(VoxelCoordinate::new(coordinate, 0, 0)),
            &VoxelValue::Empty
        );
        retained.push(successor.clone());
    }
    for (revision, view) in retained.iter().enumerate() {
        assert_eq!(
            view.revision(),
            VoxelSceneRevision::new(u64::try_from(revision)?)
        );
        for (edit_index, coordinate) in [255, 256, 512, 0].into_iter().enumerate() {
            let expected = if edit_index < revision {
                VoxelValue::Occupied(material.clone())
            } else {
                VoxelValue::Empty
            };
            let samples = view.read_region(
                &edited,
                VoxelRegion::new(
                    VoxelCoordinate::new(coordinate, 0, 0),
                    VoxelExtent::new(1, 1, 1),
                ),
            )?;
            assert_eq!(samples.first().map(VoxelSample::value), Some(&expected));
        }
    }
    let current = frontend.scene_view()?;
    let unchanged = frontend.edit(VoxelEditCommand::new(
        edited,
        VoxelCoordinate::new(0, 0, 0),
        VoxelValue::Occupied(material),
    ))?;
    assert!(unchanged.change_set().is_none());
    assert!(Arc::ptr_eq(&current.published, &unchanged.view().published));
    Ok(())
}

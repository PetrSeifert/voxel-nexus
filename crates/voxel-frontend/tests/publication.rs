use voxel_frontend::{
    DenseVoxelBatch, DenseVoxelScene, DenseVoxelVolume, VoxelCoordinate, VoxelExtent,
    VoxelFrontend, VoxelMaterial, VoxelMaterialId, VoxelRegion, VoxelSceneId, VoxelSceneRevision,
    VoxelValue, VoxelVolumeId, VoxelVolumeMetadata,
};

fn material(identity: &str) -> VoxelMaterial {
    VoxelMaterial::new(VoxelMaterialId::new(identity), [0.25, 0.5, 0.75, 1.0])
}

fn volume(identity: &str, extent: VoxelExtent, batches: Vec<DenseVoxelBatch>) -> DenseVoxelVolume {
    DenseVoxelVolume::new(
        VoxelVolumeMetadata::new(
            VoxelVolumeId::new(identity),
            extent,
            [10.0, 20.0, 30.0],
            0.5,
        ),
        batches,
    )
}

#[test]
fn published_scene_view_retains_identity_and_logical_contents()
-> Result<(), Box<dyn std::error::Error>> {
    let stone_identity = VoxelMaterialId::new("stone");
    let volume_extent = VoxelExtent::new(2, 2, 1);
    let scene = DenseVoxelScene::new(
        VoxelSceneId::new("proof-scene"),
        VoxelSceneRevision::new(7),
        vec![material("stone")],
        vec![volume(
            "terrain",
            volume_extent,
            vec![
                DenseVoxelBatch::new(
                    VoxelRegion::new(VoxelCoordinate::new(0, 1, 0), VoxelExtent::new(2, 1, 1)),
                    vec![
                        VoxelValue::Empty,
                        VoxelValue::Occupied(stone_identity.clone()),
                    ],
                ),
                DenseVoxelBatch::new(
                    VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), VoxelExtent::new(2, 1, 1)),
                    vec![
                        VoxelValue::Occupied(stone_identity.clone()),
                        VoxelValue::Empty,
                    ],
                ),
            ],
        )],
    );

    let frontend = VoxelFrontend::new();
    let retained_view = frontend.publish(scene)?;
    let independently_retained_view = frontend.scene_view()?;

    assert_eq!(retained_view.scene_id(), &VoxelSceneId::new("proof-scene"));
    assert_eq!(retained_view.revision(), VoxelSceneRevision::new(7));
    assert_eq!(retained_view.materials(), &[material("stone")]);
    assert_eq!(retained_view.volumes().len(), 1);
    assert_eq!(
        retained_view
            .volumes()
            .first()
            .map(|metadata| metadata.identity()),
        Some(&VoxelVolumeId::new("terrain"))
    );
    assert_eq!(
        retained_view
            .volumes()
            .first()
            .map(|metadata| metadata.extent()),
        Some(volume_extent)
    );

    let samples = independently_retained_view.read_region(
        &VoxelVolumeId::new("terrain"),
        VoxelRegion::new(VoxelCoordinate::new(-1, 0, 0), VoxelExtent::new(4, 2, 1)),
    )?;
    let observed = samples
        .iter()
        .map(|sample| (sample.coordinate(), sample.value().clone()))
        .collect::<Vec<_>>();
    assert_eq!(
        observed,
        vec![
            (VoxelCoordinate::new(-1, 0, 0), VoxelValue::Empty),
            (
                VoxelCoordinate::new(0, 0, 0),
                VoxelValue::Occupied(stone_identity.clone())
            ),
            (VoxelCoordinate::new(1, 0, 0), VoxelValue::Empty),
            (VoxelCoordinate::new(2, 0, 0), VoxelValue::Empty),
            (VoxelCoordinate::new(-1, 1, 0), VoxelValue::Empty),
            (VoxelCoordinate::new(0, 1, 0), VoxelValue::Empty),
            (
                VoxelCoordinate::new(1, 1, 0),
                VoxelValue::Occupied(stone_identity)
            ),
            (VoxelCoordinate::new(2, 1, 0), VoxelValue::Empty),
        ]
    );

    Ok(())
}

#[test]
fn region_contents_are_independent_of_dense_batch_shape() -> Result<(), Box<dyn std::error::Error>>
{
    let stone_identity = VoxelMaterialId::new("stone");
    let extent = VoxelExtent::new(2, 2, 1);
    let scene_with_one_batch = DenseVoxelScene::new(
        VoxelSceneId::new("one-batch"),
        VoxelSceneRevision::new(1),
        vec![material("stone")],
        vec![volume(
            "terrain",
            extent,
            vec![DenseVoxelBatch::new(
                VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), extent),
                vec![
                    VoxelValue::Occupied(stone_identity.clone()),
                    VoxelValue::Empty,
                    VoxelValue::Empty,
                    VoxelValue::Occupied(stone_identity.clone()),
                ],
            )],
        )],
    );
    let scene_with_four_batches = DenseVoxelScene::new(
        VoxelSceneId::new("four-batches"),
        VoxelSceneRevision::new(1),
        vec![material("stone")],
        vec![volume(
            "terrain",
            extent,
            vec![
                DenseVoxelBatch::new(
                    VoxelRegion::new(VoxelCoordinate::new(1, 1, 0), VoxelExtent::new(1, 1, 1)),
                    vec![VoxelValue::Occupied(stone_identity.clone())],
                ),
                DenseVoxelBatch::new(
                    VoxelRegion::new(VoxelCoordinate::new(0, 1, 0), VoxelExtent::new(1, 1, 1)),
                    vec![VoxelValue::Empty],
                ),
                DenseVoxelBatch::new(
                    VoxelRegion::new(VoxelCoordinate::new(1, 0, 0), VoxelExtent::new(1, 1, 1)),
                    vec![VoxelValue::Empty],
                ),
                DenseVoxelBatch::new(
                    VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), VoxelExtent::new(1, 1, 1)),
                    vec![VoxelValue::Occupied(stone_identity)],
                ),
            ],
        )],
    );
    let one_batch_view = VoxelFrontend::new().publish(scene_with_one_batch)?;
    let four_batch_view = VoxelFrontend::new().publish(scene_with_four_batches)?;
    let requested_region = VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), extent);

    assert_eq!(
        one_batch_view.read_region(&VoxelVolumeId::new("terrain"), requested_region)?,
        four_batch_view.read_region(&VoxelVolumeId::new("terrain"), requested_region)?
    );

    Ok(())
}

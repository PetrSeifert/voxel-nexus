use semantic_ray_oracle::{SemanticRay, SemanticRayResult, observe, observe_along_ray};
use voxel_frontend::{
    DenseVoxelBatch, DenseVoxelScene, DenseVoxelVolume, VoxelCoordinate, VoxelExtent,
    VoxelFrontend, VoxelMaterial, VoxelMaterialId, VoxelRegion, VoxelSceneId, VoxelSceneRevision,
    VoxelSceneView, VoxelValue, VoxelVolumeId, VoxelVolumeMetadata,
};

struct DeterministicSequence(u64);

impl DeterministicSequence {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }

    fn unit(&mut self) -> f64 {
        self.next() as f64 / (1_u64 << 31) as f64
    }

    fn between(&mut self, minimum: f64, maximum: f64) -> f64 {
        minimum + (maximum - minimum) * self.unit()
    }
}

fn sparse_volume(
    sequence: &mut DeterministicSequence,
    identity: &str,
    extent: VoxelExtent,
    scene_origin: [f32; 3],
    voxel_size: f32,
    materials: &[VoxelMaterialId],
    occupied_per_thousand: u64,
) -> DenseVoxelVolume {
    let [width, height, depth] = extent.dimensions();
    let values = (0..width * height * depth)
        .map(|_| {
            if sequence.next() % 1000 < occupied_per_thousand {
                let material_index = (sequence.next() as usize) % materials.len();
                materials
                    .get(material_index)
                    .cloned()
                    .map_or(VoxelValue::Empty, VoxelValue::Occupied)
            } else {
                VoxelValue::Empty
            }
        })
        .collect();
    DenseVoxelVolume::new(
        VoxelVolumeMetadata::new(
            VoxelVolumeId::new(identity),
            extent,
            scene_origin,
            voxel_size,
        ),
        vec![DenseVoxelBatch::new(
            VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), extent),
            values,
        )],
    )
}

fn randomized_view(
    sequence: &mut DeterministicSequence,
) -> Result<VoxelSceneView, Box<dyn std::error::Error>> {
    let materials = vec![
        VoxelMaterialId::new("stone"),
        VoxelMaterialId::new("moss"),
        VoxelMaterialId::new("water"),
    ];
    let scene = DenseVoxelScene::new(
        VoxelSceneId::new("randomized-scene"),
        VoxelSceneRevision::new(3),
        materials
            .iter()
            .map(|identity| VoxelMaterial::new(identity.clone(), [1.0; 4]))
            .collect(),
        vec![
            sparse_volume(
                sequence,
                "terrain",
                VoxelExtent::new(12, 7, 10),
                [0.0, 0.0, 0.0],
                1.0,
                &materials,
                80,
            ),
            sparse_volume(
                sequence,
                "overlapping",
                VoxelExtent::new(9, 9, 9),
                [2.0, 1.0, 1.5],
                0.5,
                &materials,
                40,
            ),
            sparse_volume(
                sequence,
                "offset",
                VoxelExtent::new(5, 5, 5),
                [-6.5, 3.25, -4.0],
                0.75,
                &materials,
                300,
            ),
        ],
    );
    Ok(VoxelFrontend::new().publish(scene)?)
}

fn assert_same_observation(
    view: &VoxelSceneView,
    ray: &SemanticRay,
) -> Result<(), Box<dyn std::error::Error>> {
    let exhaustive = observe(view, ray)?;
    let traversed = observe_along_ray(view, ray)?;
    assert_eq!(traversed, exhaustive, "ray {ray:?}");
    Ok(())
}

#[test]
fn traversal_agrees_with_the_exhaustive_oracle_for_randomized_rays()
-> Result<(), Box<dyn std::error::Error>> {
    let mut sequence = DeterministicSequence(0x5eed_1234);
    let mut contact_count = 0;
    for _ in 0..4 {
        let view = randomized_view(&mut sequence)?;
        for _ in 0..400 {
            let origin = [
                sequence.between(-10.0, 16.0),
                sequence.between(-4.0, 12.0),
                sequence.between(-8.0, 14.0),
            ];
            let direction = [
                sequence.between(-1.0, 1.0),
                sequence.between(-1.0, 1.0),
                sequence.between(-1.0, 1.0),
            ];
            let minimum_distance = if sequence.next().is_multiple_of(4) {
                sequence.between(0.0, 3.0)
            } else {
                0.0
            };
            let maximum_distance = minimum_distance + sequence.between(0.5, 40.0);
            let Ok(ray) = SemanticRay::new(origin, direction, minimum_distance, maximum_distance)
            else {
                continue;
            };
            if matches!(
                observe(&view, &ray)?.result(),
                SemanticRayResult::Contact(_)
            ) {
                contact_count += 1;
            }
            assert_same_observation(&view, &ray)?;
        }
    }
    assert!(
        contact_count > 100,
        "only {contact_count} rays reached a contact"
    );
    Ok(())
}

#[test]
fn traversal_agrees_on_axis_aligned_and_boundary_rays() -> Result<(), Box<dyn std::error::Error>> {
    let mut sequence = DeterministicSequence(0xb0_da_77);
    let view = randomized_view(&mut sequence)?;
    let directions = [
        [1.0, 0.0, 0.0],
        [-1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, -1.0, 0.0],
        [0.0, 0.0, 1.0],
        [0.0, 0.0, -1.0],
        [1.0, 1.0, 0.0],
        [-1.0, 0.0, 1.0],
        [0.0, -1.0, -1.0],
        [1.0, 1.0, 1.0],
        [-1.0, 1.0, -1.0],
        [2.0, 1.0, 0.0],
    ];
    for direction in directions {
        for origin_x in [-3.0, 0.0, 0.5, 2.0, 4.25, 6.0, 11.0, 13.0] {
            for origin_y in [-2.0, 0.0, 1.0, 3.25, 4.5, 7.0] {
                for origin_z in [-5.0, 0.0, 1.5, 2.75, 5.0, 10.0] {
                    let ray =
                        SemanticRay::new([origin_x, origin_y, origin_z], direction, 0.0, 30.0)?;
                    assert_same_observation(&view, &ray)?;
                    let clipped =
                        SemanticRay::new([origin_x, origin_y, origin_z], direction, 1.0, 30.0)?;
                    assert_same_observation(&view, &clipped)?;
                }
            }
        }
    }
    Ok(())
}

#[test]
fn traversal_respects_the_maximum_distance() -> Result<(), Box<dyn std::error::Error>> {
    let material = VoxelMaterialId::new("stone");
    let extent = VoxelExtent::new(8, 1, 1);
    let mut values = vec![VoxelValue::Empty; 8];
    if let Some(last) = values.last_mut() {
        *last = VoxelValue::Occupied(material.clone());
    }
    let view = VoxelFrontend::new().publish(DenseVoxelScene::new(
        VoxelSceneId::new("corridor"),
        VoxelSceneRevision::new(1),
        vec![VoxelMaterial::new(material, [1.0; 4])],
        vec![DenseVoxelVolume::new(
            VoxelVolumeMetadata::new(VoxelVolumeId::new("corridor"), extent, [0.0; 3], 1.0),
            vec![DenseVoxelBatch::new(
                VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), extent),
                values,
            )],
        )],
    ))?;
    for maximum_distance in [6.5, 7.0, 7.5] {
        let ray = SemanticRay::new([0.0, 0.5, 0.5], [1.0, 0.0, 0.0], 0.0, maximum_distance)?;
        assert_same_observation(&view, &ray)?;
    }
    let short = SemanticRay::new([0.0, 0.5, 0.5], [1.0, 0.0, 0.0], 0.0, 6.5)?;
    assert_eq!(
        observe_along_ray(&view, &short)?.result(),
        &SemanticRayResult::Miss
    );
    Ok(())
}

#[test]
fn traversal_agrees_on_non_aligned_sparse_volumes_after_structural_edits()
-> Result<(), Box<dyn std::error::Error>> {
    use voxel_frontend::{
        SparseVoxelBackground, SparseVoxelBatch, SparseVoxelScene, SparseVoxelVolume, StorageTier,
        VoxelEditCommand, VoxelRegionFill,
    };
    let stone = VoxelMaterialId::new("stone");
    let moss = VoxelMaterialId::new("moss");
    let identity = VoxelVolumeId::new("non-aligned");
    for tier in [StorageTier::Dense, StorageTier::SparsePages] {
        let frontend = VoxelFrontend::new();
        let original = frontend.publish_sparse(SparseVoxelScene::new(
            VoxelSceneId::new("structural-edits"),
            VoxelSceneRevision::new(1),
            vec![
                VoxelMaterial::new(stone.clone(), [1.0; 4]),
                VoxelMaterial::new(moss.clone(), [0.5; 4]),
            ],
            vec![
                SparseVoxelVolume::new(
                    VoxelVolumeMetadata::new(
                        identity.clone(),
                        VoxelExtent::new(17, 11, 9),
                        [-2.5, 1.25, 0.5],
                        0.5,
                    ),
                    SparseVoxelBackground::Empty,
                    vec![SparseVoxelBatch::Fill(VoxelRegionFill::new(
                        VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), VoxelExtent::new(8, 8, 8)),
                        VoxelValue::Occupied(stone.clone()),
                    ))],
                )
                .with_storage_tier(tier),
            ],
        ))?;
        let rays = [
            SemanticRay::new([-4.0, 3.0, 2.25], [1.0, 0.0, 0.0], 0.0, 20.0)?,
            SemanticRay::new([-0.75, 3.0, 2.25], [1.0, 1.0, 1.0], 0.0, 20.0)?,
            SemanticRay::new([8.0, 6.5, 4.75], [-1.0, 0.0, 0.0], 0.0, 20.0)?,
            SemanticRay::new([-2.5, 1.25, 0.5], [1.0, 1.0, 1.0], 0.25, 20.0)?,
        ];
        for ray in &rays {
            assert_same_observation(&original, ray)?;
        }
        for (coordinate, value) in [
            (
                VoxelCoordinate::new(16, 10, 8),
                VoxelValue::Occupied(stone.clone()),
            ),
            (VoxelCoordinate::new(16, 10, 8), VoxelValue::Empty),
            (VoxelCoordinate::new(3, 3, 3), VoxelValue::Empty),
            (
                VoxelCoordinate::new(4, 3, 3),
                VoxelValue::Occupied(moss.clone()),
            ),
            (
                VoxelCoordinate::new(3, 3, 3),
                VoxelValue::Occupied(stone.clone()),
            ),
        ] {
            frontend.edit(VoxelEditCommand::new(
                identity.clone(),
                coordinate,
                value.clone(),
            ))?;
            let view = frontend.scene_view()?;
            let [x, y, z] = coordinate.components().map(f64::from);
            let ray = SemanticRay::new(
                [
                    -2.5 + (x + 0.5) * 0.5,
                    1.25 + (y + 0.5) * 0.5,
                    0.5 + (z + 0.5) * 0.5,
                ],
                [1.0, 0.0, 0.0],
                0.0,
                0.1,
            )?;
            assert_same_observation(&view, &ray)?;
            let observation = observe_along_ray(&view, &ray)?;
            assert_eq!(observation.revision(), view.revision());
            match (value, observation.result()) {
                (VoxelValue::Empty, SemanticRayResult::Miss) => {}
                (VoxelValue::Occupied(material), SemanticRayResult::Contact(contact)) => {
                    assert_eq!(contact.coordinate(), coordinate);
                    assert_eq!(contact.material_identity(), &material);
                    assert_eq!(
                        contact.classification(),
                        semantic_ray_oracle::SemanticRayContactClassification::StartedInside
                    );
                }
                unexpected => panic!("unexpected edited observation: {unexpected:?}"),
            }
            for ray in &rays {
                assert_same_observation(&view, ray)?;
                assert_same_observation(&original, ray)?;
            }
        }
    }
    Ok(())
}

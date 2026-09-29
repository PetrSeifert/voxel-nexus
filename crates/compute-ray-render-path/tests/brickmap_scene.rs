use compute_ray_render_path::BrickmapSceneBundle;
use semantic_ray_oracle::{SemanticRay, SemanticRayDistanceTolerance, observe};
use voxel_frontend::*;

fn fixture() -> Result<VoxelSceneView, VoxelFrontendError> {
    VoxelFrontend::new().publish_sparse(
        SparseVoxelScene::new(
            VoxelSceneId::new("brickmap"),
            VoxelSceneRevision::new(3),
            vec![VoxelMaterial::new(VoxelMaterialId::new("stone"), [1.0; 4])],
            vec![SparseVoxelVolume::new(
                VoxelVolumeMetadata::new(
                    VoxelVolumeId::new("volume"),
                    VoxelExtent::new(25, 9, 9),
                    [0.0; 3],
                    1.0,
                ),
                SparseVoxelBackground::Empty,
                vec![
                    SparseVoxelBatch::Fill(VoxelRegionFill::new(
                        VoxelRegion::new(VoxelCoordinate::new(8, 0, 0), VoxelExtent::new(8, 8, 8)),
                        VoxelValue::Occupied(VoxelMaterialId::new("stone")),
                    )),
                    SparseVoxelBatch::Fill(VoxelRegionFill::new(
                        VoxelRegion::new(VoxelCoordinate::new(24, 8, 8), VoxelExtent::new(1, 1, 1)),
                        VoxelValue::Occupied(VoxelMaterialId::new("stone")),
                    )),
                    SparseVoxelBatch::Fill(VoxelRegionFill::new(
                        VoxelRegion::new(VoxelCoordinate::new(19, 3, 3), VoxelExtent::new(1, 1, 1)),
                        VoxelValue::Occupied(VoxelMaterialId::new("stone")),
                    )),
                ],
            )],
        )
        .with_storage_tier(StorageTier::SparsePages),
    )
}

#[test]
fn sparse_and_clipped_cells_build_and_trace() -> Result<(), Box<dyn std::error::Error>> {
    let view = fixture()?;
    let bundle = BrickmapSceneBundle::from_view(&view)?;
    assert_eq!(bundle.observations().coarse_grid_bytes, 64);
    assert_eq!(bundle.observations().mixed_brick_count, 1);
    assert_eq!(bundle.observations().pool_bytes, 1024);
    for (origin, direction) in [
        ([-1.0, 3.5, 3.5], [1.0, 0.0, 0.0]),
        ([26.0, 3.5, 3.5], [-1.0, 0.0, 0.0]),
        ([26.0, 8.5, 8.5], [-1.0, 0.0, 0.0]),
        ([16.0, 3.5, 3.5], [-1.0, 0.0, 0.0]),
        ([-1.0; 3], [1.0; 3]),
        ([0.0, 8.5, 0.0], [1.0, 0.0, 0.0]),
    ] {
        let ray = SemanticRay::new(origin, direction, 0.0, 60.0)?;
        assert!(
            bundle.observe(&ray).agrees_with(
                &observe(&view, &ray)?,
                SemanticRayDistanceTolerance::new(1e-9)?
            ),
            "{ray:?}"
        );
    }
    Ok(())
}

#[test]
fn only_mixed_cells_read_voxel_payloads() -> Result<(), Box<dyn std::error::Error>> {
    let view = fixture()?;
    let started = std::time::Instant::now();
    let (bundle, counters) = count_storage_work(|| BrickmapSceneBundle::from_view(&view));
    let bundle = bundle?;
    assert!(
        counters.voxel_values_examined > 0 && counters.voxel_values_examined <= 512,
        "{counters:?}"
    );
    assert_eq!(counters.enumeration.cells_emitted, 3);
    assert_eq!(bundle.observations().mixed_brick_count, 1);
    assert!(bundle.observations().construction_time <= started.elapsed());
    Ok(())
}

fn material_fixture(count: u32) -> Result<VoxelSceneView, VoxelFrontendError> {
    let materials: Vec<_> = (0..count)
        .map(|index| {
            VoxelMaterial::new(VoxelMaterialId::new(format!("material-{index}")), [1.0; 4])
        })
        .collect();
    let values = materials
        .iter()
        .map(|material| VoxelValue::Occupied(material.identity().clone()))
        .chain([VoxelValue::Empty])
        .collect();
    VoxelFrontend::new().publish(DenseVoxelScene::new(
        VoxelSceneId::new("packing"),
        VoxelSceneRevision::new(0),
        materials,
        vec![DenseVoxelVolume::new(
            VoxelVolumeMetadata::new(
                VoxelVolumeId::new("palette-volume"),
                VoxelExtent::new(count + 1, 1, 1),
                [0.0; 3],
                1.0,
            ),
            vec![DenseVoxelBatch::new(
                VoxelRegion::new(
                    VoxelCoordinate::new(0, 0, 0),
                    VoxelExtent::new(count + 1, 1, 1),
                ),
                values,
            )],
        )],
    ))
}

#[test]
fn every_packed_material_index_round_trips() -> Result<(), Box<dyn std::error::Error>> {
    let view = material_fixture(65_535)?;
    let bundle = BrickmapSceneBundle::from_view(&view)?;
    assert_eq!(bundle.material_identities().len(), 65_535);
    for index in 0..65_535 {
        let ray = SemanticRay::new(
            [f64::from(index) + 0.5, 0.5, 0.5],
            [0.0, 1.0, 0.0],
            0.0,
            1.0,
        )?;
        let observation = bundle.observe(&ray);
        let semantic_ray_oracle::SemanticRayResult::Contact(contact) = observation.result() else {
            panic!("missing packed index {index}");
        };
        assert_eq!(
            contact.material_identity(),
            &VoxelMaterialId::new(format!("material-{index}"))
        );
    }
    let empty = SemanticRay::new([65_535.5, 0.5, 0.5], [0.0, 1.0, 0.0], 0.0, 1.0)?;
    assert_eq!(
        bundle.observe(&empty).result(),
        &semantic_ray_oracle::SemanticRayResult::Miss
    );
    Ok(())
}

#[test]
fn too_many_occupied_materials_reports_volume_and_limit() -> Result<(), Box<dyn std::error::Error>>
{
    let error = BrickmapSceneBundle::from_view(&material_fixture(65_536)?).unwrap_err();
    assert!(matches!(
        error,
        compute_ray_render_path::BrickmapBuildError::TooManyMaterials { .. }
    ));
    assert!(error.to_string().contains("palette-volume"));
    assert!(error.to_string().contains("65,535"));
    Ok(())
}

#[test]
fn oversized_coarse_grid_fails_before_allocation() -> Result<(), Box<dyn std::error::Error>> {
    let view = VoxelFrontend::new().publish_sparse(
        SparseVoxelScene::new(
            VoxelSceneId::new("oversized"),
            VoxelSceneRevision::new(0),
            vec![],
            vec![SparseVoxelVolume::new(
                VoxelVolumeMetadata::new(
                    VoxelVolumeId::new("huge-volume"),
                    VoxelExtent::new(1 << 20, 1 << 20, 1 << 20),
                    [0.0; 3],
                    1.0,
                ),
                SparseVoxelBackground::Empty,
                vec![],
            )],
        )
        .with_storage_tier(StorageTier::SparsePages),
    )?;
    let error = BrickmapSceneBundle::from_view(&view).unwrap_err();
    assert!(matches!(
        error,
        compute_ray_render_path::BrickmapBuildError::Size {
            part: "coarse grid",
            ..
        }
    ));
    assert!(error.to_string().contains("huge-volume"));
    Ok(())
}

#[test]
fn uniform_and_empty_volumes_need_no_payload_reads_or_pool()
-> Result<(), Box<dyn std::error::Error>> {
    for value in [
        VoxelValue::Empty,
        VoxelValue::Occupied(VoxelMaterialId::new("stone")),
    ] {
        let view = VoxelFrontend::new().publish_sparse(
            SparseVoxelScene::new(
                VoxelSceneId::new("uniform"),
                VoxelSceneRevision::new(0),
                vec![VoxelMaterial::new(VoxelMaterialId::new("stone"), [1.0; 4])],
                vec![SparseVoxelVolume::new(
                    VoxelVolumeMetadata::new(
                        VoxelVolumeId::new("uniform-volume"),
                        VoxelExtent::new(17, 9, 3),
                        [0.0; 3],
                        1.0,
                    ),
                    SparseVoxelBackground::Empty,
                    vec![SparseVoxelBatch::Fill(VoxelRegionFill::new(
                        VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), VoxelExtent::new(17, 9, 3)),
                        value,
                    ))],
                )],
            )
            .with_storage_tier(StorageTier::SparsePages),
        )?;
        let (bundle, counters) = count_storage_work(|| BrickmapSceneBundle::from_view(&view));
        let bundle = bundle?;
        assert_eq!(counters.voxel_values_examined, 0);
        assert_eq!(bundle.observations().pool_bytes, 0);
        assert_eq!(bundle.observations().mixed_brick_count, 0);
    }
    Ok(())
}

#[test]
fn oblique_rays_and_clipping_agree_across_transformed_volumes()
-> Result<(), Box<dyn std::error::Error>> {
    let extent = VoxelExtent::new(19, 11, 9);
    let material = VoxelMaterialId::new("stone");
    let values: Vec<_> = (0..19 * 11 * 9)
        .map(|index| {
            if index % 31 == 0 || index % 43 == 1 {
                VoxelValue::Occupied(material.clone())
            } else {
                VoxelValue::Empty
            }
        })
        .collect();
    let view = VoxelFrontend::new().publish(DenseVoxelScene::new(
        VoxelSceneId::new("oblique"),
        VoxelSceneRevision::new(9),
        vec![VoxelMaterial::new(material, [1.0; 4])],
        ["zeta", "alpha"]
            .into_iter()
            .map(|name| {
                DenseVoxelVolume::new(
                    VoxelVolumeMetadata::new(
                        VoxelVolumeId::new(name),
                        extent,
                        [-2.0, 4.0, 1.0],
                        0.5,
                    ),
                    vec![DenseVoxelBatch::new(
                        VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), extent),
                        values.clone(),
                    )],
                )
            })
            .collect(),
    ))?;
    let bundle = BrickmapSceneBundle::from_view(&view)?;
    for index in 0..160 {
        let origin = [
            f64::from(index % 23) * 0.5 - 3.0,
            f64::from(index % 13) * 0.5 + 3.0,
            f64::from(index % 11) * 0.5,
        ];
        for direction in [
            [1.0, 0.3, -0.7],
            [-0.2, 1.0, 0.4],
            [-1.0; 3],
            [1.0; 3],
            [0.0, -1.0, 0.0],
        ] {
            let ray = SemanticRay::new(origin, direction, f64::from(index % 3) * 0.25, 20.0)?;
            let actual = bundle.observe(&ray);
            let expected = observe(&view, &ray)?;
            assert!(
                actual.agrees_with(&expected, SemanticRayDistanceTolerance::new(1e-9)?),
                "{ray:?}\n{actual:?}\n{expected:?}"
            );
        }
    }
    Ok(())
}

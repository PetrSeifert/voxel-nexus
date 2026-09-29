use compute_ray_render_path::{ComputeRayRenderPathAdapter, ComputeRepresentation};
use render_backend::{CameraState, CameraStateRevision};
use voxel_frontend::{DenseVoxelScene, VoxelFrontend, VoxelSceneId, VoxelSceneRevision};

fn volume_view(
    origin: [f32; 3],
    extent: [u32; 3],
    size: f32,
) -> Result<voxel_frontend::VoxelSceneView, voxel_frontend::VoxelFrontendError> {
    use voxel_frontend::*;
    VoxelFrontend::new().publish_sparse(
        SparseVoxelScene::new(
            VoxelSceneId::new("limits"),
            VoxelSceneRevision::new(1),
            vec![],
            vec![SparseVoxelVolume::new(
                VoxelVolumeMetadata::new(
                    VoxelVolumeId::new("volume"),
                    VoxelExtent::new(extent[0], extent[1], extent[2]),
                    origin,
                    size,
                ),
                SparseVoxelBackground::Empty,
                vec![],
            )],
        )
        .with_storage_tier(StorageTier::SparsePages),
    )
}

#[test]
fn packed_coarse_grid_and_metadata_must_fit_both_device_limits()
-> Result<(), Box<dyn std::error::Error>> {
    use compute_ray_render_path::ComputeSceneBundle;
    let bundle = ComputeSceneBundle::from_view_with_representation(
        &volume_view([0.0; 3], [9, 1, 1], 1.0)?,
        ComputeRepresentation::Brickmap { budget_bytes: 60 },
    )?;
    bundle.validate_device_limits(60, 60)?;
    assert!(
        bundle
            .validate_device_limits(59, 60)
            .err()
            .ok_or("missing range rejection")?
            .to_string()
            .contains("maxStorageBufferRange")
    );
    assert!(
        bundle
            .validate_device_limits(60, 59)
            .err()
            .ok_or("missing size rejection")?
            .to_string()
            .contains("maxBufferSize")
    );
    Ok(())
}

#[test]
fn origin_extent_and_final_bounds_are_separate_limits() -> Result<(), Box<dyn std::error::Error>> {
    use compute_ray_render_path::ComputeSceneBundle;
    let representation = ComputeRepresentation::Brickmap {
        budget_bytes: 1024 * 1024,
    };
    for (origin, extent, size, reason) in [
        ([65537.0, 0.0, 0.0], [1; 3], 1.0, "origin"),
        ([65536.0, 0.0, 0.0], [8193, 1, 1], 8.0, "final scene-space"),
        ([0.0; 3], [65537, 1, 1], 1.0, "extent"),
        ([0.0; 3], [1; 3], 0.0625, "voxel size"),
    ] {
        let view = volume_view(origin, extent, size)?;
        assert!(
            ComputeSceneBundle::from_view_with_representation(&view, representation)
                .err()
                .ok_or("missing envelope rejection")?
                .to_string()
                .contains(reason)
        );
        ComputeSceneBundle::from_view(&view)?;
    }
    ComputeSceneBundle::from_view_with_representation(
        &volume_view([65536.0, 0.0, 0.0], [8192, 1, 1], 8.0)?,
        representation,
    )?;
    Ok(())
}

#[test]
fn sparse_camera_updates_use_the_existing_camera_contract()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use render_backend::RenderPath;
    let view = volume_view([0.0; 3], [1; 3], 1.0)?;
    let camera = CameraState::new([0.0, 0.0, 5.0], [0.0; 3], [0.0, 1.0, 0.0], 50.0, 0.1, 100.0)?;
    let mut path = ComputeRayRenderPathAdapter::new_with_representation(
        view,
        camera,
        CameraStateRevision::new(1),
        ComputeRepresentation::Brickmap { budget_bytes: 1024 },
    )?;
    let distant = CameraState::new(
        [200000.0, 0.0, 5.0],
        [0.0; 3],
        [0.0, 1.0, 0.0],
        50.0,
        0.1,
        300000.0,
    )?;
    path.publish_camera_state(distant, CameraStateRevision::new(2))?;
    assert_eq!(path.camera_state(), distant);
    Ok(())
}

#[test]
fn huge_coarse_grid_is_rejected_before_construction() -> Result<(), Box<dyn std::error::Error>> {
    let view = volume_view([0.0; 3], [65536; 3], 1.0)?;
    let error = compute_ray_render_path::ComputeSceneBundle::from_view_with_representation(
        &view,
        ComputeRepresentation::Brickmap {
            budget_bytes: 128 * 1024 * 1024,
        },
    )
    .err()
    .ok_or("expected coarse-grid budget rejection")?;
    assert!(error.to_string().contains("budget"));
    Ok(())
}

#[test]
fn mixed_pool_counts_toward_budget_and_device_limits() -> Result<(), Box<dyn std::error::Error>> {
    use voxel_frontend::*;
    let material = VoxelMaterialId::new("stone");
    let view = VoxelFrontend::new().publish_sparse(SparseVoxelScene::new(
        VoxelSceneId::new("pool-limits"),
        VoxelSceneRevision::new(1),
        vec![VoxelMaterial::new(material.clone(), [1.0; 4])],
        vec![SparseVoxelVolume::new(
            VoxelVolumeMetadata::new(
                VoxelVolumeId::new("volume"),
                VoxelExtent::new(9, 1, 1),
                [0.0; 3],
                1.0,
            ),
            SparseVoxelBackground::Empty,
            vec![SparseVoxelBatch::Fill(VoxelRegionFill::new(
                VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), VoxelExtent::new(1, 1, 1)),
                VoxelValue::Occupied(material),
            ))],
        )],
    ))?;
    let bundle = compute_ray_render_path::ComputeSceneBundle::from_view_with_representation(
        &view,
        ComputeRepresentation::Brickmap { budget_bytes: 1100 },
    )?;
    bundle.validate_device_limits(1100, 1100)?;
    assert!(bundle.validate_device_limits(1099, 1100).is_err());
    assert!(bundle.validate_device_limits(1100, 1099).is_err());
    assert!(
        compute_ray_render_path::ComputeSceneBundle::from_view_with_representation(
            &view,
            ComputeRepresentation::Brickmap { budget_bytes: 1099 },
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn sparse_budget_is_enforced_without_restricting_dense() -> Result<(), Box<dyn std::error::Error>> {
    let view = VoxelFrontend::new().publish(DenseVoxelScene::new(
        VoxelSceneId::new("budget"),
        VoxelSceneRevision::new(1),
        vec![],
        vec![],
    ))?;
    let camera = CameraState::new([0.0, 0.0, 5.0], [0.0; 3], [0.0, 1.0, 0.0], 50.0, 0.1, 100.0)?;
    let result = ComputeRayRenderPathAdapter::new_with_representation(
        view.clone(),
        camera,
        CameraStateRevision::new(1),
        ComputeRepresentation::Brickmap { budget_bytes: 0 },
    );
    assert!(
        result
            .err()
            .ok_or("expected budget rejection")?
            .to_string()
            .contains("budget")
    );
    ComputeRayRenderPathAdapter::new(view, camera, CameraStateRevision::new(1))?;
    Ok(())
}

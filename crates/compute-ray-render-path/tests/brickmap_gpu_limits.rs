use compute_ray_render_path::{
    BrickmapValidationError, ComputeRayRenderPathAdapter, ComputeRepresentation,
    ComputeSceneBuildError, ComputeSceneBundle,
};
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
    let view = volume_view([0.0; 3], [9, 1, 1], 1.0)?;
    let bundle = ComputeSceneBundle::from_view_with_representation(
        &view,
        ComputeRepresentation::Brickmap { budget_bytes: 60 },
    )?;
    check_buffer_limits(&bundle, 60)?;
    let error = ComputeSceneBundle::from_view_with_representation(
        &view,
        ComputeRepresentation::Brickmap { budget_bytes: 59 },
    )
    .expect_err("the coarse grid exceeds the budget by one byte");
    let ComputeSceneBuildError::BrickmapValidation(error) = error else {
        panic!("expected a brickmap budget error, got {error:?}");
    };
    check_buffer_error(error, "configured budget", 60, 59);
    Ok(())
}

fn check_buffer_error(error: BrickmapValidationError, limit: &str, required: u64, available: u64) {
    let BrickmapValidationError::BufferLimit {
        limit: actual_limit,
        required: actual_required,
        available: actual_available,
    } = error
    else {
        panic!("expected a buffer limit error, got {error:?}");
    };
    assert_eq!(
        (actual_limit, actual_required, actual_available),
        (limit, required, available)
    );
}

fn check_buffer_limits(
    bundle: &ComputeSceneBundle,
    required: u64,
) -> Result<(), BrickmapValidationError> {
    for (storage, buffer) in [
        (required, required),
        (required, u64::MAX),
        (u64::MAX, required),
    ] {
        bundle.validate_device_limits(storage, buffer)?;
    }
    for (storage, buffer, limit) in [
        (required - 1, u64::MAX, "maxStorageBufferRange"),
        (u64::MAX, required - 1, "maxBufferSize"),
    ] {
        check_buffer_error(
            bundle
                .validate_device_limits(storage, buffer)
                .expect_err("one insufficient capability must reject the scene"),
            limit,
            required,
            required - 1,
        );
    }
    Ok(())
}

#[test]
fn origin_extent_and_final_bounds_are_separate_limits() -> Result<(), Box<dyn std::error::Error>> {
    let representation = ComputeRepresentation::Brickmap {
        budget_bytes: 1024 * 1024,
    };
    for axis in 0..3 {
        for (origin_component, length, size, rejection) in [
            (-65536.0_f32, 1, 1.0, None),
            (65536.0, 1, 1.0, None),
            ((-65536.0_f32).next_down(), 1, 1.0, Some("origin")),
            (65536.0_f32.next_up(), 1, 1.0, Some("origin")),
            (0.0, 65536, 0.125, None),
            (0.0, 65537, 0.125, Some("extent")),
            (65536.0, 8192, 8.0, None),
            (65536.0, 8193, 8.0, Some("final scene-space")),
            // The f64 bound exceeds 131072 even though its f32 rounding equals the limit.
            (65536.0, 8192, 8.0_f32.next_up(), Some("final scene-space")),
        ] {
            let mut origin = [0.0; 3];
            let mut extent = [1; 3];
            origin[axis] = origin_component;
            extent[axis] = length;
            let view = volume_view(origin, extent, size)?;
            let result = ComputeSceneBundle::from_view_with_representation(&view, representation);
            if let Some(reason) = rejection {
                let error = result.expect_err("a value just outside the envelope must be rejected");
                assert!(matches!(
                    error,
                    ComputeSceneBuildError::BrickmapValidation(
                        BrickmapValidationError::Envelope { .. }
                    )
                ));
                assert!(error.to_string().contains(reason), "axis {axis}: {error}");
            } else {
                result?;
            }
            ComputeSceneBundle::from_view(&view)?;
        }
    }
    Ok(())
}

#[test]
fn voxel_size_endpoints_are_inclusive_and_sparse_only() -> Result<(), Box<dyn std::error::Error>> {
    for (size, accepted) in [
        (0.125_f32.next_down(), false),
        (0.125, true),
        (16.0, true),
        (16.0_f32.next_up(), false),
    ] {
        let view = volume_view([0.0; 3], [1; 3], size)?;
        let result = ComputeSceneBundle::from_view_with_representation(
            &view,
            ComputeRepresentation::Brickmap { budget_bytes: 1024 },
        );
        if accepted {
            result?;
        } else {
            let error =
                result.expect_err("a voxel size just outside the envelope must be rejected");
            assert!(error.to_string().contains("voxel size"));
        }
        ComputeSceneBundle::from_view(&view)?;
    }
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
        ComputeRepresentation::Brickmap { budget_bytes: 2124 },
    )?;
    check_buffer_limits(&bundle, 2124)?;
    let error = compute_ray_render_path::ComputeSceneBundle::from_view_with_representation(
        &view,
        ComputeRepresentation::Brickmap { budget_bytes: 2123 },
    )
    .expect_err("the mixed pool exceeds the budget by one byte");
    let ComputeSceneBuildError::BrickmapValidation(error) = error else {
        panic!("expected a brickmap budget error, got {error:?}");
    };
    check_buffer_error(error, "configured budget", 2124, 2123);
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

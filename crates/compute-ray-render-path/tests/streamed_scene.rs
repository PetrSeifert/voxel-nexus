use compute_ray_render_path::{ComputeRepresentation, ComputeSceneBuildError, ComputeSceneBundle};
use std::sync::Arc;
use voxel_frontend::*;

struct Recipe {
    scene: VoxelSceneId,
    metadata: VoxelVolumeMetadata,
}

impl VoxelVolumeSource for Recipe {
    fn requires_materials(&self) -> bool {
        false
    }

    fn scene_id(&self) -> &VoxelSceneId {
        &self.scene
    }

    fn value(&self, _: VoxelCoordinate) -> Result<VoxelValue, VoxelSourceError> {
        Ok(VoxelValue::Empty)
    }

    fn materialize(&self) -> Result<SparseVoxelVolume, VoxelSourceError> {
        Ok(
            SparseVoxelVolume::new(self.metadata.clone(), SparseVoxelBackground::Empty, vec![])
                .with_storage_tier(StorageTier::SparsePages),
        )
    }
}

fn scene() -> Result<(VoxelFrontend, VoxelSceneView), Box<dyn std::error::Error>> {
    let frontend = VoxelFrontend::with_residency_limits(VoxelResidencyLimits::new(9)?);
    let scene = VoxelSceneId::new("streamed-compute");
    let metadata = VoxelVolumeMetadata::new(
        VoxelVolumeId::new("terrain"),
        VoxelExtent::new(8, 8, 8),
        [0.0; 3],
        1.0,
    );
    let view = frontend.publish_streamed(StreamedVoxelScene::new(
        scene.clone(),
        VoxelSceneRevision::new(1),
        vec![],
        vec![StreamedVoxelVolume::new(
            metadata.clone(),
            Arc::new(Recipe { scene, metadata }),
        )],
    ))?;
    Ok((frontend, view))
}

#[test]
fn streamed_dense_construction_is_rejected_without_qualification()
-> Result<(), Box<dyn std::error::Error>> {
    let (frontend, view) = scene()?;
    assert!(matches!(
        ComputeSceneBundle::from_view_with_representation(&view, ComputeRepresentation::Dense),
        Err(ComputeSceneBuildError::StreamedDense)
    ));
    assert!(matches!(
        ComputeSceneBundle::from_view(&view),
        Err(ComputeSceneBuildError::StreamedDense)
    ));
    assert_eq!(frontend.materialization_cache_stats()?.copies, 0);
    Ok(())
}

#[test]
fn production_brickmap_construction_requires_and_releases_shared_residency_copies()
-> Result<(), Box<dyn std::error::Error>> {
    let (frontend, view) = scene()?;
    let representation = ComputeRepresentation::Brickmap {
        budget_bytes: 1_000_000,
    };
    assert!(matches!(
        ComputeSceneBundle::from_view_with_representation(&view, representation),
        Err(ComputeSceneBuildError::ResidencySelectionRequired)
    ));
    assert_eq!(frontend.materialization_cache_stats()?.copies, 0);
    let selection = view.residency_selection(
        VoxelResidencySelectionId::new(1),
        [VoxelVolumeId::new("terrain")],
    )?;
    let copies = frontend.materialize_residency(&selection, &view)?;
    let bundle = ComputeSceneBundle::from_residency(copies, representation)?;
    assert_eq!(bundle.residency_selection(), Some(&selection));
    assert_eq!(bundle.volume_headers().len(), 1);
    assert_eq!(frontend.materialization_cache_stats()?.copies, 1);
    drop(bundle);
    assert_eq!(frontend.materialization_cache_stats()?.copies, 0);
    Ok(())
}

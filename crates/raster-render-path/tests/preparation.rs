use raster_render_path::{
    RasterArtifactPreparation, RasterArtifactPreparationEvent, RasterPreparationBarrier,
};
use std::sync::mpsc;
use std::thread;
use voxel_frontend::{
    DenseVoxelBatch, DenseVoxelScene, DenseVoxelVolume, VoxelCoordinate, VoxelExtent,
    VoxelFrontend, VoxelMaterial, VoxelMaterialId, VoxelRegion, VoxelSceneId, VoxelSceneRevision,
    VoxelValue, VoxelVolumeId, VoxelVolumeMetadata,
};

fn published_view() -> Result<voxel_frontend::VoxelSceneView, voxel_frontend::VoxelFrontendError> {
    let extent = VoxelExtent::new(1, 1, 1);
    let material_identity = VoxelMaterialId::new("stone");
    VoxelFrontend::new().publish(DenseVoxelScene::new(
        VoxelSceneId::new("background-preparation"),
        VoxelSceneRevision::new(27),
        vec![VoxelMaterial::new(
            material_identity.clone(),
            [0.25, 0.5, 0.75, 1.0],
        )],
        vec![DenseVoxelVolume::new(
            VoxelVolumeMetadata::new(
                VoxelVolumeId::new("diagnostic"),
                extent,
                [0.0, 0.0, 0.0],
                1.0,
            ),
            vec![DenseVoxelBatch::new(
                VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), extent),
                vec![VoxelValue::Occupied(material_identity)],
            )],
        )],
    ))
}

#[test]
fn retained_view_preparation_pauses_on_a_real_worker_until_released()
-> Result<(), Box<dyn std::error::Error>> {
    let main_thread = thread::current().id();
    let (worker_barrier, barrier_release) = RasterPreparationBarrier::held();
    let (event_sender, event_receiver) = mpsc::channel();
    let mut preparation = RasterArtifactPreparation::start(
        published_view()?,
        VoxelVolumeId::new("diagnostic"),
        Some(worker_barrier),
        move |event| {
            if event_sender.send((thread::current().id(), event)).is_err() {
                eprintln!("preparation test event receiver closed");
            }
        },
    )?;

    let (worker_thread, event) = event_receiver.recv()?;
    assert_ne!(worker_thread, main_thread);
    assert_eq!(
        event,
        RasterArtifactPreparationEvent::PausedAtBarrier {
            source_revision: VoxelSceneRevision::new(27),
        }
    );
    assert!(preparation.try_complete()?.is_none());

    barrier_release.release()?;

    let (worker_thread, event) = event_receiver.recv()?;
    assert_ne!(worker_thread, main_thread);
    assert_eq!(
        event,
        RasterArtifactPreparationEvent::Completed {
            source_revision: VoxelSceneRevision::new(27),
        }
    );
    let artifact = preparation
        .try_complete()?
        .ok_or("completed preparation did not publish its artifact")?;
    assert_eq!(artifact.source_revision(), VoxelSceneRevision::new(27));
    assert_eq!(artifact.semantic_faces().len(), 6);
    Ok(())
}

#[test]
fn derivation_failure_keeps_phase_and_source_revision_at_the_worker_boundary()
-> Result<(), Box<dyn std::error::Error>> {
    let (event_sender, event_receiver) = mpsc::channel();
    let mut preparation = RasterArtifactPreparation::start(
        published_view()?,
        VoxelVolumeId::new("missing"),
        None,
        move |event| {
            if event_sender.send(event).is_err() {
                eprintln!("preparation test event receiver closed");
            }
        },
    )?;

    assert_eq!(
        event_receiver.recv()?,
        RasterArtifactPreparationEvent::Completed {
            source_revision: VoxelSceneRevision::new(27),
        }
    );
    let error = preparation
        .try_complete()
        .expect_err("missing volume should fail background derivation");
    assert_eq!(error.source_revision(), VoxelSceneRevision::new(27));
    assert!(error.to_string().contains("background derivation"));
    assert!(error.to_string().contains("metadata"));
    assert!(error.to_string().contains("VoxelSceneRevision(27)"));
    Ok(())
}

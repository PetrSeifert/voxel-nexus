use raster_render_path::{
    RasterConvergence, RasterConvergenceEvent, RasterRenderPath, derive_raster_regions,
};
use std::time::{Duration, Instant};
use voxel_frontend::*;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let atomic = !std::env::args().any(|argument| argument == "--single");
    for tier in [StorageTier::Dense, StorageTier::SparsePages] {
        let frontend = VoxelFrontend::new();
        let extent = VoxelExtent::new(256, 128, 256);
        let volume = VoxelVolumeId::new("brush");
        let material = VoxelMaterialId::new("stone");
        let initial = frontend.publish(
            DenseVoxelScene::new(
                VoxelSceneId::new("brush"),
                VoxelSceneRevision::new(0),
                vec![VoxelMaterial::new(material.clone(), [1.0; 4])],
                vec![DenseVoxelVolume::new(
                    VoxelVolumeMetadata::new(volume.clone(), extent, [0.0; 3], 1.0),
                    vec![DenseVoxelBatch::new(
                        VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), extent),
                        vec![VoxelValue::Empty; 256 * 128 * 256],
                    )],
                )],
            )
            .with_storage_tier(tier),
        )?;
        let mut path = RasterRenderPath::new();
        path.install_artifact(derive_raster_regions(
            &initial,
            VoxelExtent::new(16, 16, 16),
        )?);
        let mut convergence = RasterConvergence::from_visible(&path)?;
        let edits: Vec<_> = (0..1000)
            .map(|index| {
                VoxelEdit::new(
                    volume.clone(),
                    VoxelCoordinate::new(index % 10, index / 10 % 10, index / 100),
                    VoxelValue::Occupied(material.clone()),
                )
            })
            .collect();
        let commands = if atomic {
            vec![VoxelEditCommand::from_edits(edits)]
        } else {
            edits
                .into_iter()
                .map(|edit| VoxelEditCommand::from_edits(vec![edit]))
                .collect()
        };
        let mut ready_revision = None;
        let mut edit_time = Duration::ZERO;
        let mut starts = 0;
        let started = Instant::now();
        for command in commands {
            let edit_started = Instant::now();
            let outcome = frontend.edit(command)?;
            edit_time += edit_started.elapsed();
            convergence.accept(outcome)?;
            let events = convergence.drain_events()?;
            starts += events
                .iter()
                .filter(|event| matches!(event, RasterConvergenceEvent::PreparationStarted { .. }))
                .count();
            for event in events {
                if let RasterConvergenceEvent::PreparationReady { revision } = event {
                    ready_revision = Some(revision);
                }
            }
        }
        let submission_time = started.elapsed();
        let deadline = Instant::now() + Duration::from_secs(60);
        while ready_revision != Some(convergence.required_revision()) {
            let events = convergence.drain_events()?;
            starts += events
                .iter()
                .filter(|event| matches!(event, RasterConvergenceEvent::PreparationStarted { .. }))
                .count();
            if events.iter().any(|event| matches!(event, RasterConvergenceEvent::PreparationReady { revision } if *revision == convergence.required_revision())) { break; }
            if Instant::now() >= deadline {
                return Err("brush convergence timed out".into());
            }
            std::thread::yield_now();
        }
        println!(
            "atomic={atomic} tier={tier:?} revisions={} preparation_starts={starts} restarts={} edit_ms={:.3} submission_ms={:.3}",
            frontend.scene_view()?.revision(),
            starts.saturating_sub(1),
            edit_time.as_secs_f64() * 1000.0,
            submission_time.as_secs_f64() * 1000.0
        );
    }
    Ok(())
}

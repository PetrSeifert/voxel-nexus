#[path = "support/allocation.rs"]
mod allocation;
use allocation::{LIVE, PEAK};
use canonical_scene::{CanonicalSceneScale, generate_canonical_scene};
use std::sync::atomic::Ordering;
use voxel_frontend::VoxelFrontend;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("scale,input_bytes,retained_scene_bytes,publication_peak_bytes");
    for scale in [
        CanonicalSceneScale::Small,
        CanonicalSceneScale::Medium,
        CanonicalSceneScale::Large,
    ] {
        let baseline = LIVE.load(Ordering::SeqCst);
        let scene = generate_canonical_scene(scale)?.into_scene();
        let input = LIVE.load(Ordering::SeqCst) - baseline;
        PEAK.store(LIVE.load(Ordering::SeqCst), Ordering::SeqCst);
        let frontend = VoxelFrontend::new();
        let view = frontend.publish_sparse(scene)?;
        let retained = LIVE.load(Ordering::SeqCst) - baseline;
        let peak = PEAK.load(Ordering::SeqCst) - baseline;
        println!("{},{input},{retained},{peak}", 64 * scale.factor());
        drop(view);
        drop(frontend);
    }
    Ok(())
}

use canonical_scene::{CanonicalSceneScale, generate_canonical_scene};
use raster_render_path::derive_raster_regions;
use std::{hint::black_box, time::Instant};
use voxel_frontend::VoxelExtent;
use voxel_frontend::VoxelFrontend;

#[test]
#[ignore = "manual release-mode timing"]
fn region_read_timing() -> Result<(), Box<dyn std::error::Error>> {
    for scale in [CanonicalSceneScale::Small, CanonicalSceneScale::Large] {
        let view = VoxelFrontend::new().publish(generate_canonical_scene(scale)?.into_scene())?;
        for _ in 0..3 {
            black_box(derive_raster_regions(&view, VoxelExtent::new(16, 16, 16))?);
        }
        let mut durations = Vec::new();
        for _ in 0..15 {
            let start = Instant::now();
            let result = derive_raster_regions(&view, VoxelExtent::new(16, 16, 16))?;
            durations.push(start.elapsed());
            black_box(result);
        }
        durations.sort();
        let median = durations.get(7).ok_or("missing timing sample")?;
        println!("{scale:?}: median={median:?}, samples={durations:?}");
    }
    Ok(())
}

use super::*;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use voxel_frontend::{
    DenseVoxelBatch, DenseVoxelScene, DenseVoxelVolume, VoxelEditCommand, VoxelFrontend,
    VoxelMaterial,
};

thread_local! {
    static ALLOCATED_BYTES: Cell<Option<usize>> = const { Cell::new(None) };
}

struct MeasuringAllocator;

#[global_allocator]
static ALLOCATOR: MeasuringAllocator = MeasuringAllocator;

fn record_allocation(bytes: usize) {
    ALLOCATED_BYTES.with(|allocated| {
        if let Some(previous) = allocated.get() {
            allocated.set(Some(previous.saturating_add(bytes)));
        }
    });
}

// The wrapper preserves System's pointer and layout contract and records only this thread.
unsafe impl GlobalAlloc for MeasuringAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            record_allocation(layout.size());
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) };
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() {
            record_allocation(layout.size());
        }
        pointer
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let pointer = unsafe { System.realloc(pointer, layout, new_size) };
        if !pointer.is_null() {
            record_allocation(new_size);
        }
        pointer
    }
}

struct AllocationMeasurement;

impl AllocationMeasurement {
    fn start() -> Self {
        ALLOCATED_BYTES.with(|allocated| allocated.set(Some(0)));
        Self
    }

    fn finish(self) -> usize {
        ALLOCATED_BYTES.with(|allocated| allocated.take().unwrap_or_default())
    }
}

impl Drop for AllocationMeasurement {
    fn drop(&mut self) {
        ALLOCATED_BYTES.with(|allocated| allocated.set(None));
    }
}

fn populated_frontend(side: i32) -> Result<VoxelFrontend, Box<dyn std::error::Error>> {
    let extent = VoxelExtent::new(128, 16, 16);
    let mut values = Vec::new();
    for coordinate_z in 0..16 {
        for coordinate_y in 0..16 {
            for coordinate_x in 0..128 {
                let local_x = coordinate_x % 16;
                let occupied = coordinate_x >= 16
                    && local_x < side
                    && coordinate_y < side
                    && coordinate_z < side
                    && (local_x + coordinate_y + coordinate_z) % 2 == 0;
                values.push(if occupied {
                    VoxelValue::Occupied(VoxelMaterialId::new("stone"))
                } else {
                    VoxelValue::Empty
                });
            }
        }
    }
    let frontend = VoxelFrontend::new();
    frontend.publish(DenseVoxelScene::new(
        VoxelSceneId::new("installation-measurement"),
        VoxelSceneRevision::new(1),
        vec![VoxelMaterial::new(VoxelMaterialId::new("stone"), [1.0; 4])],
        vec![DenseVoxelVolume::new(
            VoxelVolumeMetadata::new(VoxelVolumeId::new("terrain"), extent, [0.0; 3], 1.0),
            vec![DenseVoxelBatch::new(
                VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), extent),
                values,
            )],
        )],
    ))?;
    Ok(frontend)
}

#[test]
fn localized_installation_shares_geometry_and_allocates_independently_of_face_count()
-> Result<(), Box<dyn std::error::Error>> {
    let mut previous_allocation = None;
    for side in [4, 8, 16] {
        let frontend = populated_frontend(side)?;
        let mut render_path = RasterRenderPath::new();
        render_path.install_artifact(derive_raster_regions(
            &frontend.scene_view()?,
            VoxelExtent::new(16, 16, 16),
        )?);
        let mut convergence = RasterConvergence::from_visible(&render_path)?;
        let mut samples = Vec::new();
        for sample in 0..21 {
            let before = render_path
                .installed_artifact()
                .ok_or("missing artifact")?
                .clone();
            convergence.accept(frontend.edit(VoxelEditCommand::new(
                VoxelVolumeId::new("terrain"),
                VoxelCoordinate::new(2, 2, 2),
                if sample % 2 == 0 {
                    VoxelValue::Occupied(VoxelMaterialId::new("stone"))
                } else {
                    VoxelValue::Empty
                },
            ))?)?;
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                convergence.poll_preparation()?;
                if matches!(
                    convergence.active.as_ref().map(|active| &active.status),
                    Some(RasterActivePreparationStatus::Ready { .. })
                ) {
                    break;
                }
                if Instant::now() >= deadline {
                    return Err("preparation did not become ready".into());
                }
                thread::yield_now();
            }
            let prepared = match &convergence
                .active
                .as_ref()
                .ok_or("missing preparation")?
                .status
            {
                RasterActivePreparationStatus::Ready { regions } => regions.clone(),
                _ => return Err("preparation is not ready".into()),
            };
            assert_eq!(prepared.len(), 1);
            let measurement = AllocationMeasurement::start();
            let started = Instant::now();
            let upload = convergence.upload_ready_with_optional_device(None, &render_path)?;
            let commit = convergence.commit_at_frame_boundary(&mut render_path)?;
            let elapsed = started.elapsed();
            let allocated_bytes = measurement.finish();
            assert!(matches!(upload, RasterConvergenceUpload::Uploaded { .. }));
            assert!(matches!(commit, RasterConvergenceCommit::Committed { .. }));
            let after = render_path
                .installed_artifact()
                .ok_or("missing successor")?;
            assert_eq!(after.source_revision(), frontend.scene_view()?.revision());
            let mut unchanged_count = 0;
            for region in after.regions() {
                let source = prepared
                    .iter()
                    .chain(before.regions())
                    .find(|source| source.identity() == region.identity())
                    .ok_or("missing source region")?;
                assert!(Arc::ptr_eq(&source.geometry, &region.geometry));
                if region.identity().core_origin() != VoxelCoordinate::new(0, 0, 0) {
                    unchanged_count += 1;
                }
            }
            assert_eq!(unchanged_count, 7);
            if sample > 0 {
                samples.push(elapsed.as_nanos());
                if let Some(previous) = previous_allocation {
                    assert_eq!(allocated_bytes, previous);
                }
                previous_allocation = Some(allocated_bytes);
            }
        }
        samples.sort_unstable();
        let artifact = render_path.installed_artifact().ok_or("missing artifact")?;
        let geometry_bytes = artifact.vertex_byte_size()
            + artifact.index_byte_size()
            + artifact.semantic_face_count() * size_of::<SemanticFace>();
        println!(
            "side={side} regions=8 affected=1 faces={} geometry_bytes={geometry_bytes} geometry_bytes_copied=0 allocated_bytes={} median_ns={} min_ns={} max_ns={}",
            artifact.semantic_face_count(),
            previous_allocation.ok_or("missing allocation sample")?,
            samples.get(samples.len() / 2).ok_or("missing median")?,
            samples.first().ok_or("missing minimum")?,
            samples.last().ok_or("missing maximum")?
        );
    }
    Ok(())
}

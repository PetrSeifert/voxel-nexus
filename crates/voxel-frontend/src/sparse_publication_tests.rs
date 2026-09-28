use std::alloc::{GlobalAlloc, Layout, System};
use std::error::Error;
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use super::storage_counters::count_storage_work;
use super::*;

/// Counts live heap bytes and refuses allocations past `LIMIT`. The limit stays unbounded
/// unless a harness child process sets it, so other tests in this binary are never refused.
struct LimitingAllocator;

static ALLOCATED_BYTES: AtomicUsize = AtomicUsize::new(0);
static LIMIT_BYTES: AtomicUsize = AtomicUsize::new(usize::MAX);

#[global_allocator]
static ALLOCATOR: LimitingAllocator = LimitingAllocator;

impl LimitingAllocator {
    fn reserve(size: usize) -> bool {
        let limit = LIMIT_BYTES.load(Ordering::Relaxed);
        ALLOCATED_BYTES
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |allocated| {
                allocated.checked_add(size).filter(|total| *total <= limit)
            })
            .is_ok()
    }

    fn release(size: usize) {
        ALLOCATED_BYTES.fetch_sub(size, Ordering::Relaxed);
    }
}

unsafe impl GlobalAlloc for LimitingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if !Self::reserve(layout.size()) {
            return std::ptr::null_mut();
        }
        let pointer = unsafe { System.alloc(layout) };
        if pointer.is_null() {
            Self::release(layout.size());
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) };
        Self::release(layout.size());
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let growth = new_size.saturating_sub(layout.size());
        if !Self::reserve(growth) {
            return std::ptr::null_mut();
        }
        let reallocated = unsafe { System.realloc(pointer, layout, new_size) };
        if reallocated.is_null() {
            Self::release(growth);
        } else {
            Self::release(layout.size().saturating_sub(new_size));
        }
        reallocated
    }
}

const MEMORY_LIMIT_VARIABLE: &str = "VOXEL_FRONTEND_HARNESS_MEMORY_LIMIT_BYTES";
const HARNESS_MEMORY_LIMIT_BYTES: usize = 64 * 1024 * 1024;
const HARNESS_TIMEOUT: Duration = Duration::from_secs(60);
const HUGE_AXIS: u32 = 1 << 22;

fn stone() -> VoxelValue {
    VoxelValue::Occupied(VoxelMaterialId::new("stone"))
}

fn grass() -> VoxelValue {
    VoxelValue::Occupied(VoxelMaterialId::new("grass"))
}

fn region(origin: [i32; 3], extent: [u32; 3]) -> VoxelRegion {
    let [x, y, z] = origin;
    let [width, height, depth] = extent;
    VoxelRegion::new(
        VoxelCoordinate::new(x, y, z),
        VoxelExtent::new(width, height, depth),
    )
}

fn fill(origin: [i32; 3], extent: [u32; 3], value: VoxelValue) -> SparseVoxelBatch {
    SparseVoxelBatch::Fill(VoxelRegionFill::new(region(origin, extent), value))
}

fn detail(origin: [i32; 3], extent: [u32; 3], values: Vec<VoxelValue>) -> SparseVoxelBatch {
    SparseVoxelBatch::Detail(DenseVoxelBatch::new(region(origin, extent), values))
}

fn volume_identity() -> VoxelVolumeId {
    VoxelVolumeId::new("volume")
}

fn sparse_scene(
    extent: [u32; 3],
    batches: Vec<SparseVoxelBatch>,
    storage_tier: StorageTier,
) -> SparseVoxelScene {
    let [width, height, depth] = extent;
    SparseVoxelScene::new(
        VoxelSceneId::new("sparse"),
        VoxelSceneRevision::new(0),
        vec![
            VoxelMaterial::new(VoxelMaterialId::new("stone"), [1.0; 4]),
            VoxelMaterial::new(VoxelMaterialId::new("grass"), [1.0; 4]),
        ],
        vec![SparseVoxelVolume::new(
            VoxelVolumeMetadata::new(
                volume_identity(),
                VoxelExtent::new(width, height, depth),
                [0.0; 3],
                1.0,
            ),
            SparseVoxelBackground::Empty,
            batches,
        )],
    )
    .with_storage_tier(storage_tier)
}

fn small_content() -> Vec<SparseVoxelBatch> {
    vec![
        fill([0, 0, 0], [32, 16, 16], stone()),
        detail(
            [40, 3, 5],
            [2, 2, 2],
            vec![
                stone(),
                VoxelValue::Empty,
                grass(),
                stone(),
                VoxelValue::Empty,
                grass(),
                stone(),
                VoxelValue::Empty,
            ],
        ),
        fill([64, 0, 0], [192, 256, 256], VoxelValue::Empty),
    ]
}

#[test]
fn sparse_to_sparse_publication_work_follows_the_content_rather_than_the_extent()
-> Result<(), Box<dyn Error>> {
    let frontend = VoxelFrontend::new();
    let (view, counters) = count_storage_work(|| {
        frontend.publish_sparse(sparse_scene(
            [256; 3],
            small_content(),
            StorageTier::SparsePages,
        ))
    });
    let view = view?;

    assert_eq!(counters.validation.batches_validated, 3);
    assert_eq!(counters.validation.voxel_values_validated, 8);
    assert_eq!(counters.validation.candidate_pairs_examined, 0);
    assert_eq!(counters.publication.bricks_visited, 3);
    assert_eq!(counters.publication.staged_values_allocated, 16 * 16 * 16);
    assert_eq!(counters.publication.voxel_values_written, 8);
    assert_eq!(
        view.region_content(&volume_identity(), region([16, 0, 0], [16, 16, 16]))?,
        VoxelRegionContent::Uniform(stone())
    );
    assert_eq!(
        view.read_region(&volume_identity(), region([40, 4, 6], [2, 1, 1]))?
            .into_iter()
            .map(|sample| sample.value().clone())
            .collect::<Vec<_>>(),
        vec![stone(), VoxelValue::Empty]
    );
    Ok(())
}

#[test]
fn sparse_to_dense_publication_stages_the_full_volume() -> Result<(), Box<dyn Error>> {
    let frontend = VoxelFrontend::new();
    let (view, counters) = count_storage_work(|| {
        frontend.publish_sparse(sparse_scene([256; 3], small_content(), StorageTier::Dense))
    });
    view?;

    assert_eq!(counters.publication.bricks_visited, 0);
    assert_eq!(
        counters.publication.staged_values_allocated,
        256 * 256 * 256
    );
    assert_eq!(counters.validation.batches_validated, 3);
    Ok(())
}

#[test]
fn overlap_validation_sweeps_a_horizontal_axis_across_terrain_columns() -> Result<(), Box<dyn Error>>
{
    let tiles_per_axis = 8;
    let batches: Vec<_> = (0..tiles_per_axis)
        .flat_map(|tile_z| (0..tiles_per_axis).map(move |tile_x| (tile_x * 16, tile_z * 16)))
        .flat_map(|(x, z)| {
            [
                fill([x, 0, z], [16, 8, 16], stone()),
                fill([x, 8, z], [16, 8, 16], VoxelValue::Empty),
            ]
        })
        .collect();
    let batch_count = batches.len();
    let frontend = VoxelFrontend::new();
    let (view, counters) = count_storage_work(|| {
        frontend.publish_sparse(sparse_scene(
            [128, 16, 128],
            batches,
            StorageTier::SparsePages,
        ))
    });
    view?;

    // Batches in one tile column share their x projection: 8 columns of 16 batches.
    let x_projection_pairs = 8 * (16 * 15 / 2);
    assert_eq!(
        counters.validation.candidate_pairs_examined,
        x_projection_pairs
    );
    assert!(x_projection_pairs < batch_count * (batch_count - 1) / 2);
    Ok(())
}

#[test]
fn huge_extent_publication_stays_within_time_and_memory_limits() -> Result<(), Box<dyn Error>> {
    let mut child = Command::new(std::env::current_exe()?)
        .args([
            "--exact",
            "sparse_publication_tests::huge_extent_publication_child",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(
            MEMORY_LIMIT_VARIABLE,
            HARNESS_MEMORY_LIMIT_BYTES.to_string(),
        )
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    // Drain the pipes while waiting so a chatty child cannot block on a full pipe and be
    // mistaken for a timeout.
    let stdout_reader = drain(child.stdout.take().ok_or("missing child stdout")?);
    let stderr_reader = drain(child.stderr.take().ok_or("missing child stderr")?);
    let deadline = Instant::now() + HARNESS_TIMEOUT;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill()?;
            child.wait()?;
            return Err(format!("harness child exceeded {HARNESS_TIMEOUT:?}").into());
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let stdout = stdout_reader
        .join()
        .map_err(|_| "child stdout reader panicked")??;
    let stderr = stderr_reader
        .join()
        .map_err(|_| "child stderr reader panicked")??;
    assert!(
        status.success() && stdout.contains("1 passed"),
        "harness child failed with {status}:\n{stdout}\n{stderr}"
    );
    Ok(())
}

fn drain(mut pipe: impl Read + Send + 'static) -> std::thread::JoinHandle<std::io::Result<String>> {
    std::thread::spawn(move || {
        let mut output = Vec::new();
        pipe.read_to_end(&mut output)?;
        Ok(String::from_utf8_lossy(&output).into_owned())
    })
}

#[test]
#[ignore = "run in a memory-limited subprocess by huge_extent_publication_stays_within_time_and_memory_limits"]
fn huge_extent_publication_child() -> Result<(), Box<dyn Error>> {
    if let Ok(limit) = std::env::var(MEMORY_LIMIT_VARIABLE) {
        let limit: usize = limit.parse()?;
        LIMIT_BYTES.store(
            ALLOCATED_BYTES
                .load(Ordering::Relaxed)
                .saturating_add(limit),
            Ordering::Relaxed,
        );
    }
    let far = i32::try_from(HUGE_AXIS)? - 2;
    let batches = || {
        vec![
            fill([0, 0, 0], [16, 16, 16], stone()),
            fill([100, 0, 0], [3, 5, 7], grass()),
            detail(
                [far, far, far],
                [2, 2, 2],
                vec![
                    stone(),
                    VoxelValue::Empty,
                    grass(),
                    stone(),
                    VoxelValue::Empty,
                    grass(),
                    stone(),
                    VoxelValue::Empty,
                ],
            ),
            fill(
                [0, 0, 16],
                [HUGE_AXIS, HUGE_AXIS, HUGE_AXIS - 32],
                VoxelValue::Empty,
            ),
        ]
    };
    let frontend = VoxelFrontend::new();
    let (view, counters) = count_storage_work(|| {
        frontend.publish_sparse(sparse_scene(
            [HUGE_AXIS; 3],
            batches(),
            StorageTier::SparsePages,
        ))
    });
    let view = view?;

    assert_eq!(counters.validation.batches_validated, 4);
    assert_eq!(counters.validation.voxel_values_validated, 8);
    assert_eq!(counters.validation.candidate_pairs_examined, 1);
    assert_eq!(counters.publication.bricks_visited, 3);
    assert_eq!(
        counters.publication.staged_values_allocated,
        2 * 16 * 16 * 16
    );
    assert_eq!(counters.publication.voxel_values_written, 3 * 5 * 7 + 8);
    assert_eq!(
        view.region_content(&volume_identity(), region([0, 0, 0], [16, 16, 16]))?,
        VoxelRegionContent::Uniform(stone())
    );
    assert_eq!(
        view.region_content(&volume_identity(), region([100, 0, 0], [3, 5, 7]))?,
        VoxelRegionContent::Uniform(grass())
    );
    assert_eq!(
        view.region_content(&volume_identity(), region([96, 0, 0], [16, 16, 16]))?,
        VoxelRegionContent::Mixed
    );
    assert_eq!(
        view.region_content(
            &volume_identity(),
            region([1 << 20, 1 << 20, 1 << 20], [16, 16, 16])
        )?,
        VoxelRegionContent::Uniform(VoxelValue::Empty)
    );
    assert_eq!(
        view.read_region(&volume_identity(), region([far; 3], [2, 2, 2]))?
            .into_iter()
            .map(|sample| sample.value().clone())
            .collect::<Vec<_>>(),
        vec![
            stone(),
            VoxelValue::Empty,
            grass(),
            stone(),
            VoxelValue::Empty,
            grass(),
            stone(),
            VoxelValue::Empty,
        ]
    );

    assert!(matches!(
        frontend.publish_sparse(sparse_scene([HUGE_AXIS; 3], batches(), StorageTier::Dense)),
        Err(VoxelFrontendError::VolumeTooLarge { identity }) if identity == volume_identity()
    ));
    Ok(())
}

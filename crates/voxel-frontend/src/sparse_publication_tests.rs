use std::alloc::{GlobalAlloc, Layout, System};
use std::error::Error;
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use super::storage_counters::{
    EnumerationWorkCounters, PublicationWorkCounters, ValidationWorkCounters, count_storage_work,
};
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

// SAFETY: Every call either refuses with null or forwards the caller's pointer and layout
// unchanged to System, and the counters never allocate.
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
const CELL_EDGE: u32 = 64;

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
fn validation_allocation_follows_input_batches_without_following_volume_extent()
-> Result<(), Box<dyn Error>> {
    let mut previous: Option<(usize, (usize, usize))> = None;
    for batch_count in [8_usize, 128, 4096] {
        let mut allocations = Vec::new();
        for extent in [[8192; 3], [HUGE_AXIS; 3]] {
            let batches = (0..batch_count)
                .map(|x| {
                    fill(
                        [i32::try_from(x * 2).unwrap(), 0, 0],
                        [1; 3],
                        VoxelValue::Empty,
                    )
                })
                .collect();
            let (view, counters) = count_storage_work(|| {
                VoxelFrontend::new().publish_sparse(sparse_scene(
                    extent,
                    batches,
                    StorageTier::SparsePages,
                ))
            });
            view?;
            assert_eq!(counters.validation.batches_validated, batch_count);
            assert_eq!(counters.publication.staged_values_allocated, 0);
            let validation = counters.validation;
            assert!(validation.allocated_bytes > 0);
            assert!(validation.peak_working_bytes > 0);
            assert!(validation.peak_working_bytes <= validation.allocated_bytes);
            allocations.push((validation.allocated_bytes, validation.peak_working_bytes));
        }
        assert_eq!(allocations.first(), allocations.get(1));
        let allocated = allocations.first().copied().ok_or("missing measurement")?;
        if let Some((previous_count, previous_allocated)) = previous {
            assert_eq!(
                allocated.0 * previous_count,
                previous_allocated.0 * batch_count
            );
            assert_eq!(
                allocated.1 * previous_count,
                previous_allocated.1 * batch_count
            );
        }
        previous = Some((batch_count, allocated));
    }
    Ok(())
}

#[test]
fn huge_extent_scene_stays_within_time_and_memory_limits() -> Result<(), Box<dyn Error>> {
    let mut child = Command::new(std::env::current_exe()?)
        .args([
            "--exact",
            "sparse_publication_tests::huge_extent_scene_child",
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
#[ignore = "run in a memory-limited subprocess by huge_extent_scene_stays_within_time_and_memory_limits"]
fn huge_extent_scene_child() -> Result<(), Box<dyn Error>> {
    if let Ok(limit) = std::env::var(MEMORY_LIMIT_VARIABLE) {
        let limit: usize = limit.parse()?;
        LIMIT_BYTES.store(
            ALLOCATED_BYTES
                .load(Ordering::Relaxed)
                .saturating_add(limit),
            Ordering::Relaxed,
        );
    }
    let empty_frontend = VoxelFrontend::new();
    let empty = empty_frontend.publish_sparse(sparse_scene(
        [HUGE_AXIS; 3],
        vec![],
        StorageTier::SparsePages,
    ))?;
    let whole_volume = region([0; 3], [HUGE_AXIS; 3]);
    let (content, classification) =
        count_storage_work(|| empty.region_content(&volume_identity(), whole_volume));
    assert_eq!(content?, VoxelRegionContent::Uniform(VoxelValue::Empty));
    assert_eq!(classification.bricks_examined, 0);
    assert_eq!(classification.voxel_values_examined, 0);
    assert_eq!(classification.classification_nodes_visited, 1);
    let edited = empty_frontend.edit(VoxelEditCommand::from_edits(vec![VoxelEdit::new(
        volume_identity(),
        VoxelCoordinate::new(0, 0, 0),
        stone(),
    )]))?;
    let (content, classification) = count_storage_work(|| {
        edited
            .view()
            .region_content(&volume_identity(), whole_volume)
    });
    assert_eq!(content?, VoxelRegionContent::Mixed);
    assert_eq!(classification.bricks_examined, 1);
    assert!(classification.voxel_values_examined <= 2);
    assert!(classification.classification_nodes_visited <= 9);
    let (content, classification) = count_storage_work(|| {
        edited
            .view()
            .region_content(&volume_identity(), region([1 << 21; 3], [1 << 21; 3]))
    });
    assert_eq!(content?, VoxelRegionContent::Uniform(VoxelValue::Empty));
    assert_eq!(classification.bricks_examined, 0);
    assert_eq!(classification.voxel_values_examined, 0);
    assert!(classification.classification_nodes_visited <= 9);
    assert!(matches!(
        empty.read_region(&volume_identity(), whole_volume),
        Err(VoxelFrontendError::InvalidRegionBounds { identity }) if identity == volume_identity()
    ));
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
    assert!(counters.validation.allocated_bytes > 0);
    assert!(counters.validation.allocated_bytes < 4096, "{counters:?}");
    assert!(counters.validation.peak_working_bytes > 0);
    assert!(
        counters.validation.peak_working_bytes < 4096,
        "{counters:?}"
    );
    assert_eq!(counters.publication.bricks_visited, 3);
    assert_eq!(
        counters.publication.staged_values_allocated,
        2 * 16 * 16 * 16
    );
    assert_eq!(counters.publication.voxel_values_written, 3 * 5 * 7 + 8);
    for edge in [256, 1 << 12, HUGE_AXIS] {
        let (content, classification) = count_storage_work(|| {
            view.region_content(&volume_identity(), region([0; 3], [edge; 3]))
        });
        assert_eq!(content?, VoxelRegionContent::Mixed);
        assert!(classification.bricks_examined <= 3, "{classification:?}");
        assert!(classification.classification_nodes_visited <= 1 + 3 * 8);
        assert!(classification.voxel_values_examined <= 16 * 16 * 16);
    }
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

    let volume = volume_identity();
    for (cell_edge, expected_cells) in [
        (1, 16 * 16 * 16 + 3 * 5 * 7 + 5),
        (CELL_EDGE, 3),
        (HUGE_AXIS, 1),
    ] {
        let enumeration = view.enumerate_cells(&volume, cell_edge, 256)?;
        let (cells, counters) = count_storage_work(|| enumeration.collect::<Result<Vec<_>, _>>());
        let cells = cells?.concat();
        assert_eq!(cells.len(), expected_cells, "cell edge {cell_edge}");
        assert_eq!(counters.enumeration.bricks_examined, 3);
        assert_eq!(counters.enumeration.cells_emitted, expected_cells);
        assert_eq!(counters.publication, PublicationWorkCounters::default());
        assert_eq!(counters.validation, ValidationWorkCounters::default());
        assert_eq!(
            (counters.copied_nodes, counters.copied_brick_payloads),
            (0, 0)
        );
        for cell in &cells {
            assert_eq!(
                &view.region_content(&volume, cell.region())?,
                cell.content()
            );
        }
    }

    // Occupy empty space and break the uniform brick in one command.
    let (outcome, edit) = count_storage_work(|| {
        frontend.edit(VoxelEditCommand::from_edits(vec![
            VoxelEdit::new(volume.clone(), VoxelCoordinate::new(200, 0, 0), stone()),
            VoxelEdit::new(
                volume.clone(),
                VoxelCoordinate::new(5, 5, 5),
                VoxelValue::Empty,
            ),
        ]))
    });
    let outcome = outcome?;
    let change_set = outcome.change_set().ok_or("edit changed nothing")?;
    assert_eq!(edit.copied_brick_payloads, 2);
    // A 2^54-brick key space gives a page table of depth 9, and each edited brick copies at
    // most the nodes on its own path.
    assert!(edit.copied_nodes <= 2 * 9, "{edit:?}");
    assert_eq!(edit.enumeration, EnumerationWorkCounters::default());
    assert_eq!(edit.publication, PublicationWorkCounters::default());
    assert_eq!(edit.validation, ValidationWorkCounters::default());

    let edge = i64::from(CELL_EDGE);
    let mut changed_cells = Vec::new();
    for changed in change_set.changed_regions() {
        let changed = changed.region();
        let [start_x, start_y, start_z] = changed.origin().components().map(i64::from);
        let [width, height, depth] = changed.extent().dimensions().map(i64::from);
        for z in start_z / edge..=(start_z + depth - 1) / edge {
            for y in start_y / edge..=(start_y + height - 1) / edge {
                for x in start_x / edge..=(start_x + width - 1) / edge {
                    if !changed_cells.contains(&[x, y, z]) {
                        changed_cells.push([x, y, z]);
                    }
                }
            }
        }
    }
    changed_cells.sort_unstable();
    assert_eq!(changed_cells, [[0, 0, 0], [3, 0, 0]]);
    let edited = outcome.view();
    let (contents, reclassification) = count_storage_work(|| {
        changed_cells
            .iter()
            .map(|cell| {
                let [x, y, z] = cell.map(|index| i32::try_from(index * edge));
                Ok::<_, Box<dyn Error>>(
                    edited.region_content(&volume, region([x?, y?, z?], [CELL_EDGE; 3]))?,
                )
            })
            .collect::<Result<Vec<_>, _>>()
    });
    assert_eq!(
        contents?,
        [VoxelRegionContent::Mixed, VoxelRegionContent::Mixed]
    );
    assert!(reclassification.bricks_examined <= 2 * 4 * 4 * 4);
    assert_eq!(
        reclassification.enumeration,
        EnumerationWorkCounters::default()
    );
    let (cells, enumeration) = count_storage_work(|| {
        edited
            .enumerate_cells(&volume, CELL_EDGE, 256)?
            .collect::<Result<Vec<_>, _>>()
    });
    assert_eq!(cells?.concat().len(), 4);
    assert_eq!(enumeration.enumeration.bricks_examined, 4);

    assert!(matches!(
        frontend.publish_sparse(sparse_scene([HUGE_AXIS; 3], batches(), StorageTier::Dense)),
        Err(VoxelFrontendError::VolumeTooLarge { identity }) if identity == volume_identity()
    ));
    Ok(())
}

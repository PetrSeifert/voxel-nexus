use super::{
    allocation::{self, Category},
    streamed_fixture_recipe as recipe, streamed_world,
};
use compute_ray_render_path::{ComputeRepresentation, ComputeSceneBundle};
use raster_render_path::{RasterArtifact, derive_raster_residency};
use serde_json::{Value, json};
use std::{fs::File, io::Write, sync::Arc};
use voxel_frontend::{
    VoxelCoordinate, VoxelExtent, VoxelFrontend, VoxelRegion, VoxelResidencySelection,
    VoxelResidencySelectionId, VoxelSceneView, VoxelValue,
};

#[derive(Clone, Debug)]
pub struct Key {
    pub coordinate: (u32, u32),
}

pub const CATEGORIES: [Category; 7] = [
    Category::Control,
    Category::Materialized,
    Category::Generation,
    Category::Raster,
    Category::Brickmap,
    Category::Metadata,
    Category::History,
];

pub fn emit(output: &mut File, value: Value) -> Result<(), String> {
    serde_json::to_writer(&mut *output, &value).map_err(|error| error.to_string())?;
    writeln!(output).map_err(|error| error.to_string())
}

pub fn selection(
    view: &VoxelSceneView,
    identity: u64,
    center: (u32, u32),
    side: u32,
) -> Result<VoxelResidencySelection, String> {
    view.residency_selection(
        VoxelResidencySelectionId::new(identity),
        (center.0.saturating_sub(1)..=(center.0 + 1).min(side - 1)).flat_map(|x| {
            (center.1.saturating_sub(1)..=(center.1 + 1).min(side - 1))
                .map(move |z| recipe::volume_identity(x, z))
        }),
    )
    .map_err(|error| error.to_string())
}

pub fn fingerprint(view: &VoxelSceneView, coordinate: (u32, u32)) -> Result<u64, String> {
    let identity = recipe::volume_identity(coordinate.0, coordinate.1);
    let _query = view
        .enumerate_cells(&identity, 16, 64)
        .map_err(|error| error.to_string())?;
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    // Read one row at a time so query result storage is bounded independently of a volume payload.
    for z in 0..64 {
        for y in 0..64 {
            for sample in view
                .read_region(
                    &identity,
                    VoxelRegion::new(VoxelCoordinate::new(0, y, z), VoxelExtent::new(64, 1, 1)),
                )
                .map_err(|error| error.to_string())?
            {
                let code = match sample.value() {
                    VoxelValue::Empty => 0,
                    VoxelValue::Occupied(material)
                        if material == &voxel_frontend::VoxelMaterialId::new("stone") =>
                    {
                        1
                    }
                    VoxelValue::Occupied(_) => 2,
                };
                hash = (hash ^ code).wrapping_mul(0x100_0000_01b3);
            }
        }
    }
    Ok(hash)
}

pub fn record_fingerprint(
    output: &mut File,
    view: &VoxelSceneView,
    coordinate: (u32, u32),
    phase: &str,
) -> Result<(), String> {
    let fingerprint = fingerprint(view, coordinate)?;
    let expected = if phase == "edited" {
        0x478e_e4a9_737e_1ad7
    } else {
        0x9660_d930_8d69_ede5
    };
    if fingerprint != expected {
        return Err(format!(
            "{phase} frozen fingerprint differs: {fingerprint:016x}"
        ));
    }
    emit(
        output,
        json!({"kind":"fingerprint","phase":phase,"fingerprint":format!("{fingerprint:016x}")}),
    )
}

pub fn publish(side: u32) -> Result<Arc<VoxelFrontend>, String> {
    let frontend = Arc::new(streamed_world::frontend());
    allocation::within(Category::Metadata, || {
        frontend.publish_streamed(streamed_world::scene_with_side(side))
    })
    .map_err(|error| error.to_string())?;
    Ok(frontend)
}

pub fn edit(
    frontend: &VoxelFrontend,
    coordinate: (u32, u32),
    restore: bool,
) -> Result<voxel_frontend::VoxelEditOutcome, String> {
    allocation::within(Category::Metadata, || {
        frontend.edit(recipe::edit(coordinate.0, coordinate.1, restore))
    })
    .map_err(|error| error.to_string())
}

pub fn sample(output: &mut File, frontend: &VoxelFrontend, phase: &str) -> Result<(), String> {
    sample_with_history(output, frontend, phase, 0, 0)
}

pub fn sample_with_history(
    output: &mut File,
    frontend: &VoxelFrontend,
    phase: &str,
    historical_views: usize,
    historical_peak: usize,
) -> Result<(), String> {
    let cache = frontend
        .materialization_cache_stats()
        .map_err(|error| error.to_string())?;
    let edits = frontend
        .streamed_edit_statistics()
        .map_err(|error| error.to_string())?
        .ok_or("not streamed")?;
    let view = frontend.scene_view().map_err(|error| error.to_string())?;
    emit(
        output,
        json!({"kind":"cpu-residency","phase":phase,"copies":cache.copies,"peak_copies":cache.peak_copies,
        "query_copies":cache.query_only_copies,"generating":cache.generating,"cpu_live":CATEGORIES.map(allocation::live),
        "cpu_peak":CATEGORIES.map(allocation::peak),"cpu_allocations":CATEGORIES.map(allocation::count),
        "metadata_entries":view.volumes().len(),"material_count":view.materials().len(),
        "source_recipe_count":streamed_world::FIXTURE_RECIPE_COUNT,
        "live_historical_views":historical_views,"peak_historical_views":historical_peak,
        "edited_coordinates":edits.current_coordinates,"history_entries":edits.retained_entries,"live_versions":edits.live_versions}),
    )
}

pub fn released(output: &mut File) -> Result<(), String> {
    let live = CATEGORIES.map(allocation::live);
    emit(output, json!({"kind":"cpu-released","live": &live[1..]}))?;
    if live[1..].iter().any(|bytes| *bytes != 0) {
        return Err(format!("CPU cleanup debt: {live:?}"));
    }
    Ok(())
}

pub fn representations(
    frontend: &Arc<VoxelFrontend>,
    selection: &VoxelResidencySelection,
) -> Result<(RasterArtifact, ComputeSceneBundle), String> {
    let view = frontend.scene_view().map_err(|error| error.to_string())?;
    let raster = allocation::within(Category::Raster, || {
        derive_raster_residency(
            frontend.clone(),
            &view,
            selection.clone(),
            VoxelExtent::new(16, 16, 16),
        )
    })
    .map_err(|error| error.to_string())?;
    let copies = frontend
        .materialize_residency(selection, &view)
        .map_err(|error| error.to_string())?;
    let compute = allocation::within(Category::Brickmap, || {
        ComputeSceneBundle::from_residency(
            copies,
            ComputeRepresentation::Brickmap {
                budget_bytes: 1 << 30,
            },
        )
    })
    .map_err(|error| error.to_string())?;
    Ok((raster, compute))
}

pub fn establish(
    frontend: &VoxelFrontend,
    selection: VoxelResidencySelection,
) -> Result<(), String> {
    frontend
        .require_residency(selection)
        .map_err(|error| error.to_string())?;
    frontend
        .establish_residency()
        .map_err(|error| error.to_string())?;
    Ok(())
}

pub fn lifecycle(output: &mut File, frontend: &Arc<VoxelFrontend>) -> Result<(), String> {
    let mut history = vec![frontend.scene_view().map_err(|error| error.to_string())?];
    let mut steps = vec!["retain-generated"];
    establish(frontend, selection(&history[0], 1, (3, 3), 16)?)?;
    edit(frontend, (3, 3), false)?;
    steps.push("edit-3-3");
    history.push(frontend.scene_view().map_err(|error| error.to_string())?);
    steps.push("retain-edited");
    let edited = history
        .get(1)
        .expect("the edited historical view was just retained");
    establish(frontend, selection(edited, 2, (3, 3), 16)?)?;
    let unchanged = edited
        .volume_content_version(&recipe::volume_identity(3, 3))
        .map_err(|error| error.to_string())?;
    establish(frontend, selection(edited, 3, (4, 3), 16)?)?;
    steps.push("evict-2-2");
    edit(frontend, (2, 2), false)?;
    steps.push("edit-2-2-nonresident");
    let current = frontend.scene_view().map_err(|error| error.to_string())?;
    let reuse = current
        .volume_content_version(&recipe::volume_identity(3, 3))
        .map_err(|error| error.to_string())?
        == unchanged;
    establish(frontend, selection(&current, 4, (3, 3), 16)?)?;
    steps.push("reload-2-2");
    sample_with_history(
        output,
        frontend,
        "historical-queries",
        history.len(),
        history.len(),
    )?;
    record_fingerprint(output, &current, (2, 2), "edited")?;
    record_fingerprint(output, &history[0], (3, 3), "generated")?;
    steps.push("historical-generated");
    record_fingerprint(output, edited, (3, 3), "edited")?;
    steps.push("historical-edited");
    drop(current);
    edit(frontend, (2, 2), true)?;
    steps.push("restore-2-2");
    edit(frontend, (3, 3), true)?;
    steps.push("restore-3-3");
    history.clear();
    steps.push("drop-history");
    let restored = frontend.scene_view().map_err(|error| error.to_string())?;
    establish(frontend, selection(&restored, 5, (3, 3), 16)?)?;
    record_fingerprint(output, &restored, (2, 2), "restored")?;
    record_fingerprint(output, &restored, (3, 3), "restored")?;
    let compacted = frontend
        .streamed_edit_statistics()
        .map_err(|error| error.to_string())?
        .ok_or("missing edits")?;
    if compacted.current_coordinates != 0 || compacted.retained_entries != 0 || !reuse {
        return Err("history compaction/reuse failed".into());
    }
    steps.push("compact");
    emit(
        output,
        json!({"kind":"compaction","edit_script":steps,"live_historical_views":history.len(),"edited_coordinates":compacted.current_coordinates,"history_entries":compacted.retained_entries,"unchanged_volume_reused":reuse}),
    )?;
    // The cancellation callback runs after each generation admission finishes.
    let skipped = selection(&restored, 6, (8, 8), 16)?;
    let mut calls = 0;
    let cancelled = frontend.materialize_residency_until_cancelled(&skipped, &restored, || {
        calls += 1;
        calls > 2
    });
    if !matches!(
        cancelled,
        Err(voxel_frontend::VoxelFrontendError::ResidencySuperseded)
    ) {
        return Err("superseded selection did not drain".into());
    }
    let newest = selection(&restored, 7, (12, 12), 16)?;
    let copies = frontend
        .materialize_residency(&newest, &restored)
        .map_err(|error| error.to_string())?;
    let query = restored
        .enumerate_cells(&recipe::volume_identity(15, 15), 16, 64)
        .map_err(|error| error.to_string())?;
    let stats = frontend
        .materialization_cache_stats()
        .map_err(|error| error.to_string())?;
    let rejected = restored
        .enumerate_cells(&recipe::volume_identity(14, 15), 16, 64)
        .is_err();
    let copy_cap = restored
        .residency_limits()
        .map_err(|error| error.to_string())?
        .materialization_copy_cap();
    if stats.copies != copy_cap || !rejected {
        return Err(format!("{copy_cap}-copy admission failed: {stats:?}"));
    }
    sample(output, frontend, "disjoint-query-overlap")?;
    drop(query);
    drop(copies);
    for lap in 0..2 {
        for (index, center) in [
            (4, 3),
            (5, 3),
            (6, 4),
            (7, 5),
            (6, 4),
            (5, 3),
            (4, 3),
            (3, 3),
        ]
        .into_iter()
        .enumerate()
        {
            let target = selection(&restored, 8 + lap * 8 + index as u64, center, 16)?;
            let artifacts = representations(frontend, &target)?;
            sample(output, frontend, "pre-retirement")?;
            establish(frontend, target)?;
            drop(artifacts);
        }
        sample(output, frontend, &format!("lap-{lap}-settled"))?;
    }
    emit(
        output,
        json!({"kind":"cpu-lifecycle-result","evicted_edit":true,"historical_reads":true,"restored":true,
        "unrelated_edit_reuse":reuse,"compacted":true,"nineteen_copy_admission":stats.copies==copy_cap && rejected,"repeat_laps":2}),
    )
}

pub fn run_cpu(arguments: &[String]) -> Result<(), String> {
    let [mode, path] = arguments else {
        return Err("usage: streamed-residency-qualification CPU_MODE OUTPUT.jsonl".into());
    };
    allocation::reset_peaks();
    let mut output = File::create(path).map_err(|error| error.to_string())?;
    if mode == "cpu-calibration" {
        for phase in ["generated", "edited", "restored"] {
            let frontend = publish(16)?;
            if phase != "generated" {
                edit(&frontend, (3, 3), false)?;
            }
            if phase == "restored" {
                edit(&frontend, (3, 3), true)?;
            }
            let view = frontend.scene_view().map_err(|error| error.to_string())?;
            let target = view
                .residency_selection(
                    VoxelResidencySelectionId::new(1),
                    [recipe::volume_identity(3, 3)],
                )
                .map_err(|error| error.to_string())?;
            establish(&frontend, target.clone())?;
            let artifacts = representations(&frontend, &target)?;
            record_fingerprint(&mut output, &view, (3, 3), phase)?;
            sample(&mut output, &frontend, phase)?;
            drop(artifacts);
            drop(target);
            drop(view);
            drop(frontend);
            released(&mut output)?;
        }
    } else if mode == "cpu-baseline" {
        let frontend = VoxelFrontend::new();
        let coordinates = (0..16)
            .flat_map(|z| (0..16).map(move |x| (x, z)))
            .collect::<Vec<_>>();
        let source = allocation::within(Category::Generation, || recipe::scene(&coordinates));
        let view = allocation::within(Category::Materialized, || frontend.publish_sparse(source))
            .map_err(|error| error.to_string())?;
        let raster = allocation::within(Category::Raster, || {
            raster_render_path::derive_raster_regions(&view, VoxelExtent::new(16, 16, 16))
        })
        .map_err(|error| error.to_string())?;
        let compute = allocation::within(Category::Brickmap, || {
            ComputeSceneBundle::from_view_with_representation(
                &view,
                ComputeRepresentation::Brickmap {
                    budget_bytes: 1 << 30,
                },
            )
        })
        .map_err(|error| error.to_string())?;
        emit(
            &mut output,
            json!({"kind":"fully-resident-baseline","volumes":256,"cpu_live":CATEGORIES.map(allocation::live),"cpu_peak":CATEGORIES.map(allocation::peak)}),
        )?;
        drop(raster);
        drop(compute);
        drop(view);
        drop(frontend);
        released(&mut output)?;
    } else if mode == "cpu-lifecycle" {
        let frontend = publish(16)?;
        lifecycle(&mut output, &frontend)?;
        drop(frontend);
        released(&mut output)?;
    } else if mode == "cpu-matched-8" || mode == "cpu-matched-16" {
        let side = if mode == "cpu-matched-8" { 8 } else { 16 };
        let frontend = publish(side)?;
        edit(&frontend, (3, 3), false)?;
        let view = frontend.scene_view().map_err(|error| error.to_string())?;
        let target = selection(&view, 1, (3, 3), side)?;
        establish(&frontend, target.clone())?;
        let artifacts = representations(&frontend, &target)?;
        sample(&mut output, &frontend, mode)?;
        drop(artifacts);
        drop(target);
        drop(view);
        drop(frontend);
        released(&mut output)?;
    } else {
        return Err(format!("unknown CPU mode {mode}"));
    }
    emit(
        &mut output,
        json!({"kind":"context","mode":mode,"cpu_only":true,"gpu_dispatched":false,"production":true}),
    )
}

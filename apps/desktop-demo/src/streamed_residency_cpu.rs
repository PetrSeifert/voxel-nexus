use super::{
    allocation::{self, Category},
    streamed_residency_paths::{Artifacts, Kind, Path},
    streamed_residency_route as route,
    streamed_residency_source::{Cache, Key, Snapshot, keys},
};
use render_backend::CameraStateRevision;
use serde_json::json;
use std::{fs::File, io::Write};

fn emit(output: &mut File, value: serde_json::Value) -> Result<(), String> {
    serde_json::to_writer(&mut *output, &value).map_err(|error| error.to_string())?;
    writeln!(output).map_err(|error| error.to_string())
}
fn sample(
    output: &mut File,
    phase: &str,
    cache: &Cache,
    artifacts: &Artifacts,
) -> Result<(), String> {
    let categories = [
        Category::Control,
        Category::Materialized,
        Category::Generation,
        Category::Raster,
        Category::Brickmap,
        Category::Metadata,
        Category::History,
    ];
    emit(
        output,
        json!({"kind":"cpu-residency","phase":phase,"copies":cache.copies(),"peak_copies":cache.peak_copies,"query_copies":cache.query_count(),"generation_workers_peak":cache.generation_peak,"derivation_workers_peak":1,"generated":cache.generated,"reused":cache.reused,"discarded":cache.discarded,"artifact_cache_entries":artifacts.count(),"cpu_live":categories.map(allocation::live),"cpu_peak":categories.map(allocation::peak),"cpu_allocations":categories.map(allocation::count)}),
    )
}
fn build(
    artifacts: &mut Artifacts,
    cache: &Cache,
    source: &Snapshot,
    selection: &[Key],
) -> Result<(Path, Path), String> {
    let camera = route::camera(0.0)?;
    Ok((
        artifacts.build(
            Kind::Raster,
            cache,
            source,
            selection,
            camera,
            CameraStateRevision::new(1),
        )?,
        artifacts.build(
            Kind::Brickmap,
            cache,
            source,
            selection,
            camera,
            CameraStateRevision::new(1),
        )?,
    ))
}
pub fn main(arguments: &[String]) -> Result<(), String> {
    let [mode, output_path] = arguments else {
        return Err("usage: streamed-residency-prototype cpu-calibration|cpu-baseline|cpu-matched-8|cpu-matched-16|cpu-lifecycle OUTPUT.jsonl".into());
    };
    let mut output = File::create(output_path).map_err(|error| error.to_string())?;
    if mode == "cpu-calibration" {
        for phase in ["generated", "edited", "restored"] {
            let mut source = Snapshot::new(16);
            if phase != "generated" {
                source = source.edit((3, 3), false)?;
            }
            if phase == "restored" {
                source = source.edit((3, 3), true)?;
            }
            allocation::reset_peaks();
            let mut cache = Cache::new();
            let mut artifacts = Artifacts::new();
            let selection = vec![Key {
                coordinate: (3, 3),
                version: source.version((3, 3)),
            }];
            cache.ensure(
                &source,
                selection.first().ok_or("empty calibration selection")?,
            )?;
            cache.begin_query(&source, (3, 3))?;
            let fingerprint = cache.verify_query(&source)?;
            cache.end_query();
            let paths = build(&mut artifacts, &cache, &source, &selection)?;
            sample(&mut output, phase, &cache, &artifacts)?;
            emit(
                &mut output,
                json!({"kind":"fingerprint","phase":phase,"fingerprint":format!("{fingerprint:016x}")}),
            )?;
            drop(paths);
            drop(artifacts);
            drop(cache);
            drop(source);
            released(&mut output, phase)?;
        }
        return Ok(());
    }
    let side = if mode == "cpu-matched-8" { 8 } else { 16 };
    let mut source = Snapshot::new(side);
    let baseline = mode == "cpu-baseline";
    if mode.starts_with("cpu-matched") {
        source = source.edit((3, 3), false)?;
    }
    let mut cache = if baseline {
        Cache::with_limit(256)
    } else {
        Cache::new()
    };
    let mut artifacts = Artifacts::new();
    let mut installed = if baseline {
        (0..16)
            .flat_map(|z| {
                (0..16).map(move |x| Key {
                    coordinate: (x, z),
                    version: 1,
                })
            })
            .collect::<Vec<_>>()
    } else {
        keys(&source, (3, 3))
    };
    for key in &installed {
        cache.ensure(&source, key)?;
    }
    let mut paths = build(&mut artifacts, &cache, &source, &installed)?;
    sample(&mut output, mode, &cache, &artifacts)?;
    emit(
        &mut output,
        json!({"kind":"context","mode":mode,"metadata_entries":side*side,"cpu_only":true,"gpu_dispatched":false}),
    )?;
    if mode == "cpu-lifecycle" {
        let generated = source.clone();
        source = source.edit((3, 3), false)?;
        let edited = source.clone();
        source = source.edit((2, 2), false)?;
        for historical in [&generated, &edited] {
            cache.begin_query(historical, (3, 3))?;
            cache.verify_query(historical)?;
            sample(&mut output, "historical-query", &cache, &artifacts)?;
            cache.end_query();
        }
        if source.version((4, 4)) != generated.version((4, 4)) {
            return Err("unrelated volume version changed".into());
        }
        source = source.edit((3, 3), true)?;
        source = source.edit((2, 2), true)?;
        drop(generated);
        drop(edited);
        let skipped = keys(&source, (8, 8));
        cache.ensure(&source, skipped.first().ok_or("empty skipped target")?)?;
        let newest = keys(&source, (12, 12));
        cache.retain(&installed, &newest);
        for key in &newest {
            cache.ensure(&source, key)?;
        }
        cache.begin_query(&source, (15, 15))?;
        cache.verify_query(&source)?;
        let disjoint = build(&mut artifacts, &cache, &source, &newest)?;
        sample(&mut output, "disjoint-query-overlap", &cache, &artifacts)?;
        if cache.copies() != 19 || cache.begin_query(&source, (14, 15)).is_ok() {
            return Err("global query/admission overlap failed".into());
        }
        drop(disjoint);
        cache.end_query();
        cache.retain(&installed, &installed);
        artifacts.retain(&installed, &installed);
        for lap in 0..2 {
            for center in [
                (4, 3),
                (5, 3),
                (6, 4),
                (7, 5),
                (6, 4),
                (5, 3),
                (4, 3),
                (3, 3),
            ] {
                let newest = keys(&source, center);
                cache.retain(&installed, &newest);
                artifacts.retain(&installed, &newest);
                for key in &newest {
                    cache.ensure(&source, key)?;
                }
                let next_paths = build(&mut artifacts, &cache, &source, &newest)?;
                sample(&mut output, "pre-retirement", &cache, &artifacts)?;
                paths = next_paths;
                installed = newest;
                cache.retain(&installed, &installed);
                artifacts.retain(&installed, &installed);
                if cache.copies() != 9 {
                    return Err("settled selection retains extra copies".into());
                }
            }
            sample(
                &mut output,
                &format!("lap-{lap}-settled"),
                &cache,
                &artifacts,
            )?;
        }
        emit(
            &mut output,
            json!({"kind":"cpu-lifecycle-result","historical_views":0,"edited_coordinates":source.overlay_coordinates(),"repeat_laps":2,"coverage_and_latency_measured":false}),
        )?;
    } else if !baseline && !mode.starts_with("cpu-matched") {
        return Err("unknown CPU mode".into());
    }
    drop(paths);
    drop(cache);
    drop(artifacts);
    drop(source);
    released(&mut output, mode)
}
fn released(output: &mut File, phase: &str) -> Result<(), String> {
    let categories = [
        Category::Materialized,
        Category::Generation,
        Category::Raster,
        Category::Brickmap,
        Category::Metadata,
        Category::History,
    ];
    let live = categories.map(allocation::live);
    emit(
        output,
        json!({"kind":"cpu-released","phase":phase,"live":live}),
    )?;
    if live != [0; 6] {
        return Err("CPU qualification cleanup debt remains".into());
    }
    Ok(())
}

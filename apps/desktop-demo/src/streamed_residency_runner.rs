use super::{
    allocation::{self, Category},
    streamed_residency_paths::{Artifacts, Boundary, Install, Kind, Shared},
    streamed_residency_route as route,
    streamed_residency_source::{Cache, Key, Snapshot, keys},
    windows_adapter,
};
use ash::vk;
use compute_ray_render_path::{ComputeRepresentation, ComputeSceneBuildError, ComputeSceneBundle};
use render_backend::{
    CameraState, CameraStateRevision, GpuAllocationQualification, RenderBackend,
    RenderBackendOptions,
};
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    fs::File,
    io::Write,
    rc::Rc,
    time::{Duration, Instant},
};
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, EventLoop},
    window::{Window, WindowId},
};

fn emit(output: &mut File, value: Value) -> Result<(), String> {
    serde_json::to_writer(&mut *output, &value).map_err(|error| error.to_string())?;
    writeln!(output).map_err(|error| error.to_string())
}
fn sample(
    output: &mut File,
    phase: &str,
    cache: &Cache,
    artifacts: &Artifacts,
    state: &Rc<RefCell<Boundary>>,
    gpu: &GpuAllocationQualification,
) -> Result<(), String> {
    let memory = gpu.snapshot().map_err(|error| error.to_string())?;
    let state = state.borrow();
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
        json!({"kind":"residency", "phase":phase,"copies":cache.copies(),"peak_copies":cache.peak_copies,"query_copies":cache.query_count(),"generation_workers_peak":cache.generation_peak,"derivation_workers_peak":1,"generated":cache.generated,"reused":cache.reused,"discarded":cache.discarded,"artifact_cache_entries":artifacts.count(),"owners":state.owners(),"peak_owners":state.peak_owners,"installed_targets":state.installed_targets,"retired_owners":state.retired_owners,"presenting":format!("{:?}",state.presenting.kind),"selection":state.presenting.selection.iter().map(|key|json!([key.coordinate.0,key.coordinate.1,key.version])).collect::<Vec<_>>(),"cpu_live":categories.map(allocation::live),"cpu_peak":categories.map(allocation::peak),"cpu_allocations":categories.map(allocation::count),"gpu_live":memory.live_bytes,"gpu_peak":memory.peak_bytes,"gpu_allocations":memory.live_allocations,"gpu_allocations_peak":memory.peak_allocations,"gpu_total_peak":memory.total_peak_bytes}),
    )
}
fn initialize(window: &Window, state: Rc<RefCell<Boundary>>) -> Result<RenderBackend, String> {
    RenderBackend::initialize_with_options(
        c"Streamed residency qualification",
        &windows_adapter::WindowsPresentationAdapter::new(window),
        vk::Extent2D {
            width: 1920,
            height: 1080,
        },
        Shared(state),
        RenderBackendOptions {
            validation_enabled: true,
            presentation_throttling_enabled: false,
            gpu_timestamps_enabled: true,
        },
    )
    .map_err(|error| error.to_string())
}
fn draw(backend: &mut RenderBackend) -> Result<(), String> {
    backend.draw_frame().map_err(|error| error.to_string())?;
    Ok(())
}
fn verify(
    backend: &mut RenderBackend,
    state: &Rc<RefCell<Boundary>>,
    source: &Snapshot,
) -> Result<(), String> {
    let expected = {
        let state = state.borrow();
        if !route::covered(state.camera, &state.presenting.selection, source.side) {
            return Err("rendered probes requested outside conservative installed coverage".into());
        }
        state.presenting.request_probes(source, state.camera)?
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut observed = 0;
    while observed < expected.len() {
        draw(backend)?;
        observed += state.borrow().presenting.verify_probes(&expected)?;
        if Instant::now() > deadline {
            return Err("rendered oracle probes timed out".into());
        }
    }
    Ok(())
}
fn prepare(
    cache: &mut Cache,
    artifacts: &mut Artifacts,
    source: &Snapshot,
    state: &Rc<RefCell<Boundary>>,
    selection: &[Key],
    crossing: Instant,
    switch_requested: Option<Instant>,
) -> Result<(), String> {
    let (kind, camera, revision) = {
        let state = state.borrow();
        (state.presenting.kind, state.camera, state.camera_revision)
    };
    let candidate = artifacts.build(kind, cache, source, selection, camera, revision)?;
    let replacement = if switch_requested.is_some() {
        Some(artifacts.build(kind.other(), cache, source, selection, camera, revision)?)
    } else {
        None
    };
    state.borrow_mut().pending = Some(Install {
        candidate,
        replacement,
        crossing,
        switch_requested,
    });
    Ok(())
}
fn transition(
    backend: &mut RenderBackend,
    cache: &mut Cache,
    artifacts: &mut Artifacts,
    source: &Snapshot,
    state: &Rc<RefCell<Boundary>>,
    center: (u32, u32),
    switch: bool,
) -> Result<(), String> {
    let crossing = Instant::now();
    let selection = keys(source, center);
    let installed = state.borrow().presenting.selection.clone();
    cache.retain(&installed, &selection);
    artifacts.retain(&installed, &selection);
    for key in &selection {
        cache.ensure(source, key)?;
        draw(backend)?;
    }
    prepare(
        cache,
        artifacts,
        source,
        state,
        &selection,
        crossing,
        switch.then_some(crossing),
    )?;
    draw(backend)?;
    draw(backend)?;
    cache.retain(&selection, &selection);
    artifacts.retain(&selection, &selection);
    Ok(())
}
fn run(window: &Window, output: &mut File, mode: &str) -> Result<(), String> {
    if mode == "calibration" {
        return calibrate(window, output);
    }
    let gpu = GpuAllocationQualification::start().map_err(|error| error.to_string())?;
    let side = if mode == "matched-8" { 8 } else { 16 };
    let mut source = Snapshot::new(side);
    let baseline = mode == "baseline";
    let mut cache = if baseline {
        Cache::with_limit(256)
    } else {
        Cache::new()
    };
    let camera = route::camera(0.0)?;
    let mut artifacts = Artifacts::new();
    let selection = if baseline {
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
    for key in &selection {
        cache.ensure(&source, key)?;
    }
    let view = cache.assemble(&source, &selection)?;
    if !matches!(
        ComputeSceneBundle::qualification_streamed(&view, ComputeRepresentation::Dense),
        Err(ComputeSceneBuildError::StreamedDense)
    ) {
        return Err("streamed dense compute was not rejected with its typed error".into());
    }
    drop(view);
    let initial = if mode == "brickmap" {
        Kind::Brickmap
    } else {
        Kind::Raster
    };
    let path = artifacts.build(
        initial,
        &cache,
        &source,
        &selection,
        camera,
        CameraStateRevision::new(1),
    )?;
    let state = Rc::new(RefCell::new(Boundary::new(path, camera)));
    let mut backend = initialize(window, state.clone())?;
    if backend.presentation_extent()
        != Some(vk::Extent2D {
            width: 1920,
            height: 1080,
        })
    {
        return Err("drawable extent differs from frozen contract".into());
    }
    let runtime = backend.runtime_context();
    emit(
        output,
        json!({"kind":"device","name":runtime.device_name,"driver_version":runtime.driver_version,"api_version":runtime.api_version,"validation_enabled":runtime.validation_enabled}),
    )?;
    emit(
        output,
        json!({"kind":"context","mode":mode,"side":side,"projection":[1920,1080,60.0,0.1,34.0],"speed":4,"route_duration":route::DURATION,"crossings":route::CROSSINGS,"metadata_entries":side*side,"historical_view_limit":2,"edited_coordinate_limit":6,"presentation_images":backend.qualification_presentation_image_count(),"typed_dense_rejection":true}),
    )?;
    draw(&mut backend)?;
    draw(&mut backend)?;
    sample(output, "initial", &cache, &artifacts, &state, &gpu)?;
    if baseline {
        let replacement = artifacts.build(
            Kind::Brickmap,
            &cache,
            &source,
            &selection,
            camera,
            CameraStateRevision::new(1),
        )?;
        let candidate = artifacts.build(
            Kind::Raster,
            &cache,
            &source,
            &selection,
            camera,
            CameraStateRevision::new(1),
        )?;
        state.borrow_mut().pending = Some(Install {
            candidate,
            replacement: Some(replacement),
            crossing: Instant::now(),
            switch_requested: Some(Instant::now()),
        });
        draw(&mut backend)?;
        sample(output, "baseline-overlap", &cache, &artifacts, &state, &gpu)?;
        draw(&mut backend)?;
        sample(
            output,
            "baseline-brickmap",
            &cache,
            &artifacts,
            &state,
            &gpu,
        )?;
    } else if mode.starts_with("matched") {
        source = source.edit((3, 3), false)?;
        transition(
            &mut backend,
            &mut cache,
            &mut artifacts,
            &source,
            &state,
            (3, 3),
            true,
        )?;
        sample(
            output,
            "matched-edited-brickmap",
            &cache,
            &artifacts,
            &state,
            &gpu,
        )?;
        transition(
            &mut backend,
            &mut cache,
            &mut artifacts,
            &source,
            &state,
            (3, 3),
            true,
        )?;
        sample(
            output,
            "matched-edited-raster",
            &cache,
            &artifacts,
            &state,
            &gpu,
        )?;
    } else {
        verify(&mut backend, &state, &source)?;
        history_and_stress(
            output,
            &mut backend,
            &mut cache,
            &mut artifacts,
            &mut source,
            &state,
            &gpu,
        )?;
        for lap in 0..2 {
            travel(
                output,
                &mut backend,
                &mut cache,
                &mut artifacts,
                &source,
                &state,
                &gpu,
                lap,
            )?;
        }
    }
    emit(
        output,
        json!({"kind":"validation","warnings":backend.validation_warning_count(),"errors":backend.validation_error_count()}),
    )?;
    backend.shutdown().map_err(|error| error.to_string())?;
    drop(backend);
    drop(state);
    drop(cache);
    drop(artifacts);
    drop(source);
    let final_gpu = gpu.snapshot().map_err(|error| error.to_string())?;
    emit(
        output,
        json!({"kind":"released","cpu_residency":[allocation::live(Category::Materialized),allocation::live(Category::Generation),allocation::live(Category::Raster),allocation::live(Category::Brickmap)],"gpu_live":final_gpu.live_bytes}),
    )?;
    if final_gpu.live_bytes != [0; 3] {
        return Err("GPU allocation cleanup debt remains after shutdown".into());
    }
    Ok(())
}

fn history_and_stress(
    output: &mut File,
    backend: &mut RenderBackend,
    cache: &mut Cache,
    artifacts: &mut Artifacts,
    source: &mut Snapshot,
    state: &Rc<RefCell<Boundary>>,
    gpu: &GpuAllocationQualification,
) -> Result<(), String> {
    let generated = source.clone();
    *source = source.edit((3, 3), false)?;
    let edited = source.clone();
    transition(backend, cache, artifacts, source, state, (3, 3), false)?;
    sample(output, "revision-replacement", cache, artifacts, state, gpu)?;
    verify(backend, state, source)?;
    transition(backend, cache, artifacts, source, state, (4, 3), false)?;
    *source = source.edit((2, 2), false)?;
    let unchanged = source.version((3, 3)) == edited.version((3, 3));
    transition(backend, cache, artifacts, source, state, (4, 3), false)?;
    transition(backend, cache, artifacts, source, state, (3, 3), false)?;
    for historical in [&generated, &edited] {
        cache.begin_query(historical, (3, 3))?;
        let fingerprint = cache.verify_query(historical)?;
        sample(output, "historical-query", cache, artifacts, state, gpu)?;
        emit(
            output,
            json!({"kind":"query","revision":historical.revision(),"fingerprint":format!("{fingerprint:016x}"),"global_second_request_rejected":cache.begin_query(historical,(10,10)).is_err()}),
        )?;
        cache.end_query();
    }
    *source = source.edit((2, 2), true)?;
    *source = source.edit((3, 3), true)?;
    transition(backend, cache, artifacts, source, state, (3, 3), false)?;
    cache.begin_query(source, (2, 2))?;
    cache.verify_query(source)?;
    cache.end_query();
    drop(generated);
    drop(edited);
    emit(
        output,
        json!({"kind":"compaction","live_historical_views":0,"edited_coordinates":source.overlay_coordinates(),"unchanged_volume_reused":unchanged,"revision":source.revision()}),
    )?;
    // A skipped target admits one volume, then drains it before the newest disjoint target's next admission.
    let installed = state.borrow().presenting.selection.clone();
    let skipped = keys(source, (8, 8));
    cache.retain(&installed, &skipped);
    cache.ensure(source, skipped.first().ok_or("empty skipped selection")?)?;
    let newest = keys(source, (12, 12));
    cache.retain(&installed, &newest);
    artifacts.retain(&installed, &newest);
    for key in &newest {
        cache.ensure(source, key)?;
    }
    cache.begin_query(source, (15, 15))?;
    cache.verify_query(source)?;
    sample(
        output,
        "disjoint-query-overlap",
        cache,
        artifacts,
        state,
        gpu,
    )?;
    if cache.copies() != 19 || cache.begin_query(source, (14, 15)).is_ok() {
        return Err("disjoint overlap/query admission contract failed".into());
    }
    state.borrow_mut().camera = CameraState::new(
        [800.0, 48.0, 800.0],
        [816.0, 24.0, 800.0],
        [0.0, 1.0, 0.0],
        60.0,
        0.1,
        34.0,
    )
    .map_err(|error| error.to_string())?;
    prepare(
        cache,
        artifacts,
        source,
        state,
        &newest,
        Instant::now(),
        Some(Instant::now()),
    )?;
    draw(backend)?;
    sample(
        output,
        "disjoint-renderer-overlap",
        cache,
        artifacts,
        state,
        gpu,
    )?;
    draw(backend)?;
    cache.end_query();
    cache.retain(&newest, &newest);
    artifacts.retain(&newest, &newest);
    verify(backend, state, source)?;
    // A query copy becoming selected transfers ownership, rather than materializing the same key again.
    cache.begin_query(source, (10, 10))?;
    let before = cache.generated;
    cache.ensure(
        source,
        &Key {
            coordinate: (10, 10),
            version: source.version((10, 10)),
        },
    )?;
    if cache.generated != before {
        return Err("query/selection failed to share a content-version copy".into());
    }
    cache.end_query();
    cache.retain(&newest, &newest);
    state.borrow_mut().camera = route::camera(0.0)?;
    transition(backend, cache, artifacts, source, state, (3, 3), true)?;
    if state.borrow().presenting.kind != Kind::Raster {
        transition(backend, cache, artifacts, source, state, (3, 3), true)?;
    }
    artifacts.fail_next_upload = true;
    state.borrow_mut().expected_failure = true;
    let selection = keys(source, (3, 3));
    prepare(
        cache,
        artifacts,
        source,
        state,
        &selection,
        Instant::now(),
        None,
    )?;
    draw(backend)?;
    if state.borrow().failure.is_none() {
        return Err("injected upload failure was not observed".into());
    }
    emit(
        output,
        json!({"kind":"failure-recovery","phase":"raster-upload","error":state.borrow().failure,"presenting_preserved":state.borrow().presenting.selection==selection}),
    )?;
    sample(
        output,
        "failed-candidate-cleaned",
        cache,
        artifacts,
        state,
        gpu,
    )?;
    transition(backend, cache, artifacts, source, state, (3, 3), false)?;
    for index in 0..12 {
        let x = if index % 2 == 0 { 256.5 } else { 255.5 };
        let camera = CameraState::new(
            [x, 48.0, 224.0],
            [x + 16.0, 24.0, 224.0],
            [0.0, 1.0, 0.0],
            60.0,
            0.1,
            34.0,
        )
        .map_err(|error| error.to_string())?;
        if !route::covered(camera, &state.borrow().presenting.selection, source.side) {
            return Err("boundary churn escaped installed coverage".into());
        }
        state.borrow_mut().camera = camera;
        transition(
            backend,
            cache,
            artifacts,
            source,
            state,
            route::center(camera, source.side),
            false,
        )?;
    }
    state.borrow_mut().camera = route::camera(0.0)?;
    emit(
        output,
        json!({"kind":"boundary-churn","crossings":12,"installations":12,"hysteresis":false,"coverage_stalls":0}),
    )?;
    sample(output, "stress-settled", cache, artifacts, state, gpu)?;
    Ok(())
}
#[expect(
    clippy::too_many_arguments,
    reason = "Qualification phases expose independently accounted owners"
)]
fn travel(
    output: &mut File,
    backend: &mut RenderBackend,
    cache: &mut Cache,
    artifacts: &mut Artifacts,
    source: &Snapshot,
    state: &Rc<RefCell<Boundary>>,
    gpu: &GpuAllocationQualification,
    lap: u32,
) -> Result<(), String> {
    let start = Instant::now();
    let mut next_crossing = 0;
    let mut installed_crossings = 0;
    let mut unresolved: Option<Instant> = None;
    let mut switch_requested = None;
    let mut pending: Option<Vec<Key>> = None;
    let mut stalls = 0_u64;
    let mut frames = 0_u64;
    let mut logged_second = 0_u64;
    let mut probes = 0_u64;
    while start.elapsed().as_secs_f64() < route::DURATION {
        let seconds = start.elapsed().as_secs_f64();
        let camera = route::camera(seconds)?;
        let installed = state.borrow().presenting.selection.clone();
        if !route::covered(camera, &installed, source.side) {
            stalls += 1;
        } else {
            let mut state = state.borrow_mut();
            state.camera = camera;
            state.camera_revision = state
                .camera_revision
                .checked_successor()
                .ok_or("camera revision overflow")?;
        }
        if route::crossing_due(
            seconds,
            next_crossing,
            pending.as_ref().map(|_| next_crossing - 1),
        )? {
            let crossing_time = *route::CROSSINGS
                .get(next_crossing)
                .ok_or("missing due crossing")?;
            let origin = start + Duration::from_secs_f64(crossing_time);
            unresolved.get_or_insert(origin);
            let selection = keys(source, route::center(camera, source.side));
            cache.retain(&installed, &selection);
            artifacts.retain(&installed, &selection);
            pending = Some(selection);
            if next_crossing == 2 || next_crossing == 5 {
                switch_requested = Some(Instant::now());
            }
            emit(
                output,
                json!({"kind":"crossing","lap":lap,"index":next_crossing,"origin_seconds":crossing_time,"observed_seconds":seconds,"center":route::center(camera,source.side),"unresolved_origin_seconds":unresolved.ok_or("missing crossing origin")?.duration_since(start).as_secs_f64(),"switch_requested":switch_requested.is_some()}),
            )?;
            next_crossing += 1;
        }
        if let Some(selection) = &pending {
            let mut generated = false;
            for key in selection {
                if cache.ensure(source, key)? {
                    generated = true;
                    break;
                }
            }
            if !generated {
                prepare(
                    cache,
                    artifacts,
                    source,
                    state,
                    selection,
                    unresolved.ok_or("target has no crossing origin")?,
                    switch_requested,
                )?;
                draw(backend)?;
                let boundary = state.borrow();
                route::crossing_due(
                    start.elapsed().as_secs_f64(),
                    next_crossing,
                    Some(next_crossing - 1),
                )?;
                if boundary.presenting.selection != *selection || boundary.crossing_seconds > 2.5 {
                    return Err("crossing target was not installed within its deadline".into());
                }
                installed_crossings += 1;
                emit(
                    output,
                    json!({"kind":"installed","lap":lap,"index":next_crossing-1,"crossing_seconds":boundary.crossing_seconds,"switch_seconds":switch_requested.map(|_|boundary.switch_seconds),"fence_safe":true,"selection_matches":boundary.presenting.selection==*selection}),
                )?;
                drop(boundary);
                draw(backend)?;
                cache.retain(selection, selection);
                artifacts.retain(selection, selection);
                sample(output, "crossing-settled", cache, artifacts, state, gpu)?;
                verify(backend, state, source)?;
                probes += 4;
                pending = None;
                unresolved = None;
                switch_requested = None;
            }
        }
        draw(backend)?;
        frames += 1;
        let whole_second = seconds as u64;
        if whole_second > logged_second {
            logged_second = whole_second;
            sample(output, "travel", cache, artifacts, state, gpu)?;
        }
        std::thread::sleep(Duration::from_millis(8));
    }
    if pending.is_some()
        || next_crossing != route::CROSSINGS.len()
        || installed_crossings != route::CROSSINGS.len()
    {
        return Err("route ended with incomplete coverage demand".into());
    }
    sample(output, "lap-settled", cache, artifacts, state, gpu)?;
    emit(
        output,
        json!({"kind":"route-result","lap":lap,"frames":frames,"duration_seconds":start.elapsed().as_secs_f64(),"coverage_stalls":stalls,"crossings":next_crossing,"installed_crossings":installed_crossings,"rendered_probes":probes,"boundary_churn_installed_targets":state.borrow().installed_targets}),
    )?;
    Ok(())
}

struct Application {
    output: File,
    mode: String,
    result: Option<Result<(), String>>,
}
impl ApplicationHandler for Application {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.result = Some((|| {
            let window = event_loop
                .create_window(
                    Window::default_attributes()
                        .with_visible(false)
                        .with_decorations(false)
                        .with_inner_size(winit::dpi::PhysicalSize::new(1920, 1080)),
                )
                .map_err(|error| error.to_string())?;
            windows_adapter::set_measurement_extent(
                &window,
                vk::Extent2D {
                    width: 1920,
                    height: 1080,
                },
            )
            .map_err(|error| error.to_string())?;
            run(&window, &mut self.output, &self.mode)
        })());
        event_loop.exit();
    }
    fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, _: WindowEvent) {}
}
pub fn main() -> Result<(), String> {
    let mut arguments = std::env::args().skip(1);
    let mode = arguments
        .next()
        .ok_or("missing mode raster|brickmap|baseline|matched-8|matched-16")?;
    if ![
        "raster",
        "brickmap",
        "baseline",
        "matched-8",
        "matched-16",
        "calibration",
    ]
    .contains(&mode.as_str())
    {
        return Err("unknown qualification mode".into());
    }
    let output = arguments.next().ok_or("missing output.jsonl")?;
    if arguments.next().as_deref() != Some("--allow-gpu") || arguments.next().is_some() {
        return Err("unexpected argument".into());
    }
    let mut application = Application {
        output: File::create(output).map_err(|error| error.to_string())?,
        mode,
        result: None,
    };
    EventLoop::new()
        .map_err(|error| error.to_string())?
        .run_app(&mut application)
        .map_err(|error| error.to_string())?;
    application.result.ok_or("qualification did not run")?
}

fn calibrate(window: &Window, output: &mut File) -> Result<(), String> {
    for phase in ["generated", "edited", "restored"] {
        let gpu = GpuAllocationQualification::start().map_err(|error| error.to_string())?;
        let mut source = Snapshot::new(16);
        if phase != "generated" {
            source = source.edit((3, 3), false)?;
        }
        if phase == "restored" {
            source = source.edit((3, 3), true)?;
        }
        let mut cache = Cache::new();
        let mut artifacts = Artifacts::new();
        allocation::reset_peaks();
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
        let camera = route::camera(0.0)?;
        let path = artifacts.build(
            Kind::Raster,
            &cache,
            &source,
            &selection,
            camera,
            CameraStateRevision::new(1),
        )?;
        let state = Rc::new(RefCell::new(Boundary::new(path, camera)));
        let mut backend = initialize(window, state.clone())?;
        draw(&mut backend)?;
        draw(&mut backend)?;
        sample(
            output,
            &format!("{phase}-raster"),
            &cache,
            &artifacts,
            &state,
            &gpu,
        )?;
        let candidate = artifacts.build(
            Kind::Brickmap,
            &cache,
            &source,
            &selection,
            camera,
            CameraStateRevision::new(1),
        )?;
        state.borrow_mut().pending = Some(Install {
            candidate,
            replacement: None,
            crossing: Instant::now(),
            switch_requested: None,
        });
        draw(&mut backend)?;
        sample(
            output,
            &format!("{phase}-overlap"),
            &cache,
            &artifacts,
            &state,
            &gpu,
        )?;
        draw(&mut backend)?;
        sample(
            output,
            &format!("{phase}-brickmap"),
            &cache,
            &artifacts,
            &state,
            &gpu,
        )?;
        emit(
            output,
            json!({"kind":"fingerprint","phase":phase,"fingerprint":format!("{fingerprint:016x}")}),
        )?;
        backend.shutdown().map_err(|error| error.to_string())?;
        drop(backend);
        drop(state);
        drop(cache);
        drop(artifacts);
        drop(source);
        let memory = gpu.snapshot().map_err(|error| error.to_string())?;
        emit(
            output,
            json!({"kind":"released","phase":phase,"cpu_residency":[allocation::live(Category::Materialized),allocation::live(Category::Generation),allocation::live(Category::Raster),allocation::live(Category::Brickmap)],"gpu_live":memory.live_bytes}),
        )?;
    }
    Ok(())
}

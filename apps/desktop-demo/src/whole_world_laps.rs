//! PROTOTYPE (issue #139): whole-world laps with one long-lived Render Backend.
//!
//! Throwaway. One Render Backend lives for the whole lap. At every selection-centre move the
//! next scene (all 256 fixture volumes at their demo-band detail levels) is prepared with the
//! route clock paused, standing in for worker-thread preparation, then installed in place
//! through `request_render_path_switch` while the clock runs, so frame-boundary installation
//! is measured. The frozen switches (first centre move of the diagonal, first centre move of
//! the final leg) install the other Render Path the same way. Every frame is attributed to the
//! Render Path presenting it.
//!
//! `semantic` mode instead compares GPU Brickmap Semantic Rays through the grid table against
//! the CPU oracle for every route scene.
#[allow(dead_code)]
mod streamed_fixture_recipe;
#[allow(dead_code)]
mod whole_world_detail_levels;
#[allow(dead_code)]
mod whole_world_route;
#[allow(dead_code)]
mod windows_adapter;

use compute_ray_render_path::{
    ComputeRayRenderPathAdapter, ComputeRepresentation, ComputeSemanticRayController,
    camera_semantic_ray,
};
use raster_render_path::{RasterArtifactInstaller, RasterRenderPathAdapter, derive_raster_regions};
use render_backend::{
    CameraState, CameraStateRevision, RenderBackend, RenderBackendOptions, RenderPathSwitchOwner,
    SwitchableRenderPath,
};
use semantic_ray_oracle::{
    SemanticRayDistanceTolerance, SemanticRayProbe, observe_probe_along_ray,
};
use serde_json::json;
use std::{
    collections::VecDeque,
    fs::File,
    io::Write,
    time::{Duration, Instant},
};
use voxel_frontend::{SparseVoxelScene, VoxelExtent, VoxelFrontend, VoxelSceneView};
use whole_world_route::*;
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, EventLoop},
    window::{Window, WindowId},
};

struct Prepared {
    path: Box<dyn SwitchableRenderPath>,
    installer: Option<RasterArtifactInstaller>,
    semantic: Option<ComputeSemanticRayController>,
    view: VoxelSceneView,
}

fn build_path(
    kind: PathKind,
    scene: SparseVoxelScene,
    camera: CameraState,
    camera_revision: u64,
) -> RunResult<Prepared> {
    let frontend = VoxelFrontend::new();
    let view = frontend.publish_sparse(scene)?;
    Ok(match kind {
        PathKind::Brickmap => {
            let mut path = ComputeRayRenderPathAdapter::new_with_representation(
                view.clone(),
                camera,
                CameraStateRevision::new(camera_revision),
                ComputeRepresentation::Brickmap {
                    budget_bytes: 256 << 20,
                },
            )?;
            let semantic = path.enable_semantic_ray_observation();
            Prepared {
                path: Box::new(path),
                installer: None,
                semantic: Some(semantic),
                view,
            }
        }
        PathKind::Raster => {
            let (path, installer, _) =
                RasterRenderPathAdapter::awaiting_artifact_with_camera_control(
                    camera,
                    CameraStateRevision::new(camera_revision),
                    view.scene_id().clone(),
                    view.revision(),
                );
            installer
                .publish_complete(derive_raster_regions(&view, VoxelExtent::new(16, 16, 16))?)?;
            Prepared {
                path: Box::new(path),
                installer: Some(installer),
                semantic: None,
                view,
            }
        }
    })
}

fn initialize(window: &Window, prepared: Prepared) -> RunResult<RenderBackend> {
    let backend = RenderBackend::initialize_with_options(
        c"Whole-world re-profile prototype",
        &windows_adapter::WindowsPresentationAdapter::new(window),
        EXTENT,
        RenderPathSwitchOwner::new(prepared.path),
        RenderBackendOptions {
            validation_enabled: false,
            presentation_throttling_enabled: false,
            gpu_timestamps_enabled: true,
        },
    )?;
    if backend.presentation_extent() != Some(EXTENT) {
        return Err("measurement requires actual 1920x1080 presentation".into());
    }
    Ok(backend)
}

fn switch_idle(backend: &RenderBackend) -> bool {
    backend
        .render_path_switch_diagnostics()
        .is_none_or(|diagnostics| {
            diagnostics.replacement().is_none() && diagnostics.retiring().is_none()
        })
}

fn presenting(backend: &RenderBackend) -> String {
    backend
        .render_path_switch_diagnostics()
        .map_or_else(String::new, |diagnostics| {
            format!("{:?}", diagnostics.presenting().strategy())
        })
}

fn emit(output: &mut File, value: serde_json::Value) -> RunResult {
    writeln!(output, "{value}")?;
    Ok(())
}

/// Route time that stops while the frame thread stands in for worker-thread preparation.
struct RouteClock {
    started: Instant,
    paused: Duration,
}

impl RouteClock {
    fn seconds(&self) -> f32 {
        (self.started.elapsed() - self.paused).as_secs_f32()
    }
}

fn lap(window: &Window, output: &mut File, start_kind: PathKind) -> RunResult {
    let levels = levels()?;
    let (start, start_direction, _) = route_position(0.0).ok_or("empty route")?;
    let mut centre = [nominal(start[0]), nominal(start[1])];
    let mut camera_revision = 1_u64;
    let prepared = build_path(
        start_kind,
        build_scene(&levels, centre),
        camera(start, start_direction)?,
        camera_revision,
    )?;
    let mut installer = prepared.installer.clone();
    let mut backend = initialize(window, prepared)?;
    let initialized = Instant::now();
    let device = backend.runtime_context().device_name.clone();
    emit(
        output,
        json!({"kind": "lap", "start": format!("{start_kind:?}"), "device": device,
            "prototype": "issue-139 one long-lived backend"}),
    )?;
    let deadline = Instant::now() + Duration::from_secs(60);
    while !installer.as_ref().is_none_or(|installer| {
        installer
            .installed_source_revision()
            .ok()
            .flatten()
            .is_some()
    }) {
        backend.draw_frame()?;
        if Instant::now() > deadline {
            return Err("initial installation timed out".into());
        }
    }
    let hold_end = Instant::now() + WARMUP_HOLD;
    while Instant::now() < hold_end {
        backend.draw_frame()?;
    }
    let _ = backend.take_frame_observation();

    let mut desired = start_kind;
    let mut switched_on_diagonal = false;
    let mut switched_on_final = false;
    let mut pending: Option<(Prepared, f32)> = None;
    let mut in_flight: Option<(f32, String)> = None;
    let mut previous_position = start;
    let mut segment = 0_u64;
    let mut frames: VecDeque<(u64, f64, f32, f64, String, u64)> = VecDeque::new();
    let mut clock = RouteClock {
        started: Instant::now(),
        paused: Duration::ZERO,
    };
    loop {
        let frame_started = Instant::now();
        let route_seconds = clock.seconds();
        let Some((position, direction, leg)) = route_position(route_seconds) else {
            break;
        };
        let moved = next_centre(centre, previous_position, position);
        let teleport = (position[0] - previous_position[0])
            .hypot(position[1] - previous_position[1])
            > TELEPORT;
        previous_position = position;
        if moved != centre {
            centre = moved;
            segment += 1;
            let mut switch = false;
            if leg == DIAGONAL_LEG && !switched_on_diagonal {
                switched_on_diagonal = true;
                switch = true;
            } else if leg == FINAL_LEG && !switched_on_final {
                switched_on_final = true;
                switch = true;
            }
            if switch {
                desired = desired.other();
                emit(
                    output,
                    json!({"kind": "switch-request", "route_seconds": route_seconds,
                        "to": format!("{desired:?}")}),
                )?;
            }
            let preparation_started = Instant::now();
            camera_revision += 1;
            let prepared = build_path(
                desired,
                build_scene(&levels, centre),
                camera(position, direction)?,
                camera_revision,
            )?;
            let preparation = preparation_started.elapsed();
            clock.paused += preparation;
            emit(
                output,
                json!({"kind": "crossing", "route_seconds": route_seconds, "segment": segment,
                    "centre": centre, "teleport": teleport, "path": format!("{desired:?}"),
                    "superseded": pending.is_some(),
                    "preparation_ms": preparation.as_secs_f64() * 1000.0}),
            )?;
            // Newest only: a later scene supersedes one still waiting for the switch slot.
            pending = Some((prepared, route_seconds));
            continue;
        }
        if switch_idle(&backend) {
            if let Some((requested_at, _)) = in_flight.take() {
                emit(
                    output,
                    json!({"kind": "installed", "requested_s": requested_at,
                        "route_seconds": route_seconds, "presenting": presenting(&backend)}),
                )?;
            }
            if let Some((prepared, crossed_at)) = pending.take() {
                installer = prepared.installer.clone();
                backend
                    .request_render_path_switch(prepared.path)
                    .map_err(|error| format!("{error:?}"))?;
                in_flight = Some((crossed_at, presenting(&backend)));
            }
        }
        camera_revision += 1;
        backend
            .publish_camera_state(
                camera(position, direction)?,
                CameraStateRevision::new(camera_revision),
            )
            .map_err(|error| error.to_string())?;
        backend.draw_frame()?;
        let cpu_ms = frame_started.elapsed().as_secs_f64() * 1000.0;
        if let Some(sequence) = backend.last_submitted_frame_sequence() {
            frames.push_back((
                sequence,
                cpu_ms,
                route_seconds,
                initialized.elapsed().as_secs_f64(),
                presenting(&backend),
                segment,
            ));
        }
        if let Some(observation) = backend.take_frame_observation() {
            while let Some((sequence, ..)) = frames.front() {
                if *sequence > observation.sequence {
                    break;
                }
                let (sequence, cpu_ms, seconds, since_init, presenting, segment) = frames
                    .pop_front()
                    .expect("the front frame was just inspected");
                if sequence == observation.sequence {
                    emit(
                        output,
                        json!({"kind": "frame", "segment": segment, "route_seconds": seconds,
                            "cpu_frame_ms": cpu_ms, "gpu_frame_ms": observation.gpu_frame_milliseconds,
                            "presenting": presenting, "since_init_s": since_init}),
                    )?;
                }
            }
        }
    }
    let _ = installer;
    let stalls = backend
        .render_path_switch_diagnostics()
        .map(|diagnostics| diagnostics.coverage_stalls());
    emit(
        output,
        json!({"kind": "end", "segments": segment, "coverage_stalls": stalls,
            "switched_on_diagonal": switched_on_diagonal, "switched_on_final": switched_on_final,
            "presenting": presenting(&backend), "device": device}),
    )?;
    backend.shutdown()?;
    Ok(())
}

fn semantic(window: &Window, output: &mut File) -> RunResult {
    let levels = levels()?;
    let tolerance = SemanticRayDistanceTolerance::new(0.002)?;
    let mut backend: Option<RenderBackend> = None;
    let mut camera_revision = 1_u64;
    let (mut agreed, mut disagreed, mut contacts) = (0_usize, 0_usize, 0_usize);
    for (index, (centre, position, direction)) in route_scenes().into_iter().enumerate() {
        camera_revision += 1;
        let current_camera = camera(position, direction)?;
        let prepared = build_path(
            PathKind::Brickmap,
            build_scene(&levels, centre),
            current_camera,
            camera_revision,
        )?;
        let controller = prepared
            .semantic
            .clone()
            .expect("Brickmap paths observe rays");
        let view = prepared.view.clone();
        let backend = match backend.as_mut() {
            Some(backend) => {
                backend
                    .request_render_path_switch(prepared.path)
                    .map_err(|error| format!("{error:?}"))?;
                // The handoff waits for both paths to hold the same Camera State revision.
                camera_revision += 1;
                backend
                    .publish_camera_state(current_camera, CameraStateRevision::new(camera_revision))
                    .map_err(|error| error.to_string())?;
                while !switch_idle(backend) {
                    backend.draw_frame()?;
                }
                backend
            }
            None => backend.insert(initialize(window, prepared)?),
        };
        if index == 0 {
            emit(
                output,
                json!({"kind": "semantic", "device": backend.runtime_context().device_name}),
            )?;
        }
        // A 6x6 pixel lattice in batches of the controller's eight-probe limit.
        let pixels = (0..36)
            .map(|cell| {
                [
                    (cell % 6) * EXTENT.width / 6 + EXTENT.width / 12,
                    (cell / 6) * EXTENT.height / 6 + EXTENT.height / 12,
                ]
            })
            .collect::<Vec<_>>();
        for batch in pixels.chunks(8) {
            let probes = batch
                .iter()
                .map(|&pixel| {
                    Ok(SemanticRayProbe::new(
                        format!("{centre:?}-{pixel:?}"),
                        camera_semantic_ray(current_camera, EXTENT, pixel)?,
                    )?)
                })
                .collect::<RunResult<Vec<_>>>()?;
            let batch_started = Instant::now();
            controller.request(probes.clone())?;
            let mut actual = Vec::new();
            for _ in 0..64 {
                backend.draw_frame()?;
                actual.extend(controller.drain()?);
                if actual.len() == probes.len() {
                    break;
                }
            }
            if actual.len() != probes.len() {
                return Err("GPU Semantic Rays did not complete".into());
            }
            let gpu_ms = batch_started.elapsed().as_secs_f64() * 1000.0;
            let oracle_started = Instant::now();
            for (probe, actual) in probes.iter().zip(actual) {
                let expected = observe_probe_along_ray(&view, probe)?;
                if matches!(
                    expected.observation().result(),
                    semantic_ray_oracle::SemanticRayResult::Contact(_)
                ) {
                    contacts += 1;
                }
                if actual
                    .observation()
                    .agrees_with(expected.observation(), tolerance)
                {
                    agreed += 1;
                } else {
                    disagreed += 1;
                    emit(
                        output,
                        json!({"kind": "disagreement", "probe": probe.identity(),
                            "expected": format!("{:?}", expected.observation().result()),
                            "actual": format!("{:?}", actual.observation().result())}),
                    )?;
                }
            }
            emit(
                output,
                json!({"kind": "batch", "scene": index, "gpu_ms": gpu_ms,
                    "oracle_ms": oracle_started.elapsed().as_secs_f64() * 1000.0}),
            )?;
            output.flush()?;
        }
    }
    emit(
        output,
        json!({"kind": "semantic-summary", "agreed": agreed, "disagreed": disagreed,
            "contacts": contacts}),
    )?;
    if let Some(mut backend) = backend {
        backend.shutdown()?;
    }
    if disagreed > 0 {
        return Err(format!("{disagreed} GPU Semantic Rays disagreed with the oracle").into());
    }
    Ok(())
}

enum Mode {
    Lap(PathKind),
    Semantic,
}

struct Application {
    output: File,
    mode: Mode,
    result: Option<RunResult>,
}

impl ApplicationHandler for Application {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.result = Some((|| {
            let window = event_loop.create_window(
                Window::default_attributes()
                    .with_visible(false)
                    .with_decorations(false)
                    .with_inner_size(winit::dpi::PhysicalSize::new(EXTENT.width, EXTENT.height)),
            )?;
            windows_adapter::set_measurement_extent(&window, EXTENT)?;
            match self.mode {
                Mode::Lap(kind) => lap(&window, &mut self.output, kind),
                Mode::Semantic => semantic(&window, &mut self.output),
            }
        })());
        event_loop.exit();
    }

    fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, _: WindowEvent) {}
}

fn main() -> Result<(), String> {
    let mut arguments = std::env::args().skip(1);
    let usage = "usage: whole-world-laps <raster|brickmap|semantic> OUTPUT.jsonl";
    let mode = match arguments.next().as_deref() {
        Some("raster") => Mode::Lap(PathKind::Raster),
        Some("brickmap") => Mode::Lap(PathKind::Brickmap),
        Some("semantic") => Mode::Semantic,
        _ => return Err(usage.into()),
    };
    let output = arguments.next().ok_or(usage)?;
    let mut application = Application {
        output: File::create(output).map_err(|error| error.to_string())?,
        mode,
        result: None,
    };
    let event_loop = EventLoop::new().map_err(|error| error.to_string())?;
    event_loop
        .run_app(&mut application)
        .map_err(|error| error.to_string())?;
    application
        .result
        .ok_or("measurement did not run")?
        .map_err(|error| error.to_string())
}

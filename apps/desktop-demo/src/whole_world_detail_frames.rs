//! PROTOTYPE (issue #137): whole-world frame time with coarse detail levels.
//!
//! Throwaway. For every route segment between selection-centre moves, it publishes all 256
//! fixture volumes at their selected detail levels as one ordinary Voxel Scene, then flies the
//! frozen route through that segment in real time at 1920x1080 with fog and edge shading. Scene
//! rebuilds happen off the route clock, so the frames measure steady whole-world cost, not
//! detail convergence.
#[allow(dead_code)]
mod streamed_fixture_recipe;
#[allow(dead_code)]
mod whole_world_detail_levels;
#[allow(dead_code)]
mod windows_adapter;

use ash::vk;
use compute_ray_render_path::{ComputeRayRenderPathAdapter, ComputeRepresentation};
use raster_render_path::{RasterRenderPathAdapter, derive_raster_regions};
use render_backend::{
    CameraState, CameraStateRevision, PresentationStyle, RenderBackend, RenderBackendOptions,
    RenderPathSwitchOwner, SwitchableRenderPath,
};
use serde_json::json;
use std::{
    fs::File,
    io::Write,
    time::{Duration, Instant},
};
use streamed_fixture_recipe::{EDGE, materials, scene, volume_contents};
use voxel_frontend::{
    SparseVoxelScene, VoxelExtent, VoxelFrontend, VoxelSceneId, VoxelSceneRevision, VoxelValue,
};
use whole_world_detail_levels::{coarse_volume, downsample};
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, EventLoop},
    window::{Window, WindowId},
};

type RunResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

const GRID: i32 = 16;
const DEADBAND: f32 = 16.0;
const TELEPORT: f32 = 64.0;
const SPEED: f32 = 16.0;
const EYE_HEIGHT: f32 = 48.0;
const WORLD_CENTRE: [f32; 3] = [512.0, 24.0, 512.0];
const WARMUP_FRAMES: usize = 60;

/// One leg of the frozen route; a teleport jumps to `to` without travelling.
struct Leg {
    to: [f32; 2],
    teleport: bool,
}

fn route() -> (Vec<Leg>, [f32; 2]) {
    let leg = |x, z| Leg {
        to: [x, z],
        teleport: false,
    };
    (
        vec![
            leg(864.0, 160.0),
            leg(864.0, 500.0),
            leg(864.0, 420.0),
            leg(864.0, 864.0),
            Leg {
                to: [160.0, 864.0],
                teleport: true,
            },
            leg(864.0, 160.0),
            leg(160.0, 160.0),
        ],
        [160.0, 160.0],
    )
}

/// Camera position and travel direction at route time `seconds`, or None past the lap end.
fn route_position(seconds: f32) -> Option<([f32; 2], [f32; 2])> {
    let (legs, mut from) = route();
    let mut remaining = seconds * SPEED;
    let mut direction = [1.0, 0.0];
    for leg in legs {
        if leg.teleport {
            from = leg.to;
            continue;
        }
        let delta = [leg.to[0] - from[0], leg.to[1] - from[1]];
        let length = delta[0].hypot(delta[1]);
        direction = [delta[0] / length, delta[1] / length];
        if remaining <= length {
            return Some((
                [
                    from[0] + direction[0] * remaining,
                    from[1] + direction[1] * remaining,
                ],
                direction,
            ));
        }
        remaining -= length;
        from = leg.to;
    }
    let _ = direction;
    None
}

fn nominal(coordinate: f32) -> i32 {
    ((coordinate / EDGE as f32).floor() as i32).clamp(0, GRID - 1)
}

fn axis_centre(previous: i32, coordinate: f32) -> i32 {
    let low = previous as f32 * EDGE as f32 - DEADBAND;
    let high = (previous + 1) as f32 * EDGE as f32 + DEADBAND;
    if coordinate >= low && coordinate < high {
        previous
    } else {
        nominal(coordinate)
    }
}

fn camera(position: [f32; 2], direction: [f32; 2]) -> RunResult<CameraState> {
    let eye = [position[0], EYE_HEIGHT, position[1]];
    let to_centre = [WORLD_CENTRE[0] - eye[0], WORLD_CENTRE[2] - eye[2]];
    let target = if to_centre[0].hypot(to_centre[1]) < 64.0 {
        [
            eye[0] + direction[0] * 64.0,
            eye[1] - 16.0,
            eye[2] + direction[1] * 64.0,
        ]
    } else {
        WORLD_CENTRE
    };
    let world = GRID as f32 * EDGE as f32;
    let mut farthest: f32 = 0.0;
    for corner_x in [0.0, world] {
        for corner_y in [0.0, EDGE as f32] {
            for corner_z in [0.0, world] {
                let offset = [corner_x - eye[0], corner_y - eye[1], corner_z - eye[2]];
                farthest = farthest.max(
                    (offset[0] * offset[0] + offset[1] * offset[1] + offset[2] * offset[2]).sqrt(),
                );
            }
        }
    }
    Ok(
        CameraState::new(eye, target, [0.0, 1.0, 0.0], 60.0, 0.1, farthest + 1.0)?
            .with_radial_far_clip()
            .with_presentation_style(PresentationStyle::SCENIC),
    )
}

#[derive(Clone, Copy, PartialEq)]
enum PathKind {
    Raster,
    Brickmap,
}

#[derive(Clone, Copy, PartialEq)]
enum Coverage {
    WholeWorld,
    FullDetailOnly,
}

struct Levels {
    by_edge: Vec<(u32, Vec<VoxelValue>)>,
}

fn levels() -> RunResult<Levels> {
    let frontend = VoxelFrontend::new();
    let view = frontend.publish_sparse(scene(&[(3, 3)]))?;
    let coarse = downsample(&view, 3, 3)?;
    Ok(Levels {
        by_edge: [32, 16, 8].into_iter().zip(coarse).collect(),
    })
}

fn build_scene(
    levels: &Levels,
    centre: [i32; 2],
    coverage: Coverage,
    revision: u64,
) -> SparseVoxelScene {
    let mut volumes = Vec::new();
    for z in 0..GRID {
        for x in 0..GRID {
            let distance = (x - centre[0]).abs().max((z - centre[1]).abs());
            let edge = match distance {
                0..=3 => 64,
                4..=6 => 32,
                7..=12 => 16,
                _ => 8,
            };
            if edge == 64 {
                volumes.push(volume_contents(x as u32, z as u32, false));
            } else if coverage == Coverage::WholeWorld {
                let values = levels
                    .by_edge
                    .iter()
                    .find(|(level, _)| *level == edge)
                    .map(|(_, values)| values.clone())
                    .expect("every coarse band has a level");
                volumes.push(coarse_volume(x as u32, z as u32, edge, values));
            }
        }
    }
    SparseVoxelScene::new(
        // PROTOTYPE (issue #140): a switch in place requires one scene identity.
        VoxelSceneId::new(if std::env::var_os("PROBE_SWITCH_IN_PLACE").is_some() {
            "whole-world-detail".to_owned()
        } else {
            format!("whole-world-detail-{revision}")
        }),
        VoxelSceneRevision::new(1),
        materials(),
        volumes,
    )
}

fn build_path(
    kind: PathKind,
    scene: SparseVoxelScene,
    camera: CameraState,
    camera_revision: u64,
) -> RunResult<(
    Box<dyn SwitchableRenderPath>,
    Option<raster_render_path::RasterArtifactInstaller>,
)> {
    let frontend = VoxelFrontend::new();
    let view = frontend.publish_sparse(scene)?;
    Ok(match kind {
        PathKind::Brickmap => (
            Box::new(ComputeRayRenderPathAdapter::new_with_representation(
                view,
                camera,
                CameraStateRevision::new(camera_revision),
                ComputeRepresentation::Brickmap {
                    budget_bytes: 256 << 20,
                },
            )?),
            None,
        ),
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
            (Box::new(path), Some(installer))
        }
    })
}

// PROTOTYPE (issue #140): burst attribution probes.
fn probe(name: &str) -> bool {
    std::env::var_os(name).is_some()
}

fn run(window: &Window, output: &mut File, kind: PathKind, coverage: Coverage) -> RunResult {
    let levels = levels()?;
    let hold_centre = probe("PROBE_HOLD_CENTRE");
    let switch_in_place = probe("PROBE_SWITCH_IN_PLACE");
    let refresh_in_place = probe("PROBE_REFRESH_IN_PLACE");
    let mut long_lived: Option<(RenderBackend, Instant)> = None;
    let (start, start_direction) = route_position(0.0).ok_or("empty route")?;
    let mut centre = [nominal(start[0]), nominal(start[1])];
    let mut previous_position = start;
    let mut route_seconds = 0.0_f32;
    let mut segment = 0_u64;
    let mut camera_revision = 1_u64;
    let mut position_direction = (start, start_direction);
    loop {
        let rebuild_started = Instant::now();
        let current_camera = camera(position_direction.0, position_direction.1)?;
        let (path, installer) = build_path(
            kind,
            build_scene(&levels, centre, coverage, segment),
            current_camera,
            camera_revision,
        )?;
        let preparation_ms = rebuild_started.elapsed().as_secs_f64() * 1000.0;
        let reused = switch_in_place && long_lived.is_some();
        let (mut backend, initialized_at) =
            if let Some((mut backend, initialized_at)) = long_lived.take() {
                backend
                    .request_render_path_switch(path)
                    .map_err(|error| format!("{error:?}"))?;
                (backend, initialized_at)
            } else {
                let backend = RenderBackend::initialize_with_options(
                    c"Whole-world detail prototype",
                    &windows_adapter::WindowsPresentationAdapter::new(window),
                    vk::Extent2D {
                        width: 1920,
                        height: 1080,
                    },
                    RenderPathSwitchOwner::new(path),
                    RenderBackendOptions {
                        validation_enabled: false,
                        presentation_throttling_enabled: probe("PROBE_FIFO"),
                        gpu_timestamps_enabled: true,
                    },
                )?;
                (backend, Instant::now())
            };
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut warm = 0;
        while warm < WARMUP_FRAMES {
            backend.draw_frame()?;
            let switched = !reused
                || backend
                    .render_path_switch_diagnostics()
                    .is_none_or(|diagnostics| {
                        diagnostics.replacement().is_none() && diagnostics.retiring().is_none()
                    });
            let installed = switched
                && installer.as_ref().is_none_or(|installer| {
                    installer
                        .installed_source_revision()
                        .ok()
                        .flatten()
                        .is_some()
                });
            if installed {
                warm += 1;
            }
            if Instant::now() > deadline {
                return Err("initial installation timed out".into());
            }
        }
        let _ = backend.take_frame_observation();
        let extent = backend
            .presentation_extent()
            .ok_or("no presentation extent")?;
        if extent.width != 1920 || extent.height != 1080 {
            return Err("measurement requires actual 1920x1080 presentation".into());
        }
        writeln!(
            output,
            "{}",
            json!({"kind": "segment", "segment": segment, "centre": centre,
                "route_seconds": route_seconds, "preparation_ms": preparation_ms, "reused": reused,
                "since_init_s": initialized_at.elapsed().as_secs_f64(),
                "device": backend.runtime_context().device_name})
        )?;
        let mut pending = std::collections::VecDeque::<(u64, f64, f32, f64)>::new();
        let segment_clock = Instant::now();
        let segment_start_seconds = route_seconds;
        let finished = loop {
            let frame_started = Instant::now();
            route_seconds = segment_start_seconds + segment_clock.elapsed().as_secs_f32();
            let Some((position, direction)) = route_position(route_seconds) else {
                break true;
            };
            let displacement =
                (position[0] - previous_position[0]).hypot(position[1] - previous_position[1]);
            let next_centre = if displacement > TELEPORT {
                [nominal(position[0]), nominal(position[1])]
            } else {
                [
                    axis_centre(centre[0], position[0]),
                    axis_centre(centre[1], position[1]),
                ]
            };
            previous_position = position;
            if next_centre != centre && refresh_in_place {
                centre = next_centre;
                backend.refresh_render_path()?;
                writeln!(
                    output,
                    "{}",
                    json!({"kind": "refresh", "since_init_s": initialized_at.elapsed().as_secs_f64()})
                )?;
                continue;
            }
            if next_centre != centre && !hold_centre {
                centre = next_centre;
                position_direction = (position, direction);
                break false;
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
                pending.push_back((
                    sequence,
                    cpu_ms,
                    route_seconds,
                    initialized_at.elapsed().as_secs_f64(),
                ));
            }
            if let Some(observation) = backend.take_frame_observation() {
                while let Some(&(sequence, cpu_ms, seconds, since_init)) = pending.front() {
                    if sequence > observation.sequence {
                        break;
                    }
                    pending.pop_front();
                    if sequence == observation.sequence {
                        writeln!(
                            output,
                            "{}",
                            json!({"kind": "frame", "segment": segment, "route_seconds": seconds,
                                "cpu_frame_ms": cpu_ms, "since_init_s": since_init,
                                "unix_s": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0),
                                "gpu_frame_ms": observation.gpu_frame_milliseconds})
                        )?;
                    }
                }
            }
        };
        if switch_in_place && !finished {
            long_lived = Some((backend, initialized_at));
        } else {
            backend.shutdown()?;
            drop(backend);
        }
        // PROTOTYPE (issue #138): tests whether the post-rebuild burst is deferred teardown of
        // the previous backend overlapping the next segment's measurement.
        if let Some(settle) = std::env::var("PROTOTYPE_SETTLE_MS")
            .ok()
            .and_then(|settle| settle.parse::<u64>().ok())
        {
            std::thread::sleep(Duration::from_millis(settle));
        }
        segment += 1;
        let segment_limit = std::env::var("PROTOTYPE_SEGMENTS")
            .ok()
            .and_then(|limit| limit.parse::<u64>().ok());
        if finished || segment_limit.is_some_and(|limit| segment >= limit) {
            return Ok(());
        }
    }
}

struct Application {
    output: File,
    kind: PathKind,
    coverage: Coverage,
    result: Option<RunResult>,
}

impl ApplicationHandler for Application {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.result = Some((|| {
            let window = event_loop.create_window(
                Window::default_attributes()
                    .with_visible(std::env::var_os("PROTOTYPE_VISIBLE").is_some())
                    .with_decorations(false)
                    .with_inner_size(winit::dpi::PhysicalSize::new(1920, 1080)),
            )?;
            windows_adapter::set_measurement_extent(
                &window,
                vk::Extent2D {
                    width: 1920,
                    height: 1080,
                },
            )?;
            run(&window, &mut self.output, self.kind, self.coverage)
        })());
        event_loop.exit();
    }

    fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, _: WindowEvent) {}
}

fn main() -> Result<(), String> {
    let mut arguments = std::env::args().skip(1);
    let usage = "usage: whole-world-detail-frames <raster|brickmap> <whole-world|full-detail-only> OUTPUT.jsonl";
    let kind = match arguments.next().as_deref() {
        Some("raster") => PathKind::Raster,
        Some("brickmap") => PathKind::Brickmap,
        _ => return Err(usage.into()),
    };
    let coverage = match arguments.next().as_deref() {
        Some("whole-world") => Coverage::WholeWorld,
        Some("full-detail-only") => Coverage::FullDetailOnly,
        _ => return Err(usage.into()),
    };
    let output = arguments.next().ok_or(usage)?;
    let mut application = Application {
        output: File::create(output).map_err(|error| error.to_string())?,
        kind,
        coverage,
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

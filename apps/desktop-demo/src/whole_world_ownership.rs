//! PROTOTYPE (issue #139): configured ownership per detail level and whole-world assembly
//! overhead.
//!
//! Throwaway. Each measurement constructs one Render Path under its allocation category and GPU
//! owner, configures it in a fresh Render Backend, draws until installed, then reads the
//! category's live and peak CPU bytes and the actual Vulkan allocations attributed to it. Scenes:
//! one volume at each detail level (generated and edited contents), and the 256-volume whole
//! world for every scene the route visits, under demo and qualification bands. Summaries are
//! measured as retained bytes of published coarse volumes, singly and as whole-world sets.
#[path = "streamed_fixture_allocations.rs"]
#[allow(dead_code)]
mod allocation;
#[allow(dead_code)]
mod streamed_fixture_recipe;
#[allow(dead_code)]
mod whole_world_detail_levels;
#[allow(dead_code)]
mod whole_world_route;
#[allow(dead_code)]
mod windows_adapter;

use allocation::{Category, LIVE};
use compute_ray_render_path::{ComputeRayRenderPathAdapter, ComputeRepresentation};
use raster_render_path::{RasterArtifactInstaller, RasterRenderPathAdapter, derive_raster_regions};
use render_backend::*;
use serde_json::{Value, json};
use std::{collections::BTreeSet, fs::File, io::Write, sync::atomic::Ordering, time::Instant};
use streamed_fixture_recipe::{materials, scene_contents};
use voxel_frontend::{
    SparseVoxelScene, VoxelEditOutcome, VoxelExtent, VoxelFrontend, VoxelResidencySelection,
    VoxelSceneId, VoxelSceneRevision, VoxelSceneView, VoxelValue,
};
use whole_world_detail_levels::{coarse_volume, downsample};
use whole_world_route::*;
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, EventLoop},
    window::{Window, WindowId},
};

/// Delegates every Render Path operation inside its allocation category and GPU owner, so the
/// ledgers attribute construction, configuration and frame-boundary work to this path.
struct Tagged {
    inner: Box<dyn SwitchableRenderPath>,
    category: Category,
    class: GpuAllocationClass,
}

impl Tagged {
    fn operation<T>(&mut self, operation: impl FnOnce(&mut dyn SwitchableRenderPath) -> T) -> T {
        let (category, class) = (self.category, self.class);
        allocation::within(category, || {
            with_gpu_allocation_owner(1, class, 0, || operation(self.inner.as_mut()))
        })
    }
}

impl RenderPath for Tagged {
    fn installed_residency_coverage(&self) -> Option<&RenderPathCoverage> {
        self.inner.installed_residency_coverage()
    }
    fn submit_residency_selection(
        &mut self,
        selection: VoxelResidencySelection,
    ) -> RenderPathResult<()> {
        self.operation(|path| path.submit_residency_selection(selection))
    }
    fn submit_edit_outcome(&mut self, outcome: VoxelEditOutcome) -> RenderPathResult<()> {
        self.operation(|path| path.submit_edit_outcome(outcome))
    }
    fn publish_camera_state(
        &mut self,
        camera: CameraState,
        revision: CameraStateRevision,
    ) -> RenderPathResult<()> {
        self.operation(|path| path.publish_camera_state(camera, revision))
    }
    fn release(&mut self, device: RenderPathDeviceContext<'_>) -> RenderPathResult<()> {
        self.operation(|path| path.release(device))
    }
    fn configure(
        &mut self,
        device: RenderPathDeviceContext<'_>,
        target: RenderPathTarget<'_>,
    ) -> RenderPathResult<()> {
        self.operation(|path| path.configure(device, target))
    }
    fn advance_frame_boundary(
        &mut self,
        device: RenderPathDeviceContext<'_>,
        target: RenderPathTarget<'_>,
    ) -> RenderPathResult<()> {
        self.operation(|path| path.advance_frame_boundary(device, target))
    }
    fn shutdown(&mut self, device: RenderPathDeviceContext<'_>) -> RenderPathResult<()> {
        self.operation(|path| path.shutdown(device))
    }
    fn record(&mut self, frame: RenderPathFrameContext<'_>) -> RenderPathResult<()> {
        self.operation(|path| path.record(frame))
    }
}

impl SwitchableRenderPath for Tagged {
    fn stamp(&self) -> RenderPathStamp {
        self.inner.stamp()
    }
    fn retire_at_frame_boundary(
        &mut self,
        device: RenderPathDeviceContext<'_>,
    ) -> RenderPathResult<RenderPathRetirement> {
        self.operation(|path| path.retire_at_frame_boundary(device))
    }
}

fn emit(output: &mut File, value: Value) -> RunResult {
    writeln!(output, "{value}")?;
    output.flush()?;
    Ok(())
}

fn class_index(class: GpuAllocationClass) -> usize {
    match class {
        GpuAllocationClass::Fixed => 0,
        GpuAllocationClass::Raster => 1,
        GpuAllocationClass::Brickmap => 2,
    }
}

/// Constructs, configures and installs one Render Path for `view`, then reports its ownership.
fn configured(
    window: &Window,
    gpu: &GpuAllocationQualification,
    kind: PathKind,
    view: &VoxelSceneView,
) -> RunResult<Value> {
    let (category, class) = match kind {
        PathKind::Raster => (Category::Raster, GpuAllocationClass::Raster),
        PathKind::Brickmap => (Category::Brickmap, GpuAllocationClass::Brickmap),
    };
    let (start, direction, _) = route_position(0.0).ok_or("empty route")?;
    let camera_state = camera(start, direction)?;
    allocation::reset_peaks();
    let cpu_before = allocation::live(category);
    let gpu_before = gpu.snapshot()?;
    let started = Instant::now();
    let (path, installer): (
        Box<dyn SwitchableRenderPath>,
        Option<RasterArtifactInstaller>,
    ) = allocation::within(
        category,
        || -> RunResult<(Box<dyn SwitchableRenderPath>, _)> {
            Ok(match kind {
                PathKind::Brickmap => (
                    Box::new(ComputeRayRenderPathAdapter::new_with_representation(
                        view.clone(),
                        camera_state,
                        CameraStateRevision::new(1),
                        ComputeRepresentation::Brickmap {
                            budget_bytes: 256 << 20,
                        },
                    )?),
                    None,
                ),
                PathKind::Raster => {
                    let (path, installer, _) =
                        RasterRenderPathAdapter::awaiting_artifact_with_camera_control(
                            camera_state,
                            CameraStateRevision::new(1),
                            view.scene_id().clone(),
                            view.revision(),
                        );
                    installer.publish_complete(derive_raster_regions(
                        view,
                        VoxelExtent::new(16, 16, 16),
                    )?)?;
                    (Box::new(path), Some(installer))
                }
            })
        },
    )?;
    let construction_ms = started.elapsed().as_secs_f64() * 1000.0;
    let constructed = allocation::live(category).saturating_sub(cpu_before);
    let mut backend = RenderBackend::initialize_with_options(
        c"Whole-world ownership prototype",
        &windows_adapter::WindowsPresentationAdapter::new(window),
        EXTENT,
        RenderPathSwitchOwner::new(Box::new(Tagged {
            inner: path,
            category,
            class,
        })),
        RenderBackendOptions {
            validation_enabled: false,
            presentation_throttling_enabled: false,
            gpu_timestamps_enabled: false,
        },
    )?;
    let mut installed_frames = 0;
    for _ in 0..600 {
        backend.draw_frame()?;
        let installed = installer.as_ref().is_none_or(|installer| {
            installer
                .installed_source_revision()
                .ok()
                .flatten()
                .is_some()
        });
        if installed {
            installed_frames += 1;
            if installed_frames == 8 {
                break;
            }
        }
    }
    if installed_frames < 8 {
        return Err("installation did not complete".into());
    }
    let cpu_live = allocation::live(category).saturating_sub(cpu_before);
    let cpu_peak = allocation::peak(category).saturating_sub(cpu_before);
    let gpu_after = gpu.snapshot()?;
    let index = class_index(class);
    let gpu_live = gpu_after.live_bytes[index].saturating_sub(gpu_before.live_bytes[index]);
    let gpu_allocations =
        gpu_after.live_allocations[index].saturating_sub(gpu_before.live_allocations[index]);
    let device = backend.runtime_context().device_name.clone();
    backend.shutdown()?;
    drop(backend);
    drop(installer);
    let debt = allocation::live(category).saturating_sub(cpu_before);
    let gpu_debt = gpu
        .snapshot()?
        .live_bytes
        .get(index)
        .copied()
        .unwrap_or(0)
        .saturating_sub(gpu_before.live_bytes[index]);
    Ok(
        json!({"path": format!("{kind:?}"), "volumes": view.volumes().len(),
        "cpu_constructed_bytes": constructed, "cpu_configured_live_bytes": cpu_live,
        "cpu_peak_bytes": cpu_peak, "gpu_live_bytes": gpu_live,
        "gpu_allocations": gpu_allocations, "construction_ms": construction_ms,
        "cpu_cleanup_debt": debt, "gpu_cleanup_debt": gpu_debt, "device": device}),
    )
}

/// Bytes a published scene retains once its input is consumed: what dropping it frees.
fn retained(scene: SparseVoxelScene) -> RunResult<usize> {
    let frontend = VoxelFrontend::new();
    let view = frontend.publish_sparse(scene)?;
    let published = LIVE.load(Ordering::SeqCst);
    drop(view);
    drop(frontend);
    Ok(published.saturating_sub(LIVE.load(Ordering::SeqCst)))
}

fn coarse_scene(volumes: Vec<voxel_frontend::SparseVoxelVolume>) -> SparseVoxelScene {
    SparseVoxelScene::new(
        VoxelSceneId::new("whole-world-detail"),
        VoxelSceneRevision::new(1),
        materials(),
        volumes,
    )
}

fn state_levels(edited: bool) -> RunResult<Vec<(u32, Vec<VoxelValue>)>> {
    let frontend = VoxelFrontend::new();
    let view = frontend.publish_sparse(scene_contents(&[(3, 3)], edited))?;
    Ok([32, 16, 8]
        .into_iter()
        .zip(downsample(&view, 3, 3)?)
        .collect())
}

fn run(window: &Window, output: &mut File) -> RunResult {
    let gpu = GpuAllocationQualification::start()?;
    emit(
        output,
        json!({"kind": "context", "prototype": "issue-139 configured ownership"}),
    )?;
    for (state, edited) in [("generated", false), ("edited", true)] {
        let levels = state_levels(edited)?;
        for edge in [64, 32, 16, 8] {
            let single = || -> SparseVoxelScene {
                if edge == 64 {
                    scene_contents(&[(3, 3)], edited)
                } else {
                    let values = levels
                        .iter()
                        .find(|(level, _)| *level == edge)
                        .map(|(_, values)| values.clone())
                        .expect("every coarse level was downsampled");
                    coarse_scene(vec![coarse_volume(3, 3, edge, values)])
                }
            };
            let summary = if edge == 64 {
                None
            } else {
                Some(retained(single())?)
            };
            let frontend = VoxelFrontend::new();
            let view = frontend.publish_sparse(single())?;
            for kind in [PathKind::Raster, PathKind::Brickmap] {
                let mut record = configured(window, &gpu, kind, &view)?;
                record["kind"] = json!("level");
                record["state"] = json!(state);
                record["edge"] = json!(edge);
                record["summary_retained_bytes"] = json!(summary);
                emit(output, record)?;
            }
        }
    }
    let levels = levels()?;
    let centres = route_scenes()
        .into_iter()
        .map(|(centre, ..)| centre)
        .collect::<BTreeSet<_>>();
    for (bands_name, bands) in [("demo", DEMO_BANDS), ("qualification", QUALIFICATION_BANDS)] {
        for centre in &centres {
            let mut counts = [0_usize; 4];
            for z in 0..GRID {
                for x in 0..GRID {
                    let edge = selected_edge(bands, *centre, x, z);
                    let slot = [64, 32, 16, 8]
                        .iter()
                        .position(|level| *level == edge)
                        .expect("selected edges are detail levels");
                    counts[slot] += 1;
                }
            }
            // Summaries: the resident 8³ set and the streamed 32³/16³ set, as two containers.
            let mut resident = Vec::new();
            let mut streamed = Vec::new();
            for z in 0..GRID {
                for x in 0..GRID {
                    let edge = selected_edge(bands, *centre, x, z);
                    for (level, values) in &levels {
                        let volume = || coarse_volume(x as u32, z as u32, *level, values.clone());
                        if *level == 8 {
                            resident.push(volume());
                        } else if *level == edge {
                            streamed.push(volume());
                        }
                    }
                }
            }
            let summary_resident = retained(coarse_scene(resident))?;
            let summary_streamed = retained(coarse_scene(streamed))?;
            let frontend = VoxelFrontend::new();
            let view = frontend.publish_sparse(build_scene_with_bands(&levels, *centre, bands))?;
            for kind in [PathKind::Raster, PathKind::Brickmap] {
                let mut record = configured(window, &gpu, kind, &view)?;
                record["kind"] = json!("whole-world");
                record["bands"] = json!(bands_name);
                record["centre"] = json!(centre);
                record["counts"] = json!(counts);
                record["summary_resident_retained_bytes"] = json!(summary_resident);
                record["summary_streamed_retained_bytes"] = json!(summary_streamed);
                emit(output, record)?;
            }
        }
    }
    Ok(())
}

struct Application {
    output: File,
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
            run(&window, &mut self.output)
        })());
        event_loop.exit();
    }

    fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, _: WindowEvent) {}
}

fn main() -> Result<(), String> {
    let output = std::env::args()
        .nth(1)
        .ok_or("usage: whole-world-ownership OUTPUT.jsonl")?;
    let mut application = Application {
        output: File::create(output).map_err(|error| error.to_string())?,
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

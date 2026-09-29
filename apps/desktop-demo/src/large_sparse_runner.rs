use super::{allocation, large_sparse_probes::probes, windows_adapter};
use ash::vk;
use compute_ray_render_path::{
    ComputeCapabilityAssessment, ComputeLifecycleController, ComputeMeasurementController,
    ComputeRayRenderPathAdapter, ComputeRepresentation, ComputeSemanticRayController,
    ComputeTimingPhase,
};
use measurement_evidence::large_sparse::{GPU_SCENE_BUDGET, LargeSparseRecord, TRANSITIONS};
use render_backend::{
    CameraState, CameraStateRevision, RenderBackend, RenderBackendOptions, RenderPath,
    RenderPathDeviceContext, RenderPathFrameContext, RenderPathResult, RenderPathTarget,
};
use semantic_ray_oracle::{
    SemanticRayDistanceTolerance, SemanticRayObservation, observe_along_ray,
};
use std::{
    cell::{Cell, RefCell},
    fs::File,
    io::Write,
    rc::Rc,
    sync::atomic::Ordering,
    time::{Duration, Instant},
};
use voxel_frontend::{
    VoxelCoordinate, VoxelEdit, VoxelEditCommand, VoxelFrontend, VoxelMaterialId, VoxelSceneView,
    VoxelValue, VoxelVolumeId, count_storage_work,
};
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, EventLoop},
    window::{Window, WindowId},
};

type RunResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

struct SharedPath {
    adapter: Rc<RefCell<ComputeRayRenderPathAdapter>>,
    scene_bytes: u64,
    allocation_bytes: Rc<Cell<u64>>,
}
impl RenderPath for SharedPath {
    fn release(&mut self, device: RenderPathDeviceContext<'_>) -> RenderPathResult<()> {
        self.adapter.borrow_mut().release(device)
    }
    fn configure(
        &mut self,
        device: RenderPathDeviceContext<'_>,
        target: RenderPathTarget<'_>,
    ) -> RenderPathResult<()> {
        self.adapter.borrow_mut().configure(device, target)?;
        // Query the same buffer description used by the owner without adding owner instrumentation.
        let description = vk::BufferCreateInfo::default()
            .size(self.scene_bytes)
            .usage(vk::BufferUsageFlags::STORAGE_BUFFER)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        let buffer = unsafe { device.create_buffer(&description) }?;
        self.allocation_bytes
            .set(unsafe { device.buffer_memory_requirements(buffer) }.size);
        unsafe { device.destroy_buffer(buffer) };
        Ok(())
    }
    fn advance_frame_boundary(
        &mut self,
        device: RenderPathDeviceContext<'_>,
        target: RenderPathTarget<'_>,
    ) -> RenderPathResult<()> {
        self.adapter
            .borrow_mut()
            .advance_frame_boundary(device, target)
    }
    fn shutdown(&mut self, device: RenderPathDeviceContext<'_>) -> RenderPathResult<()> {
        self.adapter.borrow_mut().shutdown(device)
    }
    fn record(&mut self, frame: RenderPathFrameContext<'_>) -> RenderPathResult<()> {
        self.adapter.borrow_mut().record(frame)
    }
}

fn record(output: &mut File, event: LargeSparseRecord) -> RunResult {
    serde_json::to_writer(&mut *output, &event)?;
    writeln!(output)?;
    Ok(())
}
fn timing(output: &mut File, name: &str, milliseconds: f64) -> RunResult {
    record(
        output,
        LargeSparseRecord::Timing {
            name: name.into(),
            milliseconds,
        },
    )
}
fn elapsed(started: Instant) -> f64 {
    started.elapsed().as_secs_f64() * 1000.0
}
fn heap_start() -> usize {
    let baseline = allocation::LIVE.load(Ordering::SeqCst);
    allocation::PEAK.store(baseline, Ordering::SeqCst);
    baseline
}
fn heap_peak(baseline: usize) -> u64 {
    allocation::PEAK
        .load(Ordering::SeqCst)
        .saturating_sub(baseline) as u64
}
fn resource_peak(controller: &ComputeLifecycleController) -> RunResult<u64> {
    Ok(controller
        .drain_resource_observations()?
        .iter()
        .map(|event| event.resources().bytes())
        .max()
        .unwrap_or(0))
}
fn phase_timing(
    controller: &ComputeMeasurementController,
    phase: ComputeTimingPhase,
) -> RunResult<(f64, u64)> {
    let events = controller.drain()?;
    let matching = events
        .iter()
        .filter(|event| event.phase() == phase)
        .collect::<Vec<_>>();
    if matching.len() != 1 {
        return Err(format!("expected one {phase:?} event, got {}", matching.len()).into());
    }
    let event = matching.first().ok_or("missing timing")?;
    Ok((event.elapsed_milliseconds(), event.uploaded_bytes()))
}

fn verify(
    backend: &mut RenderBackend,
    controller: &ComputeSemanticRayController,
    view: &VoxelSceneView,
    phase: usize,
    output: &mut File,
    name: &str,
) -> RunResult {
    let fixtures = probes(phase)?;
    let mut observations = Vec::new();
    for batch in fixtures.chunks(8) {
        controller.request(batch.iter().map(|(probe, _)| probe.clone()).collect())?;
        let expected = observations.len() + batch.len();
        let deadline = Instant::now() + Duration::from_secs(30);
        while observations.len() < expected {
            backend.draw_frame()?;
            observations.extend(controller.drain()?);
            if Instant::now() > deadline {
                return Err("semantic observation timed out".into());
            }
        }
    }
    if observations.len() != fixtures.len() {
        return Err("extra semantic observations".into());
    }
    let tolerance = SemanticRayDistanceTolerance::new(1.0e-6)?;
    for ((probe, expected), actual) in fixtures.iter().zip(observations) {
        let oracle = observe_along_ray(view, probe.ray())?;
        let analytical =
            SemanticRayObservation::new(view.scene_id().clone(), view.revision(), expected.clone());
        if actual.probe_identity() != probe.identity()
            || !oracle.agrees_with(&analytical, tolerance)
            || !actual.observation().agrees_with(&oracle, tolerance)
        {
            return Err(format!(
                "semantic disagreement for {} at {}",
                probe.identity(),
                view.revision()
            )
            .into());
        }
    }
    record(
        output,
        LargeSparseRecord::Verified {
            phase: name.into(),
            probe_count: fixtures.len(),
        },
    )
}

fn run(window: &Window, output: &mut File, first_frame_only: bool) -> RunResult {
    let publication_scope_baseline = allocation::LIVE.load(Ordering::SeqCst);
    let terrain = canonical_scene::generate_large_terrain()?;
    let fingerprint = format!("{:016x}", terrain.content_fingerprint());
    let scene = terrain.into_scene();
    let frontend = VoxelFrontend::new();
    heap_start();
    let first_started = Instant::now();
    let (view, counters) = count_storage_work(|| frontend.publish_sparse(scene));
    let publication_ms = elapsed(first_started);
    let publication_peak = heap_peak(publication_scope_baseline);
    let view = view?;
    let camera = CameraState::new(
        [32.5, 88.0, 32.5],
        [40.5, 80.0, 40.5],
        [0.0, 1.0, 0.0],
        60.0,
        0.1,
        4096.0,
    )?;
    let preparation_baseline = heap_start();
    let (mut path, measurement) =
        ComputeRayRenderPathAdapter::new_with_representation_and_measurement(
            view.clone(),
            camera,
            CameraStateRevision::new(1),
            ComputeRepresentation::Brickmap {
                budget_bytes: GPU_SCENE_BUDGET,
            },
        )?;
    let preparation_peak = heap_peak(preparation_baseline);
    let (preparation_ms, _) = phase_timing(&measurement, ComputeTimingPhase::Preparation)?;
    let semantic = path.enable_semantic_ray_observation();
    let convergence = path.enable_convergence_control();
    let lifecycle = path.enable_lifecycle_control();
    let scene_bytes = path.scene_bundle().storage_words().len() as u64 * 4;
    let allocation_bytes = Rc::new(Cell::new(0));
    let shared = Rc::new(RefCell::new(path));
    let mut backend = RenderBackend::initialize_with_options(
        c"Large sparse measurement",
        &windows_adapter::WindowsPresentationAdapter::new(window),
        vk::Extent2D {
            width: 1920,
            height: 1080,
        },
        SharedPath {
            adapter: shared.clone(),
            scene_bytes,
            allocation_bytes: allocation_bytes.clone(),
        },
        RenderBackendOptions {
            validation_enabled: true,
            presentation_throttling_enabled: false,
            gpu_timestamps_enabled: true,
        },
    )?;
    let result = (|| -> RunResult {
        backend.draw_frame()?;
        wait_completed(&mut backend)?;
        let first_ms = elapsed(first_started);
        let (upload_ms, staging_bytes) = phase_timing(&measurement, ComputeTimingPhase::Upload)?;
        resource_peak(&lifecycle)?;
        let initial_gpu = allocation_bytes.get();
        let device_limit = match shared.borrow().capability_assessment() {
            Some(ComputeCapabilityAssessment::Qualified(record)) => {
                record.queried().max_storage_buffer_range
            }
            _ => return Err("compute path was not qualified".into()),
        };
        let extent = backend
            .presentation_extent()
            .ok_or("no presentation extent")?;
        if extent.width != 1920 || extent.height != 1080 {
            return Err("measurement requires actual 1920x1080 presentation".into());
        }
        record(
            output,
            LargeSparseRecord::Context {
                schema_version: 1,
                device: backend.runtime_context().device_name.clone(),
                driver_version: backend.runtime_context().driver_version,
                max_storage_buffer_range_bytes: u64::from(device_limit),
                dense_payload_bytes: 4_294_967_296,
                extent: [2048, 256, 2048],
                presentation_extent: [extent.width, extent.height],
                fingerprint,
            },
        )?;
        for (name, value) in [
            ("first_correct_frame", first_ms),
            ("publication", publication_ms),
            (
                "validation",
                counters.validation.elapsed.as_secs_f64() * 1000.0,
            ),
            ("preparation", preparation_ms),
            ("upload", upload_ms),
        ] {
            timing(output, name, value)?;
        }
        verify(&mut backend, &semantic, &view, 0, output, "initial")?;

        let enumeration_baseline = heap_start();
        let enumeration_started = Instant::now();
        let mut output_bytes = 0;
        let mut working_bytes = 0;
        let (enumeration, enumeration_counters) = count_storage_work(|| -> RunResult {
            for batch in view.enumerate_cells(&VoxelVolumeId::new("large-terrain"), 8, 64)? {
                let batch = batch?;
                let with_output = allocation::LIVE.load(Ordering::SeqCst);
                drop(batch);
                let without_output = allocation::LIVE.load(Ordering::SeqCst);
                output_bytes = output_bytes.max(with_output.saturating_sub(without_output));
                working_bytes =
                    working_bytes.max(without_output.saturating_sub(enumeration_baseline));
            }
            Ok(())
        });
        enumeration?;
        let enumeration_ms = elapsed(enumeration_started);
        let enumeration_peak = heap_peak(enumeration_baseline);
        timing(output, "enumeration_replay", enumeration_ms)?;
        record(
            output,
            LargeSparseRecord::Memory {
                phase: "initial".into(),
                publication_peak_heap_bytes: publication_peak,
                enumeration_peak_heap_bytes: enumeration_peak,
                enumeration_output_bytes: output_bytes as u64,
                enumeration_working_state_bytes: working_bytes as u64,
                enumeration_working_cells: enumeration_counters.enumeration.peak_working_cells,
                preparation_peak_heap_bytes: preparation_peak,
                staging_bytes,
                visible_candidate_gpu_bytes: initial_gpu,
                measured_gpu_scene_peak_bytes: initial_gpu + staging_bytes,
            },
        )?;
        if first_frame_only {
            return Ok(());
        }
        drop(view);
        let edit_gpu_peak = initial_gpu;
        let mut edit_staging_peak = 0;
        for (index, name) in TRANSITIONS.iter().enumerate() {
            measurement.drain()?;
            let (coordinate, value) = match index {
                0 => ([800, 64, 800], VoxelValue::Empty),
                1 => (
                    [800, 64, 800],
                    VoxelValue::Occupied(VoxelMaterialId::new("terrain-stone")),
                ),
                2 => (
                    [800, 32, 800],
                    VoxelValue::Occupied(VoxelMaterialId::new("terrain-stone")),
                ),
                _ => ([800, 32, 800], VoxelValue::Empty),
            };
            let [x, y, z] = coordinate;
            let started = Instant::now();
            convergence.submit(frontend.edit(VoxelEditCommand::new(
                VoxelVolumeId::new("large-terrain"),
                VoxelCoordinate::new(x, y, z),
                value,
            ))?)?;
            let view = frontend.scene_view()?;
            wait_visible(&mut backend, &convergence, view.revision())?;
            timing(output, name, elapsed(started))?;
            if convergence.status()?.installed_growth.is_some() {
                return Err("ordinary edit unexpectedly grew the pool".into());
            }
            let (_, uploaded_bytes) = phase_timing(&measurement, ComputeTimingPhase::Upload)?;
            edit_staging_peak = edit_staging_peak.max(uploaded_bytes);
            resource_peak(&lifecycle)?;
            record(
                output,
                LargeSparseRecord::Edit {
                    transition: (*name).into(),
                    required_revision: index as u64 + 2,
                    visible_revision: view.revision().to_string().parse()?,
                    uploaded_bytes,
                },
            )?;
            verify(&mut backend, &semantic, &view, index + 1, output, name)?;
        }
        record(
            output,
            LargeSparseRecord::Memory {
                phase: "edits".into(),
                publication_peak_heap_bytes: 0,
                enumeration_peak_heap_bytes: 0,
                enumeration_output_bytes: 0,
                enumeration_working_state_bytes: 0,
                enumeration_working_cells: 0,
                preparation_peak_heap_bytes: 0,
                staging_bytes: edit_staging_peak,
                visible_candidate_gpu_bytes: edit_gpu_peak,
                measured_gpu_scene_peak_bytes: edit_gpu_peak + edit_staging_peak,
            },
        )?;

        let warmup = Instant::now();
        while warmup.elapsed() < Duration::from_secs(5) {
            backend.draw_frame()?;
            backend.take_frame_observation();
            measurement.drain()?;
            resource_peak(&lifecycle)?;
        }
        let mut frames = std::collections::BTreeMap::new();
        let mut samples = 0;
        let deadline = Instant::now() + Duration::from_secs(30);
        while samples < 60 {
            let started = Instant::now();
            backend.draw_frame()?;
            let cpu = elapsed(started);
            if let Some(sequence) = backend.last_submitted_frame_sequence() {
                frames.insert(sequence, cpu);
            }
            if let Some(observation) = backend.take_frame_observation()
                && let Some(cpu) = frames.remove(&observation.sequence)
            {
                timing(output, "steady_cpu", cpu)?;
                timing(output, "steady_gpu", observation.gpu_frame_milliseconds)?;
                samples += 1;
            }
            measurement.drain()?;
            resource_peak(&lifecycle)?;
            if Instant::now() > deadline {
                return Err("steady frame collection timed out".into());
            }
        }

        let edits = (0..16_641)
            .map(|index| {
                VoxelEdit::new(
                    VoxelVolumeId::new("large-terrain"),
                    VoxelCoordinate::new((index % 256) * 8, 200, 1024 + (index / 256) * 8),
                    VoxelValue::Occupied(VoxelMaterialId::new("terrain-stone")),
                )
            })
            .collect();
        measurement.drain()?;
        let growth_baseline = heap_start();
        let growth_started = Instant::now();
        convergence.submit(frontend.edit(VoxelEditCommand::from_edits(edits))?)?;
        let view = frontend.scene_view()?;
        wait_visible(&mut backend, &convergence, view.revision())?;
        let growth_ms = elapsed(growth_started);
        let growth_heap = heap_peak(growth_baseline);
        let growth = convergence
            .status()?
            .installed_growth
            .ok_or("pool did not grow")?;
        let (growth_upload_ms, growth_staging) =
            phase_timing(&measurement, ComputeTimingPhase::Upload)?;
        let actual_peak = growth
            .actual_peak_bytes
            .ok_or("missing measured growth allocation")?;
        timing(output, "growth", growth_ms)?;
        timing(
            output,
            "growth_rebuild",
            growth.rebuild_time.as_secs_f64() * 1000.0,
        )?;
        timing(output, "growth_upload", growth_upload_ms)?;
        record(
            output,
            LargeSparseRecord::Growth {
                old_capacity: growth.old_capacity,
                new_capacity: growth.new_capacity,
                predicted_peak_bytes: growth.predicted_peak_bytes,
                actual_peak_bytes: actual_peak,
            },
        )?;
        record(
            output,
            LargeSparseRecord::Memory {
                phase: "growth".into(),
                publication_peak_heap_bytes: 0,
                enumeration_peak_heap_bytes: 0,
                enumeration_output_bytes: 0,
                enumeration_working_state_bytes: 0,
                enumeration_working_cells: 0,
                preparation_peak_heap_bytes: growth_heap,
                staging_bytes: growth_staging,
                visible_candidate_gpu_bytes: actual_peak
                    .checked_sub(growth_staging)
                    .ok_or("growth staging exceeds peak")?,
                measured_gpu_scene_peak_bytes: actual_peak,
            },
        )?;
        verify(&mut backend, &semantic, &view, 5, output, "growth")?;

        Ok(())
    })();
    backend.shutdown()?;
    if backend.validation_error_count() != 0 || backend.validation_warning_count() != 0 {
        return Err("Vulkan validation was not clean".into());
    }
    result?;
    let shutdown_resources_zero = lifecycle
        .shutdown_owned_resources()?
        .is_some_and(|resources| resources.is_zero());
    if !shutdown_resources_zero {
        return Err("shutdown retained compute resources".into());
    }
    record(
        output,
        LargeSparseRecord::Completed {
            validation_errors: backend.validation_error_count(),
            validation_warnings: backend.validation_warning_count(),
            shutdown_resources_zero,
        },
    )
}

fn wait_completed(backend: &mut RenderBackend) -> RunResult {
    let submitted = backend
        .last_submitted_frame_sequence()
        .ok_or("no submitted frame")?;
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        backend.draw_frame()?;
        if backend
            .take_frame_observation()
            .is_some_and(|observation| observation.sequence >= submitted)
        {
            return Ok(());
        }
        if Instant::now() > deadline {
            return Err("frame completion timed out".into());
        }
    }
}

fn wait_visible(
    backend: &mut RenderBackend,
    convergence: &compute_ray_render_path::ComputeConvergenceController,
    revision: voxel_frontend::VoxelSceneRevision,
) -> RunResult {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        backend.draw_frame()?;
        let status = convergence.status()?;
        if status.visible_revision() == revision && status.required_revision() == revision {
            return wait_completed(backend);
        }
        if status.paused().is_some() || Instant::now() > deadline {
            return Err(format!("convergence failed: {:?}", convergence.drain_events()?).into());
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

struct Application {
    output: File,
    first_frame_only: bool,
    result: Option<RunResult>,
}
impl ApplicationHandler for Application {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.result = Some((|| {
            let window = event_loop.create_window(
                Window::default_attributes()
                    .with_visible(false)
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
            run(&window, &mut self.output, self.first_frame_only)
        })());
        event_loop.exit();
    }
    fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, _: WindowEvent) {}
}
pub fn main() -> Result<(), String> {
    let mut arguments = std::env::args().skip(1);
    let first_frame_only = match arguments.next().as_deref() {
        Some("first-correct-frame") => true,
        Some("complete") => false,
        _ => {
            return Err(
                "usage: large-sparse-measurement <first-correct-frame|complete> OUTPUT.jsonl"
                    .into(),
            );
        }
    };
    let output = arguments.next().ok_or("missing JSON Lines output path")?;
    if arguments.next().is_some() {
        return Err("unexpected argument".into());
    }
    let mut application = Application {
        first_frame_only,
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

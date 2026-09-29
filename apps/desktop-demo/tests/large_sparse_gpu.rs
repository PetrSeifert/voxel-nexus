#![cfg(windows)]

#[allow(dead_code)]
#[path = "../src/free_fly_camera.rs"]
mod free_fly_camera;
#[allow(dead_code)]
#[path = "../src/voxel_editing.rs"]
mod voxel_editing;
#[allow(dead_code)]
#[path = "../src/windows_adapter.rs"]
mod windows_adapter;

use ash::vk;
use compute_ray_render_path::{ComputeRayRenderPathAdapter, ComputeRepresentation};
use render_backend::{
    CameraState, CameraStateRevision, RenderBackend, RenderBackendOptions, RenderPathSwitchOwner,
};
use semantic_ray_oracle::{
    AxisNormal, SemanticRay, SemanticRayContact, SemanticRayContactClassification,
    SemanticRayDistanceTolerance, SemanticRayObservation, SemanticRayProbe, SemanticRayResult,
    observe_along_ray,
};
use voxel_frontend::{
    VoxelCoordinate, VoxelFrontend, VoxelMaterialId, VoxelSceneRevision, VoxelVolumeId,
};
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, EventLoop},
    platform::windows::EventLoopBuilderExtWindows,
    window::{Window, WindowId},
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn contact(
    coordinate: [i32; 3],
    material: &str,
    distance: f64,
    classification: SemanticRayContactClassification,
) -> SemanticRayResult {
    let [x, y, z] = coordinate;
    SemanticRayResult::Contact(SemanticRayContact::new(
        VoxelVolumeId::new("large-terrain"),
        VoxelCoordinate::new(x, y, z),
        VoxelMaterialId::new(material),
        distance,
        classification,
    ))
}

fn probes(
    phase: usize,
) -> Result<Vec<(SemanticRayProbe, SemanticRayResult)>, Box<dyn std::error::Error>> {
    use SemanticRayContactClassification::{Entered, StartedInside};
    let roof = if phase == 1 { 65 } else { 64 };
    let floor = if phase == 3 { 32 } else { 31 };
    [
        (
            "long-empty-miss",
            [2047.5, 255.5, 2047.5],
            [-1.0, 0.0, 0.0],
            4096.0,
            SemanticRayResult::Miss,
        ),
        (
            "long-empty-hit",
            [-2048.0, 80.5, 0.5],
            [1.0, 0.0, 0.0],
            4096.0,
            contact(
                [0, 80, 0],
                "terrain-grass",
                2048.0,
                Entered(AxisNormal::NegativeX),
            ),
        ),
        (
            "fill-interior",
            [8.5, 8.5, 8.5],
            [1.0, 0.0, 0.0],
            100.0,
            contact([8, 8, 8], "terrain-stone", 0.0, StartedInside),
        ),
        (
            "mixed-surface",
            [32.5, 255.5, 32.5],
            [0.0, -1.0, 0.0],
            256.0,
            contact(
                [32, 81, 32],
                "terrain-grass",
                173.5,
                Entered(AxisNormal::PositiveY),
            ),
        ),
        (
            "cavity-wall",
            [800.5, 48.0, 800.5],
            [1.0, 0.0, 0.0],
            1024.0,
            contact(
                [1024, 48, 800],
                "terrain-stone",
                223.5,
                Entered(AxisNormal::NegativeX),
            ),
        ),
        (
            "cavity-miss",
            [800.5, 48.0, 800.5],
            [1.0, 0.0, 0.0],
            100.0,
            SemanticRayResult::Miss,
        ),
        (
            "edited-roof",
            [800.5, 60.0, 800.5],
            [0.0, 1.0, 0.0],
            100.0,
            contact(
                [800, roof, 800],
                "terrain-stone",
                if phase == 1 { 5.0 } else { 4.0 },
                Entered(AxisNormal::NegativeY),
            ),
        ),
        (
            "edited-floor",
            [800.5, 40.0, 800.5],
            [0.0, -1.0, 0.0],
            100.0,
            contact(
                [800, floor, 800],
                "terrain-stone",
                if phase == 3 { 7.0 } else { 8.0 },
                Entered(AxisNormal::PositiveY),
            ),
        ),
    ]
    .into_iter()
    .map(|(name, origin, direction, maximum, expected)| {
        Ok((
            SemanticRayProbe::new(name, SemanticRay::new(origin, direction, 0.0, maximum)?)?,
            expected,
        ))
    })
    .collect()
}

fn qualify(window: &Window) -> TestResult {
    let frontend = VoxelFrontend::new();
    let initial =
        frontend.publish_sparse(canonical_scene::generate_large_terrain()?.into_scene())?;
    let camera = CameraState::new(
        [32.5, 88.0, 32.5],
        [40.5, 80.0, 40.5],
        [0.0, 1.0, 0.0],
        60.0,
        0.1,
        4096.0,
    )?;
    let mut path = ComputeRayRenderPathAdapter::new_with_representation(
        initial,
        camera,
        CameraStateRevision::new(1),
        ComputeRepresentation::Brickmap {
            budget_bytes: 1 << 30,
        },
    )?;
    let controller = path.enable_semantic_ray_observation();
    let mut backend = RenderBackend::initialize_with_options(
        c"Large sparse terrain qualification",
        &windows_adapter::WindowsPresentationAdapter::new(window),
        vk::Extent2D {
            width: 64,
            height: 64,
        },
        RenderPathSwitchOwner::new(Box::new(path)),
        RenderBackendOptions {
            validation_enabled: true,
            presentation_throttling_enabled: false,
            gpu_timestamps_enabled: false,
        },
    )?;
    println!("{}", backend.runtime_context());
    let result = (|| -> TestResult {
        for phase in 0..5 {
            let view = frontend.scene_view()?;
            assert_eq!(view.revision(), VoxelSceneRevision::new(phase as u64 + 1));
            let fixtures = probes(phase)?;
            controller.request(fixtures.iter().map(|(probe, _)| probe.clone()).collect())?;
            let mut observations = Vec::new();
            for _ in 0..16 {
                backend.draw_frame()?;
                observations.extend(controller.drain()?);
                if observations.len() == fixtures.len() {
                    break;
                }
            }
            assert_eq!(observations.len(), fixtures.len());
            let tolerance = SemanticRayDistanceTolerance::new(1.0e-6)?;
            for ((probe, analytical), actual) in fixtures.iter().zip(&observations) {
                let oracle = observe_along_ray(&view, probe.ray())?;
                let analytical = SemanticRayObservation::new(
                    view.scene_id().clone(),
                    view.revision(),
                    analytical.clone(),
                );
                assert!(
                    oracle.agrees_with(&analytical, tolerance),
                    "{}: oracle {oracle:?}, analytical {analytical:?}",
                    probe.identity()
                );
                assert_eq!(actual.probe_identity(), probe.identity());
                assert_eq!(actual.observation().revision(), view.revision());
                assert!(
                    actual.observation().agrees_with(&oracle, tolerance),
                    "{}: GPU {:?}, oracle {oracle:?}",
                    probe.identity(),
                    actual.observation()
                );
                println!(
                    "phase={phase} probe={} installed_revision={} result={:?}",
                    probe.identity(),
                    actual.observation().revision(),
                    actual.observation().result()
                );
            }
            use voxel_editing::EditAction::{Break, Place};
            let (action, origin, direction, transition) = match phase {
                0 => (
                    Break,
                    [800.5, 60.0, 800.5],
                    [0.0, 1.0, 0.0],
                    "uniform-to-mixed",
                ),
                1 => (
                    Place(VoxelMaterialId::new("terrain-stone")),
                    [800.5, 60.0, 800.5],
                    [0.0, 1.0, 0.0],
                    "mixed-to-uniform",
                ),
                2 => (
                    Place(VoxelMaterialId::new("terrain-stone")),
                    [800.5, 40.0, 800.5],
                    [0.0, -1.0, 0.0],
                    "empty-to-mixed",
                ),
                3 => (
                    Break,
                    [800.5, 40.0, 800.5],
                    [0.0, -1.0, 0.0],
                    "mixed-to-empty",
                ),
                _ => break,
            };
            let observation =
                observe_along_ray(&view, &SemanticRay::new(origin, direction, 0.0, 100.0)?)?;
            let command = voxel_editing::edit_command(&action, &observation, view.volumes())
                .map_err(|error| format!("edit rejected: {error:?}"))?;
            voxel_editing::publish_edit(&frontend, command, |outcome| {
                backend
                    .submit_edit_outcome(outcome)
                    .map_err(|error| error.to_string())
            })?;
            let required = frontend.scene_view()?.revision();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
            loop {
                backend.draw_frame()?;
                let diagnostics = backend
                    .render_path_switch_diagnostics()
                    .ok_or("missing diagnostics")?;
                assert_eq!(diagnostics.presenting().required_revision(), required);
                if diagnostics.presenting().visible_revision() == required {
                    break;
                }
                if std::time::Instant::now() >= deadline {
                    return Err("large terrain failed to converge".into());
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            println!("transition={transition} Required={required} Visible={required}");
        }
        Ok(())
    })();
    backend.shutdown()?;
    assert_eq!(backend.validation_error_count(), 0);
    assert_eq!(backend.validation_warning_count(), 0);
    result
}

#[derive(Default)]
struct Qualification {
    result: Option<TestResult>,
}

impl ApplicationHandler for Qualification {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.result = Some((|| {
            let window = event_loop.create_window(
                Window::default_attributes()
                    .with_visible(false)
                    .with_inner_size(winit::dpi::PhysicalSize::new(64, 64)),
            )?;
            qualify(&window)
        })());
        event_loop.exit();
    }
    fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, _: WindowEvent) {}
}

#[test]
#[ignore = "explicit large terrain qualification requires a capable Windows Vulkan 1.3 GPU and validation layer"]
fn large_sparse_terrain_gpu_qualification() -> TestResult {
    let event_loop = EventLoop::builder().with_any_thread(true).build()?;
    let mut application = Qualification::default();
    event_loop.run_app(&mut application)?;
    application.result.ok_or("qualification did not run")?
}

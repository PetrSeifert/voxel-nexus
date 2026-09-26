#![cfg(windows)]

#[allow(dead_code)]
#[path = "../src/windows_adapter.rs"]
mod windows_adapter;

use ash::vk;
use compute_ray_render_path::ComputeRayRenderPathAdapter;
use render_backend::{CameraState, CameraStateRevision, RenderBackend, RenderBackendOptions};
use semantic_ray_oracle::{SemanticRay, SemanticRayDistanceTolerance, SemanticRayProbe, observe};
use voxel_frontend::{
    DenseVoxelBatch, DenseVoxelScene, DenseVoxelVolume, VoxelCoordinate, VoxelExtent,
    VoxelFrontend, VoxelMaterial, VoxelMaterialId, VoxelRegion, VoxelSceneId, VoxelSceneRevision,
    VoxelSceneView, VoxelValue, VoxelVolumeId, VoxelVolumeMetadata,
};
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, EventLoop},
    platform::windows::EventLoopBuilderExtWindows,
    window::{Window, WindowId},
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn scene(
    voxel_size: f32,
    extent: VoxelExtent,
    occupied_index: usize,
) -> Result<VoxelSceneView, Box<dyn std::error::Error>> {
    let material = VoxelMaterialId::new("stone");
    let mut values =
        vec![VoxelValue::Empty; usize::try_from(extent.dimensions().into_iter().product::<u32>())?];
    *values
        .get_mut(occupied_index)
        .ok_or("invalid fixture coordinate")? = VoxelValue::Occupied(material.clone());
    Ok(VoxelFrontend::new().publish(DenseVoxelScene::new(
        VoxelSceneId::new("gpu-dda-regression"),
        VoxelSceneRevision::new(1),
        vec![VoxelMaterial::new(material, [1.0; 4])],
        vec![DenseVoxelVolume::new(
            VoxelVolumeMetadata::new(VoxelVolumeId::new("volume"), extent, [0.0; 3], voxel_size),
            vec![DenseVoxelBatch::new(
                VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), extent),
                values,
            )],
        )],
    ))?)
}

fn check_gpu(
    window: &Window,
    view: VoxelSceneView,
    probes: Vec<SemanticRayProbe>,
    scale: f64,
) -> TestResult {
    let camera = CameraState::new([0.0, 0.0, 5.0], [0.0; 3], [0.0, 1.0, 0.0], 50.0, 0.1, 100.0)?;
    let mut path =
        ComputeRayRenderPathAdapter::new(view.clone(), camera, CameraStateRevision::new(1))?;
    let controller = path.enable_semantic_ray_observation();
    controller.request(probes.clone())?;
    let size = window.inner_size();
    let mut backend = RenderBackend::initialize_with_options(
        c"Compute DDA GPU regression",
        &windows_adapter::WindowsPresentationAdapter::new(window),
        vk::Extent2D {
            width: size.width,
            height: size.height,
        },
        path,
        RenderBackendOptions {
            validation_enabled: true,
            presentation_throttling_enabled: false,
            gpu_timestamps_enabled: false,
        },
    )?;
    println!("{}", backend.runtime_context());
    let result = (|| -> TestResult {
        let mut observations = Vec::new();
        for _ in 0..8 {
            backend.draw_frame()?;
            observations.extend(controller.drain()?);
            if observations.len() == probes.len() {
                break;
            }
        }
        if observations.len() != probes.len() {
            return Err("installed GPU traversal did not return every probe".into());
        }
        let tolerance = SemanticRayDistanceTolerance::new(scale * 1.0e-6)?;
        for (probe, actual) in probes.iter().zip(&observations) {
            let expected = observe(&view, probe.ray())?;
            if actual.probe_identity() != probe.identity()
                || !actual.observation().agrees_with(&expected, tolerance)
            {
                return Err(format!(
                    "{}: GPU {:?}, oracle {expected:?}",
                    probe.identity(),
                    actual.observation()
                )
                .into());
            }
        }
        Ok(())
    })();
    backend.shutdown()?;
    assert_eq!(backend.validation_error_count(), 0);
    result
}

fn run_fixtures(window: &Window) -> TestResult {
    for voxel_size in [1.0_f32, 0.125, 16.0] {
        let scale = f64::from(voxel_size);
        for negative in [false, true] {
            for first_axis in 0..3 {
                let second_axis = (first_axis + 1) % 3;
                let mut origin = [0.5; 3];
                let mut direction = [0.0; 3];
                let mut coordinate = [0_usize; 3];
                if negative {
                    origin[first_axis] = 1.5;
                    origin[second_axis] = 1.500002;
                    direction[first_axis] = -1.0;
                    direction[second_axis] = -1.0;
                    coordinate[second_axis] = 1;
                } else {
                    origin[second_axis] = 0.499998;
                    direction[first_axis] = 1.0;
                    direction[second_axis] = 1.0;
                    coordinate[first_axis] = 1;
                }
                // Use the same representable origin in the oracle and the GPU input buffer.
                let origin = origin.map(|component| f64::from((component * scale) as f32));
                let occupied_index = coordinate[0] + 2 * coordinate[1] + 4 * coordinate[2];
                let probe = SemanticRayProbe::new(
                    format!("near-crossing-scale-{scale}-negative-{negative}-axis-{first_axis}"),
                    SemanticRay::new(origin, direction, 0.0, 4.0 * scale)?,
                )?;
                let mut simultaneous_origin = origin;
                simultaneous_origin[second_axis] = simultaneous_origin[first_axis];
                let simultaneous = SemanticRayProbe::new(
                    "simultaneous-crossing-skips-zero-length-cell",
                    SemanticRay::new(simultaneous_origin, direction, 0.0, 4.0 * scale)?,
                )?;
                let extent = VoxelExtent::new(2, 2, if first_axis == 0 { 1 } else { 2 });
                check_gpu(
                    window,
                    scene(voxel_size, extent, occupied_index)?,
                    vec![probe, simultaneous],
                    scale,
                )?;
            }
        }

        let rays = [
            ("true-corner", [-1.0; 3], [1.0; 3], 0.0, 6.0),
            ("true-edge", [0.5, 0.5, 1.5], [1.0, 1.0, 0.0], 0.0, 4.0),
            (
                "maximum-inward",
                [2.0, 1.5, 1.5],
                [-1.0, 0.0, 0.0],
                0.0,
                2.0,
            ),
            (
                "internal-boundary",
                [1.0, 1.5, 1.5],
                [-1.0, 0.0, 0.0],
                0.0,
                2.0,
            ),
            (
                "clipped-before-contact",
                [-2.0, 1.5, 1.5],
                [1.0, 0.0, 0.0],
                0.0,
                1.5,
            ),
            (
                "half-open-maximum",
                [-1.0, 2.0, 1.5],
                [1.0, 0.0, 0.0],
                0.0,
                4.0,
            ),
            (
                "clipped-inside",
                [-1.0, 1.5, 1.5],
                [1.0, 0.0, 0.0],
                2.5,
                4.0,
            ),
        ];
        let probes = rays
            .into_iter()
            .map(|(name, origin, direction, minimum, maximum)| {
                Ok(SemanticRayProbe::new(
                    name,
                    SemanticRay::new(
                        origin.map(|component| component * scale),
                        direction,
                        minimum * scale,
                        maximum * scale,
                    )?,
                )?)
            })
            .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;
        check_gpu(
            window,
            scene(voxel_size, VoxelExtent::new(2, 2, 2), 7)?,
            probes,
            scale,
        )?;
        let probes = [
            ("negative-true-corner", [1.5; 3], [-1.0; 3]),
            ("negative-true-edge", [1.5, 1.5, 0.5], [-1.0, -1.0, 0.0]),
        ]
        .into_iter()
        .map(|(name, origin, direction)| {
            Ok(SemanticRayProbe::new(
                name,
                SemanticRay::new(
                    origin.map(|component| component * scale),
                    direction,
                    0.0,
                    4.0 * scale,
                )?,
            )?)
        })
        .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;
        check_gpu(
            window,
            scene(voxel_size, VoxelExtent::new(2, 2, 2), 0)?,
            probes,
            scale,
        )?;
    }
    Ok(())
}

#[derive(Default)]
struct GpuTest {
    result: Option<TestResult>,
}

impl ApplicationHandler for GpuTest {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.result = Some((|| {
            let window = event_loop.create_window(
                Window::default_attributes()
                    .with_visible(false)
                    .with_inner_size(winit::dpi::PhysicalSize::new(64, 64)),
            )?;
            run_fixtures(&window)
        })());
        event_loop.exit();
    }

    fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, _: WindowEvent) {}
}

#[test]
#[ignore = "requires a Windows Vulkan 1.3 GPU and the Vulkan validation layer"]
fn installed_gpu_dda_matches_semantic_oracle() -> TestResult {
    let event_loop = EventLoop::builder().with_any_thread(true).build()?;
    let mut application = GpuTest::default();
    event_loop.run_app(&mut application)?;
    application.result.ok_or("GPU test did not run")?
}

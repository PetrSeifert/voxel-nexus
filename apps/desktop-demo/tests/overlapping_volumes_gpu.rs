#![cfg(windows)]
use compute_ray_render_path::COMPUTE_RAY_STRATEGY;
use raster_render_path::RASTER_STRATEGY;

#[allow(dead_code)]
#[path = "../src/windows_adapter.rs"]
mod windows_adapter;

use ash::vk;
use compute_ray_render_path::ComputeRayRenderPathAdapter;
use raster_render_path::{RasterRenderPathAdapter, derive_raster_regions};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use render_backend::{
    CameraState, CameraStateRevision, RenderBackend, RenderBackendOptions, RenderPathStrategy,
    RenderPathSwitchOwner,
};
use semantic_ray_oracle::{SemanticRay, SemanticRayResult, observe};
use std::time::{Duration, Instant};
use voxel_frontend::{
    DenseVoxelBatch, DenseVoxelScene, DenseVoxelVolume, VoxelCoordinate, VoxelExtent,
    VoxelFrontend, VoxelMaterial, VoxelMaterialId, VoxelRegion, VoxelSceneId, VoxelSceneRevision,
    VoxelSceneView, VoxelValue, VoxelVolumeId, VoxelVolumeMetadata,
};
use windows_sys::Win32::Graphics::{
    Dwm::DwmFlush,
    Gdi::{CLR_INVALID, GetDC, GetPixel, ReleaseDC},
};
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, EventLoop},
    platform::windows::EventLoopBuilderExtWindows,
    window::{Window, WindowId, WindowLevel},
};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn scene(reverse: bool, zeta_depth: f32) -> TestResult<VoxelSceneView> {
    let extent = VoxelExtent::new(1, 1, 1);
    let mut volumes = [("zeta", "red", zeta_depth), ("alpha", "green", 0.0)]
        .into_iter()
        .map(|(identity, material, depth)| {
            DenseVoxelVolume::new(
                VoxelVolumeMetadata::new(
                    VoxelVolumeId::new(identity),
                    extent,
                    [0.0, 0.0, depth],
                    1.0,
                ),
                vec![DenseVoxelBatch::new(
                    VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), extent),
                    vec![VoxelValue::Occupied(VoxelMaterialId::new(material))],
                )],
            )
        })
        .collect::<Vec<_>>();
    if reverse {
        volumes.reverse();
    }
    Ok(VoxelFrontend::new().publish(DenseVoxelScene::new(
        VoxelSceneId::new("overlapping-volumes"),
        VoxelSceneRevision::new(1),
        vec![
            VoxelMaterial::new(VoxelMaterialId::new("red"), [1.0, 0.0, 0.0, 1.0]),
            VoxelMaterial::new(VoxelMaterialId::new("green"), [0.0, 1.0, 0.0, 1.0]),
        ],
        volumes,
    ))?)
}

fn raster(view: &VoxelSceneView, camera: CameraState) -> TestResult<RasterRenderPathAdapter> {
    let (path, installer, _) = RasterRenderPathAdapter::awaiting_artifact_with_camera_control(
        camera,
        CameraStateRevision::new(1),
        view.scene_id().clone(),
        view.revision(),
    );
    installer.publish_complete(derive_raster_regions(view, VoxelExtent::new(1, 1, 1))?)?;
    Ok(path)
}

fn center_pixels(window: &Window) -> TestResult<Vec<[u8; 3]>> {
    let RawWindowHandle::Win32(handle) = window.window_handle()?.as_raw() else {
        return Err("expected a Win32 window".into());
    };
    let window_handle = handle.hwnd.get() as windows_sys::Win32::Foundation::HWND;
    let size = window.inner_size();
    let center_x = i32::try_from(size.width / 2)?;
    let center_y = i32::try_from(size.height / 2)?;
    unsafe {
        let context = GetDC(window_handle);
        if context.is_null() {
            return Err(std::io::Error::last_os_error().into());
        }
        let result = (|| {
            let mut pixels = Vec::new();
            for vertical in [-4, 0, 4] {
                for horizontal in [-4, 0, 4] {
                    let color = GetPixel(context, center_x + horizontal, center_y + vertical);
                    if color == CLR_INVALID {
                        return Err("could not read the rendered client pixels".into());
                    }
                    pixels.push([color as u8, (color >> 8) as u8, (color >> 16) as u8]);
                }
            }
            Ok(pixels)
        })();
        if ReleaseDC(window_handle, context) == 0 {
            return Err("could not release the window device context".into());
        }
        result
    }
}

fn check_presenter(
    window: &Window,
    backend: &mut RenderBackend,
    strategy: RenderPathStrategy,
    expected_material: &VoxelMaterialId,
) -> TestResult {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        backend.draw_frame()?;
        let diagnostics = backend
            .render_path_switch_diagnostics()
            .ok_or("missing switch diagnostics")?;
        if diagnostics.roles().presenting() == strategy
            && diagnostics.replacement().is_none()
            && diagnostics.retiring().is_none()
        {
            break;
        }
        if Instant::now() >= deadline {
            return Err(format!("switch to {strategy:?} timed out").into());
        }
    }
    // Let the desktop compositor display completed frames before sampling the client.
    for _ in 0..3 {
        backend.draw_frame()?;
        std::thread::sleep(Duration::from_millis(30));
    }
    if unsafe { DwmFlush() } < 0 {
        return Err("could not synchronize with the desktop compositor".into());
    }
    let pixels = center_pixels(window)?;
    let expected_red = expected_material == &VoxelMaterialId::new("red");
    for [red, green, blue] in &pixels {
        let matches = if expected_red {
            *red > 100 && *green < 20 && *blue < 20
        } else {
            *green > 100 && *red < 20 && *blue < 20
        };
        if !matches {
            return Err(format!(
                "{strategy:?}: expected {expected_material:?}, rendered RGB samples {pixels:?}"
            )
            .into());
        }
    }
    println!(
        "{strategy:?}: selected {expected_material:?}, center RGB {:?}",
        pixels.first()
    );
    Ok(())
}

fn run_fixtures(event_loop: &ActiveEventLoop) -> TestResult {
    let camera = CameraState::new(
        [0.5, 0.5, 4.0],
        [0.5, 0.5, 0.5],
        [0.0, 1.0, 0.0],
        45.0,
        0.1,
        20.0,
    )?;
    for zeta_depth in [0.25, 0.0, -0.25] {
        for reverse in [false, true] {
            let window = event_loop.create_window(
                Window::default_attributes()
                    .with_title("Overlapping volumes GPU regression")
                    .with_window_level(WindowLevel::AlwaysOnTop)
                    .with_inner_size(winit::dpi::PhysicalSize::new(128, 128)),
            )?;
            let window = &window;
            let view = scene(reverse, zeta_depth)?;
            let observation = observe(
                &view,
                &SemanticRay::new([0.5, 0.5, 4.0], [0.0, 0.0, -1.0], 0.1, 20.0)?,
            )?;
            let SemanticRayResult::Contact(contact) = observation.result() else {
                return Err("fixture ray missed both volumes".into());
            };
            println!("reverse={reverse}, zeta_depth={zeta_depth}, oracle={contact:?}");
            let owner = RenderPathSwitchOwner::new(Box::new(raster(&view, camera)?));
            let size = window.inner_size();
            let mut backend = RenderBackend::initialize_with_options(
                c"Overlapping volumes GPU regression",
                &windows_adapter::WindowsPresentationAdapter::new(window),
                vk::Extent2D {
                    width: size.width,
                    height: size.height,
                },
                owner,
                RenderBackendOptions {
                    validation_enabled: true,
                    presentation_throttling_enabled: true,
                    gpu_timestamps_enabled: false,
                },
            )?;
            println!("{}", backend.runtime_context());
            let result = (|| -> TestResult {
                check_presenter(
                    window,
                    &mut backend,
                    RASTER_STRATEGY,
                    contact.material_identity(),
                )?;
                backend.request_render_path_switch(Box::new(ComputeRayRenderPathAdapter::new(
                    view.clone(),
                    camera,
                    CameraStateRevision::new(1),
                )?))?;
                check_presenter(
                    window,
                    &mut backend,
                    COMPUTE_RAY_STRATEGY,
                    contact.material_identity(),
                )?;
                backend.request_render_path_switch(Box::new(raster(&view, camera)?))?;
                check_presenter(
                    window,
                    &mut backend,
                    RASTER_STRATEGY,
                    contact.material_identity(),
                )?;
                Ok(())
            })();
            backend.shutdown()?;
            assert_eq!(backend.validation_error_count(), 0);
            result?;
        }
    }
    Ok(())
}

#[derive(Default)]
struct GpuTest {
    result: Option<TestResult>,
}

impl ApplicationHandler for GpuTest {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.result = Some(run_fixtures(event_loop));
        event_loop.exit();
    }
    fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, _: WindowEvent) {}
}

#[test]
#[ignore = "requires Windows Vulkan 1.3, validation layers, and an unobscured desktop for pixel capture"]
fn overlapping_volumes_keep_the_oracle_material_when_switching() -> TestResult {
    let event_loop = EventLoop::builder().with_any_thread(true).build()?;
    let mut application = GpuTest::default();
    event_loop.run_app(&mut application)?;
    application.result.ok_or("GPU test did not run")?
}

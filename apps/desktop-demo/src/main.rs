#[cfg(target_os = "windows")]
mod windows_adapter;

use canonical_inspection::{CanonicalCameraPose, overview_to_cavity_camera_move};
use canonical_scene::{CanonicalSceneMetadata, CanonicalSceneScale, generate_canonical_scene};
use raster_render_path::{
    CameraPose, RasterArtifactInstallationError, RasterArtifactInstallationPhase, RasterRenderPath,
    derive_raster_artifact,
};
use render_backend::{
    DeviceCandidate, QueueFamilyCapabilities, RenderPathPhase, run_render_path_phase,
};
#[cfg(target_os = "windows")]
use render_backend::{FrameOutcome, RenderBackend};
use std::process::ExitCode;
#[cfg(target_os = "windows")]
use std::time::{Duration, Instant};
use std::{error::Error, fmt};
use voxel_frontend::{VoxelFrontend, VoxelSceneRevision};
#[cfg(target_os = "windows")]
use windows_adapter::WindowsPresentationAdapter;
#[cfg(target_os = "windows")]
use winit::application::ApplicationHandler;
#[cfg(target_os = "windows")]
use winit::event::WindowEvent;
#[cfg(target_os = "windows")]
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
#[cfg(target_os = "windows")]
use winit::window::{Window, WindowAttributes, WindowId};

#[derive(Clone, Copy)]
enum CanonicalCameraSelection {
    Fixed(CanonicalCameraPose),
    MoveStep {
        step: u32,
        total_steps: u32,
        pose: CameraPose,
    },
}

impl CanonicalCameraSelection {
    fn pose(self) -> CameraPose {
        match self {
            Self::Fixed(identity) => identity.pose(),
            Self::MoveStep { pose, .. } => pose,
        }
    }

    fn report_identity(self) -> String {
        match self {
            Self::Fixed(CanonicalCameraPose::Overview) => "overview".to_owned(),
            Self::Fixed(CanonicalCameraPose::CavityMaterialCloseUp) => "cavity".to_owned(),
            Self::Fixed(CanonicalCameraPose::BoundaryCutaway) => "boundary".to_owned(),
            Self::MoveStep {
                step, total_steps, ..
            } => format!("overview-to-cavity-step-{step}-of-{total_steps}"),
        }
    }
}

#[derive(Clone)]
struct CanonicalRenderConfiguration {
    scale: CanonicalSceneScale,
    camera: CanonicalCameraSelection,
}

fn parse_canonical_configuration(
    mut arguments: impl Iterator<Item = String>,
) -> Result<(CanonicalRenderConfiguration, bool), String> {
    let mut scale = CanonicalSceneScale::Large;
    let mut camera = CanonicalCameraSelection::Fixed(CanonicalCameraPose::Overview);
    let mut camera_was_selected = false;
    let mut report_only = false;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--report-canonical-configuration" => report_only = true,
            "--scene-scale" => {
                scale = match arguments.next().as_deref() {
                    Some("64") => CanonicalSceneScale::Small,
                    Some("128") => CanonicalSceneScale::Medium,
                    Some("256") => CanonicalSceneScale::Large,
                    Some(value) => {
                        return Err(format!(
                            "unknown canonical scene scale {value:?}; expected 64, 128, or 256"
                        ));
                    }
                    None => return Err("missing canonical scene scale".to_owned()),
                };
            }
            "--camera-pose" => {
                if camera_was_selected {
                    return Err("select either one fixed camera pose or one move step".to_owned());
                }
                camera = match arguments.next().as_deref() {
                    Some("overview") => {
                        CanonicalCameraSelection::Fixed(CanonicalCameraPose::Overview)
                    }
                    Some("cavity") => {
                        CanonicalCameraSelection::Fixed(CanonicalCameraPose::CavityMaterialCloseUp)
                    }
                    Some("boundary") => {
                        CanonicalCameraSelection::Fixed(CanonicalCameraPose::BoundaryCutaway)
                    }
                    Some(value) => {
                        return Err(format!(
                            "unknown canonical camera pose {value:?}; expected overview, cavity, or boundary"
                        ));
                    }
                    None => return Err("missing canonical camera pose".to_owned()),
                };
                camera_was_selected = true;
            }
            "--camera-move-step" => {
                if camera_was_selected {
                    return Err("select either one fixed camera pose or one move step".to_owned());
                }
                let step = arguments
                    .next()
                    .ok_or_else(|| "missing camera move step".to_owned())?
                    .parse::<u32>()
                    .map_err(|error| format!("invalid camera move step: {error}"))?;
                let movement =
                    overview_to_cavity_camera_move().map_err(|error| error.to_string())?;
                let pose = movement
                    .pose_at_step(step)
                    .map_err(|error| error.to_string())?;
                camera = CanonicalCameraSelection::MoveStep {
                    step,
                    total_steps: movement.total_steps(),
                    pose,
                };
                camera_was_selected = true;
            }
            unknown => return Err(format!("unknown desktop demo argument {unknown:?}")),
        }
    }
    Ok((CanonicalRenderConfiguration { scale, camera }, report_only))
}

fn report_canonical_configuration(
    metadata: &CanonicalSceneMetadata,
    configuration: &CanonicalRenderConfiguration,
) {
    let [width, height, depth] = metadata.dimensions();
    let [origin_x, origin_y, origin_z] = metadata.scene_origin();
    let material_identities = metadata
        .material_catalogue()
        .iter()
        .map(|material| material.identity())
        .collect::<Vec<_>>()
        .join(",");
    let material_colors = metadata
        .material_catalogue()
        .iter()
        .map(|material| format_vector(material.linear_base_color()))
        .collect::<Vec<_>>()
        .join(";");
    println!(
        "Canonical scene: generator={} version={} seed={} dimensions={}x{}x{} origin={},{},{} voxel_size={} materials={} material_colors={} occupied={} exposed_faces={} exposed_face_limit={}",
        metadata.generator_identity(),
        metadata.generator_version(),
        metadata.seed(),
        width,
        height,
        depth,
        origin_x,
        origin_y,
        origin_z,
        metadata.voxel_size(),
        material_identities,
        material_colors,
        metadata.occupied_count(),
        metadata.exposed_face_count(),
        metadata.exposed_face_limit(),
    );
    let camera = configuration.camera.pose();
    println!(
        "Canonical camera: camera={} eye={} target={} up={} fov_degrees={} near={} far={}",
        configuration.camera.report_identity(),
        format_vector(camera.eye()),
        format_vector(camera.target()),
        format_vector(camera.up()),
        camera.field_of_view_degrees(),
        camera.near_plane(),
        camera.far_plane(),
    );
}

fn format_vector<const LENGTH: usize>(components: [f32; LENGTH]) -> String {
    components
        .iter()
        .map(|component| component.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

#[cfg(target_os = "windows")]
struct DesktopApplication {
    backend: Option<RenderBackend>,
    window: Option<Window>,
    application_error: Option<String>,
    drawable_occluded: bool,
    presentation_retry_at: Option<Instant>,
    canonical_configuration: CanonicalRenderConfiguration,
}

#[cfg(target_os = "windows")]
const PRESENTATION_RETRY_DELAY: Duration = Duration::from_millis(100);

#[cfg(target_os = "windows")]
impl DesktopApplication {
    fn new(canonical_configuration: CanonicalRenderConfiguration) -> Self {
        Self {
            backend: None,
            window: None,
            application_error: None,
            drawable_occluded: false,
            presentation_retry_at: None,
            canonical_configuration,
        }
    }

    fn set_drawable_extent(&mut self, drawable_extent: ash::vk::Extent2D) {
        if let Some(backend) = &mut self.backend {
            backend.set_drawable_extent(drawable_extent);
        }
        self.presentation_retry_at = None;
        if drawable_extent.width > 0
            && drawable_extent.height > 0
            && let Some(window) = &self.window
        {
            window.request_redraw();
        }
    }
}

#[cfg(target_os = "windows")]
impl ApplicationHandler for DesktopApplication {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        let attributes = WindowAttributes::default().with_title("Voxel Nexus Vulkan Demo");
        let window = match event_loop.create_window(attributes) {
            Ok(window) => window,
            Err(error) => {
                self.application_error = Some(format!("could not create the demo window: {error}"));
                event_loop.exit();
                return;
            }
        };
        let adapter = WindowsPresentationAdapter::new(&window);
        let drawable_size = window.inner_size();
        let initial_drawable_extent = ash::vk::Extent2D {
            width: drawable_size.width,
            height: drawable_size.height,
        };
        let render_path = match canonical_raster_render_path(&self.canonical_configuration) {
            Ok(render_path) => render_path,
            Err(error) => {
                self.application_error = Some(error);
                event_loop.exit();
                return;
            }
        };
        let installed_revision = render_path.installed_source_revision();
        let backend = match RenderBackend::initialize(
            c"Voxel Nexus Desktop Demo",
            &adapter,
            initial_drawable_extent,
            render_path,
        ) {
            Ok(backend) => backend,
            Err(error) => {
                self.application_error = Some(error.to_string());
                event_loop.exit();
                return;
            }
        };
        let diagnostic_report = backend.runtime_context().to_string();
        println!("{diagnostic_report}");
        if let Some(source_revision) = installed_revision {
            println!("Raster artifact revision: {source_revision}");
        }
        let runtime_context = backend.runtime_context();
        window.set_title(&format!(
            "Voxel Nexus Vulkan Demo | {} | Vulkan {}.{}.{} | Validation errors: {}",
            runtime_context.device_name,
            ash::vk::api_version_major(runtime_context.api_version),
            ash::vk::api_version_minor(runtime_context.api_version),
            ash::vk::api_version_patch(runtime_context.api_version),
            backend.validation_error_count()
        ));
        self.backend = Some(backend);
        self.window = Some(window);
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => {
                if let Some(backend) = &mut self.backend
                    && let Err(error) = backend.shutdown()
                {
                    self.application_error = Some(error.to_string());
                }
                event_loop.exit();
            }
            WindowEvent::Resized(drawable_size) => {
                let drawable_extent = if self.drawable_occluded {
                    ash::vk::Extent2D::default()
                } else {
                    ash::vk::Extent2D {
                        width: drawable_size.width,
                        height: drawable_size.height,
                    }
                };
                self.set_drawable_extent(drawable_extent);
            }
            WindowEvent::Occluded(occluded) => {
                self.drawable_occluded = occluded;
                let drawable_extent = if occluded {
                    ash::vk::Extent2D::default()
                } else {
                    let drawable_size = self
                        .window
                        .as_ref()
                        .map(Window::inner_size)
                        .unwrap_or_default();
                    ash::vk::Extent2D {
                        width: drawable_size.width,
                        height: drawable_size.height,
                    }
                };
                self.set_drawable_extent(drawable_extent);
            }
            WindowEvent::RedrawRequested => {
                let outcome = match &mut self.backend {
                    Some(backend) => match backend.draw_frame() {
                        Ok(outcome) => outcome,
                        Err(error) => {
                            self.application_error = Some(error.to_string());
                            event_loop.exit();
                            return;
                        }
                    },
                    None => FrameOutcome::Suspended,
                };
                match outcome {
                    FrameOutcome::Redraw => {
                        self.presentation_retry_at = None;
                        if let Some(window) = &self.window {
                            window.request_redraw();
                        }
                    }
                    FrameOutcome::RetryLater => {
                        self.presentation_retry_at =
                            Some(Instant::now() + PRESENTATION_RETRY_DELAY);
                    }
                    FrameOutcome::Suspended => {
                        self.presentation_retry_at = None;
                    }
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let Some(retry_at) = self.presentation_retry_at else {
            event_loop.set_control_flow(ControlFlow::Wait);
            return;
        };
        if Instant::now() < retry_at {
            event_loop.set_control_flow(ControlFlow::WaitUntil(retry_at));
            return;
        }
        self.presentation_retry_at = None;
        event_loop.set_control_flow(ControlFlow::Wait);
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
}

#[cfg(target_os = "windows")]
fn canonical_raster_render_path(
    configuration: &CanonicalRenderConfiguration,
) -> Result<RasterRenderPath, String> {
    let canonical = generate_canonical_scene(configuration.scale)
        .map_err(|error| format!("could not generate the canonical Voxel Scene: {error}"))?;
    report_canonical_configuration(canonical.metadata(), configuration);
    let volume_identity = canonical.metadata().volume_identity().clone();
    let view = VoxelFrontend::new()
        .publish(canonical.into_scene())
        .map_err(|error| format!("could not publish the canonical Voxel Scene: {error}"))?;
    let artifact = derive_raster_artifact(&view, &volume_identity)
        .map_err(|error| format!("could not derive the canonical raster artifact: {error}"))?;
    let mut render_path = RasterRenderPath::with_camera_pose(configuration.camera.pose());
    render_path.install_artifact(artifact);
    Ok(render_path)
}

#[cfg(target_os = "windows")]
fn run() -> Result<(), String> {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    if let Some(diagnostic_result) = render_path_failure_diagnostic(arguments.clone().into_iter()) {
        return diagnostic_result;
    }
    if let Some(diagnostic_result) =
        unsupported_prerequisite_diagnostic(arguments.clone().into_iter())
    {
        return diagnostic_result;
    }
    let (configuration, report_only) = parse_canonical_configuration(arguments.into_iter())?;
    if report_only {
        let canonical = generate_canonical_scene(configuration.scale)
            .map_err(|error| format!("could not generate the canonical Voxel Scene: {error}"))?;
        report_canonical_configuration(canonical.metadata(), &configuration);
        return Ok(());
    }
    let event_loop =
        EventLoop::new().map_err(|error| format!("could not start the event loop: {error}"))?;
    let mut application = DesktopApplication::new(configuration);
    event_loop
        .run_app(&mut application)
        .map_err(|error| format!("the desktop event loop failed: {error}"))?;
    match application.application_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

#[derive(Debug)]
struct InjectedRenderPathFailure;

impl fmt::Display for InjectedRenderPathFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("injected proof failure")
    }
}

impl Error for InjectedRenderPathFailure {}

fn render_path_failure_diagnostic(
    mut arguments: impl Iterator<Item = String>,
) -> Option<Result<(), String>> {
    if arguments.next().as_deref() != Some("--verify-render-path-failure") {
        return None;
    }
    let phase = match arguments.next().as_deref() {
        Some("release") => Ok(RenderPathPhase::Release),
        Some("configure") => Ok(RenderPathPhase::Configure),
        Some("record") => Ok(RenderPathPhase::Record),
        Some("upload") => {
            return Some(Err(RasterArtifactInstallationError::new(
                RasterArtifactInstallationPhase::Upload,
                VoxelSceneRevision::new(41),
                Box::new(InjectedRenderPathFailure),
            )
            .to_string()));
        }
        Some(phase) => Err(format!(
            "unknown Render Path phase {phase:?}; expected release, configure, record, or upload"
        )),
        None => Err(
            "missing Render Path phase; expected release, configure, record, or upload".to_owned(),
        ),
    };
    Some(phase.and_then(|phase| {
        run_render_path_phase::<()>(phase, || Err(Box::new(InjectedRenderPathFailure)))
            .map_err(|error| error.to_string())
    }))
}

fn unsupported_prerequisite_diagnostic(
    mut arguments: impl Iterator<Item = String>,
) -> Option<Result<(), String>> {
    if arguments.next().as_deref() != Some("--verify-unsupported-prerequisite") {
        return None;
    }
    let result = match arguments.next().as_deref() {
        Some("vulkan-1.2") => verify_rejected_candidate(DeviceCandidate {
            name: "Deterministic Vulkan 1.2 device".to_owned(),
            api_version: ash::vk::make_api_version(0, 1, 2, 0),
            driver_version: 1,
            supports_swapchain: true,
            has_surface_formats: true,
            has_present_modes: true,
            queue_families: vec![QueueFamilyCapabilities {
                supports_graphics: true,
                supports_presentation: true,
            }],
        }),
        Some("presentation") => verify_rejected_candidate(DeviceCandidate {
            name: "Deterministic device without presentation".to_owned(),
            api_version: ash::vk::API_VERSION_1_3,
            driver_version: 1,
            supports_swapchain: false,
            has_surface_formats: false,
            has_present_modes: false,
            queue_families: vec![QueueFamilyCapabilities {
                supports_graphics: true,
                supports_presentation: false,
            }],
        }),
        Some(case) => Err(format!(
            "unknown unsupported-prerequisite diagnostic {case:?}; expected vulkan-1.2 or presentation"
        )),
        None => Err(
            "missing unsupported-prerequisite diagnostic; expected vulkan-1.2 or presentation"
                .to_owned(),
        ),
    };
    Some(result)
}

fn verify_rejected_candidate(candidate: DeviceCandidate) -> Result<(), String> {
    match render_backend::select_device(vec![candidate]) {
        Ok(candidate) => Err(format!(
            "the deterministic unsupported-prerequisite diagnostic unexpectedly accepted {}",
            candidate.name
        )),
        Err(error) => Err(error.to_string()),
    }
}

#[cfg(target_os = "windows")]
fn main() -> ExitCode {
    application_exit_code(run())
}

fn application_exit_code(result: Result<(), String>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Voxel Nexus could not start: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn main() -> ExitCode {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    if let Some(result) = render_path_failure_diagnostic(arguments.clone().into_iter()) {
        return application_exit_code(result);
    }
    if let Some(result) = unsupported_prerequisite_diagnostic(arguments.clone().into_iter()) {
        return application_exit_code(result);
    }
    if arguments
        .iter()
        .any(|argument| argument == "--report-canonical-configuration")
    {
        let result =
            parse_canonical_configuration(arguments.into_iter()).and_then(|(configuration, _)| {
                let canonical = generate_canonical_scene(configuration.scale).map_err(|error| {
                    format!("could not generate the canonical Voxel Scene: {error}")
                })?;
                report_canonical_configuration(canonical.metadata(), &configuration);
                Ok(())
            });
        return application_exit_code(result);
    }
    eprintln!("The Voxel Nexus desktop demo currently supports Windows only.");
    ExitCode::SUCCESS
}

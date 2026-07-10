#[cfg(target_os = "windows")]
mod windows_adapter;

use render_backend::{DeviceCandidate, QueueFamilyCapabilities};
#[cfg(target_os = "windows")]
use render_backend::{FrameOutcome, RenderBackend};
use std::process::ExitCode;
#[cfg(target_os = "windows")]
use std::time::{Duration, Instant};
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

#[cfg(target_os = "windows")]
#[derive(Default)]
struct DesktopApplication {
    backend: Option<RenderBackend>,
    window: Option<Window>,
    application_error: Option<String>,
    drawable_occluded: bool,
    presentation_retry_at: Option<Instant>,
}

#[cfg(target_os = "windows")]
const PRESENTATION_RETRY_DELAY: Duration = Duration::from_millis(100);

#[cfg(target_os = "windows")]
impl DesktopApplication {
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
        let backend = match RenderBackend::initialize(
            c"Voxel Nexus Desktop Demo",
            &adapter,
            initial_drawable_extent,
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
            WindowEvent::CloseRequested => event_loop.exit(),
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
fn run() -> Result<(), String> {
    if let Some(diagnostic_result) = unsupported_prerequisite_diagnostic(std::env::args().skip(1)) {
        return diagnostic_result;
    }
    let event_loop =
        EventLoop::new().map_err(|error| format!("could not start the event loop: {error}"))?;
    let mut application = DesktopApplication::default();
    event_loop
        .run_app(&mut application)
        .map_err(|error| format!("the desktop event loop failed: {error}"))?;
    match application.application_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
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
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Voxel Nexus could not start: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn main() -> ExitCode {
    if let Some(result) = unsupported_prerequisite_diagnostic(std::env::args().skip(1)) {
        return match result {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("Voxel Nexus could not start: {error}");
                ExitCode::FAILURE
            }
        };
    }
    eprintln!("The Voxel Nexus desktop demo currently supports Windows only.");
    ExitCode::SUCCESS
}

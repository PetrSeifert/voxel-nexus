#[cfg(target_os = "windows")]
mod windows_adapter;

#[cfg(target_os = "windows")]
use render_backend::RenderBackend;
#[cfg(target_os = "windows")]
use std::process::ExitCode;
#[cfg(target_os = "windows")]
use windows_adapter::WindowsPresentationAdapter;
#[cfg(target_os = "windows")]
use winit::application::ApplicationHandler;
#[cfg(target_os = "windows")]
use winit::event::WindowEvent;
#[cfg(target_os = "windows")]
use winit::event_loop::{ActiveEventLoop, EventLoop};
#[cfg(target_os = "windows")]
use winit::window::{Window, WindowAttributes, WindowId};

#[cfg(target_os = "windows")]
#[derive(Default)]
struct DesktopApplication {
    backend: Option<RenderBackend>,
    window: Option<Window>,
    startup_error: Option<String>,
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
                self.startup_error = Some(format!("could not create the demo window: {error}"));
                event_loop.exit();
                return;
            }
        };
        let adapter = WindowsPresentationAdapter::new(&window);
        let backend = match RenderBackend::initialize(c"Voxel Nexus Desktop Demo", &adapter) {
            Ok(backend) => backend,
            Err(error) => {
                self.startup_error = Some(error.to_string());
                event_loop.exit();
                return;
            }
        };
        let diagnostic_report = backend.runtime_context().to_string();
        println!("{diagnostic_report}");
        window.set_title(&format!(
            "Voxel Nexus Vulkan Demo | {}",
            diagnostic_report.replace('\n', " | ")
        ));
        self.backend = Some(backend);
        self.window = Some(window);
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        if event == WindowEvent::CloseRequested {
            event_loop.exit();
        }
    }
}

#[cfg(target_os = "windows")]
fn run() -> Result<(), String> {
    let event_loop =
        EventLoop::new().map_err(|error| format!("could not start the event loop: {error}"))?;
    let mut application = DesktopApplication::default();
    event_loop
        .run_app(&mut application)
        .map_err(|error| format!("the desktop event loop failed: {error}"))?;
    match application.startup_error {
        Some(error) => Err(error),
        None => Ok(()),
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
fn main() {
    eprintln!("The Voxel Nexus desktop demo currently supports Windows only.");
}

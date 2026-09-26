#![cfg(windows)]

#[allow(dead_code)]
#[path = "../src/windows_adapter.rs"]
mod windows_adapter;

use ash::vk;
use render_backend::{
    BackendError, RenderBackend, RenderPath, RenderPathDeviceContext, RenderPathFrameContext,
    RenderPathPhase, RenderPathResult, RenderPathTarget,
};
use std::error::Error;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, EventLoop},
    platform::windows::EventLoopBuilderExtWindows,
    window::{Window, WindowId},
};

type TestResult = Result<(), Box<dyn Error>>;

struct FailingPath;

impl RenderPath for FailingPath {
    fn configure(
        &mut self,
        _device: RenderPathDeviceContext<'_>,
        _target: RenderPathTarget<'_>,
    ) -> RenderPathResult<()> {
        Ok(())
    }

    fn record(&mut self, _frame: RenderPathFrameContext<'_>) -> RenderPathResult<()> {
        Err("injected recording failure".into())
    }

    fn release(&mut self, _device: RenderPathDeviceContext<'_>) -> RenderPathResult<()> {
        Ok(())
    }
}

fn exercise_failure(window: &Window) -> TestResult {
    let size = window.inner_size();
    let mut backend = RenderBackend::initialize(
        c"frame recording failure regression",
        &windows_adapter::WindowsPresentationAdapter::new(window),
        vk::Extent2D {
            width: size.width,
            height: size.height,
        },
        FailingPath,
    )?;
    let error = backend.draw_frame().err().ok_or("recording must fail")?;
    assert!(matches!(
        error,
        BackendError::RenderPath {
            phase: RenderPathPhase::Record,
            ..
        }
    ));
    assert_eq!(
        error.to_string(),
        "Render Path record failed: injected recording failure"
    );
    assert_eq!(
        error.source().map(ToString::to_string).as_deref(),
        Some("injected recording failure")
    );
    assert!(matches!(
        backend.draw_frame(),
        Err(BackendError::FrameFailureIsTerminal)
    ));
    backend.set_drawable_extent(vk::Extent2D {
        width: 0,
        height: 0,
    });
    assert!(matches!(
        backend.draw_frame(),
        Err(BackendError::FrameFailureIsTerminal)
    ));
    backend.set_drawable_extent(vk::Extent2D {
        width: size.width,
        height: size.height,
    });
    assert!(matches!(
        backend.draw_frame(),
        Err(BackendError::FrameFailureIsTerminal)
    ));
    assert!(matches!(
        backend.refresh_render_path(),
        Err(BackendError::FrameFailureIsTerminal)
    ));
    assert_eq!(backend.last_submitted_frame_sequence(), None);
    backend.shutdown()?;
    assert!(matches!(
        backend.draw_frame(),
        Err(BackendError::FrameFailureIsTerminal)
    ));
    assert_eq!(backend.validation_error_count(), 0);
    Ok(())
}

#[derive(Default)]
struct FailureApplication {
    result: Option<TestResult>,
}

impl ApplicationHandler for FailureApplication {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.result = Some((|| {
            let window = event_loop.create_window(
                Window::default_attributes()
                    .with_title("Frame recording failure regression")
                    .with_inner_size(winit::dpi::PhysicalSize::new(320, 240)),
            )?;
            exercise_failure(&window)
        })());
        event_loop.exit();
    }

    fn window_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        _event: WindowEvent,
    ) {
    }
}

#[test]
#[ignore = "requires a Windows desktop, Vulkan 1.3, and the Vulkan validation layer"]
fn recording_failure_retry_returns_promptly() -> TestResult {
    const CHILD_ENVIRONMENT: &str = "VOXEL_NEXUS_FRAME_FAILURE_CHILD";
    if std::env::var_os(CHILD_ENVIRONMENT).is_some() {
        let event_loop = EventLoop::builder().with_any_thread(true).build()?;
        let mut application = FailureApplication::default();
        event_loop.run_app(&mut application)?;
        return application.result.ok_or("failure test did not run")?;
    }

    let mut child = Command::new(std::env::current_exe()?)
        .args([
            "--exact",
            "recording_failure_retry_returns_promptly",
            "--ignored",
            "--nocapture",
        ])
        .env(CHILD_ENVIRONMENT, "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if child.try_wait()?.is_some() {
            let output = child.wait_with_output()?;
            let standard_error = String::from_utf8_lossy(&output.stderr);
            assert!(
                output.status.success(),
                "{}\n{standard_error}",
                String::from_utf8_lossy(&output.stdout)
            );
            assert!(
                !standard_error.contains("Vulkan validation ERROR"),
                "{standard_error}"
            );
            return Ok(());
        }
        if Instant::now() >= deadline {
            child.kill()?;
            let output = child.wait_with_output()?;
            return Err(format!(
                "recording failure/retry hung: {}",
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

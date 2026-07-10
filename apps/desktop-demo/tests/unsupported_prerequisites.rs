use std::process::{Command, ExitStatus, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

fn run_diagnostic(case: &str) -> Result<Output, Box<dyn std::error::Error>> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_desktop-demo"))
        .args(["--verify-unsupported-prerequisite", case])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill()?;
            child.wait()?;
            return Err(format!("unsupported-prerequisite diagnostic hung for case {case}").into());
        }
        thread::sleep(Duration::from_millis(10));
    };
    collect_output(child, status)
}

fn collect_output(
    child: std::process::Child,
    status: ExitStatus,
) -> Result<Output, Box<dyn std::error::Error>> {
    let output = child.wait_with_output()?;
    Ok(Output { status, ..output })
}

#[test]
fn vulkan_1_2_failure_reaches_the_application_boundary() -> Result<(), Box<dyn std::error::Error>> {
    let output = run_diagnostic("vulkan-1.2")?;
    let standard_error = String::from_utf8(output.stderr)?;

    assert_eq!(output.status.code(), Some(1));
    assert!(standard_error.contains("Voxel Nexus could not start"));
    assert!(standard_error.contains("supports Vulkan 1.2.0, but Vulkan 1.3 or newer is required"));
    assert!(standard_error.contains("update the graphics driver"));
    assert!(!standard_error.contains("panicked"));
    Ok(())
}

#[test]
fn missing_presentation_failure_reaches_the_application_boundary()
-> Result<(), Box<dyn std::error::Error>> {
    let output = run_diagnostic("presentation")?;
    let standard_error = String::from_utf8(output.stderr)?;

    assert_eq!(output.status.code(), Some(1));
    assert!(standard_error.contains("Voxel Nexus could not start"));
    assert!(standard_error.contains("VK_KHR_swapchain device extension is unavailable"));
    assert!(standard_error.contains("no queue family can present to the window surface"));
    assert!(standard_error.contains("GPU and desktop session with Vulkan presentation support"));
    assert!(!standard_error.contains("panicked"));
    Ok(())
}

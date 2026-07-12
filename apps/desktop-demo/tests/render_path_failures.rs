use std::process::{Command, ExitStatus, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

fn run_diagnostic(phase: &str) -> Result<Output, Box<dyn std::error::Error>> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_desktop-demo"))
        .args(["--verify-render-path-failure", phase])
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
            return Err(format!("Render Path failure diagnostic hung for phase {phase}").into());
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
fn every_render_path_phase_failure_reaches_the_application_boundary()
-> Result<(), Box<dyn std::error::Error>> {
    for phase in ["release", "configure", "record", "shutdown"] {
        let output = run_diagnostic(phase)?;
        let standard_error = String::from_utf8(output.stderr)?;

        assert_eq!(output.status.code(), Some(1));
        assert!(standard_error.contains("Voxel Nexus could not start"));
        assert!(standard_error.contains(&format!("Render Path {phase} failed")));
        assert!(standard_error.contains("injected proof failure"));
        assert!(!standard_error.contains("panicked"));
    }
    Ok(())
}

#[test]
fn raster_upload_failure_reaches_the_application_boundary_with_source_revision()
-> Result<(), Box<dyn std::error::Error>> {
    let output = run_diagnostic("upload")?;
    let standard_error = String::from_utf8(output.stderr)?;

    assert_eq!(output.status.code(), Some(1));
    assert!(standard_error.contains("Voxel Nexus could not start"));
    assert!(standard_error.contains("raster artifact upload failed"));
    assert!(standard_error.contains("Voxel Scene Revision 41"));
    assert!(standard_error.contains("injected proof failure"));
    assert!(!standard_error.contains("panicked"));
    Ok(())
}

#[test]
fn background_derivation_failure_reaches_the_application_boundary_with_source_revision()
-> Result<(), Box<dyn std::error::Error>> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_desktop-demo"))
        .args(["--verify-background-preparation-failure", "derivation"])
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
            return Err("background derivation failure diagnostic hung".into());
        }
        thread::sleep(Duration::from_millis(10));
    };
    let output = collect_output(child, status)?;
    let standard_error = String::from_utf8(output.stderr)?;

    assert_eq!(output.status.code(), Some(1));
    assert!(standard_error.contains("Voxel Nexus could not start"));
    assert!(standard_error.contains("background derivation failed"));
    assert!(standard_error.contains("metadata"));
    assert!(standard_error.contains("VoxelSceneRevision(1)"));
    assert!(!standard_error.contains("panicked"));
    Ok(())
}

#[cfg(not(feature = "qualification"))]
#[test]
fn default_binary_rejects_qualification_modes_before_startup()
-> Result<(), Box<dyn std::error::Error>> {
    for argument in [
        "--hold-background-preparation",
        "--hold-post-upload-candidate",
        "--inject-raster-upload-failure",
        "--edit-burst-demo",
        "--compute-switch-demo",
        "--compute-switch-lifecycle-demo",
        "--portable-compute-ray-milestone-demo",
        "--portable-compute-ray-milestone-timing",
        "--compute-shutdown-qualification",
        "--measurement-mode",
        "--measurement-output",
        "--verify-background-preparation-failure",
        "--verify-render-path-failure",
        "--verify-unsupported-prerequisite",
    ] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_desktop-demo"))
            .arg(argument)
            .output()?;
        assert!(!output.status.success(), "{argument}");
        let diagnostic = String::from_utf8(output.stderr)?;
        assert!(
            diagnostic.contains("requires a build with --features qualification"),
            "{argument}: {diagnostic}"
        );
    }
    Ok(())
}

use std::process::Command;

fn report(arguments: &[&str]) -> Result<std::process::Output, Box<dyn std::error::Error>> {
    Ok(Command::new(env!("CARGO_BIN_EXE_desktop-demo"))
        .args(arguments)
        .output()?)
}

#[test]
fn desktop_command_reports_each_canonical_scale_and_fixed_pose()
-> Result<(), Box<dyn std::error::Error>> {
    let scale_cases = [
        ("64", "64x32x64", "52608", "13616"),
        ("128", "128x64x128", "420864", "54464"),
        ("256", "256x128x256", "3366912", "217856"),
    ];
    for (scale, dimensions, occupied_count, exposed_face_count) in scale_cases {
        let output = report(&[
            "--report-canonical-configuration",
            "--scene-scale",
            scale,
            "--camera-pose",
            "overview",
        ])?;
        let standard_output = String::from_utf8(output.stdout)?;
        assert!(output.status.success());
        assert!(standard_output.contains("generator=voxel-nexus-canonical-dense"));
        assert!(standard_output.contains("version=1"));
        assert!(standard_output.contains("seed=6219286665078134867"));
        assert!(standard_output.contains(&format!("dimensions={dimensions}")));
        assert!(standard_output.contains("origin=-8,-4,-8"));
        assert!(
            standard_output.contains("materials=canonical-warm,canonical-green,canonical-blue")
        );
        assert!(
            standard_output
                .contains("material_colors=0.95,0.22,0.1,1;0.12,0.75,0.28,1;0.1,0.32,0.95,1")
        );
        assert!(standard_output.contains(&format!("occupied={occupied_count}")));
        assert!(standard_output.contains(&format!("exposed_faces={exposed_face_count}")));
        assert!(standard_output.contains("camera=overview"));
        assert!(standard_output.contains("eye=20,14,22"));
        assert!(standard_output.contains("target=0,0,0"));
        assert!(standard_output.contains("up=0,1,0"));
        assert!(standard_output.contains("fov_degrees=50"));
        assert!(standard_output.contains("near=0.1"));
        assert!(standard_output.contains("far=100"));
    }

    for pose in ["cavity", "boundary"] {
        let output = report(&[
            "--report-canonical-configuration",
            "--scene-scale",
            "64",
            "--camera-pose",
            pose,
        ])?;
        let standard_output = String::from_utf8(output.stdout)?;
        assert!(output.status.success());
        assert!(standard_output.contains(&format!("camera={pose}")));
    }
    Ok(())
}

#[test]
fn desktop_command_exposes_a_deterministic_camera_move_step()
-> Result<(), Box<dyn std::error::Error>> {
    let arguments = [
        "--report-canonical-configuration",
        "--scene-scale",
        "64",
        "--camera-move-step",
        "60",
    ];
    let first = report(&arguments)?;
    let second = report(&arguments)?;
    assert!(first.status.success());
    assert_eq!(first.stdout, second.stdout);
    let standard_output = String::from_utf8(first.stdout)?;
    assert!(standard_output.contains("camera=overview-to-cavity-step-60-of-120"));
    assert!(standard_output.contains("eye=10,7.5,19.5"));
    assert!(standard_output.contains("target=-0.375,-0.25,0"));
    assert!(standard_output.contains("fov_degrees=47.5"));
    Ok(())
}

#[test]
fn desktop_command_rejects_an_out_of_range_camera_move_without_panicking()
-> Result<(), Box<dyn std::error::Error>> {
    let output = report(&[
        "--report-canonical-configuration",
        "--camera-move-step",
        "121",
    ])?;
    let standard_error = String::from_utf8(output.stderr)?;
    assert_eq!(output.status.code(), Some(1));
    assert!(standard_error.contains("camera move step 121 exceeds the final step 120"));
    assert!(!standard_error.contains("panicked"));
    Ok(())
}

#[test]
fn desktop_command_reports_the_front_face_winding_diagnostic()
-> Result<(), Box<dyn std::error::Error>> {
    let output = report(&["--report-canonical-configuration", "--winding-diagnostic"])?;
    let standard_output = String::from_utf8(output.stdout)?;

    assert!(output.status.success());
    assert!(standard_output.contains(
        "Diagnostic scene: identity=raster-front-face-winding dimensions=1x1x2 origin=0,0,0 voxel_size=1 materials=winding-diagnostic-far-blue,winding-diagnostic-near-warm occupied=2 exposed_faces=10"
    ));
    assert!(standard_output.contains(
        "Diagnostic camera: camera=winding-diagnostic eye=0.5,0.5,4 target=0.5,0.5,1 up=0,1,0 fov_degrees=60 near=0.1 far=10"
    ));
    Ok(())
}

#[test]
fn desktop_command_rejects_conflicting_scene_selections_in_any_order()
-> Result<(), Box<dyn std::error::Error>> {
    for arguments in [
        [
            "--report-canonical-configuration",
            "--scene-scale",
            "64",
            "--winding-diagnostic",
        ],
        [
            "--report-canonical-configuration",
            "--winding-diagnostic",
            "--scene-scale",
            "64",
        ],
    ] {
        let output = report(&arguments)?;
        let standard_error = String::from_utf8(output.stderr)?;

        assert_eq!(output.status.code(), Some(1));
        assert!(
            standard_error
                .contains("select either one canonical scene scale or the winding diagnostic")
        );
        assert!(!standard_error.contains("panicked"));
    }
    Ok(())
}

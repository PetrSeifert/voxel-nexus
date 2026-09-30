use canonical_inspection::{CanonicalCameraPose, overview_to_cavity_camera_move};
use canonical_scene::{CanonicalSceneMetadata, CanonicalSceneScale, generate_canonical_scene};
use raster_render_path::CameraPose;
use std::path::PathBuf;
use voxel_frontend::{
    DenseVoxelBatch, DenseVoxelScene, DenseVoxelVolume, VoxelCoordinate, VoxelExtent,
    VoxelMaterial, VoxelMaterialId, VoxelRegion, VoxelSceneId, VoxelSceneRevision, VoxelValue,
    VoxelVolumeId, VoxelVolumeMetadata,
};

#[derive(Clone, Copy)]
pub(super) enum DesktopCameraSelection {
    Fixed(CanonicalCameraPose),
    MoveStep {
        step: u32,
        total_steps: u32,
        pose: CameraPose,
    },
}

impl DesktopCameraSelection {
    pub(super) fn pose(self) -> Result<CameraPose, String> {
        match self {
            Self::Fixed(identity) => identity.pose().map_err(|error| error.to_string()),
            Self::MoveStep { pose, .. } => Ok(pose),
        }
    }

    pub(super) fn report_identity(self) -> String {
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

#[derive(Clone, Copy)]
pub(super) enum DesktopSceneSelection {
    Canonical(CanonicalSceneScale),
    WindingDiagnostic,
    LargeSparse,
    StreamedWorld,
}

#[derive(Clone)]
pub(super) struct DesktopRenderConfiguration {
    pub(super) scene: DesktopSceneSelection,
    pub(super) compute_representation: compute_ray_render_path::ComputeRepresentation,
    camera: DesktopCameraSelection,
    pub(super) raster_region_extent: u32,
    pub(super) hold_background_preparation: bool,
    pub(super) hold_post_upload_candidate: bool,
    pub(super) inject_raster_upload_failure: bool,
    pub(super) edit_burst_demo: bool,
    pub(super) interactive: bool,
    pub(super) compute_switch_demo: bool,
    pub(super) compute_switch_lifecycle_demo: bool,
    pub(super) portable_milestone_demo: bool,
    pub(super) portable_milestone_timing: bool,
    pub(super) compute_shutdown_qualification: Option<ComputeShutdownQualification>,
    pub(super) measurement: Option<MeasurementConfiguration>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ComputeShutdownQualification {
    ActivePreparation,
    HiddenCandidate,
    Presenting,
    Replacement,
}

impl DesktopRenderConfiguration {
    pub(super) fn camera_pose(&self) -> Result<CameraPose, String> {
        match self.scene {
            DesktopSceneSelection::StreamedWorld => CameraPose::new(
                [160.0, 42.0, 160.0],
                [176.0, 28.0, 176.0],
                [0.0, 1.0, 0.0],
                60.0,
                0.1,
                32.0,
            )
            .map_err(|error| error.to_string()),
            DesktopSceneSelection::LargeSparse => CameraPose::new(
                [32.5, 88.0, 32.5],
                [40.5, 80.0, 40.5],
                [0.0, 1.0, 0.0],
                60.0,
                0.1,
                4096.0,
            )
            .map_err(|error| error.to_string()),
            DesktopSceneSelection::Canonical(_) => self.camera.pose(),
            DesktopSceneSelection::WindingDiagnostic => CameraPose::new(
                [0.5, 0.5, 4.0],
                [0.5, 0.5, 1.0],
                [0.0, 1.0, 0.0],
                60.0,
                0.1,
                10.0,
            )
            .map_err(|error| error.to_string()),
        }
    }

    pub(super) fn compute_only(&self) -> bool {
        matches!(self.scene, DesktopSceneSelection::LargeSparse)
    }

    pub(super) fn admit_path_switch(&self) -> Result<(), String> {
        if self.compute_only() {
            return Err("LargeSparseSceneRequiresCompute".to_owned());
        }
        Ok(())
    }

    pub(super) fn render_path_switching_enabled(&self) -> bool {
        self.compute_switch_demo || self.interactive
    }

    pub(super) fn camera_identity(&self) -> String {
        match self.scene {
            DesktopSceneSelection::StreamedWorld => "streamed-world-start".to_owned(),
            DesktopSceneSelection::LargeSparse => "large-sparse-start".to_owned(),
            DesktopSceneSelection::Canonical(_) => self.camera.report_identity(),
            DesktopSceneSelection::WindingDiagnostic => "winding-diagnostic".to_owned(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum MeasurementMode {
    FirstCorrectFrame,
    SteadyState,
}

#[derive(Clone)]
pub(super) struct MeasurementConfiguration {
    pub(super) mode: MeasurementMode,
    pub(super) output: PathBuf,
}

pub(super) fn require_qualification_argument(argument: &str) -> Result<(), String> {
    if !cfg!(feature = "qualification")
        && matches!(
            argument,
            "--compute-switch-demo"
                | "--hold-background-preparation"
                | "--hold-post-upload-candidate"
                | "--inject-raster-upload-failure"
                | "--edit-burst-demo"
                | "--compute-switch-lifecycle-demo"
                | "--portable-compute-ray-milestone-demo"
                | "--portable-compute-ray-milestone-timing"
                | "--compute-shutdown-qualification"
                | "--measurement-mode"
                | "--measurement-output"
                | "--verify-background-preparation-failure"
                | "--verify-render-path-failure"
                | "--verify-unsupported-prerequisite"
        )
    {
        return Err(format!(
            "{argument} requires a build with --features qualification"
        ));
    }
    Ok(())
}

pub(super) fn parse_render_configuration(
    arguments: impl Iterator<Item = String>,
) -> Result<(DesktopRenderConfiguration, bool), String> {
    let arguments: Vec<_> = arguments.collect();
    if arguments
        .iter()
        .any(|argument| argument == "--streamed-world")
        && arguments.iter().any(|argument| {
            matches!(
                argument.as_str(),
                "--large-sparse-scene"
                    | "--scene-scale"
                    | "--winding-diagnostic"
                    | "--camera-pose"
                    | "--camera-move-step"
                    | "--hold-background-preparation"
                    | "--hold-post-upload-candidate"
                    | "--inject-raster-upload-failure"
                    | "--edit-burst-demo"
                    | "--compute-switch-demo"
                    | "--compute-switch-lifecycle-demo"
                    | "--portable-compute-ray-milestone-demo"
                    | "--portable-compute-ray-milestone-timing"
                    | "--compute-shutdown-qualification"
                    | "--measurement-mode"
                    | "--measurement-output"
            )
        })
    {
        return Err("StreamedWorldConflictingDemo: --streamed-world cannot combine with canonical-only demos or --large-sparse-scene".to_owned());
    }
    let mut arguments = arguments.into_iter();
    let mut large_sparse = false;
    let mut streamed_world = false;
    let mut large_sparse_conflict = false;
    let mut explicit_dense = false;
    let mut compute_representation = compute_ray_render_path::ComputeRepresentation::Dense;
    let mut budget_was_selected = false;
    let mut brickmap_budget_bytes = 128 * 1024 * 1024;
    let mut scene = DesktopSceneSelection::Canonical(CanonicalSceneScale::Large);
    let mut camera = DesktopCameraSelection::Fixed(CanonicalCameraPose::Overview);
    let mut scene_was_selected = false;
    let mut camera_was_selected = false;
    let mut report_only = false;
    let mut hold_background_preparation = false;
    let mut hold_post_upload_candidate = false;
    let mut inject_raster_upload_failure = false;
    let mut edit_burst_demo = false;
    let mut interactive = false;
    let mut camera_move_was_selected = false;
    let mut compute_switch_demo = false;
    let mut compute_switch_lifecycle_demo = false;
    let mut portable_milestone_demo = false;
    let mut portable_milestone_timing = false;
    let mut compute_shutdown_qualification = None;
    let mut raster_region_extent = 32;
    let mut measurement_mode = None;
    let mut measurement_output = None;
    while let Some(argument) = arguments.next() {
        require_qualification_argument(&argument)?;
        large_sparse_conflict |= matches!(
            argument.as_str(),
            "--interactive"
                | "--scene-scale"
                | "--winding-diagnostic"
                | "--camera-pose"
                | "--camera-move-step"
                | "--raster-region-extent"
                | "--hold-background-preparation"
                | "--hold-post-upload-candidate"
                | "--inject-raster-upload-failure"
                | "--edit-burst-demo"
                | "--compute-switch-demo"
                | "--compute-switch-lifecycle-demo"
                | "--portable-compute-ray-milestone-demo"
                | "--portable-compute-ray-milestone-timing"
                | "--compute-shutdown-qualification"
                | "--measurement-mode"
                | "--measurement-output"
        );
        match argument.as_str() {
            "--streamed-world" => streamed_world = true,
            "--large-sparse-scene" => large_sparse = true,
            "--compute-representation" => {
                compute_representation = match arguments.next().as_deref() {
                    Some("dense") => {
                        explicit_dense = true;
                        compute_ray_render_path::ComputeRepresentation::Dense
                    }
                    Some("brickmap") => compute_ray_render_path::ComputeRepresentation::Brickmap {
                        budget_bytes: brickmap_budget_bytes,
                    },
                    _ => {
                        return Err(
                            "--compute-representation requires dense or brickmap".to_owned()
                        );
                    }
                };
            }
            "--brickmap-budget-bytes" => {
                budget_was_selected = true;
                brickmap_budget_bytes = arguments
                    .next()
                    .ok_or("missing brickmap budget")?
                    .parse::<u64>()
                    .map_err(|error| error.to_string())?;
            }
            "--report-canonical-configuration" => report_only = true,
            "--hold-background-preparation" => hold_background_preparation = true,
            "--hold-post-upload-candidate" => hold_post_upload_candidate = true,
            "--inject-raster-upload-failure" => inject_raster_upload_failure = true,
            "--edit-burst-demo" => edit_burst_demo = true,
            "--interactive" => interactive = true,
            "--compute-switch-demo" => compute_switch_demo = true,
            "--compute-switch-lifecycle-demo" => {
                compute_switch_demo = true;
                compute_switch_lifecycle_demo = true;
            }
            "--portable-compute-ray-milestone-demo" => {
                compute_switch_demo = true;
                compute_switch_lifecycle_demo = true;
                portable_milestone_demo = true;
            }
            "--portable-compute-ray-milestone-timing" => {
                compute_switch_demo = true;
                compute_switch_lifecycle_demo = true;
                portable_milestone_demo = true;
                portable_milestone_timing = true;
            }
            "--compute-shutdown-qualification" => {
                compute_switch_demo = true;
                compute_shutdown_qualification = Some(match arguments.next().as_deref() {
                    Some("active-preparation") => ComputeShutdownQualification::ActivePreparation,
                    Some("hidden-candidate") => ComputeShutdownQualification::HiddenCandidate,
                    Some("presenting") => ComputeShutdownQualification::Presenting,
                    Some("replacement") => ComputeShutdownQualification::Replacement,
                    Some(value) => {
                        return Err(format!(
                            "unknown compute shutdown qualification {value:?}; expected active-preparation, hidden-candidate, presenting, or replacement"
                        ));
                    }
                    None => return Err("missing compute shutdown qualification".to_owned()),
                });
            }
            "--raster-region-extent" => {
                raster_region_extent = match arguments.next().as_deref() {
                    Some("16") => 16,
                    Some("32") => 32,
                    Some("64") => 64,
                    Some(value) => {
                        return Err(format!(
                            "unknown Raster Region extent {value:?}; expected 16, 32, or 64"
                        ));
                    }
                    None => return Err("missing Raster Region extent".to_owned()),
                };
            }
            "--winding-diagnostic" => {
                if scene_was_selected {
                    return Err(
                        "select either one canonical scene scale or the winding diagnostic"
                            .to_owned(),
                    );
                }
                if camera_was_selected {
                    return Err(
                        "the winding diagnostic cannot use a canonical camera selection".to_owned(),
                    );
                }
                scene = DesktopSceneSelection::WindingDiagnostic;
                scene_was_selected = true;
            }
            "--measurement-mode" => {
                measurement_mode = Some(match arguments.next().as_deref() {
                    Some("first-correct-frame") => MeasurementMode::FirstCorrectFrame,
                    Some("steady-state") => MeasurementMode::SteadyState,
                    Some(value) => {
                        return Err(format!(
                            "unknown measurement mode {value:?}; expected first-correct-frame or steady-state"
                        ));
                    }
                    None => return Err("missing measurement mode".to_owned()),
                });
            }
            "--measurement-output" => {
                measurement_output =
                    Some(PathBuf::from(arguments.next().ok_or_else(|| {
                        "missing measurement output path".to_owned()
                    })?));
            }
            "--scene-scale" => {
                if scene_was_selected {
                    return Err(
                        "select either one canonical scene scale or the winding diagnostic"
                            .to_owned(),
                    );
                }
                scene = DesktopSceneSelection::Canonical(match arguments.next().as_deref() {
                    Some("64") => CanonicalSceneScale::Small,
                    Some("128") => CanonicalSceneScale::Medium,
                    Some("256") => CanonicalSceneScale::Large,
                    Some(value) => {
                        return Err(format!(
                            "unknown canonical scene scale {value:?}; expected 64, 128, or 256"
                        ));
                    }
                    None => return Err("missing canonical scene scale".to_owned()),
                });
                scene_was_selected = true;
            }
            "--camera-pose" => {
                if matches!(scene, DesktopSceneSelection::WindingDiagnostic) {
                    return Err(
                        "the winding diagnostic cannot use a canonical camera selection".to_owned(),
                    );
                }
                if camera_was_selected {
                    return Err("select either one fixed camera pose or one move step".to_owned());
                }
                camera = match arguments.next().as_deref() {
                    Some("overview") => {
                        DesktopCameraSelection::Fixed(CanonicalCameraPose::Overview)
                    }
                    Some("cavity") => {
                        DesktopCameraSelection::Fixed(CanonicalCameraPose::CavityMaterialCloseUp)
                    }
                    Some("boundary") => {
                        DesktopCameraSelection::Fixed(CanonicalCameraPose::BoundaryCutaway)
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
                if matches!(scene, DesktopSceneSelection::WindingDiagnostic) {
                    return Err(
                        "the winding diagnostic cannot use a canonical camera selection".to_owned(),
                    );
                }
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
                camera = DesktopCameraSelection::MoveStep {
                    step,
                    total_steps: movement.total_steps(),
                    pose,
                };
                camera_was_selected = true;
                camera_move_was_selected = true;
            }
            unknown => return Err(format!("unknown desktop demo argument {unknown:?}")),
        }
    }
    if streamed_world {
        if explicit_dense {
            return Err(
                "StreamedWorldRequiresBrickmap: --streamed-world cannot use dense compute"
                    .to_owned(),
            );
        }
        scene = DesktopSceneSelection::StreamedWorld;
        interactive = true;
        compute_representation = compute_ray_render_path::ComputeRepresentation::Brickmap {
            budget_bytes: brickmap_budget_bytes,
        };
    }
    if large_sparse {
        if large_sparse_conflict || explicit_dense {
            return Err("--large-sparse-scene requires brickmap compute and cannot combine with other demo modes, --scene-scale, camera selections, or raster-only options".to_owned());
        }
        if !budget_was_selected {
            brickmap_budget_bytes = 1 << 30;
        }
        scene = DesktopSceneSelection::LargeSparse;
        interactive = true;
        compute_representation = compute_ray_render_path::ComputeRepresentation::Brickmap {
            budget_bytes: brickmap_budget_bytes,
        };
    }
    let measurement = match (measurement_mode, measurement_output) {
        (Some(mode), Some(output)) => Some(MeasurementConfiguration { mode, output }),
        (None, None) => None,
        _ => {
            return Err(
                "measurement mode and measurement output must be supplied together".to_owned(),
            );
        }
    };
    if compute_switch_demo
        && (hold_background_preparation
            || hold_post_upload_candidate
            || inject_raster_upload_failure
            || edit_burst_demo
            || measurement.is_some())
    {
        return Err(
            "the compute switch demo cannot be combined with raster lifecycle, edit burst, failure injection, or measurement modes"
                .to_owned(),
        );
    }
    if interactive
        && (compute_switch_demo
            || hold_background_preparation
            || hold_post_upload_candidate
            || inject_raster_upload_failure
            || edit_burst_demo
            || measurement.is_some()
            || camera_move_was_selected
            || matches!(scene, DesktopSceneSelection::WindingDiagnostic))
    {
        return Err(
            "the interactive mode combines only with --scene-scale, --camera-pose, and --raster-region-extent"
                .to_owned(),
        );
    }
    if compute_switch_lifecycle_demo && compute_shutdown_qualification.is_some() {
        return Err(
            "the automatic compute switch lifecycle demo cannot run a compute shutdown qualification"
                .to_owned(),
        );
    }
    Ok((
        DesktopRenderConfiguration {
            compute_representation: match compute_representation {
                compute_ray_render_path::ComputeRepresentation::Dense => compute_representation,
                compute_ray_render_path::ComputeRepresentation::Brickmap { .. } => {
                    compute_ray_render_path::ComputeRepresentation::Brickmap {
                        budget_bytes: brickmap_budget_bytes,
                    }
                }
            },
            scene,
            camera,
            raster_region_extent,
            hold_background_preparation,
            hold_post_upload_candidate,
            inject_raster_upload_failure,
            edit_burst_demo,
            interactive,
            compute_switch_demo,
            compute_switch_lifecycle_demo,
            portable_milestone_demo,
            portable_milestone_timing,
            compute_shutdown_qualification,
            measurement,
        },
        report_only,
    ))
}

pub(super) fn winding_diagnostic_scene() -> (DenseVoxelScene, VoxelVolumeId) {
    let volume_identity = VoxelVolumeId::new("winding-diagnostic-volume");
    let far_material_identity = VoxelMaterialId::new("winding-diagnostic-far-blue");
    let near_material_identity = VoxelMaterialId::new("winding-diagnostic-near-warm");
    let extent = VoxelExtent::new(1, 1, 2);
    let scene = DenseVoxelScene::new(
        VoxelSceneId::new("raster-front-face-winding"),
        VoxelSceneRevision::new(1),
        vec![
            VoxelMaterial::new(far_material_identity.clone(), [0.1, 0.32, 0.95, 1.0]),
            VoxelMaterial::new(near_material_identity.clone(), [0.95, 0.22, 0.1, 1.0]),
        ],
        vec![DenseVoxelVolume::new(
            VoxelVolumeMetadata::new(volume_identity.clone(), extent, [0.0, 0.0, 0.0], 1.0),
            vec![DenseVoxelBatch::new(
                VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), extent),
                vec![
                    VoxelValue::Occupied(far_material_identity),
                    VoxelValue::Occupied(near_material_identity),
                ],
            )],
        )],
    );
    (scene, volume_identity)
}

pub(super) fn report_winding_diagnostic_configuration(
    configuration: &DesktopRenderConfiguration,
) -> Result<(), String> {
    println!(
        "Diagnostic scene: identity=raster-front-face-winding dimensions=1x1x2 origin=0,0,0 voxel_size=1 materials=winding-diagnostic-far-blue,winding-diagnostic-near-warm occupied=2 exposed_faces=10"
    );
    let camera = configuration.camera_pose()?;
    println!(
        "Diagnostic camera: camera={} eye={} target={} up={} fov_degrees={} near={} far={}",
        configuration.camera_identity(),
        format_vector(camera.eye()),
        format_vector(camera.target()),
        format_vector(camera.up()),
        camera.field_of_view_degrees(),
        camera.near_plane(),
        camera.far_plane(),
    );
    Ok(())
}

pub(super) fn report_canonical_configuration(
    metadata: &CanonicalSceneMetadata,
    configuration: &DesktopRenderConfiguration,
) -> Result<(), String> {
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
    let camera = configuration.camera.pose()?;
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
    Ok(())
}

pub(super) fn report_render_configuration(
    configuration: &DesktopRenderConfiguration,
) -> Result<(), String> {
    match configuration.scene {
        DesktopSceneSelection::StreamedWorld => {
            println!(
                "Streamed scene: identity=streamed-qualification-v1 grid=16x16 volume_dimensions=64x64x64 storage=sparse-pages representation=brickmap presenter=raster camera={}",
                configuration.camera_identity()
            );
        }
        DesktopSceneSelection::LargeSparse => {
            println!(
                "Large sparse scene: dimensions=2048x256x2048 storage=sparse-pages representation=brickmap presenter=compute-ray camera={}",
                configuration.camera_identity()
            );
        }
        DesktopSceneSelection::WindingDiagnostic => {
            report_winding_diagnostic_configuration(configuration)?;
        }
        DesktopSceneSelection::Canonical(scale) => {
            let canonical = generate_canonical_scene(scale).map_err(|error| {
                format!("could not generate the canonical Voxel Scene: {error}")
            })?;
            report_canonical_configuration(canonical.metadata(), configuration)?;
        }
    }
    Ok(())
}

fn format_vector<const LENGTH: usize>(components: [f32; LENGTH]) -> String {
    components
        .iter()
        .map(|component| component.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(arguments: &[&str]) -> Result<(DesktopRenderConfiguration, bool), String> {
        parse_render_configuration(arguments.iter().map(|argument| (*argument).to_owned()))
    }

    #[test]
    fn streamed_world_launches_interactive_with_brickmap_and_switching() -> Result<(), String> {
        let (configuration, report) =
            parse(&["--streamed-world", "--report-canonical-configuration"])?;
        assert!(configuration.interactive);
        assert!(configuration.render_path_switching_enabled());
        assert!(!configuration.compute_only());
        assert!(configuration.admit_path_switch().is_ok());
        assert!(matches!(
            configuration.compute_representation,
            compute_ray_render_path::ComputeRepresentation::Brickmap { .. }
        ));
        assert_eq!(configuration.camera_identity(), "streamed-world-start");
        assert!(report);
        Ok(())
    }

    #[test]
    fn streamed_world_rejects_dense_and_canonical_only_modes_in_either_order() {
        for (options, reason) in [
            (
                vec!["--compute-representation", "dense"],
                "StreamedWorldRequiresBrickmap",
            ),
            (vec!["--scene-scale", "64"], "StreamedWorldConflictingDemo"),
            (vec!["--winding-diagnostic"], "StreamedWorldConflictingDemo"),
            (
                vec!["--camera-pose", "overview"],
                "StreamedWorldConflictingDemo",
            ),
            (
                vec!["--camera-move-step", "0"],
                "StreamedWorldConflictingDemo",
            ),
            (vec!["--large-sparse-scene"], "StreamedWorldConflictingDemo"),
            (
                vec!["--compute-switch-demo"],
                "StreamedWorldConflictingDemo",
            ),
            (
                vec!["--compute-switch-lifecycle-demo"],
                "StreamedWorldConflictingDemo",
            ),
            (
                vec!["--portable-compute-ray-milestone-demo"],
                "StreamedWorldConflictingDemo",
            ),
            (
                vec!["--portable-compute-ray-milestone-timing"],
                "StreamedWorldConflictingDemo",
            ),
            (
                vec!["--compute-shutdown-qualification", "presenting"],
                "StreamedWorldConflictingDemo",
            ),
            (vec!["--edit-burst-demo"], "StreamedWorldConflictingDemo"),
            (
                vec!["--hold-background-preparation"],
                "StreamedWorldConflictingDemo",
            ),
            (
                vec!["--hold-post-upload-candidate"],
                "StreamedWorldConflictingDemo",
            ),
            (
                vec!["--inject-raster-upload-failure"],
                "StreamedWorldConflictingDemo",
            ),
            (
                vec![
                    "--measurement-mode",
                    "steady-state",
                    "--measurement-output",
                    "unused.json",
                ],
                "StreamedWorldConflictingDemo",
            ),
        ] {
            for first in [true, false] {
                let mut arguments = options.clone();
                arguments.insert(if first { 0 } else { arguments.len() }, "--streamed-world");
                arguments.push("--interactive");
                let error = parse(&arguments)
                    .err()
                    .expect("the combination must be rejected");
                assert!(error.contains(reason), "{arguments:?}: {error}");
            }
        }
    }

    #[test]
    fn large_sparse_rejects_incompatible_arguments_in_either_order() {
        for options in [
            vec!["--interactive"],
            vec!["--scene-scale", "64"],
            vec!["--winding-diagnostic"],
            vec!["--camera-pose", "overview"],
            vec!["--camera-move-step", "0"],
            vec!["--raster-region-extent", "32"],
            vec!["--compute-representation", "dense"],
            vec!["--hold-background-preparation"],
            vec!["--hold-post-upload-candidate"],
            vec!["--inject-raster-upload-failure"],
            vec!["--edit-burst-demo"],
            vec!["--compute-switch-demo"],
            vec!["--compute-switch-lifecycle-demo"],
            vec!["--portable-compute-ray-milestone-demo"],
            vec!["--portable-compute-ray-milestone-timing"],
            vec!["--compute-shutdown-qualification", "presenting"],
            vec![
                "--measurement-mode",
                "steady-state",
                "--measurement-output",
                "unused.json",
            ],
        ] {
            for first in [true, false] {
                let mut arguments = options.clone();
                arguments.insert(
                    if first { 0 } else { arguments.len() },
                    "--large-sparse-scene",
                );
                assert!(parse(&arguments).is_err(), "{arguments:?}");
            }
        }
        assert!(
            parse(&["--interactive"])
                .unwrap()
                .0
                .admit_path_switch()
                .is_ok()
        );
    }

    #[test]
    fn large_sparse_accepts_explicit_brickmap_budget_and_report() -> Result<(), String> {
        let (configuration, report) = parse(&[
            "--large-sparse-scene",
            "--compute-representation",
            "brickmap",
            "--brickmap-budget-bytes",
            "123456789",
            "--report-canonical-configuration",
        ])?;
        assert!(report);
        assert!(matches!(
            configuration.compute_representation,
            compute_ray_render_path::ComputeRepresentation::Brickmap {
                budget_bytes: 123456789
            }
        ));
        Ok(())
    }

    #[test]
    fn large_sparse_selects_interactive_brickmap_with_one_gib_budget() -> Result<(), String> {
        let (configuration, report_only) = parse(&["--large-sparse-scene"])?;
        assert!(configuration.interactive);
        assert!(configuration.compute_only());
        assert!(!report_only);
        assert!(matches!(
            configuration.compute_representation,
            compute_ray_render_path::ComputeRepresentation::Brickmap {
                budget_bytes: 1_073_741_824
            }
        ));
        assert_eq!(
            configuration.admit_path_switch().unwrap_err(),
            "LargeSparseSceneRequiresCompute"
        );
        assert!(configuration.camera_pose()?.far_plane() >= 2048.0);
        Ok(())
    }

    #[test]
    fn interactive_mode_combines_with_scene_scale_and_starting_pose() -> Result<(), String> {
        let (configuration, report_only) = parse(&[
            "--interactive",
            "--scene-scale",
            "64",
            "--camera-pose",
            "cavity",
        ])?;
        assert!(configuration.interactive);
        assert!(!report_only);
        assert!(!configuration.compute_switch_demo);
        assert_eq!(configuration.camera_identity(), "cavity");
        Ok(())
    }

    #[test]
    fn interactive_mode_excludes_other_demo_modes() {
        for arguments in [
            &["--interactive", "--winding-diagnostic"][..],
            &["--interactive", "--camera-move-step", "3"][..],
            &["--winding-diagnostic", "--interactive"][..],
        ] {
            assert!(parse(arguments).is_err(), "{arguments:?}");
        }
    }

    #[test]
    #[cfg(feature = "qualification")]
    fn interactive_mode_excludes_qualification_demo_modes() {
        for argument in [
            "--compute-switch-demo",
            "--edit-burst-demo",
            "--hold-background-preparation",
            "--hold-post-upload-candidate",
            "--inject-raster-upload-failure",
        ] {
            assert!(parse(&["--interactive", argument]).is_err(), "{argument}");
        }
    }
}

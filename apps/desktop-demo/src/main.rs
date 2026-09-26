#[cfg(target_os = "windows")]
mod camera_scenario;
#[cfg(target_os = "windows")]
mod compute_edit_scenario;
#[cfg(target_os = "windows")]
mod compute_switch_scenario;
#[cfg(target_os = "windows")]
mod evidence;
#[cfg(target_os = "windows")]
mod raster_scenario;
#[cfg(target_os = "windows")]
mod scenarios;
#[cfg(target_os = "windows")]
use evidence::*;
#[cfg(target_os = "windows")]
use scenarios::*;

#[cfg(target_os = "windows")]
mod windows_adapter;

use canonical_inspection::{CanonicalCameraPose, overview_to_cavity_camera_move};
use canonical_scene::{
    CanonicalSceneMetadata, CanonicalSceneScale, canonical_edit_semantic_ray_probes,
    generate_canonical_scene,
};
#[cfg(target_os = "windows")]
use compute_ray_render_path::{
    ComputeCandidateDisposition, ComputeConvergenceController, ComputeConvergenceEvent,
    ComputeLifecycleController, ComputeMeasurementController, ComputeRayRenderPathAdapter,
    ComputeSemanticRayController, ComputeSemanticRayProbeObservation,
};
#[cfg(target_os = "windows")]
use measurement_evidence::{MeasurementEvent, ResourceCounts, VoxelSceneRevisionIdentity};
use raster_render_path::{
    CameraPose, RasterArtifactInstallationError, RasterArtifactInstallationPhase,
    RasterArtifactInstaller, RasterArtifactPreparation, RasterArtifactPreparationEvent,
    RasterConvergenceCharacterization, RasterConvergenceStatus, RasterLifecycleController,
    RasterPreparationBarrier, RasterPreparationBarrierRelease, RasterSafeRetirementDisposition,
};
#[cfg(target_os = "windows")]
use raster_render_path::{
    RasterRenderPathAdapter, RasterSemanticFaceController, RasterSemanticFaceObservation,
};
#[cfg(target_os = "windows")]
use render_backend::{
    CameraStateRevision, FrameOutcome, PresentationConfigurationId, RenderBackend,
    RenderBackendOptions, RenderPathHandoffControl, RenderPathReadiness, RenderPathStrategy,
    RenderPathSwitchDiagnostics, RenderPathSwitchOwner,
};
use render_backend::{
    DeviceCandidate, QueueFamilyCapabilities, RenderPathPhase, run_render_path_phase,
};
#[cfg(target_os = "windows")]
use semantic_ray_oracle::{
    SemanticRayContactClassification, SemanticRayDistanceTolerance, SemanticRayObservation,
    SemanticRayProbe, SemanticRayProbeObservation, SemanticRayResult, observe_probe,
};
#[cfg(target_os = "windows")]
use std::collections::VecDeque;
#[cfg(target_os = "windows")]
use std::fs::File;
#[cfg(target_os = "windows")]
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, Mutex, mpsc};
#[cfg(target_os = "windows")]
use std::time::{Duration, Instant};
use std::{error::Error, fmt};
use voxel_frontend::{
    DenseVoxelBatch, DenseVoxelScene, DenseVoxelVolume, VoxelCoordinate, VoxelEditCommand,
    VoxelEditOutcome, VoxelExtent, VoxelFrontend, VoxelMaterial, VoxelMaterialId, VoxelRegion,
    VoxelSceneId, VoxelSceneRevision, VoxelSceneView, VoxelValue, VoxelVolumeId,
    VoxelVolumeMetadata,
};
#[cfg(target_os = "windows")]
use windows_adapter::{WindowsPresentationAdapter, WindowsTextOverlay, set_measurement_extent};
#[cfg(target_os = "windows")]
use winit::application::ApplicationHandler;
#[cfg(target_os = "windows")]
use winit::event::{ElementState, WindowEvent};
#[cfg(target_os = "windows")]
use winit::event_loop::EventLoopProxy;
#[cfg(target_os = "windows")]
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
#[cfg(target_os = "windows")]
use winit::keyboard::{Key, NamedKey};
#[cfg(target_os = "windows")]
use winit::platform::windows::EventLoopBuilderExtWindows;
#[cfg(target_os = "windows")]
use winit::window::{Window, WindowAttributes, WindowId};

#[derive(Clone, Copy)]
enum DesktopCameraSelection {
    Fixed(CanonicalCameraPose),
    MoveStep {
        step: u32,
        total_steps: u32,
        pose: CameraPose,
    },
}

impl DesktopCameraSelection {
    fn pose(self) -> Result<CameraPose, String> {
        match self {
            Self::Fixed(identity) => identity.pose().map_err(|error| error.to_string()),
            Self::MoveStep { pose, .. } => Ok(pose),
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

#[derive(Clone, Copy)]
enum DesktopSceneSelection {
    Canonical(CanonicalSceneScale),
    WindingDiagnostic,
}

#[derive(Clone)]
struct DesktopRenderConfiguration {
    scene: DesktopSceneSelection,
    camera: DesktopCameraSelection,
    raster_region_extent: u32,
    hold_background_preparation: bool,
    hold_post_upload_candidate: bool,
    inject_raster_upload_failure: bool,
    edit_burst_demo: bool,
    compute_switch_demo: bool,
    compute_switch_lifecycle_demo: bool,
    portable_milestone_demo: bool,
    portable_milestone_timing: bool,
    compute_shutdown_qualification: Option<ComputeShutdownQualification>,
    measurement: Option<MeasurementConfiguration>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ComputeShutdownQualification {
    ActivePreparation,
    HiddenCandidate,
    Presenting,
    Replacement,
}

impl DesktopRenderConfiguration {
    fn camera_pose(&self) -> Result<CameraPose, String> {
        match self.scene {
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

    fn camera_identity(&self) -> String {
        match self.scene {
            DesktopSceneSelection::Canonical(_) => self.camera.report_identity(),
            DesktopSceneSelection::WindingDiagnostic => "winding-diagnostic".to_owned(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MeasurementMode {
    FirstCorrectFrame,
    SteadyState,
}

#[derive(Clone)]
struct MeasurementConfiguration {
    mode: MeasurementMode,
    output: PathBuf,
}

fn parse_render_configuration(
    mut arguments: impl Iterator<Item = String>,
) -> Result<(DesktopRenderConfiguration, bool), String> {
    let mut scene = DesktopSceneSelection::Canonical(CanonicalSceneScale::Large);
    let mut camera = DesktopCameraSelection::Fixed(CanonicalCameraPose::Overview);
    let mut scene_was_selected = false;
    let mut camera_was_selected = false;
    let mut report_only = false;
    let mut hold_background_preparation = false;
    let mut hold_post_upload_candidate = false;
    let mut inject_raster_upload_failure = false;
    let mut edit_burst_demo = false;
    let mut compute_switch_demo = false;
    let mut compute_switch_lifecycle_demo = false;
    let mut portable_milestone_demo = false;
    let mut portable_milestone_timing = false;
    let mut compute_shutdown_qualification = None;
    let mut raster_region_extent = 32;
    let mut measurement_mode = None;
    let mut measurement_output = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--report-canonical-configuration" => report_only = true,
            "--hold-background-preparation" => hold_background_preparation = true,
            "--hold-post-upload-candidate" => hold_post_upload_candidate = true,
            "--inject-raster-upload-failure" => inject_raster_upload_failure = true,
            "--edit-burst-demo" => edit_burst_demo = true,
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
            }
            unknown => return Err(format!("unknown desktop demo argument {unknown:?}")),
        }
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
    if compute_switch_lifecycle_demo && compute_shutdown_qualification.is_some() {
        return Err(
            "the automatic compute switch lifecycle demo cannot run a compute shutdown qualification"
                .to_owned(),
        );
    }
    Ok((
        DesktopRenderConfiguration {
            scene,
            camera,
            raster_region_extent,
            hold_background_preparation,
            hold_post_upload_candidate,
            inject_raster_upload_failure,
            edit_burst_demo,
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

fn winding_diagnostic_scene() -> (DenseVoxelScene, VoxelVolumeId) {
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

fn report_winding_diagnostic_configuration(
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

fn report_canonical_configuration(
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

fn report_render_configuration(configuration: &DesktopRenderConfiguration) -> Result<(), String> {
    match configuration.scene {
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

fn measurement_revision(
    revision: VoxelSceneRevision,
) -> Result<VoxelSceneRevisionIdentity, String> {
    VoxelSceneRevisionIdentity::new(revision.to_string()).map_err(|error| error.to_string())
}

#[cfg(target_os = "windows")]
#[derive(Clone, Copy)]
enum DesktopEvent {
    Preparation(RasterArtifactPreparationEvent),
    ReleasePreparation,
    SelectCamera(CanonicalCameraPose),
    StartCameraMove,
    ReleaseEditCpuBarrier,
    ReleaseEditPostUploadLifecycleBarrier,
    ReleaseEditPostUploadBarrier,
    ReleaseComputeHandoff,
}

#[cfg(target_os = "windows")]
const RELEASE_PREPARATION_MESSAGE: u32 = windows_sys::Win32::UI::WindowsAndMessaging::WM_APP + 27;
#[cfg(target_os = "windows")]
const OVERVIEW_CAMERA_MESSAGE: u32 = windows_sys::Win32::UI::WindowsAndMessaging::WM_APP + 28;
#[cfg(target_os = "windows")]
const CAVITY_CAMERA_MESSAGE: u32 = windows_sys::Win32::UI::WindowsAndMessaging::WM_APP + 29;
#[cfg(target_os = "windows")]
const BOUNDARY_CAMERA_MESSAGE: u32 = windows_sys::Win32::UI::WindowsAndMessaging::WM_APP + 30;
#[cfg(target_os = "windows")]
const START_CAMERA_MOVE_MESSAGE: u32 = windows_sys::Win32::UI::WindowsAndMessaging::WM_APP + 31;
#[cfg(target_os = "windows")]
const RELEASE_EDIT_CPU_BARRIER_MESSAGE: u32 =
    windows_sys::Win32::UI::WindowsAndMessaging::WM_APP + 33;
#[cfg(target_os = "windows")]
const RELEASE_EDIT_POST_UPLOAD_BARRIER_MESSAGE: u32 =
    windows_sys::Win32::UI::WindowsAndMessaging::WM_APP + 34;
#[cfg(target_os = "windows")]
const RELEASE_EDIT_POST_UPLOAD_LIFECYCLE_BARRIER_MESSAGE: u32 =
    windows_sys::Win32::UI::WindowsAndMessaging::WM_APP + 35;
#[cfg(target_os = "windows")]
const RELEASE_COMPUTE_HANDOFF_MESSAGE: u32 =
    windows_sys::Win32::UI::WindowsAndMessaging::WM_APP + 36;

#[cfg(target_os = "windows")]
struct DesktopRuntime {
    backend: Option<RenderBackend>,
    text_overlay: Option<WindowsTextOverlay>,
    window: Option<Window>,
    application_error: Option<String>,
    drawable_occluded: bool,
    presentation_retry_at: Option<Instant>,
    render_configuration: DesktopRenderConfiguration,
    event_proxy: EventLoopProxy<DesktopEvent>,
    preparation: Option<RasterArtifactPreparation>,
    preparation_release: Option<RasterPreparationBarrierRelease>,
    artifact_installer: Option<RasterArtifactInstaller>,
    camera_state: CameraPose,
    camera_state_revision: CameraStateRevision,
    published_revision: Option<VoxelSceneRevision>,
    drawable_extent: ash::vk::Extent2D,
    frontend: Option<VoxelFrontend>,
    lifecycle_controller: Option<RasterLifecycleController>,
    render_path_handoff_control: Option<RenderPathHandoffControl>,
    raster_preparation_target: Option<RasterPreparationTarget>,
    raster_replacement_installer: Option<RasterArtifactInstaller>,
    raster_replacement_lifecycle_controller: Option<RasterLifecycleController>,
    interactive_switch: Option<InteractiveRenderPathSwitch>,
    compute_convergence_controller: Option<ComputeConvergenceController>,
    compute_lifecycle_controller: Option<ComputeLifecycleController>,
    compute_measurement_controller: Option<ComputeMeasurementController>,
}

#[cfg(target_os = "windows")]
impl DesktopRuntime {
    fn set_status(&self, status: &str) {
        if let Some(window) = &self.window {
            window.set_title(&format!("Voxel Nexus Vulkan Demo | {status}"));
        }
    }

    fn fail(&mut self, event_loop: &ActiveEventLoop, error: impl ToString) {
        self.application_error = Some(error.to_string());
        event_loop.exit();
    }

    fn record_close_error(&mut self, error: impl ToString) {
        let error = error.to_string();
        if self.application_error.is_none() {
            self.application_error = Some(error);
        } else {
            eprintln!("additional error during desktop close: {error}");
        }
    }

    fn publish_camera_state(&mut self, pose: CameraPose) -> Result<(), String> {
        let next_revision = self
            .camera_state_revision
            .checked_successor()
            .ok_or_else(|| "the Camera State Revision identity overflowed".to_owned())?;
        self.backend
            .as_mut()
            .ok_or_else(|| {
                "the Render Backend is unavailable for Camera State publication".to_owned()
            })?
            .publish_camera_state(pose, next_revision)
            .map_err(|error| error.to_string())?;
        self.camera_state = pose;
        self.camera_state_revision = next_revision;
        Ok(())
    }
}

#[cfg(target_os = "windows")]
struct DesktopApplication {
    desktop: DesktopRuntime,
    scenario_state: ScenarioState,
}

#[cfg(target_os = "windows")]
impl DesktopApplication {
    fn new(
        render_configuration: DesktopRenderConfiguration,
        event_proxy: EventLoopProxy<DesktopEvent>,
    ) -> Result<Self, String> {
        let camera_state = render_configuration.camera_pose()?;
        let scenario_state = ScenarioState::new(&render_configuration)?;
        Ok(Self {
            scenario_state,
            desktop: DesktopRuntime {
                backend: None,
                text_overlay: None,
                window: None,
                application_error: None,
                drawable_occluded: false,
                presentation_retry_at: None,
                render_configuration,
                event_proxy,
                preparation: None,
                preparation_release: None,
                artifact_installer: None,
                camera_state,
                camera_state_revision: CameraStateRevision::new(1),
                published_revision: None,
                drawable_extent: ash::vk::Extent2D::default(),
                frontend: None,
                lifecycle_controller: None,
                render_path_handoff_control: None,
                raster_preparation_target: None,
                raster_replacement_installer: None,
                raster_replacement_lifecycle_controller: None,
                interactive_switch: None,
                compute_convergence_controller: None,
                compute_lifecycle_controller: None,
                compute_measurement_controller: None,
            },
        })
    }

    fn scenarios(&mut self) -> ScenarioExecution<'_> {
        ScenarioExecution {
            desktop: &mut self.desktop,
            state: &mut self.scenario_state,
        }
    }
}

#[cfg(target_os = "windows")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RasterPreparationTarget {
    Initial,
    Replacement,
}

#[cfg(target_os = "windows")]
struct InteractiveRenderPathSwitch {
    source: RenderPathStrategy,
    replacement: RenderPathStrategy,
    revision: VoxelSceneRevision,
    handoff_reported: bool,
    retiring_raster: Option<RasterLifecycleController>,
    requested_at: Instant,
}

#[cfg(target_os = "windows")]
const PRESENTATION_RETRY_DELAY: Duration = Duration::from_millis(100);
#[cfg(target_os = "windows")]
const STEADY_MEASUREMENT_EXTENT: ash::vk::Extent2D = ash::vk::Extent2D {
    width: 1920,
    height: 1080,
};

#[cfg(target_os = "windows")]
#[cfg(target_os = "windows")]
impl ApplicationHandler<DesktopEvent> for DesktopApplication {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.desktop.window.is_some() {
            return;
        }

        let mut attributes = WindowAttributes::default().with_title("Voxel Nexus Vulkan Demo");
        if matches!(
            self.desktop
                .render_configuration
                .measurement
                .as_ref()
                .map(|measurement| measurement.mode),
            Some(MeasurementMode::SteadyState)
        ) {
            attributes = attributes
                .with_inner_size(winit::dpi::PhysicalSize::new(
                    STEADY_MEASUREMENT_EXTENT.width,
                    STEADY_MEASUREMENT_EXTENT.height,
                ))
                .with_decorations(false);
        }
        let window = match event_loop.create_window(attributes) {
            Ok(window) => window,
            Err(error) => {
                self.desktop.application_error =
                    Some(format!("could not create the demo window: {error}"));
                event_loop.exit();
                return;
            }
        };
        let text_overlay = if self.desktop.render_configuration.edit_burst_demo
            || self.desktop.render_configuration.compute_switch_demo
        {
            match WindowsTextOverlay::new(&window) {
                Ok(overlay) => Some(overlay),
                Err(error) => {
                    self.desktop.application_error = Some(error);
                    event_loop.exit();
                    return;
                }
            }
        } else {
            None
        };
        if matches!(
            self.desktop
                .render_configuration
                .measurement
                .as_ref()
                .map(|measurement| measurement.mode),
            Some(MeasurementMode::SteadyState)
        ) && let Err(error) = set_measurement_extent(&window, STEADY_MEASUREMENT_EXTENT)
        {
            self.desktop.application_error = Some(error);
            event_loop.exit();
            return;
        }
        let adapter = WindowsPresentationAdapter::new(&window);
        let drawable_size = window.inner_size();
        let initial_drawable_extent = ash::vk::Extent2D {
            width: drawable_size.width,
            height: drawable_size.height,
        };
        self.desktop.drawable_extent = initial_drawable_extent;
        let (scene, occupied_voxels) = match self.desktop.render_configuration.scene {
            DesktopSceneSelection::WindingDiagnostic => {
                let (scene, _) = winding_diagnostic_scene();
                if let Err(error) =
                    report_winding_diagnostic_configuration(&self.desktop.render_configuration)
                {
                    self.desktop.fail(event_loop, error);
                    return;
                }
                (scene, 2)
            }
            DesktopSceneSelection::Canonical(scale) => {
                let canonical = match generate_canonical_scene(scale) {
                    Ok(canonical) => canonical,
                    Err(error) => {
                        self.desktop.application_error = Some(format!(
                            "could not generate the canonical Voxel Scene: {error}"
                        ));
                        event_loop.exit();
                        return;
                    }
                };
                if let Err(error) = report_canonical_configuration(
                    canonical.metadata(),
                    &self.desktop.render_configuration,
                ) {
                    self.desktop.fail(event_loop, error);
                    return;
                }
                let occupied_voxels = canonical.metadata().occupied_count();
                (canonical.into_scene(), occupied_voxels)
            }
        };
        self.scenario_state.evidence.occupied_voxels = occupied_voxels;
        let frontend = VoxelFrontend::new();
        let view = match frontend.publish(scene) {
            Ok(view) => view,
            Err(error) => {
                self.desktop.application_error = Some(format!(
                    "could not publish the configured Voxel Scene: {error}"
                ));
                event_loop.exit();
                return;
            }
        };
        self.scenarios()
            .prepare_scene_qualification(event_loop, &view);
        if event_loop.exiting() {
            return;
        }
        self.desktop.frontend = Some(frontend);
        let published_revision = view.revision();
        if let Err(error) = self
            .scenario_state
            .evidence
            .scene_published(published_revision)
        {
            self.desktop.fail(event_loop, error);
            return;
        }
        let (mut render_path, artifact_installer, _) =
            RasterRenderPathAdapter::awaiting_artifact_with_camera_control(
                self.desktop.camera_state,
                self.desktop.camera_state_revision,
                view.scene_id().clone(),
                published_revision,
            );
        self.scenarios().configure_raster_qualification(
            event_loop,
            &mut render_path,
            &artifact_installer,
            &view,
        );
        if event_loop.exiting() {
            return;
        }
        let options = if self.desktop.render_configuration.portable_milestone_timing {
            RenderBackendOptions {
                validation_enabled: false,
                presentation_throttling_enabled: false,
                gpu_timestamps_enabled: false,
            }
        } else if matches!(
            self.desktop
                .render_configuration
                .measurement
                .as_ref()
                .map(|measurement| measurement.mode),
            Some(MeasurementMode::SteadyState)
        ) {
            RenderBackendOptions {
                validation_enabled: false,
                presentation_throttling_enabled: false,
                gpu_timestamps_enabled: true,
            }
        } else {
            RenderBackendOptions::default()
        };
        let render_path = RenderPathSwitchOwner::new(Box::new(render_path));
        let render_path_handoff_control = render_path.handoff_control();
        if self
            .desktop
            .render_configuration
            .compute_switch_lifecycle_demo
            || self
                .desktop
                .render_configuration
                .compute_shutdown_qualification
                == Some(ComputeShutdownQualification::Replacement)
        {
            render_path_handoff_control.hold();
        }
        let backend = match RenderBackend::initialize_with_options(
            c"Voxel Nexus Desktop Demo",
            &adapter,
            initial_drawable_extent,
            render_path,
            options,
        ) {
            Ok(backend) => backend,
            Err(error) => {
                self.desktop.application_error = Some(error.to_string());
                event_loop.exit();
                return;
            }
        };
        let switching_diagnostics = match backend.render_path_switch_diagnostics() {
            Some(diagnostics) => diagnostics,
            None => {
                self.desktop.application_error =
                    Some("Render Path switching diagnostics are unavailable".to_owned());
                event_loop.exit();
                return;
            }
        };
        println!(
            "Presenting Render Path: {:?}; Required={} Visible={} CameraStateRevision={:?} Readiness={:?}",
            switching_diagnostics.presenting().strategy(),
            switching_diagnostics.presenting().required_revision(),
            switching_diagnostics.presenting().visible_revision(),
            switching_diagnostics.presenting().camera_state_revision(),
            switching_diagnostics.presenting().readiness(),
        );
        let presentation_extent = match backend.presentation_extent() {
            Some(extent) => extent,
            None => {
                self.desktop.application_error =
                    Some("the Render Backend has no configured presentation extent".to_owned());
                event_loop.exit();
                return;
            }
        };
        println!(
            "Vulkan drawable extent: {}x{}",
            presentation_extent.width, presentation_extent.height
        );
        if options.gpu_timestamps_enabled && presentation_extent != STEADY_MEASUREMENT_EXTENT {
            self.desktop.application_error = Some(format!(
                "steady-state measurement requires an actual 1920x1080 presentation extent, but Vulkan configured {}x{}",
                presentation_extent.width, presentation_extent.height
            ));
            event_loop.exit();
            return;
        }
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
        self.desktop.backend = Some(backend);
        self.desktop.text_overlay = text_overlay;
        self.desktop.window = Some(window);
        self.desktop.artifact_installer = Some(artifact_installer);
        self.desktop.render_path_handoff_control = Some(render_path_handoff_control);
        self.desktop.published_revision = Some(published_revision);
        let (barrier, preparation_release) = if self
            .desktop
            .render_configuration
            .hold_background_preparation
        {
            let (barrier, release) = RasterPreparationBarrier::held();
            (Some(barrier), Some(release))
        } else {
            (None, None)
        };
        let event_proxy = self.desktop.event_proxy.clone();
        let preparation = match RasterArtifactPreparation::start_regions(
            view,
            VoxelExtent::new(
                self.desktop.render_configuration.raster_region_extent,
                self.desktop.render_configuration.raster_region_extent,
                self.desktop.render_configuration.raster_region_extent,
            ),
            barrier,
            move |event| {
                if event_proxy
                    .send_event(DesktopEvent::Preparation(event))
                    .is_err()
                {
                    eprintln!("desktop event loop closed before preparation notification");
                }
            },
        ) {
            Ok(preparation) => preparation,
            Err(error) => {
                self.desktop.application_error = Some(error.to_string());
                event_loop.exit();
                return;
            }
        };
        self.desktop.preparation = Some(preparation);
        self.desktop.raster_preparation_target = Some(RasterPreparationTarget::Initial);
        self.desktop.preparation_release = preparation_release;
        self.desktop
            .set_status(&format!("preparing revision {published_revision}"));
        if self.desktop.render_configuration.compute_switch_demo
            && let Err(error) = self.scenarios().set_render_path_overlay()
        {
            self.desktop.application_error = Some(error);
            event_loop.exit();
            return;
        }
        if let Some(window) = &self.desktop.window {
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
                self.scenarios().before_close();
                if let Some(release) = self.desktop.preparation_release.take()
                    && let Err(error) = release.release()
                {
                    self.desktop.record_close_error(error);
                }
                if let Some(mut preparation) = self.desktop.preparation.take()
                    && let Err(error) = preparation.cancel_and_join()
                {
                    self.desktop.record_close_error(error);
                }
                self.scenarios().verify_held_raster_candidate();
                if let Some(backend) = &mut self.desktop.backend
                    && let Err(error) = backend.shutdown()
                {
                    self.desktop.record_close_error(error);
                }
                self.scenarios().after_shutdown();
                event_loop.exit();
            }
            WindowEvent::Resized(drawable_size) => {
                let steady_measurement = matches!(
                    self.desktop
                        .render_configuration
                        .measurement
                        .as_ref()
                        .map(|measurement| measurement.mode),
                    Some(MeasurementMode::SteadyState)
                );
                let drawable_size = if steady_measurement
                    && (drawable_size.width != STEADY_MEASUREMENT_EXTENT.width
                        || drawable_size.height != STEADY_MEASUREMENT_EXTENT.height)
                {
                    let Some(window) = &self.desktop.window else {
                        self.desktop.fail(
                            event_loop,
                            "the steady-state measurement window is unavailable",
                        );
                        return;
                    };
                    if let Err(error) = set_measurement_extent(window, STEADY_MEASUREMENT_EXTENT) {
                        self.desktop.fail(event_loop, error);
                        return;
                    }
                    let corrected_size = window.inner_size();
                    if corrected_size.width != STEADY_MEASUREMENT_EXTENT.width
                        || corrected_size.height != STEADY_MEASUREMENT_EXTENT.height
                    {
                        self.desktop.fail(
                            event_loop,
                            format!(
                                "steady-state measurement extent changed from 1920x1080 to {}x{}",
                                corrected_size.width, corrected_size.height
                            ),
                        );
                        return;
                    }
                    corrected_size
                } else {
                    drawable_size
                };
                let drawable_extent = if self.desktop.drawable_occluded {
                    ash::vk::Extent2D::default()
                } else {
                    ash::vk::Extent2D {
                        width: drawable_size.width,
                        height: drawable_size.height,
                    }
                };
                if let Err(error) = self.desktop.set_drawable_extent(drawable_extent) {
                    self.desktop.fail(event_loop, error);
                    return;
                }
                self.scenarios().report_drawable_extent(drawable_extent);
            }
            WindowEvent::Occluded(occluded) => {
                if occluded
                    && matches!(
                        self.desktop
                            .render_configuration
                            .measurement
                            .as_ref()
                            .map(|measurement| measurement.mode),
                        Some(MeasurementMode::SteadyState)
                    )
                {
                    self.desktop.fail(
                        event_loop,
                        "steady-state measurement window became occluded",
                    );
                    return;
                }
                self.desktop.drawable_occluded = occluded;
                let drawable_extent = if occluded {
                    ash::vk::Extent2D::default()
                } else {
                    let drawable_size = self
                        .desktop
                        .window
                        .as_ref()
                        .map(Window::inner_size)
                        .unwrap_or_default();
                    ash::vk::Extent2D {
                        width: drawable_size.width,
                        height: drawable_size.height,
                    }
                };
                if let Err(error) = self.desktop.set_drawable_extent(drawable_extent) {
                    self.desktop.fail(event_loop, error);
                    return;
                }
                self.scenarios().report_drawable_extent(drawable_extent);
            }
            WindowEvent::KeyboardInput { event, .. } => {
                self.scenarios().keyboard_input(event_loop, &event);
            }
            WindowEvent::RedrawRequested => {
                if should_wait_for_initial_raster_artifact(
                    &self.desktop.render_configuration,
                    self.scenario_state.evidence.first_matching_frame_presented,
                ) {
                    let initial_artifact_revision = match &self.desktop.artifact_installer {
                        Some(installer) => match installer.installed_source_revision() {
                            Ok(revision) => revision,
                            Err(error) => {
                                self.desktop.fail(event_loop, error);
                                return;
                            }
                        },
                        None => None,
                    };
                    if initial_artifact_revision.is_none() {
                        return;
                    }
                }
                let frame_started_at = Instant::now();
                let (outcome, gpu_observation, submitted_frame_sequence, presentation_extent) =
                    match &mut self.desktop.backend {
                        Some(backend) => match backend.draw_frame() {
                            Ok(outcome) => (
                                outcome,
                                backend.take_frame_observation(),
                                backend.last_submitted_frame_sequence(),
                                backend.presentation_extent(),
                            ),
                            Err(error) => {
                                self.desktop.application_error = Some(error.to_string());
                                event_loop.exit();
                                return;
                            }
                        },
                        None => (FrameOutcome::Suspended, None, None, None),
                    };
                self.scenarios().after_draw(
                    event_loop,
                    frame_started_at,
                    submitted_frame_sequence,
                    gpu_observation,
                    presentation_extent,
                );
                if event_loop.exiting() {
                    return;
                }
                match outcome {
                    FrameOutcome::Presented => {
                        self.desktop.presentation_retry_at = None;
                        self.scenarios()
                            .after_presented(event_loop, submitted_frame_sequence);
                    }
                    FrameOutcome::Recreate => {
                        self.desktop.presentation_retry_at = None;
                        if let Some(window) = &self.desktop.window {
                            window.request_redraw();
                        }
                    }
                    FrameOutcome::RetryLater => {
                        self.desktop.presentation_retry_at =
                            Some(Instant::now() + PRESENTATION_RETRY_DELAY);
                    }
                    FrameOutcome::Suspended => {
                        self.desktop.presentation_retry_at = None;
                        match self
                            .scenarios()
                            .restore_compute_lifecycle_after_suspension()
                        {
                            Ok(true) | Ok(false) => {}
                            Err(error) => {
                                self.desktop.fail(event_loop, error);
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: DesktopEvent) {
        self.scenarios().user_event(event_loop, event);
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let Some(retry_at) = self.desktop.presentation_retry_at else {
            event_loop.set_control_flow(ControlFlow::Wait);
            return;
        };
        if Instant::now() < retry_at {
            event_loop.set_control_flow(ControlFlow::WaitUntil(retry_at));
            return;
        }
        self.desktop.presentation_retry_at = None;
        event_loop.set_control_flow(ControlFlow::Wait);
        if let Some(window) = &self.desktop.window {
            window.request_redraw();
        }
    }
}

#[cfg(target_os = "windows")]
fn desktop_event_for_windows_message(message: u32) -> Option<DesktopEvent> {
    match message {
        RELEASE_PREPARATION_MESSAGE => Some(DesktopEvent::ReleasePreparation),
        OVERVIEW_CAMERA_MESSAGE => Some(DesktopEvent::SelectCamera(CanonicalCameraPose::Overview)),
        CAVITY_CAMERA_MESSAGE => Some(DesktopEvent::SelectCamera(
            CanonicalCameraPose::CavityMaterialCloseUp,
        )),
        BOUNDARY_CAMERA_MESSAGE => Some(DesktopEvent::SelectCamera(
            CanonicalCameraPose::BoundaryCutaway,
        )),
        START_CAMERA_MOVE_MESSAGE => Some(DesktopEvent::StartCameraMove),
        RELEASE_EDIT_CPU_BARRIER_MESSAGE => Some(DesktopEvent::ReleaseEditCpuBarrier),
        RELEASE_EDIT_POST_UPLOAD_BARRIER_MESSAGE => {
            Some(DesktopEvent::ReleaseEditPostUploadBarrier)
        }
        RELEASE_EDIT_POST_UPLOAD_LIFECYCLE_BARRIER_MESSAGE => {
            Some(DesktopEvent::ReleaseEditPostUploadLifecycleBarrier)
        }
        RELEASE_COMPUTE_HANDOFF_MESSAGE => Some(DesktopEvent::ReleaseComputeHandoff),
        _ => None,
    }
}

#[cfg(target_os = "windows")]
fn run() -> Result<(), String> {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    if let Some(diagnostic_result) =
        background_preparation_failure_diagnostic(arguments.clone().into_iter())
    {
        return diagnostic_result;
    }
    if let Some(diagnostic_result) = render_path_failure_diagnostic(arguments.clone().into_iter()) {
        return diagnostic_result;
    }
    if let Some(diagnostic_result) =
        unsupported_prerequisite_diagnostic(arguments.clone().into_iter())
    {
        return diagnostic_result;
    }
    let (configuration, report_only) = parse_render_configuration(arguments.into_iter())?;
    if report_only {
        report_render_configuration(&configuration)?;
        return Ok(());
    }
    let event_proxy_slot = Arc::new(Mutex::new(None::<EventLoopProxy<DesktopEvent>>));
    let mut event_loop_builder = EventLoop::<DesktopEvent>::with_user_event();
    event_loop_builder.with_msg_hook({
        let event_proxy_slot = event_proxy_slot.clone();
        move |raw_message| {
            if raw_message.is_null() {
                return false;
            }
            let message = unsafe {
                (*(raw_message as *const windows_sys::Win32::UI::WindowsAndMessaging::MSG)).message
            };
            let Some(event) = desktop_event_for_windows_message(message) else {
                return false;
            };
            let event_proxy = match event_proxy_slot.lock() {
                Ok(event_proxy) => event_proxy.clone(),
                Err(_) => {
                    eprintln!("desktop verification event proxy state is unavailable");
                    return true;
                }
            };
            match event_proxy {
                Some(event_proxy) => {
                    if event_proxy.send_event(event).is_err() {
                        eprintln!("desktop event loop closed before verification event delivery");
                    }
                }
                None => eprintln!("desktop verification event arrived before event-loop startup"),
            }
            true
        }
    });
    let event_loop = event_loop_builder
        .build()
        .map_err(|error| format!("could not start the event loop: {error}"))?;
    let event_proxy = event_loop.create_proxy();
    let mut event_proxy_destination = event_proxy_slot
        .lock()
        .map_err(|_| "desktop verification event proxy state is unavailable".to_owned())?;
    *event_proxy_destination = Some(event_proxy.clone());
    drop(event_proxy_destination);
    let mut application = DesktopApplication::new(configuration, event_proxy)?;
    event_loop
        .run_app(&mut application)
        .map_err(|error| format!("the desktop event loop failed: {error}"))?;
    match application.desktop.application_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

fn background_preparation_failure_diagnostic(
    mut arguments: impl Iterator<Item = String>,
) -> Option<Result<(), String>> {
    if arguments.next().as_deref() != Some("--verify-background-preparation-failure") {
        return None;
    }
    let result = match arguments.next().as_deref() {
        Some("derivation") => (|| {
            let canonical =
                generate_canonical_scene(CanonicalSceneScale::Small).map_err(|error| {
                    format!("could not generate the diagnostic Voxel Scene: {error}")
                })?;
            let view = VoxelFrontend::new()
                .publish(canonical.into_scene())
                .map_err(|error| {
                    format!("could not publish the diagnostic Voxel Scene: {error}")
                })?;
            let (completion_sender, completion_receiver) = mpsc::sync_channel(1);
            let mut preparation = RasterArtifactPreparation::start(
                view,
                voxel_frontend::VoxelVolumeId::new("injected-missing-volume"),
                None,
                move |event| {
                    if matches!(event, RasterArtifactPreparationEvent::Completed { .. })
                        && completion_sender.send(()).is_err()
                    {
                        eprintln!("background preparation diagnostic receiver closed");
                    }
                },
            )
            .map_err(|error| error.to_string())?;
            completion_receiver
                .recv()
                .map_err(|error| format!("background preparation diagnostic hung: {error}"))?;
            match preparation.try_complete() {
                Err(error) => Err(error.to_string()),
                Ok(Some(_)) => Err(
                    "the injected background derivation failure produced an artifact".to_owned(),
                ),
                Ok(None) => {
                    Err("the injected background derivation failure did not complete".to_owned())
                }
            }
        })(),
        Some(case) => Err(format!(
            "unknown background-preparation diagnostic {case:?}; expected derivation"
        )),
        None => Err("missing background-preparation diagnostic; expected derivation".to_owned()),
    };
    Some(result)
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
        Some("advance-frame-boundary") => Ok(RenderPathPhase::AdvanceFrameBoundary),
        Some("record") => Ok(RenderPathPhase::Record),
        Some("shutdown") => Ok(RenderPathPhase::Shutdown),
        Some("upload") => {
            return Some(Err(RasterArtifactInstallationError::new(
                RasterArtifactInstallationPhase::Upload,
                VoxelSceneRevision::new(41),
                Box::new(InjectedRenderPathFailure),
            )
            .to_string()));
        }
        Some(phase) => Err(format!(
            "unknown Render Path phase {phase:?}; expected release, configure, advance-frame-boundary, record, shutdown, or upload"
        )),
        None => Err(
            "missing Render Path phase; expected release, configure, advance-frame-boundary, record, shutdown, or upload"
                .to_owned(),
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
                supports_compute: true,
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
                supports_compute: true,
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

#[cfg(all(test, target_os = "windows"))]
mod measurement_tests {
    use super::{
        CanonicalCameraPose, ComputeShutdownQualification, CpuFrameMeasurement, MeasurementEvent,
        SteadyFrameCollection, compute_edit_burst_admission, fixed_edit_burst,
        format_convergence_characterization, format_convergence_overlay,
        format_render_path_overlay, parse_render_configuration, render_path_switch_admission,
        should_request_compute_edit_burst, should_request_render_path_switch,
        should_start_edit_burst, should_wait_for_initial_raster_artifact,
    };

    #[test]
    fn fixed_candidate_raster_region_extent_is_explicitly_configurable() -> Result<(), String> {
        let (configuration, report_only) = parse_render_configuration(
            [
                "--scene-scale",
                "256",
                "--raster-region-extent",
                "64",
                "--edit-burst-demo",
            ]
            .into_iter()
            .map(str::to_owned),
        )?;

        assert_eq!(configuration.raster_region_extent, 64);
        assert!(!report_only);
        Ok(())
    }

    #[test]
    fn compute_switch_demo_is_explicit_and_does_not_replace_raster_qualification_modes()
    -> Result<(), String> {
        let (configuration, _) =
            parse_render_configuration(["--compute-switch-demo"].into_iter().map(str::to_owned))?;
        assert!(configuration.compute_switch_demo);
        assert!(!configuration.compute_switch_lifecycle_demo);

        let (lifecycle_configuration, _) = parse_render_configuration(
            ["--compute-switch-lifecycle-demo"]
                .into_iter()
                .map(str::to_owned),
        )?;
        assert!(lifecycle_configuration.compute_switch_demo);
        assert!(lifecycle_configuration.compute_switch_lifecycle_demo);

        let (milestone_configuration, _) = parse_render_configuration(
            ["--portable-compute-ray-milestone-demo"]
                .into_iter()
                .map(str::to_owned),
        )?;
        assert!(milestone_configuration.compute_switch_demo);
        assert!(milestone_configuration.compute_switch_lifecycle_demo);
        assert!(milestone_configuration.portable_milestone_demo);

        let (timing_configuration, _) = parse_render_configuration(
            ["--portable-compute-ray-milestone-timing"]
                .into_iter()
                .map(str::to_owned),
        )?;
        assert!(timing_configuration.portable_milestone_demo);
        assert!(timing_configuration.portable_milestone_timing);

        for (argument, expected) in [
            (
                "active-preparation",
                ComputeShutdownQualification::ActivePreparation,
            ),
            (
                "hidden-candidate",
                ComputeShutdownQualification::HiddenCandidate,
            ),
            ("presenting", ComputeShutdownQualification::Presenting),
            ("replacement", ComputeShutdownQualification::Replacement),
        ] {
            let (qualification, _) = parse_render_configuration(
                ["--compute-shutdown-qualification", argument]
                    .into_iter()
                    .map(str::to_owned),
            )?;
            assert!(qualification.compute_switch_demo);
            assert_eq!(qualification.compute_shutdown_qualification, Some(expected));
        }

        let incompatible = parse_render_configuration(
            ["--compute-switch-demo", "--edit-burst-demo"]
                .into_iter()
                .map(str::to_owned),
        );
        assert!(incompatible.is_err());
        Ok(())
    }

    #[test]
    fn every_compute_switch_mode_waits_for_the_initial_raster_artifact() -> Result<(), String> {
        for arguments in [
            vec!["--compute-switch-demo"],
            vec!["--compute-switch-lifecycle-demo"],
            vec!["--portable-compute-ray-milestone-demo"],
            vec!["--compute-shutdown-qualification", "presenting"],
        ] {
            let (configuration, _) =
                parse_render_configuration(arguments.into_iter().map(str::to_owned))?;
            assert!(should_wait_for_initial_raster_artifact(
                &configuration,
                false
            ));
            assert!(!should_wait_for_initial_raster_artifact(
                &configuration,
                true
            ));
        }

        let (raster_configuration, _) = parse_render_configuration(std::iter::empty())?;
        assert!(!should_wait_for_initial_raster_artifact(
            &raster_configuration,
            false
        ));
        Ok(())
    }

    #[test]
    fn characterization_line_retains_every_phase_disposition_and_lifecycle_event() {
        let report = format_convergence_characterization(&RasterConvergenceCharacterization {
            phases: RasterConvergencePhaseTimings {
                submission_bookkeeping_milliseconds: 1.0,
                queued_wait_milliseconds: 2.0,
                cpu_derivation_milliseconds: 3.0,
                upload_milliseconds: 4.0,
                frame_boundary_commit_milliseconds: 5.0,
            },
            work: RasterRegionWorkDisposition {
                scheduled: 6,
                completed: 7,
                cancelled: 8,
                stale: 9,
            },
            installed: RasterGpuResourceUsage {
                bytes: 10,
                resources: 11,
            },
            hidden: RasterGpuResourceUsage {
                bytes: 12,
                resources: 13,
            },
            retired: RasterGpuResourceUsage {
                bytes: 14,
                resources: 15,
            },
            peak: RasterGpuResourceUsage {
                bytes: 16,
                resources: 17,
            },
            cancellation_observations: vec![RasterCancellationObservation {
                revision: VoxelSceneRevision::new(2),
                scheduled_regions: 18,
                completed_regions: 19,
                cancelled_regions: 20,
            }],
            safe_retirements: vec![RasterSafeRetirementEvent {
                revision: VoxelSceneRevision::new(3),
                disposition: RasterSafeRetirementDisposition::StaleCandidate,
                resources: RasterGpuResourceUsage {
                    bytes: 21,
                    resources: 22,
                },
            }],
        });

        for required in [
            "submission_bookkeeping_ms=1.000000",
            "queued_wait_ms=2.000000",
            "cpu_derivation_ms=3.000000",
            "upload_ms=4.000000",
            "frame_boundary_commit_ms=5.000000",
            "scheduled_regions=6 completed_regions=7 cancelled_regions=8 stale_regions=9",
            "installed_bytes=10 installed_resources=11 hidden_bytes=12 hidden_resources=13",
            "retired_bytes=14 retired_resources=15 peak_bytes=16 peak_resources=17",
            "cancellation_events=2:18:19:20",
            "safe_retirement_events=3:stale-candidate:21:22",
        ] {
            assert!(
                report.contains(required),
                "missing {required:?} from {report}"
            );
        }
    }

    use canonical_scene::{CanonicalSceneScale, generate_canonical_scene};
    use raster_render_path::{
        CameraPose, RasterCancellationObservation, RasterConvergenceCharacterization,
        RasterConvergencePhaseTimings, RasterConvergenceStatus, RasterGpuResourceUsage,
        RasterRegionWorkDisposition, RasterRenderPathAdapter, RasterSafeRetirementDisposition,
        RasterSafeRetirementEvent,
    };
    use render_backend::{CameraStateRevision, FrameObservation, RenderPathSwitchOwner};
    use std::time::{Duration, Instant};
    use voxel_frontend::{VoxelEditOutcome, VoxelFrontend, VoxelSceneRevision};
    use winit::{
        event::ElementState,
        keyboard::{Key, NamedKey},
    };

    #[test]
    fn only_one_non_repeated_space_press_can_start_an_awaiting_edit_burst() {
        let space = Key::Named(NamedKey::Space);
        assert!(should_start_edit_burst(
            true,
            true,
            ElementState::Pressed,
            false,
            &space
        ));
        assert!(!should_start_edit_burst(
            false,
            true,
            ElementState::Pressed,
            false,
            &space
        ));
        assert!(!should_start_edit_burst(
            true,
            false,
            ElementState::Pressed,
            false,
            &space
        ));
        assert!(!should_start_edit_burst(
            true,
            true,
            ElementState::Pressed,
            true,
            &space
        ));
        assert!(!should_start_edit_burst(
            true,
            true,
            ElementState::Released,
            false,
            &space
        ));
    }

    #[test]
    fn only_one_non_repeated_tab_press_requests_an_interactive_switch() {
        let tab = Key::Named(NamedKey::Tab);
        assert!(should_request_render_path_switch(
            true,
            ElementState::Pressed,
            false,
            &tab
        ));
        assert!(!should_request_render_path_switch(
            false,
            ElementState::Pressed,
            false,
            &tab
        ));
        assert!(!should_request_render_path_switch(
            true,
            ElementState::Pressed,
            true,
            &tab
        ));
        assert!(!should_request_render_path_switch(
            true,
            ElementState::Released,
            false,
            &tab
        ));
        assert!(!should_request_render_path_switch(
            true,
            ElementState::Pressed,
            false,
            &Key::Named(NamedKey::Space)
        ));
    }

    #[test]
    fn space_requests_are_detected_even_when_compute_burst_admission_will_reject_them() {
        let space = Key::Named(NamedKey::Space);
        assert!(should_request_compute_edit_burst(
            true,
            ElementState::Pressed,
            false,
            &space
        ));
        assert!(!should_request_compute_edit_burst(
            false,
            ElementState::Pressed,
            false,
            &space
        ));
        assert!(!should_request_compute_edit_burst(
            true,
            ElementState::Pressed,
            true,
            &space
        ));
        assert!(!should_request_compute_edit_burst(
            true,
            ElementState::Released,
            false,
            &space
        ));
    }

    #[test]
    fn compute_edit_burst_admission_requires_an_idle_compute_presenter_and_awaiting_plan()
    -> Result<(), Box<dyn std::error::Error>> {
        let frontend = VoxelFrontend::new();
        let view =
            frontend.publish(generate_canonical_scene(CanonicalSceneScale::Small)?.into_scene())?;
        let compute = compute_ray_render_path::ComputeRayRenderPathAdapter::new(
            view,
            CanonicalCameraPose::Overview.pose()?,
            CameraStateRevision::new(1),
        )?;
        let owner = RenderPathSwitchOwner::new(Box::new(compute));
        let diagnostics = owner.diagnostics();

        assert!(compute_edit_burst_admission(&diagnostics, true).is_ok());
        assert!(compute_edit_burst_admission(&diagnostics, false).is_err());

        let revision = VoxelSceneRevision::new(1);
        let (raster, _, _) = RasterRenderPathAdapter::awaiting_artifact_with_camera_control(
            CanonicalCameraPose::Overview.pose()?,
            CameraStateRevision::new(1),
            voxel_frontend::VoxelSceneId::new("raster"),
            revision,
        );
        let raster_diagnostics = RenderPathSwitchOwner::new(Box::new(raster)).diagnostics();
        assert!(compute_edit_burst_admission(&raster_diagnostics, true).is_err());
        Ok(())
    }

    #[test]
    fn render_path_overlay_reports_the_idle_presenter_and_rejects_a_preparing_presenter()
    -> Result<(), Box<dyn std::error::Error>> {
        let revision = VoxelSceneRevision::new(4);
        let (raster, _, _) = RasterRenderPathAdapter::awaiting_artifact_with_camera_control(
            CameraPose::new(
                [2.0, 2.0, 2.0],
                [0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                60.0,
                0.1,
                100.0,
            )?,
            CameraStateRevision::new(7),
            voxel_frontend::VoxelSceneId::new("revision-four"),
            revision,
        );
        let owner = RenderPathSwitchOwner::new(Box::new(raster));
        let diagnostics = owner.diagnostics();

        assert!(render_path_switch_admission(&diagnostics).is_err());
        assert_eq!(
            format_render_path_overlay(
                &diagnostics,
                "inactive",
                "overview",
                "Tab-rejected-presenter-not-ready",
            ),
            "Presenter=Raster Switch=idle ReplacementRevision=none Required=4 Visible=4 Burst=inactive Camera=overview Control=Tab-rejected-presenter-not-ready"
        );
        Ok(())
    }

    #[test]
    fn in_client_overlay_text_reports_both_revisions_and_region_counts() {
        assert_eq!(
            format_convergence_overlay(
                "cpu-barrier-held",
                RasterConvergenceStatus {
                    required_revision: VoxelSceneRevision::new(3),
                    visible_revision: VoxelSceneRevision::new(1),
                    affected_region_count: 2,
                    unaffected_region_count: 254,
                },
                "cavity",
            ),
            "EditBurst=cpu-barrier-held Required=3 Visible=1 Affected=2 Unaffected=254 Camera=cavity"
        );
    }

    #[test]
    fn fixed_edit_burst_has_three_ordered_value_changing_commands_and_checked_final_revision()
    -> Result<(), Box<dyn std::error::Error>> {
        for scale in [
            CanonicalSceneScale::Small,
            CanonicalSceneScale::Medium,
            CanonicalSceneScale::Large,
        ] {
            let frontend = VoxelFrontend::new();
            let view = frontend.publish(generate_canonical_scene(scale)?.into_scene())?;
            let mut plan = fixed_edit_burst(&view, 16)?;
            assert_eq!(plan.commands.len(), 3);
            assert_eq!(plan.expected_final_revision, VoxelSceneRevision::new(4));
            assert!(plan.take_next_owned_command().is_err());
            plan.claim_space_keypress()?;
            for expected_revision in 2..=4 {
                let command = plan.take_next_owned_command()?;
                let outcome = frontend.edit(command)?;
                let VoxelEditOutcome::Changed { view, .. } = outcome else {
                    return Err("fixed command did not change its Voxel Value".into());
                };
                assert_eq!(view.revision(), VoxelSceneRevision::new(expected_revision));
            }
        }
        Ok(())
    }

    #[test]
    fn steady_collection_pairs_sequences_and_excludes_pre_warmup_frames()
    -> Result<(), Box<dyn std::error::Error>> {
        let matching_presentation_at = Instant::now();
        let mut collection = SteadyFrameCollection::new(matching_presentation_at);
        collection.submit(CpuFrameMeasurement {
            sequence: 10,
            started_at: matching_presentation_at + Duration::from_secs(4),
            milliseconds: 2.0,
        });
        assert_eq!(
            collection.complete(FrameObservation {
                sequence: 10,
                gpu_frame_milliseconds: 1.0,
            })?,
            None
        );

        collection.submit(CpuFrameMeasurement {
            sequence: 11,
            started_at: matching_presentation_at + Duration::from_secs(6),
            milliseconds: 2.5,
        });
        assert_eq!(
            collection.complete(FrameObservation {
                sequence: 11,
                gpu_frame_milliseconds: 1.5,
            })?,
            Some(MeasurementEvent::SteadyFrame {
                sequence: 11,
                cpu_frame_milliseconds: 2.5,
                gpu_frame_milliseconds: 1.5,
            })
        );
        Ok(())
    }
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
    if let Some(result) = background_preparation_failure_diagnostic(arguments.clone().into_iter()) {
        return application_exit_code(result);
    }
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
        let result = parse_render_configuration(arguments.into_iter())
            .and_then(|(configuration, _)| report_render_configuration(&configuration));
        return application_exit_code(result);
    }
    eprintln!("The Voxel Nexus desktop demo currently supports Windows only.");
    ExitCode::SUCCESS
}

#[cfg(target_os = "windows")]
impl DesktopRuntime {
    fn set_drawable_extent(&mut self, drawable_extent: ash::vk::Extent2D) -> Result<(), String> {
        self.drawable_extent = drawable_extent;
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
        if let Some(overlay) = &self.text_overlay {
            let scale_factor = self
                .window
                .as_ref()
                .map(Window::scale_factor)
                .unwrap_or(1.0);
            overlay.layout(drawable_extent, scale_factor)?;
        }
        Ok(())
    }
}

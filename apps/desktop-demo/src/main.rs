#[cfg(target_os = "windows")]
mod windows_adapter;

use canonical_inspection::{CanonicalCameraPose, overview_to_cavity_camera_move};
use canonical_scene::{CanonicalSceneMetadata, CanonicalSceneScale, generate_canonical_scene};
#[cfg(target_os = "windows")]
use measurement_evidence::{MeasurementEvent, ResourceCounts, VoxelSceneRevisionIdentity};
use raster_render_path::{
    CameraPose, RasterArtifactInstallationError, RasterArtifactInstallationPhase,
    RasterArtifactInstaller, RasterArtifactPreparation, RasterArtifactPreparationEvent,
    RasterCameraController, RasterConvergenceStatus, RasterLifecycleController,
    RasterPreparationBarrier, RasterPreparationBarrierRelease, RasterRenderPath,
};
use render_backend::{
    DeviceCandidate, QueueFamilyCapabilities, RenderPathPhase, run_render_path_phase,
};
#[cfg(target_os = "windows")]
use render_backend::{FrameOutcome, RenderBackend, RenderBackendOptions};
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
    VoxelSceneId, VoxelSceneRevision, VoxelValue, VoxelVolumeId, VoxelVolumeMetadata,
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
    measurement: Option<MeasurementConfiguration>,
}

impl DesktopRenderConfiguration {
    fn camera_pose(&self) -> CameraPose {
        match self.scene {
            DesktopSceneSelection::Canonical(_) => self.camera.pose(),
            DesktopSceneSelection::WindingDiagnostic => CameraPose::new(
                [0.5, 0.5, 4.0],
                [0.5, 0.5, 1.0],
                [0.0, 1.0, 0.0],
                60.0,
                0.1,
                10.0,
            ),
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
    Ok((
        DesktopRenderConfiguration {
            scene,
            camera,
            raster_region_extent,
            hold_background_preparation,
            hold_post_upload_candidate,
            inject_raster_upload_failure,
            edit_burst_demo,
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

fn report_winding_diagnostic_configuration(configuration: &DesktopRenderConfiguration) {
    println!(
        "Diagnostic scene: identity=raster-front-face-winding dimensions=1x1x2 origin=0,0,0 voxel_size=1 materials=winding-diagnostic-far-blue,winding-diagnostic-near-warm occupied=2 exposed_faces=10"
    );
    let camera = configuration.camera_pose();
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
}

fn report_canonical_configuration(
    metadata: &CanonicalSceneMetadata,
    configuration: &DesktopRenderConfiguration,
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

fn report_render_configuration(configuration: &DesktopRenderConfiguration) -> Result<(), String> {
    match configuration.scene {
        DesktopSceneSelection::WindingDiagnostic => {
            report_winding_diagnostic_configuration(configuration);
        }
        DesktopSceneSelection::Canonical(scale) => {
            let canonical = generate_canonical_scene(scale).map_err(|error| {
                format!("could not generate the canonical Voxel Scene: {error}")
            })?;
            report_canonical_configuration(canonical.metadata(), configuration);
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
struct EditBurstPlan {
    commands: VecDeque<VoxelEditCommand>,
    expected_final_revision: VoxelSceneRevision,
    input_owner: Option<EditBurstInputOwner>,
}

#[cfg(target_os = "windows")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EditBurstInputOwner {
    SpaceKeypress,
}

#[cfg(target_os = "windows")]
impl EditBurstPlan {
    fn claim_space_keypress(&mut self) -> Result<(), String> {
        if self.input_owner.is_some() {
            return Err("the edit burst was already claimed by a Space keypress".to_owned());
        }
        self.input_owner = Some(EditBurstInputOwner::SpaceKeypress);
        Ok(())
    }

    fn take_next_owned_command(&mut self) -> Result<VoxelEditCommand, String> {
        if self.input_owner != Some(EditBurstInputOwner::SpaceKeypress) {
            return Err("the edit burst commands are not owned by a Space keypress".to_owned());
        }
        self.commands
            .pop_front()
            .ok_or_else(|| "the edit burst has no remaining command".to_owned())
    }
}

#[cfg(target_os = "windows")]
enum EditBurstStage {
    AwaitingKey(EditBurstPlan),
    WaitingForCpuBarrier(EditBurstPlan),
    WaitingForSecondRequirement(EditBurstPlan),
    CpuBarrierHeld(EditBurstPlan),
    WaitingForCpuCancellation(EditBurstPlan),
    WaitingForPostUploadCandidate(EditBurstPlan),
    PostUploadCandidateHeld(EditBurstPlan),
    WaitingForPostUploadCandidateAfterLifecycle(EditBurstPlan),
    WaitingForFinalRequirement(EditBurstPlan),
    PostUploadBarrierHeld(EditBurstPlan),
    WaitingForCandidateRejection(EditBurstPlan),
    WaitingForFinalVisibility(EditBurstPlan),
    Complete,
}

#[cfg(target_os = "windows")]
impl EditBurstStage {
    fn overlay_label(&self) -> &'static str {
        match self {
            Self::AwaitingKey(_) => "awaiting-key",
            Self::WaitingForCpuBarrier(_) => "waiting-cpu-barrier",
            Self::WaitingForSecondRequirement(_) => "second-requirement",
            Self::CpuBarrierHeld(_) => "cpu-barrier-held",
            Self::WaitingForCpuCancellation(_) => "cpu-cancellation",
            Self::WaitingForPostUploadCandidate(_) => "post-upload-candidate",
            Self::PostUploadCandidateHeld(_) => "post-upload-candidate-held",
            Self::WaitingForPostUploadCandidateAfterLifecycle(_) => {
                "post-upload-candidate-after-lifecycle"
            }
            Self::WaitingForFinalRequirement(_) => "final-requirement",
            Self::PostUploadBarrierHeld(_) => "post-upload-barrier-held",
            Self::WaitingForCandidateRejection(_) => "candidate-rejection",
            Self::WaitingForFinalVisibility(_) => "final-visibility",
            Self::Complete => "complete",
        }
    }

    fn waits_for_external_event(&self) -> bool {
        matches!(
            self,
            Self::AwaitingKey(_)
                | Self::CpuBarrierHeld(_)
                | Self::PostUploadCandidateHeld(_)
                | Self::PostUploadBarrierHeld(_)
                | Self::Complete
        )
    }
}

#[cfg(target_os = "windows")]
fn format_convergence_overlay(
    stage: &str,
    status: RasterConvergenceStatus,
    camera: &str,
) -> String {
    format!(
        "EditBurst={stage} Required={} Visible={} Affected={} Unaffected={} Camera={camera}",
        status.required_revision,
        status.visible_revision,
        status.affected_region_count,
        status.unaffected_region_count
    )
}

#[cfg(target_os = "windows")]
fn should_start_edit_burst(
    edit_burst_demo: bool,
    awaiting_key: bool,
    state: ElementState,
    repeat: bool,
    key: &Key,
) -> bool {
    edit_burst_demo
        && awaiting_key
        && state == ElementState::Pressed
        && !repeat
        && matches!(key, Key::Named(NamedKey::Space))
}

#[cfg(target_os = "windows")]
fn voxel_value_identity(value: &VoxelValue) -> String {
    match value {
        VoxelValue::Empty => "empty".to_owned(),
        VoxelValue::Occupied(identity) => format!("occupied:{identity:?}"),
    }
}

#[cfg(target_os = "windows")]
fn fixed_edit_burst(
    view: &voxel_frontend::VoxelSceneView,
    raster_region_extent: u32,
) -> Result<EditBurstPlan, String> {
    let volume_identity = VoxelVolumeId::new("canonical-volume");
    let coordinates = [
        VoxelCoordinate::new(0, 0, 0),
        VoxelCoordinate::new(40, 0, 0),
        VoxelCoordinate::new(80, 0, 0),
    ];
    let mut commands = VecDeque::new();
    for (index, coordinate) in coordinates.into_iter().enumerate() {
        let samples = view
            .read_region(
                &volume_identity,
                VoxelRegion::new(coordinate, VoxelExtent::new(1, 1, 1)),
            )
            .map_err(|error| error.to_string())?;
        let old_value = samples
            .first()
            .ok_or_else(|| format!("edit burst coordinate {coordinate:?} has no Voxel Sample"))?
            .value()
            .clone();
        let requested_value = match &old_value {
            VoxelValue::Empty => VoxelValue::Occupied(VoxelMaterialId::new("canonical-warm")),
            VoxelValue::Occupied(_) => VoxelValue::Empty,
        };
        println!(
            "Edit burst command: order={} volume={volume_identity:?} coordinate={coordinate:?} old={} requested={}",
            index + 1,
            voxel_value_identity(&old_value),
            voxel_value_identity(&requested_value)
        );
        commands.push_back(VoxelEditCommand::new(
            volume_identity.clone(),
            coordinate,
            requested_value,
        ));
    }
    let expected_final_revision =
        (0..commands.len()).try_fold(view.revision(), |revision, _| {
            revision.checked_successor().ok_or_else(|| {
                "the checked expected final Voxel Scene Revision overflowed".to_owned()
            })
        })?;
    println!(
        "Edit burst inputs: scene={:?} generator=voxel-nexus-canonical-dense generator_version=1 initial_revision={} raster_region_extent={raster_region_extent}x{raster_region_extent}x{raster_region_extent} camera=overview installed_revision={} installed_complete=true expected_final_revision={expected_final_revision}",
        view.scene_id(),
        view.revision(),
        view.revision()
    );
    Ok(EditBurstPlan {
        commands,
        expected_final_revision,
        input_owner: None,
    })
}

#[cfg(target_os = "windows")]
struct MeasurementSession {
    mode: MeasurementMode,
    output: BufWriter<File>,
    publication_at: Option<Instant>,
    derivation_at: Option<Instant>,
    installation_at: Option<Instant>,
    steady_frames: Option<SteadyFrameCollection>,
}

#[cfg(target_os = "windows")]
struct CpuFrameMeasurement {
    sequence: u64,
    started_at: Instant,
    milliseconds: f64,
}

#[cfg(target_os = "windows")]
struct SteadyFrameCollection {
    warmup_ends_at: Instant,
    collection_ends_at: Instant,
    recorded_frame_count: u64,
    first_submitted_sequence: Option<u64>,
    pending_cpu_frames: VecDeque<CpuFrameMeasurement>,
}

#[cfg(target_os = "windows")]
impl SteadyFrameCollection {
    fn new(matching_presentation_at: Instant) -> Self {
        let warmup_ends_at = matching_presentation_at + Duration::from_secs(5);
        Self {
            warmup_ends_at,
            collection_ends_at: warmup_ends_at + Duration::from_secs(30),
            recorded_frame_count: 0,
            first_submitted_sequence: None,
            pending_cpu_frames: VecDeque::new(),
        }
    }

    fn submit(&mut self, frame: CpuFrameMeasurement) {
        self.first_submitted_sequence.get_or_insert(frame.sequence);
        self.pending_cpu_frames.push_back(frame);
    }

    fn complete(
        &mut self,
        gpu_observation: render_backend::FrameObservation,
    ) -> Result<Option<MeasurementEvent>, String> {
        let cpu_frame_position = self
            .pending_cpu_frames
            .iter()
            .position(|frame| frame.sequence == gpu_observation.sequence);
        let Some(cpu_frame_position) = cpu_frame_position else {
            if self
                .first_submitted_sequence
                .is_none_or(|first_sequence| gpu_observation.sequence < first_sequence)
            {
                return Ok(None);
            }
            return Err(format!(
                "GPU observation sequence {} has no matching CPU frame",
                gpu_observation.sequence
            ));
        };
        let cpu_frame = self
            .pending_cpu_frames
            .remove(cpu_frame_position)
            .ok_or_else(|| "matching CPU frame disappeared before measurement".to_owned())?;
        if cpu_frame.started_at < self.warmup_ends_at
            || cpu_frame.started_at >= self.collection_ends_at
        {
            return Ok(None);
        }
        self.recorded_frame_count = self
            .recorded_frame_count
            .checked_add(1)
            .ok_or_else(|| "steady frame count overflowed".to_owned())?;
        Ok(Some(MeasurementEvent::SteadyFrame {
            sequence: gpu_observation.sequence,
            cpu_frame_milliseconds: cpu_frame.milliseconds,
            gpu_frame_milliseconds: gpu_observation.gpu_frame_milliseconds,
        }))
    }

    fn collection_has_ended(&self, now: Instant) -> bool {
        now >= self.collection_ends_at
    }
}

#[cfg(target_os = "windows")]
impl MeasurementSession {
    fn new(configuration: &MeasurementConfiguration) -> Result<Self, String> {
        let output = File::create(&configuration.output).map_err(|error| {
            format!(
                "could not create measurement output {}: {error}",
                configuration.output.display()
            )
        })?;
        Ok(Self {
            mode: configuration.mode,
            output: BufWriter::new(output),
            publication_at: None,
            derivation_at: None,
            installation_at: None,
            steady_frames: None,
        })
    }

    fn elapsed_milliseconds(&self, at: Instant) -> Result<f64, String> {
        self.publication_at
            .map(|publication_at| at.duration_since(publication_at).as_secs_f64() * 1_000.0)
            .ok_or_else(|| "measurement began before Voxel Scene Revision publication".to_owned())
    }

    fn record(&mut self, event: MeasurementEvent) -> Result<(), String> {
        let line = event
            .to_json_line()
            .map_err(|error| format!("could not serialize measurement event: {error}"))?;
        writeln!(self.output, "{line}")
            .and_then(|()| self.output.flush())
            .map_err(|error| format!("could not write measurement event: {error}"))
    }

    fn begin_steady_frames(&mut self, matching_presentation_at: Instant) {
        self.steady_frames = Some(SteadyFrameCollection::new(matching_presentation_at));
    }

    fn submit_cpu_frame(&mut self, frame: CpuFrameMeasurement) {
        if let Some(steady_frames) = &mut self.steady_frames {
            steady_frames.submit(frame);
        }
    }

    fn complete_gpu_frame(
        &mut self,
        observation: render_backend::FrameObservation,
    ) -> Result<(), String> {
        let Some(steady_frames) = &mut self.steady_frames else {
            return Ok(());
        };
        let event = steady_frames.complete(observation)?;
        if let Some(event) = event {
            self.record(event)?;
        }
        Ok(())
    }

    fn steady_collection_has_ended(&self, now: Instant) -> bool {
        self.steady_frames
            .as_ref()
            .is_some_and(|steady_frames| steady_frames.collection_has_ended(now))
    }

    fn recorded_steady_frame_count(&self) -> u64 {
        self.steady_frames
            .as_ref()
            .map(|steady_frames| steady_frames.recorded_frame_count)
            .unwrap_or(0)
    }
}

#[cfg(target_os = "windows")]
struct DesktopApplication {
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
    camera_controller: Option<RasterCameraController>,
    published_revision: Option<VoxelSceneRevision>,
    first_matching_frame_presented: bool,
    pending_camera_report: Option<String>,
    camera_move_step: Option<u32>,
    preparation_is_paused: bool,
    drawable_extent: ash::vk::Extent2D,
    measurement: Option<MeasurementSession>,
    occupied_voxels: u64,
    frontend: Option<VoxelFrontend>,
    lifecycle_controller: Option<RasterLifecycleController>,
    lifecycle_edit: Option<VoxelEditCommand>,
    post_upload_hold_reported: bool,
    last_presented_camera: Option<String>,
    edit_burst_stage: Option<EditBurstStage>,
    raster_region_count: usize,
    last_overlay_report: Option<String>,
    edit_burst_started_at: Option<Instant>,
}

#[cfg(target_os = "windows")]
const PRESENTATION_RETRY_DELAY: Duration = Duration::from_millis(100);
#[cfg(target_os = "windows")]
const STEADY_MEASUREMENT_EXTENT: ash::vk::Extent2D = ash::vk::Extent2D {
    width: 1920,
    height: 1080,
};

#[cfg(target_os = "windows")]
impl DesktopApplication {
    fn new(
        render_configuration: DesktopRenderConfiguration,
        event_proxy: EventLoopProxy<DesktopEvent>,
    ) -> Result<Self, String> {
        let measurement = render_configuration
            .measurement
            .as_ref()
            .map(MeasurementSession::new)
            .transpose()?;
        Ok(Self {
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
            camera_controller: None,
            published_revision: None,
            first_matching_frame_presented: false,
            pending_camera_report: None,
            camera_move_step: None,
            preparation_is_paused: false,
            drawable_extent: ash::vk::Extent2D::default(),
            measurement,
            occupied_voxels: 0,
            frontend: None,
            lifecycle_controller: None,
            lifecycle_edit: None,
            post_upload_hold_reported: false,
            last_presented_camera: None,
            edit_burst_stage: None,
            raster_region_count: 0,
            last_overlay_report: None,
            edit_burst_started_at: None,
        })
    }

    fn set_status(&self, status: &str) {
        if let Some(window) = &self.window {
            window.set_title(&format!("Voxel Nexus Vulkan Demo | {status}"));
        }
    }

    fn set_convergence_overlay(&mut self, status: RasterConvergenceStatus) -> Result<(), String> {
        let stage = self
            .edit_burst_stage
            .as_ref()
            .map(EditBurstStage::overlay_label)
            .unwrap_or("inactive");
        let camera = self.last_presented_camera.as_deref().unwrap_or("initial");
        let report = format_convergence_overlay(stage, status, camera);
        if self.last_overlay_report.as_deref() != Some(&report) {
            println!("Edit burst overlay: {report}");
            self.last_overlay_report = Some(report.clone());
        }
        self.text_overlay
            .as_ref()
            .ok_or_else(|| "the in-client convergence overlay is unavailable".to_owned())?
            .set_text(&report)?;
        self.set_status(&report);
        Ok(())
    }

    fn publish_next_burst_command(&self, plan: &mut EditBurstPlan) -> Result<(), String> {
        let command = plan.take_next_owned_command()?;
        let outcome = self
            .frontend
            .as_ref()
            .ok_or_else(|| "the edit burst Voxel Frontend is unavailable".to_owned())?
            .edit(command)
            .map_err(|error| error.to_string())?;
        let revision = match &outcome {
            VoxelEditOutcome::Changed { view, .. } => view.revision(),
            VoxelEditOutcome::Unchanged(_) => {
                return Err("a fixed edit burst command did not change its Voxel Value".to_owned());
            }
        };
        self.lifecycle_controller
            .as_ref()
            .ok_or_else(|| "the edit burst lifecycle controller is unavailable".to_owned())?
            .submit(outcome)
            .map_err(|error| error.to_string())?;
        println!("Edit burst command published: revision={revision}");
        Ok(())
    }

    fn start_edit_burst(&mut self, event_loop: &ActiveEventLoop) {
        let Some(EditBurstStage::AwaitingKey(mut plan)) = self.edit_burst_stage.take() else {
            self.fail(
                event_loop,
                "the fixed edit burst is not awaiting its keypress",
            );
            return;
        };
        if let Err(error) = plan.claim_space_keypress() {
            self.fail(event_loop, error);
            return;
        }
        self.edit_burst_started_at = Some(Instant::now());
        if let Err(error) = self.publish_next_burst_command(&mut plan) {
            self.fail(event_loop, error);
            return;
        }
        println!("Edit burst started by one keypress");
        self.edit_burst_stage = Some(EditBurstStage::WaitingForCpuBarrier(plan));
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn advance_edit_burst(&mut self, event_loop: &ActiveEventLoop) {
        let Some(stage) = self.edit_burst_stage.take() else {
            return;
        };
        let Some(controller) = self.lifecycle_controller.clone() else {
            self.edit_burst_stage = Some(stage);
            return;
        };
        let next_stage = match stage {
            EditBurstStage::AwaitingKey(plan) => EditBurstStage::AwaitingKey(plan),
            EditBurstStage::WaitingForCpuBarrier(mut plan) => {
                let observation = match controller.cpu_barrier_observation() {
                    Ok(observation) => observation,
                    Err(error) => {
                        self.fail(event_loop, error);
                        return;
                    }
                };
                if observation.is_some_and(|observation| observation.reached_revision.is_some()) {
                    println!("Edit burst CPU barrier reached: scheduled_regions=1");
                    if let Err(error) = self.publish_next_burst_command(&mut plan) {
                        self.fail(event_loop, error);
                        return;
                    }
                    EditBurstStage::WaitingForSecondRequirement(plan)
                } else {
                    EditBurstStage::WaitingForCpuBarrier(plan)
                }
            }
            EditBurstStage::WaitingForSecondRequirement(plan) => {
                let status = match controller.convergence_status() {
                    Ok(status) => status,
                    Err(error) => {
                        self.fail(event_loop, error);
                        return;
                    }
                };
                if status.is_some_and(|status| {
                    status.required_revision.checked_successor()
                        == Some(plan.expected_final_revision)
                }) {
                    println!("Edit burst CPU barrier held with newer requirement installed");
                    EditBurstStage::CpuBarrierHeld(plan)
                } else {
                    EditBurstStage::WaitingForSecondRequirement(plan)
                }
            }
            EditBurstStage::CpuBarrierHeld(plan) => EditBurstStage::CpuBarrierHeld(plan),
            EditBurstStage::WaitingForCpuCancellation(plan) => {
                let observation = match controller.cpu_barrier_observation() {
                    Ok(observation) => observation,
                    Err(error) => {
                        self.fail(event_loop, error);
                        return;
                    }
                };
                if observation.is_some_and(|observation| {
                    observation.finished
                        && observation.cancelled
                        && observation.scheduled_region_count == 1
                }) {
                    println!(
                        "Obsolete CPU generation cancelled: scheduled_regions_before_hold=1 scheduled_regions_total=1"
                    );
                    EditBurstStage::WaitingForPostUploadCandidate(plan)
                } else {
                    EditBurstStage::WaitingForCpuCancellation(plan)
                }
            }
            EditBurstStage::WaitingForPostUploadCandidate(plan) => {
                let revision = match controller.post_upload_revision() {
                    Ok(revision) => revision,
                    Err(error) => {
                        self.fail(event_loop, error);
                        return;
                    }
                };
                if revision.is_some_and(|revision| {
                    revision.checked_successor() == Some(plan.expected_final_revision)
                }) {
                    println!("Superseded candidate held after upload: revision={revision:?}");
                    EditBurstStage::PostUploadCandidateHeld(plan)
                } else {
                    EditBurstStage::WaitingForPostUploadCandidate(plan)
                }
            }
            EditBurstStage::PostUploadCandidateHeld(plan) => {
                EditBurstStage::PostUploadCandidateHeld(plan)
            }
            EditBurstStage::WaitingForPostUploadCandidateAfterLifecycle(mut plan) => {
                let revision = match controller.post_upload_revision() {
                    Ok(revision) => revision,
                    Err(error) => {
                        self.fail(event_loop, error);
                        return;
                    }
                };
                if revision.is_some_and(|revision| {
                    revision.checked_successor() == Some(plan.expected_final_revision)
                }) {
                    println!(
                        "Post-upload candidate restored after lifecycle: revision={revision:?}"
                    );
                    if let Err(error) = self.publish_next_burst_command(&mut plan) {
                        self.fail(event_loop, error);
                        return;
                    }
                    EditBurstStage::WaitingForFinalRequirement(plan)
                } else {
                    EditBurstStage::WaitingForPostUploadCandidateAfterLifecycle(plan)
                }
            }
            EditBurstStage::WaitingForFinalRequirement(plan) => {
                let status = match controller.convergence_status() {
                    Ok(status) => status,
                    Err(error) => {
                        self.fail(event_loop, error);
                        return;
                    }
                };
                if status
                    .is_some_and(|status| status.required_revision == plan.expected_final_revision)
                {
                    println!("Post-upload barrier held with newest requirement installed");
                    EditBurstStage::PostUploadBarrierHeld(plan)
                } else {
                    EditBurstStage::WaitingForFinalRequirement(plan)
                }
            }
            EditBurstStage::PostUploadBarrierHeld(plan) => {
                EditBurstStage::PostUploadBarrierHeld(plan)
            }
            EditBurstStage::WaitingForCandidateRejection(plan) => {
                let rejection = match controller.rejected_candidate() {
                    Ok(rejection) => rejection,
                    Err(error) => {
                        self.fail(event_loop, error);
                        return;
                    }
                };
                if let Some(rejection) = rejection {
                    if rejection.revision.checked_successor() != Some(plan.expected_final_revision)
                    {
                        self.fail(
                            event_loop,
                            format!(
                                "unexpected rejected candidate revision {}; expected the predecessor of {}",
                                rejection.revision, plan.expected_final_revision
                            ),
                        );
                        return;
                    }
                    println!(
                        "Superseded candidate rejected at commit: revision={} retired_resources={}",
                        rejection.revision, rejection.retired_resource_count
                    );
                    EditBurstStage::WaitingForFinalVisibility(plan)
                } else {
                    EditBurstStage::WaitingForCandidateRejection(plan)
                }
            }
            EditBurstStage::WaitingForFinalVisibility(plan) => {
                let status = match controller.convergence_status() {
                    Ok(status) => status,
                    Err(error) => {
                        self.fail(event_loop, error);
                        return;
                    }
                };
                if status.is_some_and(|status| {
                    status.required_revision == plan.expected_final_revision
                        && status.visible_revision == plan.expected_final_revision
                }) {
                    let latency_milliseconds = match self.edit_burst_started_at.take() {
                        Some(started_at) => started_at.elapsed().as_secs_f64() * 1_000.0,
                        None => {
                            self.fail(event_loop, "the edit burst keypress timestamp is missing");
                            return;
                        }
                    };
                    let resource_peak = match controller.gpu_resource_peak() {
                        Ok(resource_peak) => resource_peak,
                        Err(error) => {
                            self.fail(event_loop, error);
                            return;
                        }
                    };
                    println!(
                        "Edit burst converged atomically: visible_revision={} expected_final_revision={}",
                        plan.expected_final_revision, plan.expected_final_revision
                    );
                    println!(
                        "Edit burst final-visible measurement: elapsed_ms={latency_milliseconds:.6} peak_live_gpu_bytes={} peak_live_gpu_resources={}",
                        resource_peak.bytes, resource_peak.resources
                    );
                    EditBurstStage::Complete
                } else {
                    EditBurstStage::WaitingForFinalVisibility(plan)
                }
            }
            EditBurstStage::Complete => EditBurstStage::Complete,
        };
        self.edit_burst_stage = Some(next_stage);
    }

    fn release_edit_cpu_barrier(&mut self, event_loop: &ActiveEventLoop) {
        let Some(EditBurstStage::CpuBarrierHeld(plan)) = self.edit_burst_stage.take() else {
            self.fail(event_loop, "the edit burst CPU barrier is not held");
            return;
        };
        let result = self
            .lifecycle_controller
            .as_ref()
            .ok_or_else(|| "the edit burst lifecycle controller is unavailable".to_owned())
            .and_then(|controller| {
                controller
                    .release_cpu_barrier()
                    .map_err(|error| error.to_string())
            });
        if let Err(error) = result {
            self.fail(event_loop, error);
            return;
        }
        println!("Edit burst CPU barrier released after newer requirement");
        self.edit_burst_stage = Some(EditBurstStage::WaitingForCpuCancellation(plan));
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn release_edit_post_upload_barrier(&mut self, event_loop: &ActiveEventLoop) {
        let Some(EditBurstStage::PostUploadBarrierHeld(plan)) = self.edit_burst_stage.take() else {
            self.fail(event_loop, "the edit burst post-upload barrier is not held");
            return;
        };
        let result = self
            .lifecycle_controller
            .as_ref()
            .ok_or_else(|| "the edit burst lifecycle controller is unavailable".to_owned())
            .and_then(|controller| {
                controller
                    .release_post_upload()
                    .map_err(|error| error.to_string())
            });
        if let Err(error) = result {
            self.fail(event_loop, error);
            return;
        }
        println!("Post-upload barrier released for newest requirement");
        self.edit_burst_stage = Some(EditBurstStage::WaitingForCandidateRejection(plan));
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn release_edit_post_upload_lifecycle_barrier(&mut self, event_loop: &ActiveEventLoop) {
        let Some(EditBurstStage::PostUploadCandidateHeld(plan)) = self.edit_burst_stage.take()
        else {
            self.fail(
                event_loop,
                "the edit burst post-upload lifecycle barrier is not held",
            );
            return;
        };
        println!("Post-upload lifecycle barrier released; waiting for restored candidate");
        self.edit_burst_stage = Some(EditBurstStage::WaitingForPostUploadCandidateAfterLifecycle(
            plan,
        ));
        if let Some(window) = &self.window {
            window.request_redraw();
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

    fn set_drawable_extent(&mut self, drawable_extent: ash::vk::Extent2D) -> Result<(), String> {
        self.drawable_extent = drawable_extent;
        if let Some(backend) = &mut self.backend {
            backend.set_drawable_extent(drawable_extent);
        }
        self.presentation_retry_at = None;
        if self.preparation_is_paused {
            if drawable_extent.width == 0 || drawable_extent.height == 0 {
                println!("Desktop lifecycle serviced while preparation paused: suspended");
                self.set_status("preparation-paused suspended");
            } else {
                println!(
                    "Desktop lifecycle serviced while preparation paused: resize={}x{}",
                    drawable_extent.width, drawable_extent.height
                );
            }
        }
        if self.render_configuration.hold_post_upload_candidate {
            if drawable_extent.width == 0 || drawable_extent.height == 0 {
                println!("Desktop lifecycle serviced while post-upload candidate held: suspended");
                self.set_status("post-upload-held suspended");
            } else {
                println!(
                    "Desktop lifecycle serviced while post-upload candidate held: resize={}x{}",
                    drawable_extent.width, drawable_extent.height
                );
                self.set_status(&format!(
                    "post-upload-held lifecycle-responsive {}x{}",
                    drawable_extent.width, drawable_extent.height
                ));
            }
        }
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

    fn complete_preparation(&mut self, event_loop: &ActiveEventLoop) {
        let Some(mut preparation) = self.preparation.take() else {
            self.fail(
                event_loop,
                "background preparation completed more than once",
            );
            return;
        };
        let artifact = match preparation.try_complete() {
            Ok(Some(artifact)) => artifact,
            Ok(None) => {
                self.preparation = Some(preparation);
                self.fail(
                    event_loop,
                    "background preparation signaled completion without an artifact",
                );
                return;
            }
            Err(error) => {
                self.fail(event_loop, error);
                return;
            }
        };
        self.raster_region_count = artifact.regions().len();
        if let Some(measurement) = &mut self.measurement {
            let derived_at = Instant::now();
            let source_revision = match measurement_revision(artifact.source_revision()) {
                Ok(source_revision) => source_revision,
                Err(error) => {
                    self.fail(event_loop, error);
                    return;
                }
            };
            let resource_counts = (|| {
                let exposed_quads = u64::try_from(artifact.semantic_faces().len())
                    .map_err(|_| "exposed quad count cannot be represented".to_owned())?;
                let vertices = u64::try_from(artifact.vertices().len())
                    .map_err(|_| "vertex count cannot be represented".to_owned())?;
                let indices = u64::try_from(artifact.indices().len())
                    .map_err(|_| "index count cannot be represented".to_owned())?;
                let vertex_bytes = u64::try_from(artifact.vertex_byte_size())
                    .map_err(|_| "vertex byte count cannot be represented".to_owned())?;
                let index_bytes = u64::try_from(artifact.index_byte_size())
                    .map_err(|_| "index byte count cannot be represented".to_owned())?;
                let geometry_bytes = vertex_bytes
                    .checked_add(index_bytes)
                    .ok_or_else(|| "raster artifact byte count overflowed".to_owned())?;
                Ok::<_, String>(ResourceCounts {
                    occupied_voxels: self.occupied_voxels,
                    exposed_quads,
                    vertices,
                    indices,
                    draw_calls: u64::from(indices > 0),
                    cpu_artifact_bytes: geometry_bytes,
                    gpu_buffer_bytes: geometry_bytes,
                })
            })();
            let elapsed_milliseconds = measurement.elapsed_milliseconds(derived_at);
            match (resource_counts, elapsed_milliseconds) {
                (Ok(resources), Ok(elapsed_milliseconds)) => {
                    if let Err(error) = measurement.record(MeasurementEvent::ArtifactDerived {
                        source_revision,
                        elapsed_milliseconds,
                        resources,
                    }) {
                        self.fail(event_loop, error);
                        return;
                    }
                    measurement.derivation_at = Some(derived_at);
                }
                (Err(error), _) | (_, Err(error)) => {
                    self.fail(event_loop, error);
                    return;
                }
            }
        }
        let Some(installer) = &self.artifact_installer else {
            self.fail(event_loop, "the raster artifact installer is unavailable");
            return;
        };
        if let Err(error) = installer.publish_complete(artifact) {
            self.fail(event_loop, error);
            return;
        }
        let Some(backend) = &mut self.backend else {
            self.fail(
                event_loop,
                "the Render Backend is unavailable for artifact installation",
            );
            return;
        };
        if let Err(error) = backend.refresh_render_path() {
            let installed_revision = installer
                .installed_source_revision()
                .map(|revision| format!("{revision:?}"))
                .unwrap_or_else(|status_error| format!("unavailable ({status_error})"));
            self.fail(
                event_loop,
                format!(
                    "{error}; installed raster artifact revision after failure: {installed_revision}"
                ),
            );
            return;
        }
        let installed_revision = match installer.installed_source_revision() {
            Ok(Some(revision)) => revision,
            Ok(None) => {
                self.fail(
                    event_loop,
                    "the Render Path refresh did not install a complete raster artifact",
                );
                return;
            }
            Err(error) => {
                self.fail(event_loop, error);
                return;
            }
        };
        if Some(installed_revision) != self.published_revision {
            self.fail(
                event_loop,
                format!(
                    "installed raster artifact revision {installed_revision} does not match the published Voxel Scene Revision"
                ),
            );
            return;
        }
        if let Some(measurement) = &mut self.measurement {
            let installation_at = Instant::now();
            let elapsed_milliseconds = match measurement.elapsed_milliseconds(installation_at) {
                Ok(elapsed_milliseconds) => elapsed_milliseconds,
                Err(error) => {
                    self.fail(event_loop, error);
                    return;
                }
            };
            if let Err(error) = measurement.record(MeasurementEvent::ArtifactInstalled {
                source_revision: match measurement_revision(installed_revision) {
                    Ok(source_revision) => source_revision,
                    Err(error) => {
                        self.fail(event_loop, error);
                        return;
                    }
                },
                elapsed_milliseconds,
            }) {
                self.fail(event_loop, error);
                return;
            }
            measurement.installation_at = Some(installation_at);
        }
        println!("Raster artifact installed: revision={installed_revision} count=1");
        if self.render_configuration.edit_burst_demo {
            let plan = match self
                .frontend
                .as_ref()
                .ok_or_else(|| "the edit burst Voxel Frontend is unavailable".to_owned())
                .and_then(|frontend| frontend.scene_view().map_err(|error| error.to_string()))
                .and_then(|view| {
                    fixed_edit_burst(&view, self.render_configuration.raster_region_extent)
                }) {
                Ok(plan) => plan,
                Err(error) => {
                    self.fail(event_loop, error);
                    return;
                }
            };
            self.edit_burst_stage = Some(EditBurstStage::AwaitingKey(plan));
            if let Err(error) = self.set_convergence_overlay(RasterConvergenceStatus {
                required_revision: installed_revision,
                visible_revision: installed_revision,
                affected_region_count: 0,
                unaffected_region_count: self.raster_region_count,
            }) {
                self.fail(event_loop, error);
                return;
            }
            println!("Edit burst ready: press Space");
        }
        if let Some(command) = self.lifecycle_edit.take() {
            let outcome = match self.frontend.as_ref() {
                Some(frontend) => match frontend.edit(command) {
                    Ok(outcome) => outcome,
                    Err(error) => {
                        self.fail(event_loop, error);
                        return;
                    }
                },
                None => {
                    self.fail(event_loop, "the lifecycle Voxel Frontend is unavailable");
                    return;
                }
            };
            if let Some(controller) = &self.lifecycle_controller
                && let Err(error) = controller.submit(outcome)
            {
                self.fail(event_loop, error);
                return;
            }
        }
        if !self.render_configuration.edit_burst_demo {
            self.set_status(&format!("artifact-ready revision {installed_revision}"));
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn select_camera(&mut self, event_loop: &ActiveEventLoop, selection: DesktopCameraSelection) {
        let Some(camera_controller) = &self.camera_controller else {
            self.fail(event_loop, "the raster camera controller is unavailable");
            return;
        };
        if let Err(error) = camera_controller.set_pose(selection.pose()) {
            self.fail(event_loop, error);
            return;
        }
        self.pending_camera_report = Some(selection.report_identity());
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn advance_camera_move(&mut self, event_loop: &ActiveEventLoop) {
        let Some(step) = self.camera_move_step else {
            return;
        };
        let movement = match overview_to_cavity_camera_move() {
            Ok(movement) => movement,
            Err(error) => {
                self.fail(event_loop, error);
                return;
            }
        };
        if step >= movement.total_steps() {
            self.camera_move_step = None;
            println!(
                "Deterministic camera move completed: steps={}",
                movement.total_steps()
            );
            self.set_status("camera-move-complete");
            return;
        }
        let next_step = match step.checked_add(1) {
            Some(step) => step,
            None => {
                self.fail(event_loop, "the deterministic camera move step overflowed");
                return;
            }
        };
        let pose = match movement.pose_at_step(next_step) {
            Ok(pose) => pose,
            Err(error) => {
                self.fail(event_loop, error);
                return;
            }
        };
        let Some(camera_controller) = &self.camera_controller else {
            self.fail(event_loop, "the raster camera controller is unavailable");
            return;
        };
        if let Err(error) = camera_controller.set_pose(pose) {
            self.fail(event_loop, error);
            return;
        }
        self.camera_move_step = Some(next_step);
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
}

#[cfg(target_os = "windows")]
impl ApplicationHandler<DesktopEvent> for DesktopApplication {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        let mut attributes = WindowAttributes::default().with_title("Voxel Nexus Vulkan Demo");
        if matches!(
            self.render_configuration
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
                self.application_error = Some(format!("could not create the demo window: {error}"));
                event_loop.exit();
                return;
            }
        };
        let text_overlay = if self.render_configuration.edit_burst_demo {
            match WindowsTextOverlay::new(&window) {
                Ok(overlay) => Some(overlay),
                Err(error) => {
                    self.application_error = Some(error);
                    event_loop.exit();
                    return;
                }
            }
        } else {
            None
        };
        if matches!(
            self.render_configuration
                .measurement
                .as_ref()
                .map(|measurement| measurement.mode),
            Some(MeasurementMode::SteadyState)
        ) && let Err(error) = set_measurement_extent(&window, STEADY_MEASUREMENT_EXTENT)
        {
            self.application_error = Some(error);
            event_loop.exit();
            return;
        }
        let adapter = WindowsPresentationAdapter::new(&window);
        let drawable_size = window.inner_size();
        let initial_drawable_extent = ash::vk::Extent2D {
            width: drawable_size.width,
            height: drawable_size.height,
        };
        self.drawable_extent = initial_drawable_extent;
        let (scene, occupied_voxels) = match self.render_configuration.scene {
            DesktopSceneSelection::WindingDiagnostic => {
                let (scene, _) = winding_diagnostic_scene();
                report_winding_diagnostic_configuration(&self.render_configuration);
                (scene, 2)
            }
            DesktopSceneSelection::Canonical(scale) => {
                let canonical = match generate_canonical_scene(scale) {
                    Ok(canonical) => canonical,
                    Err(error) => {
                        self.application_error = Some(format!(
                            "could not generate the canonical Voxel Scene: {error}"
                        ));
                        event_loop.exit();
                        return;
                    }
                };
                report_canonical_configuration(canonical.metadata(), &self.render_configuration);
                let occupied_voxels = canonical.metadata().occupied_count();
                (canonical.into_scene(), occupied_voxels)
            }
        };
        self.occupied_voxels = occupied_voxels;
        let frontend = VoxelFrontend::new();
        let view = match frontend.publish(scene) {
            Ok(view) => view,
            Err(error) => {
                self.application_error = Some(format!(
                    "could not publish the configured Voxel Scene: {error}"
                ));
                event_loop.exit();
                return;
            }
        };
        if self.render_configuration.hold_post_upload_candidate {
            let volume_identity = VoxelVolumeId::new("canonical-volume");
            let coordinate = VoxelCoordinate::new(0, 0, 0);
            let sample = match view.read_region(
                &volume_identity,
                VoxelRegion::new(coordinate, VoxelExtent::new(1, 1, 1)),
            ) {
                Ok(samples) => match samples.first() {
                    Some(sample) => sample.value().clone(),
                    None => {
                        self.application_error = Some(
                            "the post-upload lifecycle edit coordinate had no Voxel Sample"
                                .to_owned(),
                        );
                        event_loop.exit();
                        return;
                    }
                },
                Err(error) => {
                    self.application_error = Some(error.to_string());
                    event_loop.exit();
                    return;
                }
            };
            let requested = match sample {
                VoxelValue::Empty => VoxelValue::Occupied(VoxelMaterialId::new("canonical-warm")),
                VoxelValue::Occupied(_) => VoxelValue::Empty,
            };
            self.lifecycle_edit = Some(VoxelEditCommand::new(
                volume_identity,
                coordinate,
                requested,
            ));
        }
        self.frontend = Some(frontend);
        let published_revision = view.revision();
        if let Some(measurement) = &mut self.measurement {
            let publication_at = Instant::now();
            measurement.publication_at = Some(publication_at);
            if let Err(error) = measurement.record(MeasurementEvent::SceneRevisionPublished {
                source_revision: match measurement_revision(published_revision) {
                    Ok(source_revision) => source_revision,
                    Err(error) => {
                        self.application_error = Some(error);
                        event_loop.exit();
                        return;
                    }
                },
                elapsed_milliseconds: 0.0,
            }) {
                self.application_error = Some(error);
                event_loop.exit();
                return;
            }
        }
        let (mut render_path, artifact_installer, camera_controller) =
            RasterRenderPath::awaiting_artifact_with_camera_control(
                self.render_configuration.camera_pose(),
                published_revision,
            );
        if self.render_configuration.hold_post_upload_candidate
            || self.render_configuration.hold_background_preparation
            || self.render_configuration.edit_burst_demo
        {
            self.lifecycle_controller = Some(render_path.enable_lifecycle_control(
                self.render_configuration.hold_post_upload_candidate
                    || self.render_configuration.edit_burst_demo,
            ));
        }
        if self.render_configuration.edit_burst_demo
            && let Some(controller) = &self.lifecycle_controller
            && let Err(error) = controller.hold_next_cpu_generation_after_regions(1)
        {
            self.application_error = Some(error.to_string());
            event_loop.exit();
            return;
        }
        if self.render_configuration.inject_raster_upload_failure
            && let Err(error) = artifact_installer.inject_next_upload_failure()
        {
            self.application_error = Some(error.to_string());
            event_loop.exit();
            return;
        }
        let options = if matches!(
            self.render_configuration
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
        let backend = match RenderBackend::initialize_with_options(
            c"Voxel Nexus Desktop Demo",
            &adapter,
            initial_drawable_extent,
            render_path,
            options,
        ) {
            Ok(backend) => backend,
            Err(error) => {
                self.application_error = Some(error.to_string());
                event_loop.exit();
                return;
            }
        };
        let presentation_extent = match backend.presentation_extent() {
            Some(extent) => extent,
            None => {
                self.application_error =
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
            self.application_error = Some(format!(
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
        self.backend = Some(backend);
        self.text_overlay = text_overlay;
        self.window = Some(window);
        self.artifact_installer = Some(artifact_installer);
        self.camera_controller = Some(camera_controller);
        self.published_revision = Some(published_revision);
        let (barrier, preparation_release) =
            if self.render_configuration.hold_background_preparation {
                let (barrier, release) = RasterPreparationBarrier::held();
                (Some(barrier), Some(release))
            } else {
                (None, None)
            };
        let event_proxy = self.event_proxy.clone();
        let preparation = match RasterArtifactPreparation::start_regions(
            view,
            VoxelExtent::new(
                self.render_configuration.raster_region_extent,
                self.render_configuration.raster_region_extent,
                self.render_configuration.raster_region_extent,
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
                self.application_error = Some(error.to_string());
                event_loop.exit();
                return;
            }
        };
        self.preparation = Some(preparation);
        self.preparation_release = preparation_release;
        self.set_status(&format!("preparing revision {published_revision}"));
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
                if let Some(release) = self.preparation_release.take()
                    && let Err(error) = release.release()
                {
                    self.record_close_error(error);
                }
                if let Some(mut preparation) = self.preparation.take()
                    && let Err(error) = preparation.cancel_and_join()
                {
                    self.record_close_error(error);
                }
                if self.render_configuration.hold_post_upload_candidate {
                    match self
                        .lifecycle_controller
                        .as_ref()
                        .map(RasterLifecycleController::post_upload_revision)
                        .transpose()
                    {
                        Ok(Some(Some(revision))) => {
                            println!(
                                "Closing with post-upload hidden raster candidate: revision={revision}"
                            );
                        }
                        Ok(_) => self.record_close_error(
                            "desktop close did not retain the required post-upload hidden candidate",
                        ),
                        Err(error) => self.record_close_error(error),
                    }
                }
                if let Some(backend) = &mut self.backend
                    && let Err(error) = backend.shutdown()
                {
                    self.record_close_error(error);
                }
                if let Some(controller) = &self.lifecycle_controller {
                    match controller.shutdown_owned_resource_count() {
                        Ok(Some(0)) => {
                            println!("Render Path-owned raster resources after shutdown: 0");
                        }
                        Ok(Some(count)) => self.record_close_error(format!(
                            "Render Path shutdown retained {count} owned raster resources"
                        )),
                        Ok(None) => self.record_close_error(
                            "Render Path shutdown did not report owned raster resource disposal",
                        ),
                        Err(error) => self.record_close_error(error),
                    }
                }
                event_loop.exit();
            }
            WindowEvent::Resized(drawable_size) => {
                let steady_measurement = matches!(
                    self.render_configuration
                        .measurement
                        .as_ref()
                        .map(|measurement| measurement.mode),
                    Some(MeasurementMode::SteadyState)
                );
                let drawable_size = if steady_measurement
                    && (drawable_size.width != STEADY_MEASUREMENT_EXTENT.width
                        || drawable_size.height != STEADY_MEASUREMENT_EXTENT.height)
                {
                    let Some(window) = &self.window else {
                        self.fail(
                            event_loop,
                            "the steady-state measurement window is unavailable",
                        );
                        return;
                    };
                    if let Err(error) = set_measurement_extent(window, STEADY_MEASUREMENT_EXTENT) {
                        self.fail(event_loop, error);
                        return;
                    }
                    let corrected_size = window.inner_size();
                    if corrected_size.width != STEADY_MEASUREMENT_EXTENT.width
                        || corrected_size.height != STEADY_MEASUREMENT_EXTENT.height
                    {
                        self.fail(
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
                let drawable_extent = if self.drawable_occluded {
                    ash::vk::Extent2D::default()
                } else {
                    ash::vk::Extent2D {
                        width: drawable_size.width,
                        height: drawable_size.height,
                    }
                };
                if let Err(error) = self.set_drawable_extent(drawable_extent) {
                    self.fail(event_loop, error);
                }
            }
            WindowEvent::Occluded(occluded) => {
                if occluded
                    && matches!(
                        self.render_configuration
                            .measurement
                            .as_ref()
                            .map(|measurement| measurement.mode),
                        Some(MeasurementMode::SteadyState)
                    )
                {
                    self.fail(
                        event_loop,
                        "steady-state measurement window became occluded",
                    );
                    return;
                }
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
                if let Err(error) = self.set_drawable_extent(drawable_extent) {
                    self.fail(event_loop, error);
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if should_start_edit_burst(
                    self.render_configuration.edit_burst_demo,
                    matches!(self.edit_burst_stage, Some(EditBurstStage::AwaitingKey(_))),
                    event.state,
                    event.repeat,
                    &event.logical_key,
                ) {
                    self.start_edit_burst(event_loop);
                }
            }
            WindowEvent::RedrawRequested => {
                let frame_started_at = Instant::now();
                let (outcome, gpu_observation, submitted_frame_sequence, presentation_extent) =
                    match &mut self.backend {
                        Some(backend) => match backend.draw_frame() {
                            Ok(outcome) => (
                                outcome,
                                backend.take_frame_observation(),
                                backend.last_submitted_frame_sequence(),
                                backend.presentation_extent(),
                            ),
                            Err(error) => {
                                self.application_error = Some(error.to_string());
                                event_loop.exit();
                                return;
                            }
                        },
                        None => (FrameOutcome::Suspended, None, None, None),
                    };
                if matches!(
                    self.render_configuration
                        .measurement
                        .as_ref()
                        .map(|measurement| measurement.mode),
                    Some(MeasurementMode::SteadyState)
                ) && presentation_extent != Some(STEADY_MEASUREMENT_EXTENT)
                {
                    self.fail(
                        event_loop,
                        "steady-state measurement lost its 1920x1080 presentation extent",
                    );
                    return;
                }
                let cpu_frame_milliseconds = frame_started_at.elapsed().as_secs_f64() * 1_000.0;
                if let Some(measurement) = &mut self.measurement
                    && measurement.mode == MeasurementMode::SteadyState
                    && let Some(sequence) = submitted_frame_sequence
                {
                    measurement.submit_cpu_frame(CpuFrameMeasurement {
                        sequence,
                        started_at: frame_started_at,
                        milliseconds: cpu_frame_milliseconds,
                    });
                }
                if let Some(measurement) = &mut self.measurement
                    && measurement.mode == MeasurementMode::SteadyState
                    && let Some(gpu_observation) = gpu_observation
                    && let Err(error) = measurement.complete_gpu_frame(gpu_observation)
                {
                    self.fail(event_loop, error);
                    return;
                }
                match outcome {
                    FrameOutcome::Presented => {
                        self.presentation_retry_at = None;
                        let installed_revision = match &self.artifact_installer {
                            Some(installer) => match installer.installed_source_revision() {
                                Ok(revision) => revision,
                                Err(error) => {
                                    self.fail(event_loop, error);
                                    return;
                                }
                            },
                            None => None,
                        };
                        if !self.first_matching_frame_presented
                            && installed_revision.is_some()
                            && installed_revision == self.published_revision
                        {
                            let revision = installed_revision.unwrap_or(VoxelSceneRevision::new(0));
                            self.first_matching_frame_presented = true;
                            println!("First matching raster frame presented: revision={revision}");
                            if let Some(measurement) = &mut self.measurement {
                                let presented_at = Instant::now();
                                let elapsed_milliseconds =
                                    match measurement.elapsed_milliseconds(presented_at) {
                                        Ok(elapsed_milliseconds) => elapsed_milliseconds,
                                        Err(error) => {
                                            self.fail(event_loop, error);
                                            return;
                                        }
                                    };
                                if let Err(error) = measurement.record(
                                    MeasurementEvent::MatchingArtifactPresented {
                                        source_revision: match measurement_revision(revision) {
                                            Ok(source_revision) => source_revision,
                                            Err(error) => {
                                                self.fail(event_loop, error);
                                                return;
                                            }
                                        },
                                        elapsed_milliseconds,
                                    },
                                ) {
                                    self.fail(event_loop, error);
                                    return;
                                }
                                match measurement.mode {
                                    MeasurementMode::FirstCorrectFrame => {
                                        if let Some(backend) = &mut self.backend
                                            && let Err(error) = backend.shutdown()
                                        {
                                            self.fail(event_loop, error);
                                            return;
                                        }
                                        event_loop.exit();
                                        return;
                                    }
                                    MeasurementMode::SteadyState => {
                                        measurement.begin_steady_frames(presented_at);
                                    }
                                }
                            }
                        }
                        let now = Instant::now();
                        if let Some(measurement) = &mut self.measurement
                            && measurement.mode == MeasurementMode::SteadyState
                        {
                            if measurement.steady_collection_has_ended(now) {
                                if measurement.recorded_steady_frame_count() == 0 {
                                    self.fail(
                                        event_loop,
                                        "steady-state collection produced no valid GPU timestamp samples",
                                    );
                                    return;
                                }
                                if let Some(backend) = &mut self.backend
                                    && let Err(error) = backend.shutdown()
                                {
                                    self.fail(event_loop, error);
                                    return;
                                }
                                event_loop.exit();
                                return;
                            }
                            if let Some(window) = &self.window {
                                window.request_redraw();
                            }
                        }
                        if let Some(camera) = self.pending_camera_report.take() {
                            println!("Canonical camera presented: camera={camera}");
                            self.set_status(&format!("camera-presented {camera}"));
                            self.last_presented_camera = Some(camera);
                        }
                        if self.preparation_is_paused {
                            self.set_status(&format!(
                                "preparation-paused lifecycle-responsive {}x{}",
                                self.drawable_extent.width, self.drawable_extent.height
                            ));
                        }
                        self.advance_camera_move(event_loop);
                        if let Some(controller) = &self.lifecycle_controller {
                            match controller.post_upload_revision() {
                                Ok(Some(revision)) => {
                                    if !self.post_upload_hold_reported {
                                        println!(
                                            "Post-upload raster candidate held: revision={revision}"
                                        );
                                        let status = match &self.last_presented_camera {
                                            Some(camera) => format!(
                                                "camera-presented {camera} post-upload-held revision {revision}"
                                            ),
                                            None => {
                                                format!("post-upload-held revision {revision}")
                                            }
                                        };
                                        self.set_status(&status);
                                        self.post_upload_hold_reported = true;
                                    }
                                }
                                Ok(None) => {
                                    self.post_upload_hold_reported = false;
                                    if let Some(window) = &self.window {
                                        window.request_redraw();
                                    }
                                }
                                Err(error) => {
                                    self.fail(event_loop, error);
                                }
                            }
                        }
                        self.advance_edit_burst(event_loop);
                        if self.render_configuration.edit_burst_demo
                            && let Some(controller) = &self.lifecycle_controller
                        {
                            match controller.convergence_status() {
                                Ok(Some(status)) => {
                                    if let Err(error) = self.set_convergence_overlay(status) {
                                        self.fail(event_loop, error);
                                        return;
                                    }
                                }
                                Ok(None) => {}
                                Err(error) => {
                                    self.fail(event_loop, error);
                                    return;
                                }
                            }
                        }
                        if self.render_configuration.edit_burst_demo
                            && self
                                .edit_burst_stage
                                .as_ref()
                                .is_some_and(|stage| !stage.waits_for_external_event())
                            && let Some(window) = &self.window
                        {
                            window.request_redraw();
                        }
                    }
                    FrameOutcome::Recreate => {
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

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: DesktopEvent) {
        match event {
            DesktopEvent::Preparation(RasterArtifactPreparationEvent::PausedAtBarrier {
                source_revision,
            }) => {
                self.preparation_is_paused = true;
                println!("Background raster preparation paused: revision={source_revision}");
                self.set_status(&format!("preparation-paused revision {source_revision}"));
            }
            DesktopEvent::Preparation(RasterArtifactPreparationEvent::Completed { .. }) => {
                self.complete_preparation(event_loop);
            }
            DesktopEvent::ReleasePreparation => {
                let Some(release) = self.preparation_release.take() else {
                    self.fail(event_loop, "background preparation is not paused");
                    return;
                };
                if let Err(error) = release.release() {
                    self.fail(event_loop, error);
                    return;
                }
                self.preparation_is_paused = false;
                println!("Background raster preparation released");
                self.set_status("preparation-released");
            }
            DesktopEvent::SelectCamera(pose) => {
                self.select_camera(event_loop, DesktopCameraSelection::Fixed(pose));
            }
            DesktopEvent::StartCameraMove => {
                let movement = match overview_to_cavity_camera_move() {
                    Ok(movement) => movement,
                    Err(error) => {
                        self.fail(event_loop, error);
                        return;
                    }
                };
                let Some(camera_controller) = &self.camera_controller else {
                    self.fail(event_loop, "the raster camera controller is unavailable");
                    return;
                };
                let pose = match movement.pose_at_step(0) {
                    Ok(pose) => pose,
                    Err(error) => {
                        self.fail(event_loop, error);
                        return;
                    }
                };
                if let Err(error) = camera_controller.set_pose(pose) {
                    self.fail(event_loop, error);
                    return;
                }
                self.camera_move_step = Some(0);
                println!(
                    "Deterministic camera move started: steps={}",
                    movement.total_steps()
                );
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            DesktopEvent::ReleaseEditCpuBarrier => self.release_edit_cpu_barrier(event_loop),
            DesktopEvent::ReleaseEditPostUploadLifecycleBarrier => {
                self.release_edit_post_upload_lifecycle_barrier(event_loop);
            }
            DesktopEvent::ReleaseEditPostUploadBarrier => {
                self.release_edit_post_upload_barrier(event_loop);
            }
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
    match application.application_error {
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
            "unknown Render Path phase {phase:?}; expected release, configure, record, shutdown, or upload"
        )),
        None => Err(
            "missing Render Path phase; expected release, configure, record, shutdown, or upload"
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

#[cfg(all(test, target_os = "windows"))]
mod measurement_tests {
    use super::{
        CpuFrameMeasurement, MeasurementEvent, SteadyFrameCollection, fixed_edit_burst,
        format_convergence_overlay, parse_render_configuration, should_start_edit_burst,
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
    use canonical_scene::{CanonicalSceneScale, generate_canonical_scene};
    use raster_render_path::RasterConvergenceStatus;
    use render_backend::FrameObservation;
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
        let frontend = VoxelFrontend::new();
        let view =
            frontend.publish(generate_canonical_scene(CanonicalSceneScale::Large)?.into_scene())?;
        let mut plan = fixed_edit_burst(&view, 32)?;
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

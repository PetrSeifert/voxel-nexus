#[cfg(target_os = "windows")]
mod windows_adapter;

use canonical_inspection::{CanonicalCameraPose, overview_to_cavity_camera_move};
use canonical_scene::{CanonicalSceneMetadata, CanonicalSceneScale, generate_canonical_scene};
#[cfg(target_os = "windows")]
use measurement_evidence::{MeasurementEvent, ResourceCounts, VoxelSceneRevisionIdentity};
use raster_render_path::{
    CameraPose, RasterArtifactInstallationError, RasterArtifactInstallationPhase,
    RasterArtifactInstaller, RasterArtifactPreparation, RasterArtifactPreparationEvent,
    RasterCameraController, RasterPreparationBarrier, RasterPreparationBarrierRelease,
    RasterRenderPath,
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
use voxel_frontend::{VoxelFrontend, VoxelSceneRevision};
#[cfg(target_os = "windows")]
use windows_adapter::{WindowsPresentationAdapter, set_measurement_extent};
#[cfg(target_os = "windows")]
use winit::application::ApplicationHandler;
#[cfg(target_os = "windows")]
use winit::event::WindowEvent;
#[cfg(target_os = "windows")]
use winit::event_loop::EventLoopProxy;
#[cfg(target_os = "windows")]
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
#[cfg(target_os = "windows")]
use winit::platform::windows::EventLoopBuilderExtWindows;
#[cfg(target_os = "windows")]
use winit::window::{Window, WindowAttributes, WindowId};

#[derive(Clone, Copy)]
enum CanonicalCameraSelection {
    Fixed(CanonicalCameraPose),
    MoveStep {
        step: u32,
        total_steps: u32,
        pose: CameraPose,
    },
}

impl CanonicalCameraSelection {
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

#[derive(Clone)]
struct CanonicalRenderConfiguration {
    scale: CanonicalSceneScale,
    camera: CanonicalCameraSelection,
    hold_background_preparation: bool,
    inject_raster_upload_failure: bool,
    measurement: Option<MeasurementConfiguration>,
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

fn parse_canonical_configuration(
    mut arguments: impl Iterator<Item = String>,
) -> Result<(CanonicalRenderConfiguration, bool), String> {
    let mut scale = CanonicalSceneScale::Large;
    let mut camera = CanonicalCameraSelection::Fixed(CanonicalCameraPose::Overview);
    let mut camera_was_selected = false;
    let mut report_only = false;
    let mut hold_background_preparation = false;
    let mut inject_raster_upload_failure = false;
    let mut measurement_mode = None;
    let mut measurement_output = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--report-canonical-configuration" => report_only = true,
            "--hold-background-preparation" => hold_background_preparation = true,
            "--inject-raster-upload-failure" => inject_raster_upload_failure = true,
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
                scale = match arguments.next().as_deref() {
                    Some("64") => CanonicalSceneScale::Small,
                    Some("128") => CanonicalSceneScale::Medium,
                    Some("256") => CanonicalSceneScale::Large,
                    Some(value) => {
                        return Err(format!(
                            "unknown canonical scene scale {value:?}; expected 64, 128, or 256"
                        ));
                    }
                    None => return Err("missing canonical scene scale".to_owned()),
                };
            }
            "--camera-pose" => {
                if camera_was_selected {
                    return Err("select either one fixed camera pose or one move step".to_owned());
                }
                camera = match arguments.next().as_deref() {
                    Some("overview") => {
                        CanonicalCameraSelection::Fixed(CanonicalCameraPose::Overview)
                    }
                    Some("cavity") => {
                        CanonicalCameraSelection::Fixed(CanonicalCameraPose::CavityMaterialCloseUp)
                    }
                    Some("boundary") => {
                        CanonicalCameraSelection::Fixed(CanonicalCameraPose::BoundaryCutaway)
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
                camera = CanonicalCameraSelection::MoveStep {
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
        CanonicalRenderConfiguration {
            scale,
            camera,
            hold_background_preparation,
            inject_raster_upload_failure,
            measurement,
        },
        report_only,
    ))
}

fn report_canonical_configuration(
    metadata: &CanonicalSceneMetadata,
    configuration: &CanonicalRenderConfiguration,
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
    window: Option<Window>,
    application_error: Option<String>,
    drawable_occluded: bool,
    presentation_retry_at: Option<Instant>,
    canonical_configuration: CanonicalRenderConfiguration,
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
        canonical_configuration: CanonicalRenderConfiguration,
        event_proxy: EventLoopProxy<DesktopEvent>,
    ) -> Result<Self, String> {
        let measurement = canonical_configuration
            .measurement
            .as_ref()
            .map(MeasurementSession::new)
            .transpose()?;
        Ok(Self {
            backend: None,
            window: None,
            application_error: None,
            drawable_occluded: false,
            presentation_retry_at: None,
            canonical_configuration,
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
        })
    }

    fn set_status(&self, status: &str) {
        if let Some(window) = &self.window {
            window.set_title(&format!("Voxel Nexus Vulkan Demo | {status}"));
        }
    }

    fn fail(&mut self, event_loop: &ActiveEventLoop, error: impl ToString) {
        self.application_error = Some(error.to_string());
        event_loop.exit();
    }

    fn set_drawable_extent(&mut self, drawable_extent: ash::vk::Extent2D) {
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
        if drawable_extent.width > 0
            && drawable_extent.height > 0
            && let Some(window) = &self.window
        {
            window.request_redraw();
        }
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
        self.set_status(&format!("artifact-ready revision {installed_revision}"));
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn select_camera(&mut self, event_loop: &ActiveEventLoop, selection: CanonicalCameraSelection) {
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
            self.canonical_configuration
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
        if matches!(
            self.canonical_configuration
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
        let canonical = match generate_canonical_scene(self.canonical_configuration.scale) {
            Ok(canonical) => canonical,
            Err(error) => {
                self.application_error = Some(format!(
                    "could not generate the canonical Voxel Scene: {error}"
                ));
                event_loop.exit();
                return;
            }
        };
        report_canonical_configuration(canonical.metadata(), &self.canonical_configuration);
        self.occupied_voxels = canonical.metadata().occupied_count();
        let volume_identity = canonical.metadata().volume_identity().clone();
        let view = match VoxelFrontend::new().publish(canonical.into_scene()) {
            Ok(view) => view,
            Err(error) => {
                self.application_error = Some(format!(
                    "could not publish the canonical Voxel Scene: {error}"
                ));
                event_loop.exit();
                return;
            }
        };
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
        let (render_path, artifact_installer, camera_controller) =
            RasterRenderPath::awaiting_artifact_with_camera_control(
                self.canonical_configuration.camera.pose(),
                published_revision,
            );
        if self.canonical_configuration.inject_raster_upload_failure
            && let Err(error) = artifact_installer.inject_next_upload_failure()
        {
            self.application_error = Some(error.to_string());
            event_loop.exit();
            return;
        }
        let options = if matches!(
            self.canonical_configuration
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
        self.window = Some(window);
        self.artifact_installer = Some(artifact_installer);
        self.camera_controller = Some(camera_controller);
        self.published_revision = Some(published_revision);
        let (barrier, preparation_release) =
            if self.canonical_configuration.hold_background_preparation {
                let (barrier, release) = RasterPreparationBarrier::held();
                (Some(barrier), Some(release))
            } else {
                (None, None)
            };
        let event_proxy = self.event_proxy.clone();
        let preparation =
            match RasterArtifactPreparation::start(view, volume_identity, barrier, move |event| {
                if event_proxy
                    .send_event(DesktopEvent::Preparation(event))
                    .is_err()
                {
                    eprintln!("desktop event loop closed before preparation notification");
                }
            }) {
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
                    self.application_error = Some(error.to_string());
                }
                if let Some(backend) = &mut self.backend
                    && let Err(error) = backend.shutdown()
                {
                    self.application_error = Some(error.to_string());
                }
                event_loop.exit();
            }
            WindowEvent::Resized(drawable_size) => {
                let steady_measurement = matches!(
                    self.canonical_configuration
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
                self.set_drawable_extent(drawable_extent);
            }
            WindowEvent::Occluded(occluded) => {
                if occluded
                    && matches!(
                        self.canonical_configuration
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
                self.set_drawable_extent(drawable_extent);
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
                    self.canonical_configuration
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
                        }
                        if self.preparation_is_paused {
                            self.set_status(&format!(
                                "preparation-paused lifecycle-responsive {}x{}",
                                self.drawable_extent.width, self.drawable_extent.height
                            ));
                        }
                        self.advance_camera_move(event_loop);
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
                self.select_camera(event_loop, CanonicalCameraSelection::Fixed(pose));
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
    let (configuration, report_only) = parse_canonical_configuration(arguments.into_iter())?;
    if report_only {
        let canonical = generate_canonical_scene(configuration.scale)
            .map_err(|error| format!("could not generate the canonical Voxel Scene: {error}"))?;
        report_canonical_configuration(canonical.metadata(), &configuration);
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
        Some("upload") => {
            return Some(Err(RasterArtifactInstallationError::new(
                RasterArtifactInstallationPhase::Upload,
                VoxelSceneRevision::new(41),
                Box::new(InjectedRenderPathFailure),
            )
            .to_string()));
        }
        Some(phase) => Err(format!(
            "unknown Render Path phase {phase:?}; expected release, configure, record, or upload"
        )),
        None => Err(
            "missing Render Path phase; expected release, configure, record, or upload".to_owned(),
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
    use super::{CpuFrameMeasurement, MeasurementEvent, SteadyFrameCollection};
    use render_backend::FrameObservation;
    use std::time::{Duration, Instant};

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
        let result =
            parse_canonical_configuration(arguments.into_iter()).and_then(|(configuration, _)| {
                let canonical = generate_canonical_scene(configuration.scale).map_err(|error| {
                    format!("could not generate the canonical Voxel Scene: {error}")
                })?;
                report_canonical_configuration(canonical.metadata(), &configuration);
                Ok(())
            });
        return application_exit_code(result);
    }
    eprintln!("The Voxel Nexus desktop demo currently supports Windows only.");
    ExitCode::SUCCESS
}

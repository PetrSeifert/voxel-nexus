use super::configuration::{
    ComputeShutdownQualification, DesktopRenderConfiguration, DesktopSceneSelection,
    MeasurementMode, parse_render_configuration, report_canonical_configuration,
    report_render_configuration, report_winding_diagnostic_configuration,
    require_qualification_argument, winding_diagnostic_scene,
};
use super::diagnostics::{
    background_preparation_failure_diagnostic, render_path_failure_diagnostic,
    unsupported_prerequisite_diagnostic,
};
#[cfg(not(feature = "qualification"))]
use super::qualification_unavailable::*;
#[cfg(target_os = "windows")]
use super::scenarios::*;
#[cfg(target_os = "windows")]
use super::windows_adapter::{
    WindowsPresentationAdapter, WindowsTextOverlay, set_measurement_extent,
};
use canonical_inspection::CanonicalCameraPose;
use canonical_scene::generate_canonical_scene;
#[cfg(target_os = "windows")]
use compute_ray_render_path::{
    ComputeConvergenceController, ComputeLifecycleController, ComputeMeasurementController,
};
#[cfg(target_os = "windows")]
use raster_render_path::RasterRenderPathAdapter;
use raster_render_path::{
    CameraPose, RasterArtifactInstaller, RasterArtifactPreparation, RasterArtifactPreparationEvent,
    RasterLifecycleController,
};
#[cfg(feature = "qualification")]
use raster_render_path::{RasterPreparationBarrier, RasterPreparationBarrierRelease};
#[cfg(target_os = "windows")]
use render_backend::{
    CameraStateRevision, FrameOutcome, RenderBackend, RenderBackendOptions,
    RenderPathHandoffControl, RenderPathStrategy, RenderPathSwitchOwner,
};
use std::sync::{Arc, Mutex};
#[cfg(target_os = "windows")]
use std::time::{Duration, Instant};
use voxel_frontend::{VoxelExtent, VoxelFrontend, VoxelResidencySelectionId, VoxelSceneRevision};
#[cfg(target_os = "windows")]
use winit::application::ApplicationHandler;
#[cfg(target_os = "windows")]
use winit::event::{DeviceEvent, DeviceId, WindowEvent};
#[cfg(target_os = "windows")]
use winit::event_loop::EventLoopProxy;
#[cfg(target_os = "windows")]
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
#[cfg(target_os = "windows")]
use winit::platform::windows::EventLoopBuilderExtWindows;
#[cfg(target_os = "windows")]
use winit::window::{Window, WindowAttributes, WindowId};

#[cfg(target_os = "windows")]
#[derive(Clone, Copy)]
pub(super) enum DesktopEvent {
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
pub(super) struct DesktopRuntime {
    pub(super) backend: Option<RenderBackend>,
    pub(super) text_overlay: Option<WindowsTextOverlay>,
    pub(super) window: Option<Window>,
    pub(super) application_error: Option<String>,
    drawable_occluded: bool,
    presentation_retry_at: Option<Instant>,
    pub(super) render_configuration: DesktopRenderConfiguration,
    pub(super) event_proxy: EventLoopProxy<DesktopEvent>,
    pub(super) preparation: Option<RasterArtifactPreparation>,
    pub(super) preparation_release: Option<RasterPreparationBarrierRelease>,
    pub(super) artifact_installer: Option<RasterArtifactInstaller>,
    pub(super) camera_state: CameraPose,
    pub(super) camera_state_revision: CameraStateRevision,
    pub(super) pending_camera: Option<(CameraPose, CameraStateRevision)>,
    residency_identity: u64,
    residency_revision: Option<VoxelSceneRevision>,
    pub(super) streamed_crossing: super::streamed_crossing::StreamedCrossingLatency,
    pub(super) peak_residency_bytes: usize,
    pub(super) published_revision: Option<VoxelSceneRevision>,
    pub(super) drawable_extent: ash::vk::Extent2D,
    last_drawable_extent: ash::vk::Extent2D,
    pub(super) frontend: Option<Arc<VoxelFrontend>>,
    pub(super) lifecycle_controller: Option<RasterLifecycleController>,
    pub(super) render_path_handoff_control: Option<RenderPathHandoffControl>,
    pub(super) raster_preparation_target: Option<RasterPreparationTarget>,
    pub(super) raster_replacement_installer: Option<RasterArtifactInstaller>,
    pub(super) raster_replacement_lifecycle_controller: Option<RasterLifecycleController>,
    pub(super) interactive_switch: Option<InteractiveRenderPathSwitch>,
    pub(super) compute_convergence_controller: Option<ComputeConvergenceController>,
    pub(super) compute_lifecycle_controller: Option<ComputeLifecycleController>,
    pub(super) compute_measurement_controller: Option<ComputeMeasurementController>,
}

#[cfg(target_os = "windows")]
impl DesktopRuntime {
    pub(super) fn set_status(&self, status: &str) {
        if let Some(window) = &self.window {
            window.set_title(&format!("Voxel Nexus Vulkan Demo | {status}"));
        }
    }

    pub(super) fn fail(&mut self, event_loop: &ActiveEventLoop, error: impl ToString) {
        self.application_error = Some(error.to_string());
        event_loop.exit();
    }

    pub(super) fn record_close_error(&mut self, error: impl ToString) {
        let error = error.to_string();
        if self.application_error.is_none() {
            self.application_error = Some(error);
        } else {
            eprintln!("additional error during desktop close: {error}");
        }
    }

    pub(super) fn publish_camera_state(&mut self, pose: CameraPose) -> Result<(), String> {
        self.update_streamed_residency(pose)?;
        let next_revision = self
            .pending_camera
            .map_or(self.camera_state_revision, |(_, revision)| revision)
            .checked_successor()
            .ok_or_else(|| "the Camera State Revision identity overflowed".to_owned())?;
        self.backend
            .as_mut()
            .ok_or_else(|| {
                "the Render Backend is unavailable for Camera State publication".to_owned()
            })?
            .publish_camera_state(pose, next_revision)
            .map_err(|error| error.to_string())?;
        if self
            .switch_diagnostics()?
            .held_camera_state_revision()
            .is_some()
        {
            self.pending_camera = Some((pose, next_revision));
            return Ok(());
        }
        self.pending_camera = None;
        self.camera_state = pose;
        self.camera_state_revision = next_revision;
        Ok(())
    }

    pub(super) fn update_streamed_residency(&mut self, pose: CameraPose) -> Result<(), String> {
        let Some(neighbourhood) = self.render_configuration.streamed_neighbourhood() else {
            return Ok(());
        };
        let requested_at = Instant::now();
        let frontend = self
            .frontend
            .as_ref()
            .ok_or("the Voxel Frontend is unavailable")?;
        let volumes = super::streamed_world::residency_volumes(pose, neighbourhood);
        let required = frontend
            .required_residency()
            .map_err(|error| error.to_string())?;
        let view = self.latest_scene_view()?;
        let crossing = required
            .as_ref()
            .is_none_or(|selection| selection.volumes() != volumes);
        if !crossing && self.residency_revision == Some(view.revision()) {
            return Ok(());
        }
        let identity = self
            .residency_identity
            .checked_add(1)
            .ok_or("the Voxel Residency Selection identity overflowed")?;
        let selection = view
            .residency_selection(VoxelResidencySelectionId::new(identity), volumes)
            .map_err(|error| error.to_string())?;
        frontend
            .require_residency(selection.clone())
            .map_err(|error| error.to_string())?;
        // Render preparation must find complete shared copies rather than compete
        // with the frontend's single materialization worker.
        while !frontend
            .establish_residency()
            .map_err(|error| error.to_string())?
        {
            std::thread::yield_now();
        }
        self.backend
            .as_mut()
            .ok_or("the Render Backend is unavailable")?
            .submit_residency_selection(selection)
            .map_err(|error| error.to_string())?;
        self.residency_identity = identity;
        self.residency_revision = Some(view.revision());
        if crossing {
            self.streamed_crossing
                .crossed(VoxelResidencySelectionId::new(identity), requested_at);
        }
        Ok(())
    }

    /// Completes crossings the Presenting Render Path has installed, then summarizes residency.
    /// Runs after every presented frame, so latency includes the first frame showing coverage.
    pub(super) fn streamed_report(&mut self) -> Result<Option<String>, String> {
        let Some(neighbourhood) = self.render_configuration.streamed_neighbourhood() else {
            return Ok(None);
        };
        let residency = self
            .frontend
            .as_ref()
            .ok_or("the Voxel Frontend is unavailable")?
            .materialization_cache_stats()
            .map_err(|error| error.to_string())?;
        self.peak_residency_bytes = self.peak_residency_bytes.max(residency.storage_bytes);
        let gpu_memory = self
            .backend
            .as_ref()
            .ok_or("the Render Backend is unavailable")?
            .render_path_gpu_memory();
        if let Some(installed) = self
            .switch_diagnostics()?
            .presenting()
            .installed_selection()
        {
            for latency in self.streamed_crossing.installed(installed, Instant::now()) {
                println!(
                    "Streamed crossing: neighbourhood={neighbourhood} installed_selection={installed} latency_ms={:.1} worst_ms={:.1} residency_copies={} residency_bytes={} peak_residency_bytes={} render_path_gpu_bytes={} peak_render_path_gpu_bytes={}",
                    latency.as_secs_f64() * 1000.0,
                    self.streamed_crossing
                        .worst()
                        .unwrap_or_default()
                        .as_secs_f64()
                        * 1000.0,
                    residency.copies,
                    residency.storage_bytes,
                    self.peak_residency_bytes,
                    gpu_memory.live_bytes,
                    gpu_memory.peak_bytes,
                );
            }
        }
        let milliseconds = |latency: Option<Duration>| {
            latency.map_or_else(
                || "none".to_owned(),
                |latency| format!("{}ms", latency.as_millis()),
            )
        };
        let mebibytes = |bytes: u64| bytes as f64 / (1024.0 * 1024.0);
        Ok(Some(format!(
            "Neighbourhood={neighbourhood} Crossing={} WorstCrossing={} MaterializedResidency={}copies/{:.1}MiB PeakMaterializedResidency={:.1}MiB RenderPathGpuMemory={:.1}MiB PeakRenderPathGpuMemory={:.1}MiB",
            milliseconds(self.streamed_crossing.last()),
            milliseconds(self.streamed_crossing.worst()),
            residency.copies,
            mebibytes(residency.storage_bytes as u64),
            mebibytes(self.peak_residency_bytes as u64),
            mebibytes(gpu_memory.live_bytes),
            mebibytes(gpu_memory.peak_bytes),
        )))
    }

    pub(super) fn accept_pending_camera(&mut self) -> Result<(), String> {
        if let Some((pose, revision)) = self.pending_camera {
            let diagnostics = self.switch_diagnostics()?;
            if diagnostics.held_camera_state_revision().is_none()
                && diagnostics.presenting().camera_state_revision() == revision
            {
                self.camera_state = pose;
                self.camera_state_revision = revision;
                self.pending_camera = None;
            }
        }
        Ok(())
    }
}

#[cfg(target_os = "windows")]
pub(super) struct DesktopApplication {
    desktop: DesktopRuntime,
    scenario_state: ScenarioState,
}

#[cfg(target_os = "windows")]
impl DesktopApplication {
    fn new(
        render_configuration: DesktopRenderConfiguration,
        event_proxy: EventLoopProxy<DesktopEvent>,
    ) -> Result<Self, String> {
        let scenario_state = ScenarioState::new(&render_configuration)?;
        // The free-fly controller re-expresses the starting pose, so starting from its Camera
        // State avoids publishing an unchanged view on the first frame.
        let camera_state = match &scenario_state.interactive {
            Some(interactive) => interactive.camera_state()?,
            None => render_configuration.camera_pose()?,
        };
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
                pending_camera: None,
                residency_identity: 0,
                residency_revision: None,
                streamed_crossing: Default::default(),
                peak_residency_bytes: 0,
                published_revision: None,
                drawable_extent: ash::vk::Extent2D::default(),
                last_drawable_extent: ash::vk::Extent2D::default(),
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
pub(super) enum RasterPreparationTarget {
    Initial,
    Replacement,
}

#[cfg(target_os = "windows")]
pub(super) struct InteractiveRenderPathSwitch {
    pub(super) source: RenderPathStrategy,
    pub(super) replacement: RenderPathStrategy,
    pub(super) revision: VoxelSceneRevision,
    pub(super) handoff_reported: bool,
    pub(super) retiring_raster: Option<RasterLifecycleController>,
    pub(super) requested_at: Instant,
}

#[cfg(target_os = "windows")]
const PRESENTATION_RETRY_DELAY: Duration = Duration::from_millis(100);
#[cfg(target_os = "windows")]
pub(super) const STEADY_MEASUREMENT_EXTENT: ash::vk::Extent2D = ash::vk::Extent2D {
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
            || self
                .desktop
                .render_configuration
                .render_path_switching_enabled()
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
        self.desktop.last_drawable_extent = initial_drawable_extent;
        let frontend = Arc::new(match self.desktop.render_configuration.scene {
            DesktopSceneSelection::StreamedWorld(neighbourhood) => {
                super::streamed_world::frontend(neighbourhood)
            }
            _ => VoxelFrontend::new(),
        });
        let (publication, occupied_voxels) = match self.desktop.render_configuration.scene {
            DesktopSceneSelection::StreamedWorld(_) => {
                (frontend.publish_streamed(super::streamed_world::scene()), 0)
            }
            DesktopSceneSelection::LargeSparse => {
                let terrain = match canonical_scene::generate_large_terrain() {
                    Ok(terrain) => terrain,
                    Err(error) => {
                        self.desktop.fail(event_loop, error);
                        return;
                    }
                };
                (frontend.publish_sparse(terrain.into_scene()), 0)
            }
            DesktopSceneSelection::WindingDiagnostic => {
                let (scene, _) = winding_diagnostic_scene();
                if let Err(error) =
                    report_winding_diagnostic_configuration(&self.desktop.render_configuration)
                {
                    self.desktop.fail(event_loop, error);
                    return;
                }
                (frontend.publish(scene), 2)
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
                (
                    frontend.publish_sparse(canonical.into_scene()),
                    occupied_voxels,
                )
            }
        };
        self.scenario_state.evidence.occupied_voxels = occupied_voxels;
        let view = match publication {
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
        self.desktop.frontend = Some(frontend.clone());
        if let Some(neighbourhood) = self.desktop.render_configuration.streamed_neighbourhood() {
            let initial = view.residency_selection(
                VoxelResidencySelectionId::new(1),
                super::streamed_world::residency_volumes(self.desktop.camera_state, neighbourhood),
            );
            if let Err(error) = initial.and_then(|selection| {
                frontend.require_residency(selection)?;
                frontend.establish_residency()
            }) {
                self.desktop.fail(event_loop, error);
                return;
            }
            self.desktop.residency_identity = 1;
            self.desktop.residency_revision = Some(view.revision());
        }
        let published_revision = view.revision();
        if let Err(error) = self
            .scenario_state
            .evidence
            .scene_published(published_revision)
        {
            self.desktop.fail(event_loop, error);
            return;
        }
        let (render_path, artifact_installer): (Box<dyn render_backend::SwitchableRenderPath>, _) =
            if view.is_streamed() {
                let mut path = match self.desktop.build_streamed_raster(&view) {
                    Ok(path) => path,
                    Err(error) => {
                        self.desktop.fail(event_loop, error);
                        return;
                    }
                };
                self.desktop.lifecycle_controller = Some(path.enable_lifecycle_control());
                (Box::new(path), None)
            } else if self.desktop.render_configuration.compute_only() {
                let mut path = match compute_ray_render_path::ComputeRayRenderPathAdapter::new_with_representation(
                    view.clone(), self.desktop.camera_state, self.desktop.camera_state_revision,
                    self.desktop.render_configuration.compute_representation,
                ) {
                    Ok(path) => path,
                    Err(error) => {
                        self.desktop.fail(event_loop, error);
                        return;
                    }
                };
                self.desktop.compute_convergence_controller =
                    Some(path.enable_convergence_control());
                self.desktop.compute_lifecycle_controller = Some(path.enable_lifecycle_control());
                (Box::new(path), None)
            } else {
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
                (Box::new(render_path), Some(artifact_installer))
            };
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
        let render_path = RenderPathSwitchOwner::new(render_path);
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
        self.desktop.artifact_installer = artifact_installer;
        self.desktop.render_path_handoff_control = Some(render_path_handoff_control);
        self.desktop.published_revision = Some(published_revision);
        if view.is_streamed() {
            if let Err(error) = self.scenarios().set_render_path_overlay() {
                self.desktop.fail(event_loop, error);
                return;
            }
            if let Some(window) = &self.desktop.window {
                window.request_redraw();
            }
            return;
        }
        if self.desktop.render_configuration.compute_only() {
            if let Err(error) = self.scenarios().set_render_path_overlay() {
                self.desktop.fail(event_loop, error);
                return;
            }
            if let Some(window) = &self.desktop.window {
                window.request_redraw();
            }
            return;
        }
        #[cfg(feature = "qualification")]
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
        #[cfg(not(feature = "qualification"))]
        let preparation_release = None;
        let event_proxy = self.desktop.event_proxy.clone();
        #[cfg(feature = "qualification")]
        let start_preparation = RasterArtifactPreparation::start_regions_with_barrier;
        #[cfg(not(feature = "qualification"))]
        let start_preparation = RasterArtifactPreparation::start_regions;
        let preparation = match start_preparation(
            view,
            VoxelExtent::new(
                self.desktop.render_configuration.raster_region_extent,
                self.desktop.render_configuration.raster_region_extent,
                self.desktop.render_configuration.raster_region_extent,
            ),
            #[cfg(feature = "qualification")]
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
        if self
            .desktop
            .render_configuration
            .render_path_switching_enabled()
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
            WindowEvent::MouseInput { state, button, .. } => {
                self.scenarios()
                    .interactive_mouse_input(event_loop, state, button);
            }
            WindowEvent::Focused(focused) => {
                self.scenarios()
                    .interactive_focus_changed(event_loop, focused);
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
                self.scenarios().before_interactive_draw(event_loop);
                if event_loop.exiting() {
                    return;
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

    fn device_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _device_id: DeviceId,
        event: DeviceEvent,
    ) {
        if let DeviceEvent::MouseMotion { delta } = event {
            self.scenarios().interactive_mouse_motion(delta);
        }
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
pub(super) fn run() -> Result<(), String> {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    if arguments
        .iter()
        .any(|argument| argument == "--streamed-world")
    {
        parse_render_configuration(arguments.clone().into_iter())?;
    }
    for argument in &arguments {
        require_qualification_argument(argument)?;
    }
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
            // SAFETY: Checked non-null above; winit passes a pointer to the `MSG` being dispatched.
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

#[cfg(target_os = "windows")]
impl DesktopRuntime {
    pub(super) fn set_drawable_extent(
        &mut self,
        drawable_extent: ash::vk::Extent2D,
    ) -> Result<(), String> {
        if self.render_configuration.streamed_neighbourhood().is_some()
            && self.backend.is_some()
            && self.last_drawable_extent.width > 0
            && self.last_drawable_extent.height > 0
            && drawable_extent.width > 0
            && drawable_extent.height > 0
        {
            let previous_aspect =
                self.last_drawable_extent.width as f32 / self.last_drawable_extent.height as f32;
            let next_aspect = drawable_extent.width as f32 / drawable_extent.height as f32;
            let camera = self.camera_state;
            // The accepted camera must stay covered even when a newer move is held.
            // Shortening before reconfiguration preserves its previous corner-distance bound.
            let far = camera.far_plane() * (previous_aspect / next_aspect).min(1.0);
            let pose = camera
                .with_clip_planes(camera.near_plane().min(far * 0.25), far)
                .map_err(|error| error.to_string())?;
            let revision = self
                .pending_camera
                .map_or(self.camera_state_revision, |(_, revision)| revision)
                .checked_successor()
                .ok_or("the Camera State Revision identity overflowed")?;
            self.backend
                .as_mut()
                .ok_or("the Render Backend is unavailable")?
                .publish_camera_state(pose, revision)
                .map_err(|error| error.to_string())?;
            self.camera_state = pose;
            self.camera_state_revision = revision;
            self.pending_camera = None;
        }
        if drawable_extent.width > 0 && drawable_extent.height > 0 {
            self.last_drawable_extent = drawable_extent;
        }
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

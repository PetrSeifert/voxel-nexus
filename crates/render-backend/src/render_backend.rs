use ash::{Entry, vk};
use std::ffi::CStr;

#[cfg(feature = "qualification")]
mod allocation_qualification;
mod render_path_switching;
mod residency_coverage;
#[cfg(feature = "qualification")]
pub use allocation_qualification::{
    GpuAllocationClass, GpuAllocationQualification, GpuAllocationQualificationError,
    GpuAllocationSnapshot, with_gpu_allocation_owner, with_gpu_scene_buffer,
};
pub use residency_coverage::RenderPathCoverage;

pub use render_path_switching::{
    CameraStateRevision, RenderPathHandoffControl, RenderPathHandoffMismatch, RenderPathReadiness,
    RenderPathRetirement, RenderPathRoleStatus, RenderPathStamp, RenderPathStrategy,
    RenderPathSwitchDiagnostics, RenderPathSwitchEvent, RenderPathSwitchOwner,
    RenderPathSwitchRequestError, SwitchableRenderPath,
};

const VALIDATION_LAYER_NAME: &CStr = c"VK_LAYER_KHRONOS_validation";

pub struct RenderBackend {
    frame_failed: bool,
    rendering: Option<PresentationResources>,
    path: Box<dyn RenderPath>,
    device: LogicalDevice,
    presentation: InstanceSurface,
    selected_device: InspectedDevice,
    graphics_queue: vk::Queue,
    presentation_queue: vk::Queue,
    runtime_context: RuntimeContext,
    drawable_extent: vk::Extent2D,
    swapchain_needs_recreation: bool,
    next_configuration_id: u64,
    frame_sequences: BackendFrameSequences,
    path_is_configured: bool,
    path_is_shutdown: bool,
    memory_properties: vk::PhysicalDeviceMemoryProperties,
    render_path_device_capabilities: RenderPathDeviceCapabilities,
    gpu_memory: GpuMemoryLedger,
    options: RenderBackendOptions,
}

struct BackendFrameSequences {
    next: u64,
}

impl BackendFrameSequences {
    fn new() -> Self {
        Self { next: 1 }
    }

    fn pending(&self) -> u64 {
        self.next
    }

    fn record_submission(&mut self) -> Result<(), BackendError> {
        self.next = self
            .next
            .checked_add(1)
            .ok_or(BackendError::FrameSequenceIdentityExhausted)?;
        Ok(())
    }
}

impl RenderBackend {
    #[cfg(feature = "qualification")]
    pub fn qualification_presentation_image_count(&self) -> usize {
        self.rendering.as_ref().map_or(0, |rendering| {
            rendering.render_path_target().attachments().count()
        })
    }
    /// Device memory that Render Paths currently hold, and the most they have held at once.
    pub fn render_path_gpu_memory(&self) -> RenderPathGpuMemory {
        self.gpu_memory.usage()
    }

    pub fn initialize(
        application_name: &CStr,
        adapter: &impl PresentationAdapter,
        initial_drawable_extent: vk::Extent2D,
        path: impl RenderPath + 'static,
    ) -> Result<Self, BackendError> {
        Self::initialize_with_options(
            application_name,
            adapter,
            initial_drawable_extent,
            path,
            RenderBackendOptions::default(),
        )
    }

    pub fn initialize_with_options(
        application_name: &CStr,
        adapter: &impl PresentationAdapter,
        initial_drawable_extent: vk::Extent2D,
        path: impl RenderPath + 'static,
        options: RenderBackendOptions,
    ) -> Result<Self, BackendError> {
        // SAFETY: The system Vulkan loader is trusted to run its initialization code on load.
        let entry = unsafe { Entry::load()? };
        require_vulkan_1_3_loader(&entry)?;
        if options.validation_enabled {
            require_validation_layer(&entry)?;
        }
        let presentation =
            create_instance_surface(entry, application_name, adapter, options.validation_enabled)?;

        let inspected_devices = inspect_devices(&presentation)?;
        let selected_device = select_inspected_device(inspected_devices)?;
        let memory_properties = unsafe {
            presentation
                .instance
                .get_physical_device_memory_properties(selected_device.physical_device)
        };
        let render_path_device_capabilities = selected_device.render_path_device_capabilities;
        let device = LogicalDevice::new(&presentation.instance, &selected_device)?;
        let graphics_queue =
            unsafe { device.get_device_queue(selected_device.graphics_queue_family_index, 0) };
        let presentation_queue =
            unsafe { device.get_device_queue(selected_device.presentation_queue_family_index, 0) };
        let rendering = PresentationResources::new(
            &presentation,
            &device,
            &selected_device,
            initial_drawable_extent,
            PresentationConfigurationId(0),
            options,
        )?;
        if options.gpu_timestamps_enabled && selected_device.timestamp_valid_bits == 0 {
            return Err(BackendError::TimestampQueriesUnsupported);
        }
        let present_mode = rendering
            .as_ref()
            .map(|rendering| rendering.present_mode)
            .unwrap_or(vk::PresentModeKHR::FIFO);
        let runtime_context = RuntimeContext {
            device_name: selected_device.candidate.name.clone(),
            driver_version: selected_device.candidate.driver_version,
            api_version: selected_device.candidate.api_version,
            validation_enabled: options.validation_enabled,
            present_mode,
            timestamp_valid_bits: selected_device.timestamp_valid_bits,
            timestamp_period_nanoseconds: selected_device.timestamp_period_nanoseconds,
        };

        let mut backend = Self {
            frame_failed: false,
            rendering,
            path: Box::new(path),
            device,
            presentation,
            selected_device,
            graphics_queue,
            presentation_queue,
            runtime_context,
            drawable_extent: initial_drawable_extent,
            swapchain_needs_recreation: false,
            next_configuration_id: 1,
            frame_sequences: BackendFrameSequences::new(),
            path_is_configured: false,
            path_is_shutdown: false,
            memory_properties,
            render_path_device_capabilities,
            gpu_memory: GpuMemoryLedger::new(),
            options,
        };
        backend.configure_path()?;
        Ok(backend)
    }

    pub fn runtime_context(&self) -> &RuntimeContext {
        &self.runtime_context
    }

    pub fn render_path_switch_diagnostics(&self) -> Option<RenderPathSwitchDiagnostics> {
        self.path.switch_diagnostics()
    }

    pub fn submit_edit_outcome(
        &mut self,
        outcome: voxel_frontend::VoxelEditOutcome,
    ) -> RenderPathResult<()> {
        self.path.submit_edit_outcome(outcome)
    }

    pub fn submit_residency_selection(
        &mut self,
        selection: voxel_frontend::VoxelResidencySelection,
    ) -> RenderPathResult<()> {
        self.path.submit_residency_selection(selection)
    }

    pub fn request_render_path_switch(
        &mut self,
        replacement: Box<dyn SwitchableRenderPath>,
    ) -> Result<(), RenderPathSwitchRequestError> {
        self.path.request_switch(replacement)
    }

    pub fn publish_camera_state(
        &mut self,
        camera_state: CameraState,
        camera_state_revision: CameraStateRevision,
    ) -> RenderPathResult<()> {
        self.path
            .publish_camera_state(camera_state, camera_state_revision)
    }

    pub fn validation_warning_count(&self) -> usize {
        self.presentation.validation_diagnostics.warning_count()
    }

    pub fn validation_error_count(&self) -> usize {
        self.presentation.validation_diagnostics.error_count()
    }

    pub fn take_frame_observation(&mut self) -> Option<FrameObservation> {
        self.rendering
            .as_mut()
            .and_then(PresentationResources::take_frame_observation)
    }

    pub fn last_submitted_frame_sequence(&self) -> Option<u64> {
        self.rendering
            .as_ref()
            .and_then(|rendering| rendering.last_submitted_frame_sequence)
    }

    pub fn presentation_extent(&self) -> Option<vk::Extent2D> {
        self.rendering.as_ref().map(|rendering| rendering.extent)
    }

    pub fn set_drawable_extent(&mut self, drawable_extent: vk::Extent2D) {
        self.drawable_extent = drawable_extent;
        self.swapchain_needs_recreation = true;
    }

    pub fn refresh_render_path(&mut self) -> Result<(), BackendError> {
        if self.frame_failed {
            return Err(BackendError::FrameFailureIsTerminal);
        }
        unsafe { self.device.device_wait_idle() }.map_err(BackendError::WaitForDevice)?;
        self.release_path()?;
        self.configure_path()
    }

    /// A frame error is terminal for this backend. The failing call preserves the original
    /// error; later calls return `FrameFailureIsTerminal`, including after resize or refresh.
    /// Call `shutdown` and create a new backend to resume rendering.
    pub fn draw_frame(&mut self) -> Result<FrameOutcome, BackendError> {
        if self.frame_failed {
            return Err(BackendError::FrameFailureIsTerminal);
        }
        let result = self.draw_frame_inner();
        // Acquisition may have signaled a semaphore without a submission consuming it.
        // Keep all presentation resources for idle shutdown, never for another frame.
        self.frame_failed = result.is_err();
        result
    }

    fn draw_frame_inner(&mut self) -> Result<FrameOutcome, BackendError> {
        if drawable_extent_is_zero(self.drawable_extent) {
            if self.path_is_configured || self.rendering.is_some() {
                unsafe { self.device.device_wait_idle() }.map_err(BackendError::WaitForDevice)?;
                self.release_path()?;
                self.rendering = None;
            }
            return self.ensure_validation_clean(FrameOutcome::Suspended);
        }
        if self.swapchain_needs_recreation || self.rendering.is_none() {
            let swapchain_ready = self.recreate_swapchain()?;
            if !swapchain_ready {
                return self.ensure_validation_clean(FrameOutcome::RetryLater);
            }
        }
        let Some(rendering) = &mut self.rendering else {
            return self.ensure_validation_clean(FrameOutcome::Suspended);
        };
        let submitted_frame_sequence = self.frame_sequences.pending();
        let outcome = match rendering.draw_frame(
            self.path.as_mut(),
            RenderPathDeviceContext {
                device: &self.device,
                memory_properties: self.memory_properties,
                capabilities: self.render_path_device_capabilities,
                gpu_memory: &self.gpu_memory,
            },
            self.graphics_queue,
            self.presentation_queue,
            submitted_frame_sequence,
        )? {
            PresentationOutcome::Presented => FrameOutcome::Presented,
            PresentationOutcome::Invalidated => {
                self.swapchain_needs_recreation = true;
                FrameOutcome::Recreate
            }
        };
        if rendering.last_submitted_frame_sequence == Some(submitted_frame_sequence) {
            self.frame_sequences.record_submission()?;
        }
        self.ensure_validation_clean(outcome)
    }

    fn recreate_swapchain(&mut self) -> Result<bool, BackendError> {
        unsafe { self.device.device_wait_idle() }.map_err(BackendError::WaitForDevice)?;
        self.release_path()?;
        self.rendering = None;
        let configuration_id = PresentationConfigurationId(self.next_configuration_id);
        self.next_configuration_id = self
            .next_configuration_id
            .checked_add(1)
            .ok_or(BackendError::PresentationConfigurationIdentityExhausted)?;
        let rendering = PresentationResources::new(
            &self.presentation,
            &self.device,
            &self.selected_device,
            self.drawable_extent,
            configuration_id,
            self.options,
        )?;
        let swapchain_ready = rendering.is_some();
        self.rendering = rendering;
        self.configure_path()?;
        self.swapchain_needs_recreation = !swapchain_ready;
        Ok(swapchain_ready)
    }

    fn configure_path(&mut self) -> Result<(), BackendError> {
        let Some(rendering) = &self.rendering else {
            return Ok(());
        };
        self.path_is_configured = true;
        run_render_path_phase(RenderPathPhase::Configure, || {
            self.path.configure(
                RenderPathDeviceContext {
                    device: &self.device,
                    memory_properties: self.memory_properties,
                    capabilities: self.render_path_device_capabilities,
                    gpu_memory: &self.gpu_memory,
                },
                rendering.render_path_target(),
            )
        })?;
        Ok(())
    }

    fn release_path(&mut self) -> Result<(), BackendError> {
        if !self.path_is_configured {
            return Ok(());
        }
        run_render_path_phase(RenderPathPhase::Release, || {
            self.path.release(RenderPathDeviceContext {
                device: &self.device,
                memory_properties: self.memory_properties,
                capabilities: self.render_path_device_capabilities,
                gpu_memory: &self.gpu_memory,
            })
        })?;
        self.path_is_configured = false;
        Ok(())
    }

    fn shutdown_path(&mut self) -> Result<(), BackendError> {
        if self.path_is_shutdown {
            return Ok(());
        }
        run_render_path_phase(RenderPathPhase::Shutdown, || {
            self.path.shutdown(RenderPathDeviceContext {
                device: &self.device,
                memory_properties: self.memory_properties,
                capabilities: self.render_path_device_capabilities,
                gpu_memory: &self.gpu_memory,
            })
        })?;
        self.path_is_shutdown = true;
        Ok(())
    }

    pub fn shutdown(&mut self) -> Result<(), BackendError> {
        unsafe { self.device.device_wait_idle() }.map_err(BackendError::WaitForDevice)?;
        self.release_path()?;
        self.shutdown_path()?;
        self.rendering = None;
        Ok(())
    }

    fn ensure_validation_clean(&self, outcome: FrameOutcome) -> Result<FrameOutcome, BackendError> {
        let validation_error_count = self.validation_error_count();
        if validation_error_count > 0 {
            return Err(BackendError::ValidationErrors {
                count: validation_error_count,
            });
        }
        Ok(outcome)
    }
}

impl Drop for RenderBackend {
    fn drop(&mut self) {
        if let Err(error) = unsafe { self.device.device_wait_idle() } {
            eprintln!("Vulkan device did not become idle during shutdown: {error}");
            return;
        }
        if let Err(error) = self.release_path() {
            eprintln!("{error}");
        }
        if let Err(error) = self.shutdown_path() {
            eprintln!("{error}");
        }
    }
}

mod camera;
pub use camera::{
    CameraConfigurationError, CameraState, DeterministicCameraMove, PresentationStyle,
};

mod configuration;
pub use configuration::{
    BackendError, DeviceCandidate, DeviceRejection, DeviceRequirement, DeviceSelectionError,
    QueueFamilyCapabilities, RenderBackendOptions, RenderPathDeviceCapabilities, RuntimeContext,
    SurfaceSupport, SwapchainConfiguration, SwapchainConfigurationError,
    SwapchainConfigurationState,
};

mod render_path;

pub use render_path::{
    PresentationAdapter, PresentationConfigurationId, RenderPath, RenderPathAttachment,
    RenderPathAttachmentIdentity, RenderPathCameraStateError, RenderPathDeviceContext,
    RenderPathEditError, RenderPathFrameContext, RenderPathFrameTarget, RenderPathPhase,
    RenderPathResult, RenderPathTarget, run_render_path_phase,
};

mod gpu_memory;
use gpu_memory::GpuMemoryLedger;
pub use gpu_memory::RenderPathGpuMemory;

mod frame_observation;
pub use frame_observation::{
    FrameObservation, FrameObservationBuffer, FrameObservationError, FrameOutcome,
};

mod device;
pub use device::select_device;
use device::*;

mod presentation;
pub use presentation::select_swapchain_configuration;
use presentation::*;

#[cfg(test)]
mod tests;

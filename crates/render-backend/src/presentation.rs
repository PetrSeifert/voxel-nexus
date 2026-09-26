use super::configuration::{
    BackendError, RenderBackendOptions, RenderPathDeviceCapabilities, SurfaceSupport,
    SwapchainConfiguration, SwapchainConfigurationError, SwapchainConfigurationState,
};
use super::device::{InspectedDevice, InstanceSurface, query_surface_support};
use super::frame_observation::{FrameObservation, FrameObservationBuffer};
use super::render_path::{
    PresentationConfigurationId, PresentationImage, RenderPath, RenderPathAttachment,
    RenderPathAttachmentIdentity, RenderPathDeviceContext, RenderPathFrameContext,
    RenderPathFrameTarget, RenderPathPhase, RenderPathTarget, run_render_path_phase,
};
use ash::vk;
use std::marker::PhantomData;

pub(super) struct PresentationResources {
    device: ash::Device,
    swapchain_loader: ash::khr::swapchain::Device,
    swapchain: vk::SwapchainKHR,
    images: Vec<PresentationImage>,
    image_views: Vec<vk::ImageView>,
    command_pool: vk::CommandPool,
    command_buffer: vk::CommandBuffer,
    image_available: vk::Semaphore,
    render_finished: Vec<vk::Semaphore>,
    frame_fence: vk::Fence,
    pub(super) extent: vk::Extent2D,
    format: vk::Format,
    configuration_id: PresentationConfigurationId,
    pub(super) present_mode: vk::PresentModeKHR,
    timestamp_query_pool: vk::QueryPool,
    timestamp_valid_bits: u32,
    timestamp_period_nanoseconds: f64,
    pending_frame_sequence: Option<u64>,
    pub(super) last_submitted_frame_sequence: Option<u64>,
    frame_observations: FrameObservationBuffer,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PresentationOutcome {
    Presented,
    Invalidated,
}

pub(super) trait FrameBoundaryOperations {
    type Acquired;

    fn wait_for_preceding_frame(&mut self) -> Result<(), BackendError>;

    fn advance_render_path(&mut self) -> Result<(), BackendError>;

    fn acquire_image(&mut self) -> Result<Self::Acquired, BackendError>;
}

pub(super) fn run_frame_boundary_operations<Operations: FrameBoundaryOperations>(
    operations: &mut Operations,
) -> Result<Operations::Acquired, BackendError> {
    operations.wait_for_preceding_frame()?;
    operations.advance_render_path()?;
    operations.acquire_image()
}

pub(super) struct VulkanFrameBoundaryOperations<'frame> {
    presentation: &'frame mut PresentationResources,
    path: &'frame mut dyn RenderPath,
    memory_properties: vk::PhysicalDeviceMemoryProperties,
    capabilities: RenderPathDeviceCapabilities,
}

impl FrameBoundaryOperations for VulkanFrameBoundaryOperations<'_> {
    type Acquired = Option<(u32, bool)>;

    fn wait_for_preceding_frame(&mut self) -> Result<(), BackendError> {
        unsafe {
            self.presentation
                .device
                .wait_for_fences(&[self.presentation.frame_fence], true, u64::MAX)
                .map_err(BackendError::WaitForFrame)?;
        }
        self.presentation.collect_timestamp_observation()
    }

    fn advance_render_path(&mut self) -> Result<(), BackendError> {
        run_render_path_phase(RenderPathPhase::AdvanceFrameBoundary, || {
            self.path.advance_frame_boundary(
                RenderPathDeviceContext {
                    device: &self.presentation.device,
                    memory_properties: self.memory_properties,
                    capabilities: self.capabilities,
                },
                self.presentation.render_path_target(),
            )
        })
    }

    fn acquire_image(&mut self) -> Result<Self::Acquired, BackendError> {
        match unsafe {
            self.presentation.swapchain_loader.acquire_next_image(
                self.presentation.swapchain,
                u64::MAX,
                self.presentation.image_available,
                vk::Fence::null(),
            )
        } {
            Ok(acquired_image) => Ok(Some(acquired_image)),
            Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => Ok(None),
            Err(error) => Err(BackendError::AcquireSwapchainImage(error)),
        }
    }
}

impl PresentationResources {
    pub(super) fn new(
        presentation: &InstanceSurface,
        device: &ash::Device,
        selected_device: &InspectedDevice,
        initial_drawable_extent: vk::Extent2D,
        configuration_id: PresentationConfigurationId,
        options: RenderBackendOptions,
    ) -> Result<Option<Self>, BackendError> {
        let surface_support = query_surface_support(presentation, selected_device.physical_device)?;
        let configuration = select_swapchain_configuration_for_mode(
            &surface_support,
            initial_drawable_extent,
            options.presentation_throttling_enabled,
        )?;
        let SwapchainConfigurationState::Ready(configuration) = configuration else {
            return Ok(None);
        };
        let swapchain_loader = ash::khr::swapchain::Device::new(&presentation.instance, device);
        let mut resources = Self {
            device: device.clone(),
            swapchain_loader,
            swapchain: vk::SwapchainKHR::null(),
            images: Vec::new(),
            image_views: Vec::new(),
            command_pool: vk::CommandPool::null(),
            command_buffer: vk::CommandBuffer::null(),
            image_available: vk::Semaphore::null(),
            render_finished: Vec::new(),
            frame_fence: vk::Fence::null(),
            extent: vk::Extent2D::default(),
            format: configuration.format,
            configuration_id,
            present_mode: configuration.present_mode,
            timestamp_query_pool: vk::QueryPool::null(),
            timestamp_valid_bits: selected_device.timestamp_valid_bits,
            timestamp_period_nanoseconds: selected_device.timestamp_period_nanoseconds,
            pending_frame_sequence: None,
            last_submitted_frame_sequence: None,
            frame_observations: FrameObservationBuffer::default(),
        };

        resources.extent = configuration.extent;
        resources.create_swapchain(presentation, selected_device, &configuration)?;
        let images = unsafe {
            resources
                .swapchain_loader
                .get_swapchain_images(resources.swapchain)
        }
        .map_err(BackendError::GetSwapchainImages)?;
        resources.create_image_views(&images, configuration.format)?;
        resources.create_commands(selected_device.graphics_queue_family_index)?;
        resources.create_synchronization()?;
        if options.gpu_timestamps_enabled {
            resources.create_timestamp_queries()?;
        }
        Ok(Some(resources))
    }

    fn create_timestamp_queries(&mut self) -> Result<(), BackendError> {
        let create_info = vk::QueryPoolCreateInfo::default()
            .query_type(vk::QueryType::TIMESTAMP)
            .query_count(2);
        self.timestamp_query_pool = unsafe { self.device.create_query_pool(&create_info, None) }
            .map_err(BackendError::CreateTimestampQueryPool)?;
        Ok(())
    }

    fn create_swapchain(
        &mut self,
        presentation: &InstanceSurface,
        selected_device: &InspectedDevice,
        configuration: &SwapchainConfiguration,
    ) -> Result<(), BackendError> {
        let queue_family_indices = [
            selected_device.graphics_queue_family_index,
            selected_device.presentation_queue_family_index,
        ];
        let mut create_info = vk::SwapchainCreateInfoKHR::default()
            .surface(presentation.surface)
            .min_image_count(configuration.image_count)
            .image_format(configuration.format)
            .image_color_space(configuration.color_space)
            .image_extent(configuration.extent)
            .image_array_layers(1)
            .image_usage(vk::ImageUsageFlags::COLOR_ATTACHMENT)
            .pre_transform(configuration.pre_transform)
            .composite_alpha(configuration.composite_alpha)
            .present_mode(configuration.present_mode)
            .clipped(true);
        if queue_family_indices[0] != queue_family_indices[1] {
            create_info = create_info
                .image_sharing_mode(vk::SharingMode::CONCURRENT)
                .queue_family_indices(&queue_family_indices);
        } else {
            create_info = create_info.image_sharing_mode(vk::SharingMode::EXCLUSIVE);
        }
        self.swapchain = unsafe { self.swapchain_loader.create_swapchain(&create_info, None) }
            .map_err(BackendError::CreateSwapchain)?;
        Ok(())
    }

    fn create_image_views(
        &mut self,
        images: &[vk::Image],
        format: vk::Format,
    ) -> Result<(), BackendError> {
        for image in images {
            let subresource_range = vk::ImageSubresourceRange::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                .level_count(1)
                .layer_count(1);
            let create_info = vk::ImageViewCreateInfo::default()
                .image(*image)
                .view_type(vk::ImageViewType::TYPE_2D)
                .format(format)
                .subresource_range(subresource_range);
            let image_view = unsafe { self.device.create_image_view(&create_info, None) }
                .map_err(BackendError::CreateImageView)?;
            self.images.push(PresentationImage {
                image: *image,
                view: image_view,
            });
            self.image_views.push(image_view);
        }
        Ok(())
    }

    fn create_commands(&mut self, queue_family_index: u32) -> Result<(), BackendError> {
        let pool_create_info = vk::CommandPoolCreateInfo::default()
            .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER)
            .queue_family_index(queue_family_index);
        self.command_pool = unsafe { self.device.create_command_pool(&pool_create_info, None) }
            .map_err(BackendError::CreateCommandPool)?;
        let allocate_info = vk::CommandBufferAllocateInfo::default()
            .command_pool(self.command_pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(1);
        let mut command_buffers = unsafe { self.device.allocate_command_buffers(&allocate_info) }
            .map_err(BackendError::AllocateCommandBuffer)?;
        self.command_buffer = command_buffers
            .pop()
            .ok_or(BackendError::AllocateCommandBuffer(
                vk::Result::ERROR_UNKNOWN,
            ))?;
        Ok(())
    }

    fn create_synchronization(&mut self) -> Result<(), BackendError> {
        let semaphore_info = vk::SemaphoreCreateInfo::default();
        self.image_available = unsafe { self.device.create_semaphore(&semaphore_info, None) }
            .map_err(BackendError::CreateFrameSynchronization)?;
        for _ in &self.images {
            let render_finished = unsafe { self.device.create_semaphore(&semaphore_info, None) }
                .map_err(BackendError::CreateFrameSynchronization)?;
            self.render_finished.push(render_finished);
        }
        let fence_info = vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED);
        self.frame_fence = unsafe { self.device.create_fence(&fence_info, None) }
            .map_err(BackendError::CreateFrameSynchronization)?;
        Ok(())
    }

    pub(super) fn draw_frame(
        &mut self,
        path: &mut dyn RenderPath,
        memory_properties: vk::PhysicalDeviceMemoryProperties,
        capabilities: RenderPathDeviceCapabilities,
        graphics_queue: vk::Queue,
        presentation_queue: vk::Queue,
        submitted_frame_sequence: u64,
    ) -> Result<PresentationOutcome, BackendError> {
        self.last_submitted_frame_sequence = None;
        let acquired_image = run_frame_boundary_operations(&mut VulkanFrameBoundaryOperations {
            presentation: self,
            path,
            memory_properties,
            capabilities,
        })?;
        let Some((image_index, acquire_suboptimal)) = acquired_image else {
            return Ok(PresentationOutcome::Invalidated);
        };
        let image_index_usize = usize::try_from(image_index)
            .map_err(|_| BackendError::SubmitFrame(vk::Result::ERROR_UNKNOWN))?;
        let render_finished = self
            .render_finished
            .get(image_index_usize)
            .copied()
            .ok_or(BackendError::SubmitFrame(vk::Result::ERROR_UNKNOWN))?;
        unsafe {
            self.device
                .reset_command_buffer(self.command_buffer, vk::CommandBufferResetFlags::empty())
                .map_err(BackendError::ResetFrame)?;
        }
        self.record_commands(path, image_index, submitted_frame_sequence)?;

        let wait_semaphores = [self.image_available];
        let wait_stages = [vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT];
        let command_buffers = [self.command_buffer];
        let signal_semaphores = [render_finished];
        let submit_info = vk::SubmitInfo::default()
            .wait_semaphores(&wait_semaphores)
            .wait_dst_stage_mask(&wait_stages)
            .command_buffers(&command_buffers)
            .signal_semaphores(&signal_semaphores);
        unsafe {
            self.device
                .reset_fences(&[self.frame_fence])
                .map_err(BackendError::ResetFrame)?;
            self.device
                .queue_submit(graphics_queue, &[submit_info], self.frame_fence)
                .map_err(BackendError::SubmitFrame)?;
        }
        self.last_submitted_frame_sequence = Some(submitted_frame_sequence);
        self.pending_frame_sequence = (self.timestamp_query_pool != vk::QueryPool::null())
            .then_some(submitted_frame_sequence);

        let swapchains = [self.swapchain];
        let image_indices = [image_index];
        let present_info = vk::PresentInfoKHR::default()
            .wait_semaphores(&signal_semaphores)
            .swapchains(&swapchains)
            .image_indices(&image_indices);
        let present_suboptimal = match unsafe {
            self.swapchain_loader
                .queue_present(presentation_queue, &present_info)
        } {
            Ok(suboptimal) => suboptimal,
            Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => true,
            Err(error) => return Err(BackendError::PresentFrame(error)),
        };
        if acquire_suboptimal || present_suboptimal {
            Ok(PresentationOutcome::Invalidated)
        } else {
            Ok(PresentationOutcome::Presented)
        }
    }

    fn record_commands(
        &self,
        path: &mut dyn RenderPath,
        image_index: u32,
        frame_sequence: u64,
    ) -> Result<(), BackendError> {
        let target_index = usize::try_from(image_index)
            .map_err(|_| BackendError::RecordCommands(vk::Result::ERROR_UNKNOWN))?;
        let image = self
            .images
            .get(target_index)
            .copied()
            .ok_or(BackendError::RecordCommands(vk::Result::ERROR_UNKNOWN))?;
        let begin_info = vk::CommandBufferBeginInfo::default()
            .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
        unsafe {
            self.device
                .begin_command_buffer(self.command_buffer, &begin_info)
                .map_err(BackendError::RecordCommands)?;
            if self.timestamp_query_pool != vk::QueryPool::null() {
                self.device.cmd_reset_query_pool(
                    self.command_buffer,
                    self.timestamp_query_pool,
                    0,
                    2,
                );
                self.device.cmd_write_timestamp(
                    self.command_buffer,
                    vk::PipelineStageFlags::TOP_OF_PIPE,
                    self.timestamp_query_pool,
                    0,
                );
            }
        }
        run_render_path_phase(RenderPathPhase::Record, || {
            path.record(RenderPathFrameContext {
                device: &self.device,
                command_buffer: self.command_buffer,
                target: RenderPathFrameTarget {
                    frame_sequence,
                    configuration_id: self.configuration_id,
                    attachment: RenderPathAttachment {
                        identity: RenderPathAttachmentIdentity(target_index),
                        _image: image.image,
                        view: image.view,
                        lifetime: PhantomData,
                    },
                    format: self.format,
                    extent: self.extent,
                },
            })
        })?;
        unsafe {
            if self.timestamp_query_pool != vk::QueryPool::null() {
                self.device.cmd_write_timestamp(
                    self.command_buffer,
                    vk::PipelineStageFlags::BOTTOM_OF_PIPE,
                    self.timestamp_query_pool,
                    1,
                );
            }
            self.device
                .end_command_buffer(self.command_buffer)
                .map_err(BackendError::RecordCommands)?;
        }
        Ok(())
    }

    fn collect_timestamp_observation(&mut self) -> Result<(), BackendError> {
        let Some(frame_sequence) = self.pending_frame_sequence else {
            return Ok(());
        };
        let mut timestamps = [0_u64; 2];
        unsafe {
            self.device.get_query_pool_results(
                self.timestamp_query_pool,
                0,
                &mut timestamps,
                vk::QueryResultFlags::TYPE_64 | vk::QueryResultFlags::WAIT,
            )
        }
        .map_err(BackendError::ReadTimestampQueries)?;
        let observation = FrameObservation::from_gpu_timestamps(
            frame_sequence,
            timestamps[0],
            timestamps[1],
            self.timestamp_valid_bits,
            self.timestamp_period_nanoseconds,
        )?;
        self.frame_observations.publish(observation)?;
        self.pending_frame_sequence = None;
        Ok(())
    }

    pub(super) fn take_frame_observation(&mut self) -> Option<FrameObservation> {
        self.frame_observations.take()
    }

    pub(super) fn render_path_target(&self) -> RenderPathTarget<'_> {
        RenderPathTarget {
            configuration_id: self.configuration_id,
            format: self.format,
            extent: self.extent,
            images: &self.images,
        }
    }
}

impl Drop for PresentationResources {
    fn drop(&mut self) {
        unsafe {
            if self.timestamp_query_pool != vk::QueryPool::null() {
                self.device
                    .destroy_query_pool(self.timestamp_query_pool, None);
            }
            if self.frame_fence != vk::Fence::null() {
                self.device.destroy_fence(self.frame_fence, None);
            }
            for render_finished in &self.render_finished {
                self.device.destroy_semaphore(*render_finished, None);
            }
            if self.image_available != vk::Semaphore::null() {
                self.device.destroy_semaphore(self.image_available, None);
            }
            if self.command_pool != vk::CommandPool::null() {
                self.device.destroy_command_pool(self.command_pool, None);
            }
            for image_view in &self.image_views {
                self.device.destroy_image_view(*image_view, None);
            }
            if self.swapchain != vk::SwapchainKHR::null() {
                self.swapchain_loader
                    .destroy_swapchain(self.swapchain, None);
            }
        }
    }
}

pub fn select_swapchain_configuration(
    support: &SurfaceSupport,
    drawable_extent: vk::Extent2D,
) -> Result<SwapchainConfigurationState, SwapchainConfigurationError> {
    select_swapchain_configuration_for_mode(support, drawable_extent, true)
}

fn select_swapchain_configuration_for_mode(
    support: &SurfaceSupport,
    drawable_extent: vk::Extent2D,
    presentation_throttling_enabled: bool,
) -> Result<SwapchainConfigurationState, SwapchainConfigurationError> {
    if drawable_extent_is_zero(drawable_extent) {
        return Ok(SwapchainConfigurationState::Suspended);
    }
    let surface_format = support
        .formats
        .iter()
        .copied()
        .find(|format| {
            format.format == vk::Format::B8G8R8A8_SRGB
                && format.color_space == vk::ColorSpaceKHR::SRGB_NONLINEAR
        })
        .or_else(|| support.formats.first().copied())
        .ok_or(SwapchainConfigurationError::NoSurfaceFormats)?;
    let required_present_mode = if presentation_throttling_enabled {
        vk::PresentModeKHR::FIFO
    } else {
        vk::PresentModeKHR::IMMEDIATE
    };
    let present_mode = support
        .present_modes
        .iter()
        .copied()
        .find(|mode| *mode == required_present_mode)
        .ok_or({
            if presentation_throttling_enabled {
                SwapchainConfigurationError::NoPresentModes
            } else {
                SwapchainConfigurationError::ImmediatePresentationUnavailable
            }
        })?;
    let extent = if support.capabilities.current_extent.width != u32::MAX {
        support.capabilities.current_extent
    } else {
        vk::Extent2D {
            width: drawable_extent.width.clamp(
                support.capabilities.min_image_extent.width,
                support.capabilities.max_image_extent.width,
            ),
            height: drawable_extent.height.clamp(
                support.capabilities.min_image_extent.height,
                support.capabilities.max_image_extent.height,
            ),
        }
    };
    let preferred_image_count = support.capabilities.min_image_count.saturating_add(1);
    let image_count = if support.capabilities.max_image_count == 0 {
        preferred_image_count
    } else {
        preferred_image_count.min(support.capabilities.max_image_count)
    };
    let composite_alpha = [
        vk::CompositeAlphaFlagsKHR::OPAQUE,
        vk::CompositeAlphaFlagsKHR::PRE_MULTIPLIED,
        vk::CompositeAlphaFlagsKHR::POST_MULTIPLIED,
        vk::CompositeAlphaFlagsKHR::INHERIT,
    ]
    .into_iter()
    .find(|mode| {
        support
            .capabilities
            .supported_composite_alpha
            .contains(*mode)
    })
    .ok_or(SwapchainConfigurationError::NoCompositeAlphaMode)?;
    let configuration = SwapchainConfiguration {
        extent,
        image_count,
        format: surface_format.format,
        color_space: surface_format.color_space,
        present_mode,
        composite_alpha,
        pre_transform: support.capabilities.current_transform,
    };
    if drawable_extent_is_zero(configuration.extent) {
        Ok(SwapchainConfigurationState::Suspended)
    } else {
        Ok(SwapchainConfigurationState::Ready(configuration))
    }
}

pub(super) fn drawable_extent_is_zero(extent: vk::Extent2D) -> bool {
    extent.width == 0 || extent.height == 0
}

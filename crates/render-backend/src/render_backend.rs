use ash::{Entry, Instance, vk};
use std::ffi::{CStr, CString, c_char, c_void};
use std::fmt;
use std::io::Cursor;
use std::ops::Deref;
use std::sync::atomic::{AtomicUsize, Ordering};
use thiserror::Error;

const VALIDATION_LAYER_NAME: &CStr = c"VK_LAYER_KHRONOS_validation";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeContext {
    pub device_name: String,
    pub driver_version: u32,
    pub api_version: u32,
}

impl fmt::Display for RuntimeContext {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "Vulkan device: {}\nDriver version: {} ({:#010x})\nVulkan API version: {}.{}.{}\nVulkan validation: enabled",
            self.device_name,
            self.driver_version,
            self.driver_version,
            vk::api_version_major(self.api_version),
            vk::api_version_minor(self.api_version),
            vk::api_version_patch(self.api_version)
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueueFamilyCapabilities {
    pub supports_graphics: bool,
    pub supports_presentation: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceCandidate {
    pub name: String,
    pub api_version: u32,
    pub driver_version: u32,
    pub supports_swapchain: bool,
    pub has_surface_formats: bool,
    pub has_present_modes: bool,
    pub queue_families: Vec<QueueFamilyCapabilities>,
}

#[derive(Clone, Debug)]
pub struct SurfaceSupport {
    pub capabilities: vk::SurfaceCapabilitiesKHR,
    pub formats: Vec<vk::SurfaceFormatKHR>,
    pub present_modes: Vec<vk::PresentModeKHR>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SwapchainConfiguration {
    pub extent: vk::Extent2D,
    pub image_count: u32,
    pub format: vk::Format,
    pub color_space: vk::ColorSpaceKHR,
    pub present_mode: vk::PresentModeKHR,
    pub composite_alpha: vk::CompositeAlphaFlagsKHR,
    pub pre_transform: vk::SurfaceTransformFlagsKHR,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SwapchainConfigurationState {
    Suspended,
    Ready(SwapchainConfiguration),
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum DeviceSelectionError {
    #[error("no Vulkan 1.3 device with presentation support is available")]
    NoSuitableDevice,
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum SwapchainConfigurationError {
    #[error("the presentation surface reported no usable formats")]
    NoSurfaceFormats,
    #[error("the presentation surface reported no usable presentation modes")]
    NoPresentModes,
    #[error("the presentation surface reported no usable composite-alpha mode")]
    NoCompositeAlphaMode,
}

#[derive(Debug, Error)]
pub enum BackendError {
    #[error("could not load the Vulkan loader: {0}")]
    LoadVulkan(#[from] ash::LoadingError),
    #[error("the Vulkan loader supports API {major}.{minor}, but Vulkan 1.3 is required")]
    VulkanLoaderTooOld { major: u32, minor: u32 },
    #[error("could not query the Vulkan loader API version: {0}")]
    QueryVulkanLoaderVersion(vk::Result),
    #[error("could not enumerate Vulkan instance layers: {0}")]
    EnumerateInstanceLayers(vk::Result),
    #[error("Vulkan validation is required, but VK_LAYER_KHRONOS_validation is unavailable")]
    ValidationLayerUnavailable,
    #[error("the platform adapter could not prepare Vulkan presentation: {0}")]
    PlatformAdapter(String),
    #[error("could not create the Vulkan instance: {0}")]
    CreateInstance(vk::Result),
    #[error("could not create the Vulkan validation messenger: {0}")]
    CreateValidationMessenger(vk::Result),
    #[error("could not enumerate Vulkan physical devices: {0}")]
    EnumeratePhysicalDevices(vk::Result),
    #[error("could not inspect Vulkan device presentation support: {0}")]
    InspectPresentationSupport(vk::Result),
    #[error(transparent)]
    SelectDevice(#[from] DeviceSelectionError),
    #[error("could not create the Vulkan logical device: {0}")]
    CreateDevice(vk::Result),
    #[error(transparent)]
    ConfigureSwapchain(#[from] SwapchainConfigurationError),
    #[error("could not create the Vulkan swapchain: {0}")]
    CreateSwapchain(vk::Result),
    #[error("could not obtain the Vulkan swapchain images: {0}")]
    GetSwapchainImages(vk::Result),
    #[error("could not create a Vulkan image view: {0}")]
    CreateImageView(vk::Result),
    #[error("could not create the Vulkan render pass: {0}")]
    CreateRenderPass(vk::Result),
    #[error("could not read a reproducibly built shader artifact: {0}")]
    ReadShaderArtifact(#[from] std::io::Error),
    #[error("could not create a Vulkan shader module: {0}")]
    CreateShaderModule(vk::Result),
    #[error("could not create the Vulkan graphics pipeline layout: {0}")]
    CreatePipelineLayout(vk::Result),
    #[error("could not create the Vulkan triangle graphics pipeline: {0}")]
    CreateGraphicsPipeline(vk::Result),
    #[error("could not create a Vulkan framebuffer: {0}")]
    CreateFramebuffer(vk::Result),
    #[error("could not create the Vulkan command pool: {0}")]
    CreateCommandPool(vk::Result),
    #[error("could not allocate a Vulkan command buffer: {0}")]
    AllocateCommandBuffer(vk::Result),
    #[error("could not create Vulkan frame synchronization: {0}")]
    CreateFrameSynchronization(vk::Result),
    #[error("could not wait for the previous Vulkan frame: {0}")]
    WaitForFrame(vk::Result),
    #[error("could not wait for the Vulkan device before rebuilding presentation: {0}")]
    WaitForDevice(vk::Result),
    #[error("could not acquire the next Vulkan presentation image: {0}")]
    AcquireSwapchainImage(vk::Result),
    #[error("could not reset Vulkan frame synchronization: {0}")]
    ResetFrame(vk::Result),
    #[error("could not record the Vulkan triangle commands: {0}")]
    RecordCommands(vk::Result),
    #[error("could not submit the Vulkan triangle commands: {0}")]
    SubmitFrame(vk::Result),
    #[error("could not present the Vulkan triangle frame: {0}")]
    PresentFrame(vk::Result),
    #[error("Vulkan validation reported {count} error(s) during presentation")]
    ValidationErrors { count: usize },
}

/// Supplies the platform-owned Vulkan instance extensions and presentation surface.
///
/// # Safety
///
/// Implementations must return a surface created from the supplied instance, and the
/// platform window behind that surface must outlive the Render Backend.
pub unsafe trait PresentationAdapter {
    fn required_instance_extensions(&self) -> Result<Vec<CString>, String>;

    /// # Safety
    ///
    /// The returned surface must belong to `instance` and the platform window must
    /// outlive the Render Backend that owns it.
    unsafe fn create_surface(
        &self,
        entry: &Entry,
        instance: &Instance,
    ) -> Result<vk::SurfaceKHR, String>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrameOutcome {
    Presented,
    RedrawNeeded,
    Suspended,
}

pub struct RenderBackend {
    rendering: Option<RenderingResources>,
    device: LogicalDevice,
    presentation: InstanceSurface,
    selected_device: InspectedDevice,
    graphics_queue: vk::Queue,
    presentation_queue: vk::Queue,
    runtime_context: RuntimeContext,
    drawable_extent: vk::Extent2D,
    swapchain_needs_recreation: bool,
}

impl RenderBackend {
    pub fn initialize(
        application_name: &CStr,
        adapter: &impl PresentationAdapter,
        initial_drawable_extent: vk::Extent2D,
    ) -> Result<Self, BackendError> {
        let entry = unsafe { Entry::load()? };
        require_vulkan_1_3_loader(&entry)?;
        require_validation_layer(&entry)?;
        let presentation = create_instance_surface(entry, application_name, adapter)?;

        let inspected_devices = inspect_devices(&presentation)?;
        let selected_device = inspected_devices
            .into_iter()
            .find(|inspected_device| inspected_device.candidate.is_suitable())
            .ok_or(DeviceSelectionError::NoSuitableDevice)?;
        let device = LogicalDevice::new(&presentation.instance, &selected_device)?;
        let graphics_queue =
            unsafe { device.get_device_queue(selected_device.graphics_queue_family_index, 0) };
        let presentation_queue =
            unsafe { device.get_device_queue(selected_device.presentation_queue_family_index, 0) };
        let rendering = RenderingResources::new(
            &presentation,
            &device,
            &selected_device,
            initial_drawable_extent,
            vk::SwapchainKHR::null(),
        )?;
        let runtime_context = RuntimeContext {
            device_name: selected_device.candidate.name.clone(),
            driver_version: selected_device.candidate.driver_version,
            api_version: selected_device.candidate.api_version,
        };

        Ok(Self {
            rendering,
            device,
            presentation,
            selected_device,
            graphics_queue,
            presentation_queue,
            runtime_context,
            drawable_extent: initial_drawable_extent,
            swapchain_needs_recreation: false,
        })
    }

    pub fn runtime_context(&self) -> &RuntimeContext {
        &self.runtime_context
    }

    pub fn validation_error_count(&self) -> usize {
        self.presentation.validation_diagnostics.error_count()
    }

    pub fn set_drawable_extent(&mut self, drawable_extent: vk::Extent2D) {
        self.drawable_extent = drawable_extent;
        self.swapchain_needs_recreation = true;
    }

    pub fn draw_frame(&mut self) -> Result<FrameOutcome, BackendError> {
        if drawable_extent_is_zero(self.drawable_extent) {
            return self.ensure_validation_clean(FrameOutcome::Suspended);
        }
        if self.swapchain_needs_recreation || self.rendering.is_none() {
            self.recreate_swapchain()?;
        }
        let Some(rendering) = &mut self.rendering else {
            return self.ensure_validation_clean(FrameOutcome::Suspended);
        };
        let outcome = match rendering.draw_frame(self.graphics_queue, self.presentation_queue)? {
            PresentationOutcome::Presented => FrameOutcome::Presented,
            PresentationOutcome::Invalidated => {
                self.swapchain_needs_recreation = true;
                FrameOutcome::RedrawNeeded
            }
        };
        self.ensure_validation_clean(outcome)
    }

    fn recreate_swapchain(&mut self) -> Result<(), BackendError> {
        unsafe { self.device.device_wait_idle() }.map_err(BackendError::WaitForDevice)?;
        let old_swapchain = self
            .rendering
            .as_ref()
            .map_or(vk::SwapchainKHR::null(), |rendering| rendering.swapchain);
        let rendering = RenderingResources::new(
            &self.presentation,
            &self.device,
            &self.selected_device,
            self.drawable_extent,
            old_swapchain,
        )?;
        self.rendering = rendering;
        self.swapchain_needs_recreation = false;
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
        }
    }
}

struct LogicalDevice(ash::Device);

impl LogicalDevice {
    fn new(instance: &Instance, selected_device: &InspectedDevice) -> Result<Self, BackendError> {
        let mut queue_family_indices = vec![selected_device.graphics_queue_family_index];
        if selected_device.presentation_queue_family_index
            != selected_device.graphics_queue_family_index
        {
            queue_family_indices.push(selected_device.presentation_queue_family_index);
        }
        let queue_priorities = [1.0];
        let queue_create_infos: Vec<_> = queue_family_indices
            .iter()
            .map(|queue_family_index| {
                vk::DeviceQueueCreateInfo::default()
                    .queue_family_index(*queue_family_index)
                    .queue_priorities(&queue_priorities)
            })
            .collect();
        let extension_names = [ash::khr::swapchain::NAME.as_ptr()];
        let device_create_info = vk::DeviceCreateInfo::default()
            .queue_create_infos(&queue_create_infos)
            .enabled_extension_names(&extension_names);
        let device = unsafe {
            instance.create_device(selected_device.physical_device, &device_create_info, None)
        }
        .map_err(BackendError::CreateDevice)?;
        Ok(Self(device))
    }
}

impl Deref for LogicalDevice {
    type Target = ash::Device;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Drop for LogicalDevice {
    fn drop(&mut self) {
        unsafe { self.0.destroy_device(None) };
    }
}

struct InstanceSurface {
    _entry: Entry,
    instance: Instance,
    debug_loader: ash::ext::debug_utils::Instance,
    debug_messenger: vk::DebugUtilsMessengerEXT,
    validation_diagnostics: Box<ValidationDiagnostics>,
    surface_loader: ash::khr::surface::Instance,
    surface: vk::SurfaceKHR,
}

impl Drop for InstanceSurface {
    fn drop(&mut self) {
        unsafe {
            self.surface_loader.destroy_surface(self.surface, None);
            self.debug_loader
                .destroy_debug_utils_messenger(self.debug_messenger, None);
            self.instance.destroy_instance(None);
        }
    }
}

#[derive(Clone)]
struct InspectedDevice {
    physical_device: vk::PhysicalDevice,
    candidate: DeviceCandidate,
    graphics_queue_family_index: u32,
    presentation_queue_family_index: u32,
}

#[derive(Default)]
struct ValidationDiagnostics {
    errors: AtomicUsize,
}

impl ValidationDiagnostics {
    fn error_count(&self) -> usize {
        self.errors.load(Ordering::SeqCst)
    }
}

struct RenderingResources {
    device: ash::Device,
    swapchain_loader: ash::khr::swapchain::Device,
    swapchain: vk::SwapchainKHR,
    image_views: Vec<vk::ImageView>,
    render_pass: vk::RenderPass,
    pipeline_layout: vk::PipelineLayout,
    pipeline: vk::Pipeline,
    framebuffers: Vec<vk::Framebuffer>,
    command_pool: vk::CommandPool,
    command_buffer: vk::CommandBuffer,
    image_available: vk::Semaphore,
    render_finished: Vec<vk::Semaphore>,
    frame_fence: vk::Fence,
    extent: vk::Extent2D,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PresentationOutcome {
    Presented,
    Invalidated,
}

impl RenderingResources {
    fn new(
        presentation: &InstanceSurface,
        device: &ash::Device,
        selected_device: &InspectedDevice,
        initial_drawable_extent: vk::Extent2D,
        old_swapchain: vk::SwapchainKHR,
    ) -> Result<Option<Self>, BackendError> {
        let surface_support = query_surface_support(presentation, selected_device.physical_device)?;
        let configuration =
            select_swapchain_configuration(&surface_support, initial_drawable_extent)?;
        let SwapchainConfigurationState::Ready(configuration) = configuration else {
            return Ok(None);
        };
        let swapchain_loader = ash::khr::swapchain::Device::new(&presentation.instance, device);
        let mut resources = Self {
            device: device.clone(),
            swapchain_loader,
            swapchain: vk::SwapchainKHR::null(),
            image_views: Vec::new(),
            render_pass: vk::RenderPass::null(),
            pipeline_layout: vk::PipelineLayout::null(),
            pipeline: vk::Pipeline::null(),
            framebuffers: Vec::new(),
            command_pool: vk::CommandPool::null(),
            command_buffer: vk::CommandBuffer::null(),
            image_available: vk::Semaphore::null(),
            render_finished: Vec::new(),
            frame_fence: vk::Fence::null(),
            extent: vk::Extent2D::default(),
        };

        resources.extent = configuration.extent;
        resources.create_swapchain(presentation, selected_device, &configuration, old_swapchain)?;
        let images = unsafe {
            resources
                .swapchain_loader
                .get_swapchain_images(resources.swapchain)
        }
        .map_err(BackendError::GetSwapchainImages)?;
        resources.create_image_views(&images, configuration.format)?;
        resources.create_render_pass(configuration.format)?;
        resources.create_graphics_pipeline()?;
        resources.create_framebuffers()?;
        resources.create_commands(selected_device.graphics_queue_family_index)?;
        resources.create_synchronization()?;
        Ok(Some(resources))
    }

    fn create_swapchain(
        &mut self,
        presentation: &InstanceSurface,
        selected_device: &InspectedDevice,
        configuration: &SwapchainConfiguration,
        old_swapchain: vk::SwapchainKHR,
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
            .old_swapchain(old_swapchain)
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
            self.image_views.push(image_view);
        }
        Ok(())
    }

    fn create_render_pass(&mut self, format: vk::Format) -> Result<(), BackendError> {
        let attachment = vk::AttachmentDescription::default()
            .format(format)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::STORE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .final_layout(vk::ImageLayout::PRESENT_SRC_KHR);
        let color_attachment = vk::AttachmentReference::default()
            .attachment(0)
            .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);
        let color_attachments = [color_attachment];
        let subpass = vk::SubpassDescription::default()
            .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
            .color_attachments(&color_attachments);
        let dependency = vk::SubpassDependency::default()
            .src_subpass(vk::SUBPASS_EXTERNAL)
            .dst_subpass(0)
            .src_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
            .dst_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
            .dst_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE);
        let attachments = [attachment];
        let subpasses = [subpass];
        let dependencies = [dependency];
        let create_info = vk::RenderPassCreateInfo::default()
            .attachments(&attachments)
            .subpasses(&subpasses)
            .dependencies(&dependencies);
        self.render_pass = unsafe { self.device.create_render_pass(&create_info, None) }
            .map_err(BackendError::CreateRenderPass)?;
        Ok(())
    }

    fn create_graphics_pipeline(&mut self) -> Result<(), BackendError> {
        let vertex_code = ash::util::read_spv(&mut Cursor::new(include_bytes!(concat!(
            env!("OUT_DIR"),
            "/triangle.vert.spv"
        ))))?;
        let fragment_code = ash::util::read_spv(&mut Cursor::new(include_bytes!(concat!(
            env!("OUT_DIR"),
            "/triangle.frag.spv"
        ))))?;
        let vertex_module = create_shader_module(&self.device, &vertex_code)?;
        let fragment_module = match create_shader_module(&self.device, &fragment_code) {
            Ok(module) => module,
            Err(error) => {
                unsafe { self.device.destroy_shader_module(vertex_module, None) };
                return Err(error);
            }
        };
        let result = self.create_graphics_pipeline_with_modules(vertex_module, fragment_module);
        unsafe {
            self.device.destroy_shader_module(fragment_module, None);
            self.device.destroy_shader_module(vertex_module, None);
        }
        result
    }

    fn create_graphics_pipeline_with_modules(
        &mut self,
        vertex_module: vk::ShaderModule,
        fragment_module: vk::ShaderModule,
    ) -> Result<(), BackendError> {
        let vertex_stage = vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::VERTEX)
            .module(vertex_module)
            .name(c"main");
        let fragment_stage = vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::FRAGMENT)
            .module(fragment_module)
            .name(c"main");
        let shader_stages = [vertex_stage, fragment_stage];
        let vertex_input = vk::PipelineVertexInputStateCreateInfo::default();
        let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
        let viewport = vk::Viewport {
            x: 0.0,
            y: 0.0,
            width: self.extent.width as f32,
            height: self.extent.height as f32,
            min_depth: 0.0,
            max_depth: 1.0,
        };
        let scissor = vk::Rect2D {
            offset: vk::Offset2D::default(),
            extent: self.extent,
        };
        let viewports = [viewport];
        let scissors = [scissor];
        let viewport_state = vk::PipelineViewportStateCreateInfo::default()
            .viewports(&viewports)
            .scissors(&scissors);
        let rasterization = vk::PipelineRasterizationStateCreateInfo::default()
            .polygon_mode(vk::PolygonMode::FILL)
            .cull_mode(vk::CullModeFlags::NONE)
            .front_face(vk::FrontFace::CLOCKWISE)
            .line_width(1.0);
        let multisample = vk::PipelineMultisampleStateCreateInfo::default()
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);
        let color_blend_attachment = vk::PipelineColorBlendAttachmentState::default()
            .color_write_mask(vk::ColorComponentFlags::RGBA);
        let color_blend_attachments = [color_blend_attachment];
        let color_blend =
            vk::PipelineColorBlendStateCreateInfo::default().attachments(&color_blend_attachments);
        self.pipeline_layout = unsafe {
            self.device
                .create_pipeline_layout(&vk::PipelineLayoutCreateInfo::default(), None)
        }
        .map_err(BackendError::CreatePipelineLayout)?;
        let pipeline_info = vk::GraphicsPipelineCreateInfo::default()
            .stages(&shader_stages)
            .vertex_input_state(&vertex_input)
            .input_assembly_state(&input_assembly)
            .viewport_state(&viewport_state)
            .rasterization_state(&rasterization)
            .multisample_state(&multisample)
            .color_blend_state(&color_blend)
            .layout(self.pipeline_layout)
            .render_pass(self.render_pass)
            .subpass(0);
        match unsafe {
            self.device
                .create_graphics_pipelines(vk::PipelineCache::null(), &[pipeline_info], None)
        } {
            Ok(mut pipelines) => {
                self.pipeline = pipelines.pop().ok_or(BackendError::CreateGraphicsPipeline(
                    vk::Result::ERROR_UNKNOWN,
                ))?;
                Ok(())
            }
            Err((pipelines, error)) => {
                for pipeline in pipelines {
                    unsafe { self.device.destroy_pipeline(pipeline, None) };
                }
                Err(BackendError::CreateGraphicsPipeline(error))
            }
        }
    }

    fn create_framebuffers(&mut self) -> Result<(), BackendError> {
        for image_view in &self.image_views {
            let attachments = [*image_view];
            let create_info = vk::FramebufferCreateInfo::default()
                .render_pass(self.render_pass)
                .attachments(&attachments)
                .width(self.extent.width)
                .height(self.extent.height)
                .layers(1);
            let framebuffer = unsafe { self.device.create_framebuffer(&create_info, None) }
                .map_err(BackendError::CreateFramebuffer)?;
            self.framebuffers.push(framebuffer);
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
        for _ in &self.framebuffers {
            let render_finished = unsafe { self.device.create_semaphore(&semaphore_info, None) }
                .map_err(BackendError::CreateFrameSynchronization)?;
            self.render_finished.push(render_finished);
        }
        let fence_info = vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED);
        self.frame_fence = unsafe { self.device.create_fence(&fence_info, None) }
            .map_err(BackendError::CreateFrameSynchronization)?;
        Ok(())
    }

    fn draw_frame(
        &mut self,
        graphics_queue: vk::Queue,
        presentation_queue: vk::Queue,
    ) -> Result<PresentationOutcome, BackendError> {
        unsafe {
            self.device
                .wait_for_fences(&[self.frame_fence], true, u64::MAX)
                .map_err(BackendError::WaitForFrame)?;
        }
        let (image_index, acquire_suboptimal) = match unsafe {
            self.swapchain_loader.acquire_next_image(
                self.swapchain,
                u64::MAX,
                self.image_available,
                vk::Fence::null(),
            )
        } {
            Ok(acquired_image) => acquired_image,
            Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
                return Ok(PresentationOutcome::Invalidated);
            }
            Err(error) => return Err(BackendError::AcquireSwapchainImage(error)),
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
                .reset_fences(&[self.frame_fence])
                .map_err(BackendError::ResetFrame)?;
            self.device
                .reset_command_buffer(self.command_buffer, vk::CommandBufferResetFlags::empty())
                .map_err(BackendError::ResetFrame)?;
        }
        self.record_commands(image_index)?;

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
                .queue_submit(graphics_queue, &[submit_info], self.frame_fence)
                .map_err(BackendError::SubmitFrame)?;
        }

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

    fn record_commands(&self, image_index: u32) -> Result<(), BackendError> {
        let framebuffer_index = usize::try_from(image_index)
            .map_err(|_| BackendError::RecordCommands(vk::Result::ERROR_UNKNOWN))?;
        let framebuffer = self
            .framebuffers
            .get(framebuffer_index)
            .copied()
            .ok_or(BackendError::RecordCommands(vk::Result::ERROR_UNKNOWN))?;
        let begin_info = vk::CommandBufferBeginInfo::default()
            .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
        unsafe {
            self.device
                .begin_command_buffer(self.command_buffer, &begin_info)
                .map_err(BackendError::RecordCommands)?;
        }
        let clear_values = [vk::ClearValue {
            color: vk::ClearColorValue {
                float32: [0.03, 0.04, 0.08, 1.0],
            },
        }];
        let render_pass_info = vk::RenderPassBeginInfo::default()
            .render_pass(self.render_pass)
            .framebuffer(framebuffer)
            .render_area(vk::Rect2D {
                offset: vk::Offset2D::default(),
                extent: self.extent,
            })
            .clear_values(&clear_values);
        unsafe {
            self.device.cmd_begin_render_pass(
                self.command_buffer,
                &render_pass_info,
                vk::SubpassContents::INLINE,
            );
            self.device.cmd_bind_pipeline(
                self.command_buffer,
                vk::PipelineBindPoint::GRAPHICS,
                self.pipeline,
            );
            self.device.cmd_draw(self.command_buffer, 3, 1, 0, 0);
            self.device.cmd_end_render_pass(self.command_buffer);
            self.device
                .end_command_buffer(self.command_buffer)
                .map_err(BackendError::RecordCommands)?;
        }
        Ok(())
    }
}

impl Drop for RenderingResources {
    fn drop(&mut self) {
        unsafe {
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
            for framebuffer in &self.framebuffers {
                self.device.destroy_framebuffer(*framebuffer, None);
            }
            if self.pipeline != vk::Pipeline::null() {
                self.device.destroy_pipeline(self.pipeline, None);
            }
            if self.pipeline_layout != vk::PipelineLayout::null() {
                self.device
                    .destroy_pipeline_layout(self.pipeline_layout, None);
            }
            if self.render_pass != vk::RenderPass::null() {
                self.device.destroy_render_pass(self.render_pass, None);
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

fn create_instance_surface(
    entry: Entry,
    application_name: &CStr,
    adapter: &impl PresentationAdapter,
) -> Result<InstanceSurface, BackendError> {
    let extension_names = adapter
        .required_instance_extensions()
        .map_err(BackendError::PlatformAdapter)?;
    let mut extension_name_pointers: Vec<*const c_char> = extension_names
        .iter()
        .map(|extension_name| extension_name.as_ptr())
        .collect();
    extension_name_pointers.push(ash::ext::debug_utils::NAME.as_ptr());
    let layer_names = [VALIDATION_LAYER_NAME.as_ptr()];
    let application_info = vk::ApplicationInfo::default()
        .application_name(application_name)
        .application_version(0)
        .engine_name(c"Voxel Nexus")
        .engine_version(0)
        .api_version(vk::API_VERSION_1_3);
    let instance_create_info = vk::InstanceCreateInfo::default()
        .application_info(&application_info)
        .enabled_extension_names(&extension_name_pointers)
        .enabled_layer_names(&layer_names);
    let instance = unsafe { entry.create_instance(&instance_create_info, None) }
        .map_err(BackendError::CreateInstance)?;
    let debug_loader = ash::ext::debug_utils::Instance::new(&entry, &instance);
    let validation_diagnostics = Box::new(ValidationDiagnostics::default());
    let validation_diagnostics_pointer = (&raw const *validation_diagnostics)
        .cast_mut()
        .cast::<c_void>();
    let messenger_info = validation_messenger_create_info(validation_diagnostics_pointer);
    let debug_messenger =
        match unsafe { debug_loader.create_debug_utils_messenger(&messenger_info, None) } {
            Ok(messenger) => messenger,
            Err(error) => {
                unsafe { instance.destroy_instance(None) };
                return Err(BackendError::CreateValidationMessenger(error));
            }
        };
    let surface = match unsafe { adapter.create_surface(&entry, &instance) } {
        Ok(surface) => surface,
        Err(error) => {
            unsafe {
                debug_loader.destroy_debug_utils_messenger(debug_messenger, None);
                instance.destroy_instance(None);
            }
            return Err(BackendError::PlatformAdapter(error));
        }
    };
    let surface_loader = ash::khr::surface::Instance::new(&entry, &instance);
    Ok(InstanceSurface {
        _entry: entry,
        instance,
        debug_loader,
        debug_messenger,
        validation_diagnostics,
        surface_loader,
        surface,
    })
}

fn validation_messenger_create_info(
    validation_diagnostics: *mut c_void,
) -> vk::DebugUtilsMessengerCreateInfoEXT<'static> {
    vk::DebugUtilsMessengerCreateInfoEXT::default()
        .message_severity(
            vk::DebugUtilsMessageSeverityFlagsEXT::WARNING
                | vk::DebugUtilsMessageSeverityFlagsEXT::ERROR,
        )
        .message_type(
            vk::DebugUtilsMessageTypeFlagsEXT::GENERAL
                | vk::DebugUtilsMessageTypeFlagsEXT::VALIDATION
                | vk::DebugUtilsMessageTypeFlagsEXT::PERFORMANCE,
        )
        .pfn_user_callback(Some(validation_callback))
        .user_data(validation_diagnostics)
}

unsafe extern "system" fn validation_callback(
    severity: vk::DebugUtilsMessageSeverityFlagsEXT,
    message_type: vk::DebugUtilsMessageTypeFlagsEXT,
    callback_data: *const vk::DebugUtilsMessengerCallbackDataEXT<'_>,
    user_data: *mut c_void,
) -> vk::Bool32 {
    if severity.contains(vk::DebugUtilsMessageSeverityFlagsEXT::ERROR) && !user_data.is_null() {
        let diagnostics = unsafe { &*user_data.cast::<ValidationDiagnostics>() };
        diagnostics.errors.fetch_add(1, Ordering::SeqCst);
    }
    let message = if callback_data.is_null() {
        c"validation callback supplied no diagnostic data"
    } else {
        let message_pointer = unsafe { (*callback_data).p_message };
        if message_pointer.is_null() {
            c"validation callback supplied no diagnostic message"
        } else {
            unsafe { CStr::from_ptr(message_pointer) }
        }
    };
    eprintln!("Vulkan validation {severity:?} {message_type:?}: {message:?}");
    vk::FALSE
}

fn require_vulkan_1_3_loader(entry: &Entry) -> Result<(), BackendError> {
    let loader_version = unsafe { entry.try_enumerate_instance_version() }
        .map_err(BackendError::QueryVulkanLoaderVersion)?
        .unwrap_or(vk::API_VERSION_1_0);
    if loader_version < vk::API_VERSION_1_3 {
        return Err(BackendError::VulkanLoaderTooOld {
            major: vk::api_version_major(loader_version),
            minor: vk::api_version_minor(loader_version),
        });
    }
    Ok(())
}

fn require_validation_layer(entry: &Entry) -> Result<(), BackendError> {
    let layer_properties = unsafe { entry.enumerate_instance_layer_properties() }
        .map_err(BackendError::EnumerateInstanceLayers)?;
    let available = layer_properties.iter().any(|property| {
        let name = unsafe { CStr::from_ptr(property.layer_name.as_ptr()) };
        name == VALIDATION_LAYER_NAME
    });
    if !available {
        return Err(BackendError::ValidationLayerUnavailable);
    }
    Ok(())
}

fn inspect_devices(presentation: &InstanceSurface) -> Result<Vec<InspectedDevice>, BackendError> {
    let physical_devices = unsafe { presentation.instance.enumerate_physical_devices() }
        .map_err(BackendError::EnumeratePhysicalDevices)?;
    physical_devices
        .into_iter()
        .map(|physical_device| inspect_device(presentation, physical_device))
        .collect()
}

fn inspect_device(
    presentation: &InstanceSurface,
    physical_device: vk::PhysicalDevice,
) -> Result<InspectedDevice, BackendError> {
    let properties = unsafe {
        presentation
            .instance
            .get_physical_device_properties(physical_device)
    };
    let name = unsafe { CStr::from_ptr(properties.device_name.as_ptr()) }
        .to_string_lossy()
        .into_owned();
    let extension_properties = unsafe {
        presentation
            .instance
            .enumerate_device_extension_properties(physical_device)
    }
    .map_err(BackendError::InspectPresentationSupport)?;
    let supports_swapchain = extension_properties.iter().any(|extension| {
        let extension_name = unsafe { CStr::from_ptr(extension.extension_name.as_ptr()) };
        extension_name == ash::khr::swapchain::NAME
    });
    let queue_properties = unsafe {
        presentation
            .instance
            .get_physical_device_queue_family_properties(physical_device)
    };
    let mut queue_families = Vec::with_capacity(queue_properties.len());
    for (queue_family_index, queue_property) in queue_properties.iter().enumerate() {
        let queue_family_index = u32::try_from(queue_family_index)
            .map_err(|_| BackendError::InspectPresentationSupport(vk::Result::ERROR_UNKNOWN))?;
        let supports_presentation = unsafe {
            presentation
                .surface_loader
                .get_physical_device_surface_support(
                    physical_device,
                    queue_family_index,
                    presentation.surface,
                )
        }
        .map_err(BackendError::InspectPresentationSupport)?;
        queue_families.push(QueueFamilyCapabilities {
            supports_graphics: queue_property
                .queue_flags
                .contains(vk::QueueFlags::GRAPHICS),
            supports_presentation,
        });
    }
    let surface_support = query_surface_support(presentation, physical_device)?;
    let graphics_queue_family_index = queue_families
        .iter()
        .position(|queue_family| queue_family.supports_graphics)
        .and_then(|index| u32::try_from(index).ok())
        .unwrap_or(u32::MAX);
    let presentation_queue_family_index = queue_families
        .iter()
        .position(|queue_family| queue_family.supports_presentation)
        .and_then(|index| u32::try_from(index).ok())
        .unwrap_or(u32::MAX);

    Ok(InspectedDevice {
        physical_device,
        candidate: DeviceCandidate {
            name,
            api_version: properties.api_version,
            driver_version: properties.driver_version,
            supports_swapchain,
            has_surface_formats: !surface_support.formats.is_empty(),
            has_present_modes: !surface_support.present_modes.is_empty(),
            queue_families,
        },
        graphics_queue_family_index,
        presentation_queue_family_index,
    })
}

fn query_surface_support(
    presentation: &InstanceSurface,
    physical_device: vk::PhysicalDevice,
) -> Result<SurfaceSupport, BackendError> {
    let capabilities = unsafe {
        presentation
            .surface_loader
            .get_physical_device_surface_capabilities(physical_device, presentation.surface)
    }
    .map_err(BackendError::InspectPresentationSupport)?;
    let formats = unsafe {
        presentation
            .surface_loader
            .get_physical_device_surface_formats(physical_device, presentation.surface)
    }
    .map_err(BackendError::InspectPresentationSupport)?;
    let present_modes = unsafe {
        presentation
            .surface_loader
            .get_physical_device_surface_present_modes(physical_device, presentation.surface)
    }
    .map_err(BackendError::InspectPresentationSupport)?;
    Ok(SurfaceSupport {
        capabilities,
        formats,
        present_modes,
    })
}

fn create_shader_module(
    device: &ash::Device,
    code: &[u32],
) -> Result<vk::ShaderModule, BackendError> {
    let create_info = vk::ShaderModuleCreateInfo::default().code(code);
    unsafe { device.create_shader_module(&create_info, None) }
        .map_err(BackendError::CreateShaderModule)
}

impl DeviceCandidate {
    fn is_suitable(&self) -> bool {
        self.api_version >= vk::API_VERSION_1_3
            && self.supports_swapchain
            && self.has_surface_formats
            && self.has_present_modes
            && self
                .queue_families
                .iter()
                .any(|queue_family| queue_family.supports_graphics)
            && self
                .queue_families
                .iter()
                .any(|queue_family| queue_family.supports_presentation)
    }
}

pub fn select_device(
    candidates: Vec<DeviceCandidate>,
) -> Result<DeviceCandidate, DeviceSelectionError> {
    candidates
        .into_iter()
        .find(DeviceCandidate::is_suitable)
        .ok_or(DeviceSelectionError::NoSuitableDevice)
}

pub fn select_swapchain_configuration(
    support: &SurfaceSupport,
    drawable_extent: vk::Extent2D,
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
    let present_mode = support
        .present_modes
        .iter()
        .copied()
        .find(|mode| *mode == vk::PresentModeKHR::FIFO)
        .ok_or(SwapchainConfigurationError::NoPresentModes)?;
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

fn drawable_extent_is_zero(extent: vk::Extent2D) -> bool {
    extent.width == 0 || extent.height == 0
}

use super::camera::CameraState;
use super::configuration::{BackendError, RenderPathDeviceCapabilities};
use super::render_path_switching::{
    CameraStateRevision, RenderPathSwitchDiagnostics, RenderPathSwitchRequestError,
    SwitchableRenderPath,
};
use ash::{Entry, Instance, vk};
use std::ffi::CString;
use std::fmt;
use std::marker::PhantomData;
use thiserror::Error;

pub fn run_render_path_phase<T>(
    phase: RenderPathPhase,
    operation: impl FnOnce() -> RenderPathResult<T>,
) -> Result<T, BackendError> {
    operation().map_err(|source| BackendError::boxed_render_path_failure(phase, source))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RenderPathPhase {
    Release,
    Configure,
    AdvanceFrameBoundary,
    Record,
    Shutdown,
}

impl fmt::Display for RenderPathPhase {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Release => "release",
            Self::Configure => "configure",
            Self::AdvanceFrameBoundary => "advance frame boundary",
            Self::Record => "record",
            Self::Shutdown => "shutdown",
        })
    }
}

pub type RenderPathResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync + 'static>>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PresentationConfigurationId(pub(super) u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct PresentationImage {
    pub(super) image: vk::Image,
    pub(super) view: vk::ImageView,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RenderPathAttachmentIdentity(pub(super) usize);

pub struct RenderPathAttachment<'target> {
    pub(super) identity: RenderPathAttachmentIdentity,
    pub(super) _image: vk::Image,
    pub(super) view: vk::ImageView,
    pub(super) lifetime: PhantomData<&'target PresentationImage>,
}

impl RenderPathAttachment<'_> {
    pub fn identity(&self) -> RenderPathAttachmentIdentity {
        self.identity
    }
}

#[derive(Clone, Copy)]
pub struct RenderPathTarget<'target> {
    pub(super) configuration_id: PresentationConfigurationId,
    pub(super) format: vk::Format,
    pub(super) extent: vk::Extent2D,
    pub(super) images: &'target [PresentationImage],
}

impl<'target> RenderPathTarget<'target> {
    pub fn configuration_id(&self) -> PresentationConfigurationId {
        self.configuration_id
    }

    pub fn format(&self) -> vk::Format {
        self.format
    }

    pub fn extent(&self) -> vk::Extent2D {
        self.extent
    }

    pub fn attachments(&self) -> impl Iterator<Item = RenderPathAttachment<'_>> {
        self.images
            .iter()
            .enumerate()
            .map(|(index, image)| RenderPathAttachment {
                identity: RenderPathAttachmentIdentity(index),
                _image: image.image,
                view: image.view,
                lifetime: PhantomData,
            })
    }
}

pub struct RenderPathFrameTarget<'frame> {
    pub(super) frame_sequence: u64,
    pub(super) configuration_id: PresentationConfigurationId,
    pub(super) attachment: RenderPathAttachment<'frame>,
    pub(super) format: vk::Format,
    pub(super) extent: vk::Extent2D,
}

impl RenderPathFrameTarget<'_> {
    pub fn frame_sequence(&self) -> u64 {
        self.frame_sequence
    }

    pub fn configuration_id(&self) -> PresentationConfigurationId {
        self.configuration_id
    }

    pub fn attachment(&self) -> &RenderPathAttachment<'_> {
        &self.attachment
    }

    pub fn format(&self) -> vk::Format {
        self.format
    }

    pub fn extent(&self) -> vk::Extent2D {
        self.extent
    }
}

#[derive(Clone, Copy)]
pub struct RenderPathDeviceContext<'device> {
    pub(super) device: &'device ash::Device,
    pub(super) memory_properties: vk::PhysicalDeviceMemoryProperties,
    pub(super) capabilities: RenderPathDeviceCapabilities,
}

impl RenderPathDeviceContext<'_> {
    pub fn capabilities(&self) -> RenderPathDeviceCapabilities {
        self.capabilities
    }

    pub fn memory_type_index(
        &self,
        memory_type_bits: u32,
        required_properties: vk::MemoryPropertyFlags,
    ) -> Option<u32> {
        self.memory_properties
            .memory_types
            .iter()
            .take(usize::try_from(self.memory_properties.memory_type_count).ok()?)
            .enumerate()
            .find(|(index, memory_type)| {
                u32::try_from(*index)
                    .ok()
                    .and_then(|index| 1_u32.checked_shl(index))
                    .is_some_and(|bit| memory_type_bits & bit != 0)
                    && memory_type.property_flags.contains(required_properties)
            })
            .and_then(|(index, _)| u32::try_from(index).ok())
    }

    /// # Safety
    /// Every queue-family index and pointer in `create_info` must be valid for this device.
    pub unsafe fn create_buffer(
        &self,
        create_info: &vk::BufferCreateInfo<'_>,
    ) -> Result<vk::Buffer, vk::Result> {
        #[cfg(feature = "qualification")]
        super::allocation_qualification::buffer(create_info.size);
        unsafe { self.device.create_buffer(create_info, None) }
    }

    /// # Safety
    /// `buffer` must be a live buffer created by this device.
    pub unsafe fn buffer_memory_requirements(&self, buffer: vk::Buffer) -> vk::MemoryRequirements {
        unsafe { self.device.get_buffer_memory_requirements(buffer) }
    }

    /// # Safety
    /// The allocation size and memory type must be valid for this device.
    pub unsafe fn allocate_memory(
        &self,
        allocate_info: &vk::MemoryAllocateInfo<'_>,
    ) -> Result<vk::DeviceMemory, vk::Result> {
        let memory = unsafe { self.device.allocate_memory(allocate_info, None) }?;
        #[cfg(feature = "qualification")]
        super::allocation_qualification::allocated(memory, allocate_info.allocation_size);
        Ok(memory)
    }

    /// # Safety
    /// Both handles must belong to this device and satisfy the buffer memory requirements.
    pub unsafe fn bind_buffer_memory(
        &self,
        buffer: vk::Buffer,
        memory: vk::DeviceMemory,
    ) -> Result<(), vk::Result> {
        unsafe { self.device.bind_buffer_memory(buffer, memory, 0) }
    }

    /// # Safety
    /// `memory` must be host-visible, coherent, and allocated for at least `bytes.len()` bytes.
    pub unsafe fn write_memory(
        &self,
        memory: vk::DeviceMemory,
        bytes: &[u8],
    ) -> Result<(), vk::Result> {
        let size = u64::try_from(bytes.len()).map_err(|_| vk::Result::ERROR_OUT_OF_HOST_MEMORY)?;
        let destination = unsafe {
            self.device
                .map_memory(memory, 0, size, vk::MemoryMapFlags::empty())?
        };
        // SAFETY: The caller guarantees the allocation covers `bytes.len()`, which is the mapped size.
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), destination.cast(), bytes.len()) };
        unsafe { self.device.unmap_memory(memory) };
        Ok(())
    }

    /// # Safety
    /// `memory` must be host-visible, coherent, and allocated for at least `size` bytes.
    /// No submitted work may access the memory until this call returns.
    pub unsafe fn write_memory_ranges(
        &self,
        memory: vk::DeviceMemory,
        size: u64,
        ranges: &[(usize, &[u8])],
    ) -> Result<(), vk::Result> {
        let length = usize::try_from(size).map_err(|_| vk::Result::ERROR_OUT_OF_HOST_MEMORY)?;
        if ranges.iter().any(|(offset, bytes)| {
            offset
                .checked_add(bytes.len())
                .is_none_or(|end| end > length)
        }) {
            return Err(vk::Result::ERROR_OUT_OF_HOST_MEMORY);
        }
        // Validate every range and map once so a failure cannot leave a partial revision.
        let destination = unsafe {
            self.device
                .map_memory(memory, 0, size, vk::MemoryMapFlags::empty())?
        };
        for (offset, bytes) in ranges {
            // SAFETY: Every range was checked above to end within the mapped `size`.
            unsafe {
                std::ptr::copy_nonoverlapping(
                    bytes.as_ptr(),
                    destination.cast::<u8>().add(*offset),
                    bytes.len(),
                );
            }
        }
        unsafe { self.device.unmap_memory(memory) };
        Ok(())
    }

    /// # Safety
    /// `memory` must be host-visible, coherent, and allocated for at least `bytes.len()` bytes.
    /// Submitted writes to the range must be available to the host before this call.
    pub unsafe fn read_memory(
        &self,
        memory: vk::DeviceMemory,
        bytes: &mut [u8],
    ) -> Result<(), vk::Result> {
        let size = u64::try_from(bytes.len()).map_err(|_| vk::Result::ERROR_OUT_OF_HOST_MEMORY)?;
        let source = unsafe {
            self.device
                .map_memory(memory, 0, size, vk::MemoryMapFlags::empty())?
        };
        // SAFETY: The caller guarantees the allocation covers `bytes.len()`, which is the mapped size.
        unsafe { std::ptr::copy_nonoverlapping(source.cast(), bytes.as_mut_ptr(), bytes.len()) };
        unsafe { self.device.unmap_memory(memory) };
        Ok(())
    }

    /// # Safety
    /// `buffer` must belong to this device and no submitted work may still use it.
    pub unsafe fn destroy_buffer(&self, buffer: vk::Buffer) {
        unsafe { self.device.destroy_buffer(buffer, None) };
    }

    /// # Safety
    /// `memory` must belong to this device and no live resource may remain bound to it.
    pub unsafe fn free_memory(&self, memory: vk::DeviceMemory) {
        #[cfg(feature = "qualification")]
        super::allocation_qualification::freed(memory);
        unsafe { self.device.free_memory(memory, None) };
    }

    /// # Safety
    /// Every pointer and queue-family index in `create_info` must be valid for this device.
    pub unsafe fn create_image(
        &self,
        create_info: &vk::ImageCreateInfo<'_>,
    ) -> Result<vk::Image, vk::Result> {
        #[cfg(feature = "qualification")]
        super::allocation_qualification::image();
        unsafe { self.device.create_image(create_info, None) }
    }

    /// # Safety
    /// `image` must be a live image created by this device.
    pub unsafe fn image_memory_requirements(&self, image: vk::Image) -> vk::MemoryRequirements {
        unsafe { self.device.get_image_memory_requirements(image) }
    }

    /// # Safety
    /// Both handles must belong to this device and satisfy the image memory requirements.
    pub unsafe fn bind_image_memory(
        &self,
        image: vk::Image,
        memory: vk::DeviceMemory,
    ) -> Result<(), vk::Result> {
        unsafe { self.device.bind_image_memory(image, memory, 0) }
    }

    /// # Safety
    /// The referenced image and subresource range must be valid for this device.
    pub unsafe fn create_image_view(
        &self,
        create_info: &vk::ImageViewCreateInfo<'_>,
    ) -> Result<vk::ImageView, vk::Result> {
        unsafe { self.device.create_image_view(create_info, None) }
    }

    /// # Safety
    /// `image_view` must belong to this device and no submitted work may still use it.
    pub unsafe fn destroy_image_view(&self, image_view: vk::ImageView) {
        unsafe { self.device.destroy_image_view(image_view, None) };
    }

    /// # Safety
    /// `image` must belong to this device and no submitted work or live view may use it.
    pub unsafe fn destroy_image(&self, image: vk::Image) {
        unsafe { self.device.destroy_image(image, None) };
    }
    /// # Safety
    ///
    /// Every handle and pointer referenced by `create_info` must be valid for this device.
    pub unsafe fn create_render_pass(
        &self,
        create_info: &vk::RenderPassCreateInfo<'_>,
    ) -> Result<vk::RenderPass, vk::Result> {
        unsafe { self.device.create_render_pass(create_info, None) }
    }

    /// # Safety
    ///
    /// The shader code referenced by `create_info` must satisfy Vulkan's validity rules.
    pub unsafe fn create_shader_module(
        &self,
        create_info: &vk::ShaderModuleCreateInfo<'_>,
    ) -> Result<vk::ShaderModule, vk::Result> {
        unsafe { self.device.create_shader_module(create_info, None) }
    }

    /// # Safety
    ///
    /// Every descriptor-set layout and push-constant range must satisfy Vulkan's validity rules.
    pub unsafe fn create_pipeline_layout(
        &self,
        create_info: &vk::PipelineLayoutCreateInfo<'_>,
    ) -> Result<vk::PipelineLayout, vk::Result> {
        unsafe { self.device.create_pipeline_layout(create_info, None) }
    }

    /// # Safety
    ///
    /// Every handle and pointer in `create_infos` must remain valid for the calls.
    pub unsafe fn create_graphics_pipelines(
        &self,
        pipeline_cache: vk::PipelineCache,
        create_infos: &[vk::GraphicsPipelineCreateInfo<'_>],
    ) -> Result<Vec<vk::Pipeline>, (Vec<vk::Pipeline>, vk::Result)> {
        unsafe {
            self.device
                .create_graphics_pipelines(pipeline_cache, create_infos, None)
        }
    }

    /// # Safety
    ///
    /// Every handle and pointer in `create_infos` must remain valid for the call.
    pub unsafe fn create_compute_pipelines(
        &self,
        pipeline_cache: vk::PipelineCache,
        create_infos: &[vk::ComputePipelineCreateInfo<'_>],
    ) -> Result<Vec<vk::Pipeline>, (Vec<vk::Pipeline>, vk::Result)> {
        unsafe {
            self.device
                .create_compute_pipelines(pipeline_cache, create_infos, None)
        }
    }

    /// # Safety
    ///
    /// Every binding in `create_info` must satisfy Vulkan's descriptor limits.
    pub unsafe fn create_descriptor_set_layout(
        &self,
        create_info: &vk::DescriptorSetLayoutCreateInfo<'_>,
    ) -> Result<vk::DescriptorSetLayout, vk::Result> {
        unsafe { self.device.create_descriptor_set_layout(create_info, None) }
    }

    /// # Safety
    ///
    /// `layout` must belong to this device and no live pipeline layout may depend on it.
    pub unsafe fn destroy_descriptor_set_layout(&self, layout: vk::DescriptorSetLayout) {
        unsafe { self.device.destroy_descriptor_set_layout(layout, None) };
    }

    /// # Safety
    ///
    /// Pool sizes and flags in `create_info` must satisfy Vulkan's validity rules.
    pub unsafe fn create_descriptor_pool(
        &self,
        create_info: &vk::DescriptorPoolCreateInfo<'_>,
    ) -> Result<vk::DescriptorPool, vk::Result> {
        unsafe { self.device.create_descriptor_pool(create_info, None) }
    }

    /// # Safety
    ///
    /// `pool` must belong to this device and no submitted work may use its descriptor sets.
    pub unsafe fn destroy_descriptor_pool(&self, pool: vk::DescriptorPool) {
        unsafe { self.device.destroy_descriptor_pool(pool, None) };
    }

    /// # Safety
    ///
    /// The pool and layouts referenced by `allocate_info` must be live and compatible.
    pub unsafe fn allocate_descriptor_sets(
        &self,
        allocate_info: &vk::DescriptorSetAllocateInfo<'_>,
    ) -> Result<Vec<vk::DescriptorSet>, vk::Result> {
        unsafe { self.device.allocate_descriptor_sets(allocate_info) }
    }

    /// # Safety
    ///
    /// Every descriptor and referenced resource must be valid for this device.
    pub unsafe fn update_descriptor_sets(&self, writes: &[vk::WriteDescriptorSet<'_>]) {
        unsafe { self.device.update_descriptor_sets(writes, &[]) };
    }

    /// # Safety
    ///
    /// Sampler parameters in `create_info` must satisfy the enabled device features and limits.
    pub unsafe fn create_sampler(
        &self,
        create_info: &vk::SamplerCreateInfo<'_>,
    ) -> Result<vk::Sampler, vk::Result> {
        unsafe { self.device.create_sampler(create_info, None) }
    }

    /// # Safety
    ///
    /// `sampler` must belong to this device and no submitted work may still use it.
    pub unsafe fn destroy_sampler(&self, sampler: vk::Sampler) {
        unsafe { self.device.destroy_sampler(sampler, None) };
    }

    /// # Safety
    ///
    /// `render_pass` must belong to this device and be compatible with `attachment`.
    pub unsafe fn create_framebuffer(
        &self,
        render_pass: vk::RenderPass,
        attachment: &RenderPathAttachment<'_>,
        depth_attachment: vk::ImageView,
        extent: vk::Extent2D,
    ) -> Result<vk::Framebuffer, vk::Result> {
        let attachments = [attachment.view, depth_attachment];
        let create_info = vk::FramebufferCreateInfo::default()
            .render_pass(render_pass)
            .attachments(&attachments)
            .width(extent.width)
            .height(extent.height)
            .layers(1);
        unsafe { self.device.create_framebuffer(&create_info, None) }
    }

    /// # Safety
    ///
    /// `render_pass` must belong to this device and be compatible with `attachment`.
    pub unsafe fn create_color_framebuffer(
        &self,
        render_pass: vk::RenderPass,
        attachment: &RenderPathAttachment<'_>,
        extent: vk::Extent2D,
    ) -> Result<vk::Framebuffer, vk::Result> {
        let attachments = [attachment.view];
        let create_info = vk::FramebufferCreateInfo::default()
            .render_pass(render_pass)
            .attachments(&attachments)
            .width(extent.width)
            .height(extent.height)
            .layers(1);
        unsafe { self.device.create_framebuffer(&create_info, None) }
    }

    /// # Safety
    ///
    /// `framebuffer` must belong to this device and no submitted work may still use it.
    pub unsafe fn destroy_framebuffer(&self, framebuffer: vk::Framebuffer) {
        unsafe { self.device.destroy_framebuffer(framebuffer, None) };
    }

    /// # Safety
    ///
    /// `pipeline` must belong to this device and no submitted work may still use it.
    pub unsafe fn destroy_pipeline(&self, pipeline: vk::Pipeline) {
        unsafe { self.device.destroy_pipeline(pipeline, None) };
    }

    /// # Safety
    ///
    /// `pipeline_layout` must belong to this device and no live object may depend on it.
    pub unsafe fn destroy_pipeline_layout(&self, pipeline_layout: vk::PipelineLayout) {
        unsafe { self.device.destroy_pipeline_layout(pipeline_layout, None) };
    }

    /// # Safety
    ///
    /// `render_pass` must belong to this device and no live object may depend on it.
    pub unsafe fn destroy_render_pass(&self, render_pass: vk::RenderPass) {
        unsafe { self.device.destroy_render_pass(render_pass, None) };
    }

    /// # Safety
    ///
    /// `shader_module` must belong to this device and not already have been destroyed.
    pub unsafe fn destroy_shader_module(&self, shader_module: vk::ShaderModule) {
        unsafe { self.device.destroy_shader_module(shader_module, None) };
    }
}

pub struct RenderPathFrameContext<'frame> {
    pub(super) device: &'frame ash::Device,
    pub(super) command_buffer: vk::CommandBuffer,
    pub(super) target: RenderPathFrameTarget<'frame>,
}

impl RenderPathFrameContext<'_> {
    pub fn target(&self) -> &RenderPathFrameTarget<'_> {
        &self.target
    }

    /// # Safety
    ///
    /// The render pass and framebuffer must be compatible and valid for the current target.
    pub unsafe fn begin_render_pass(
        &self,
        begin_info: &vk::RenderPassBeginInfo<'_>,
        contents: vk::SubpassContents,
    ) {
        unsafe {
            self.device
                .cmd_begin_render_pass(self.command_buffer, begin_info, contents)
        };
    }

    /// # Safety
    ///
    /// `pipeline` must be valid, compatible with the active render pass, and use `bind_point`.
    pub unsafe fn bind_pipeline(&self, bind_point: vk::PipelineBindPoint, pipeline: vk::Pipeline) {
        unsafe {
            self.device
                .cmd_bind_pipeline(self.command_buffer, bind_point, pipeline)
        };
    }

    /// # Safety
    ///
    /// The descriptor sets must be live and compatible with `layout` and `bind_point`.
    pub unsafe fn bind_descriptor_sets(
        &self,
        bind_point: vk::PipelineBindPoint,
        layout: vk::PipelineLayout,
        descriptor_sets: &[vk::DescriptorSet],
    ) {
        unsafe {
            self.device.cmd_bind_descriptor_sets(
                self.command_buffer,
                bind_point,
                layout,
                0,
                descriptor_sets,
                &[],
            )
        };
    }

    /// # Safety
    ///
    /// The bound compute state must be valid and each group count must fit the device limit.
    pub unsafe fn dispatch(&self, group_count: [u32; 3]) {
        let [group_count_x, group_count_y, group_count_z] = group_count;
        unsafe {
            self.device.cmd_dispatch(
                self.command_buffer,
                group_count_x,
                group_count_y,
                group_count_z,
            )
        };
    }

    /// # Safety
    ///
    /// Each image barrier must describe a live image and valid old and new layouts.
    pub unsafe fn image_pipeline_barrier(
        &self,
        source_stage: vk::PipelineStageFlags,
        destination_stage: vk::PipelineStageFlags,
        barriers: &[vk::ImageMemoryBarrier<'_>],
    ) {
        unsafe {
            self.device.cmd_pipeline_barrier(
                self.command_buffer,
                source_stage,
                destination_stage,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                barriers,
            )
        };
    }

    /// # Safety
    /// Each buffer barrier must describe a live buffer and valid access masks.
    pub unsafe fn buffer_pipeline_barrier(
        &self,
        source_stage: vk::PipelineStageFlags,
        destination_stage: vk::PipelineStageFlags,
        barriers: &[vk::BufferMemoryBarrier<'_>],
    ) {
        unsafe {
            self.device.cmd_pipeline_barrier(
                self.command_buffer,
                source_stage,
                destination_stage,
                vk::DependencyFlags::empty(),
                &[],
                barriers,
                &[],
            )
        };
    }

    /// # Safety
    /// A compatible live vertex buffer must be supplied during command recording.
    pub unsafe fn bind_vertex_buffer(&self, buffer: vk::Buffer) {
        unsafe {
            self.device
                .cmd_bind_vertex_buffers(self.command_buffer, 0, &[buffer], &[0])
        };
    }

    /// # Safety
    /// A live `u32` index buffer must be supplied during command recording.
    pub unsafe fn bind_index_buffer(&self, buffer: vk::Buffer) {
        unsafe {
            self.device
                .cmd_bind_index_buffer(self.command_buffer, buffer, 0, vk::IndexType::UINT32)
        };
    }

    /// # Safety
    /// `layout` must declare a vertex push-constant range covering all supplied bytes.
    pub unsafe fn push_vertex_constants(&self, layout: vk::PipelineLayout, bytes: &[u8]) {
        unsafe {
            self.device.cmd_push_constants(
                self.command_buffer,
                layout,
                vk::ShaderStageFlags::VERTEX,
                0,
                bytes,
            )
        };
    }

    /// # Safety
    /// All graphics state and buffers required for the indexed draw must be valid and bound.
    pub unsafe fn draw_indexed(&self, index_count: u32) {
        unsafe {
            self.device
                .cmd_draw_indexed(self.command_buffer, index_count, 1, 0, 0, 0)
        };
    }

    /// # Safety
    ///
    /// Bound state must satisfy Vulkan's requirements for this draw.
    pub unsafe fn draw(
        &self,
        vertex_count: u32,
        instance_count: u32,
        first_vertex: u32,
        first_instance: u32,
    ) {
        unsafe {
            self.device.cmd_draw(
                self.command_buffer,
                vertex_count,
                instance_count,
                first_vertex,
                first_instance,
            )
        };
    }

    /// # Safety
    ///
    /// A render pass must be active in this frame context.
    pub unsafe fn end_render_pass(&self) {
        unsafe { self.device.cmd_end_render_pass(self.command_buffer) };
    }
}

pub trait RenderPath {
    fn submit_edit_outcome(
        &mut self,
        _outcome: voxel_frontend::VoxelEditOutcome,
    ) -> RenderPathResult<()> {
        Err(Box::new(RenderPathEditError::SubmissionUnavailable))
    }

    fn publish_camera_state(
        &mut self,
        _camera_state: CameraState,
        _camera_state_revision: CameraStateRevision,
    ) -> RenderPathResult<()> {
        Err(Box::new(RenderPathCameraStateError::PublicationUnavailable))
    }

    fn switch_diagnostics(&self) -> Option<RenderPathSwitchDiagnostics> {
        None
    }

    fn request_switch(
        &mut self,
        _replacement: Box<dyn SwitchableRenderPath>,
    ) -> Result<(), RenderPathSwitchRequestError> {
        Err(RenderPathSwitchRequestError::SwitchingUnavailable)
    }

    fn release(&mut self, device: RenderPathDeviceContext<'_>) -> RenderPathResult<()>;

    fn configure(
        &mut self,
        device: RenderPathDeviceContext<'_>,
        target: RenderPathTarget<'_>,
    ) -> RenderPathResult<()>;

    fn advance_frame_boundary(
        &mut self,
        _device: RenderPathDeviceContext<'_>,
        _target: RenderPathTarget<'_>,
    ) -> RenderPathResult<()> {
        Ok(())
    }

    fn shutdown(&mut self, _device: RenderPathDeviceContext<'_>) -> RenderPathResult<()> {
        Ok(())
    }

    fn record(&mut self, frame: RenderPathFrameContext<'_>) -> RenderPathResult<()>;
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum RenderPathEditError {
    #[error("the presenting Render Path does not accept Voxel Edit outcomes")]
    SubmissionUnavailable,
    /// The replacement was prepared from an earlier Voxel Scene Revision and cannot follow edits,
    /// so accepting one would defer the handoff indefinitely.
    #[error("a Render Path switch is preparing a replacement, so Voxel Edit outcomes are rejected")]
    SwitchInProgress,
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum RenderPathCameraStateError {
    #[error("the active Render Path does not accept shared Camera State publication")]
    PublicationUnavailable,
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

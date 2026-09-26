use super::RasterRenderPath;
use super::convergence::RasterConvergenceError;
use super::installation::{
    RasterArtifactInstallationError, RasterArtifactInstallationPhase, RasterArtifactInstallerError,
    RasterRegionInstallation, RasterRegionInstallationGeneration,
};
use super::lifecycle::{RasterCameraControlError, RasterLifecycleControlError};
use super::meshing::{RasterArtifact, RasterRegionIdentity, RasterRegionResult, RasterVertex};
use ash::vk;
use render_backend::CameraConfigurationError;
use render_backend::{
    RenderPath, RenderPathDeviceContext, RenderPathFrameContext, RenderPathResult, RenderPathTarget,
};
use std::io::Cursor;
use std::mem::size_of;
use thiserror::Error;
use voxel_frontend::VoxelEditOutcome;

#[derive(Debug, Error)]
pub(super) enum RasterResourceError {
    #[error("no complete raster artifact is installed")]
    MissingArtifact,
    #[error(transparent)]
    ArtifactGate(#[from] RasterArtifactInstallerError),
    #[error(transparent)]
    CameraControl(#[from] RasterCameraControlError),
    #[error(transparent)]
    LifecycleControl(#[from] RasterLifecycleControlError),
    #[cfg(any(test, feature = "qualification"))]
    #[error("injected GPU upload failure")]
    InjectedUploadFailure,
    #[error("the raster artifact index count cannot be represented for indexed drawing")]
    IndexCount,
    #[error("the Raster Vertex stride cannot be represented for graphics state")]
    VertexStride,
    #[error("could not configure the raster camera: {0}")]
    Camera(#[from] CameraConfigurationError),
    #[error("the static {0} buffer byte length cannot be represented by Vulkan")]
    BufferSize(&'static str),
    #[error("could not create the static {kind} buffer: {source}")]
    CreateBuffer {
        kind: &'static str,
        source: vk::Result,
    },
    #[error("no host-visible coherent memory type can hold the static {0} buffer")]
    MissingBufferMemory(&'static str),
    #[error("could not allocate the static {kind} buffer memory: {source}")]
    AllocateBufferMemory {
        kind: &'static str,
        source: vk::Result,
    },
    #[error("could not bind the static {kind} buffer memory: {source}")]
    BindBufferMemory {
        kind: &'static str,
        source: vk::Result,
    },
    #[error("could not upload the static {kind} data: {source}")]
    UploadBuffer {
        kind: &'static str,
        source: vk::Result,
    },
    #[error("could not create the depth image: {0}")]
    CreateDepthImage(vk::Result),
    #[error("no device-local memory type can hold the depth image")]
    MissingDepthMemory,
    #[error("could not allocate the depth image memory: {0}")]
    AllocateDepthMemory(vk::Result),
    #[error("could not bind the depth image memory: {0}")]
    BindDepthMemory(vk::Result),
    #[error("could not create the depth image view: {0}")]
    CreateDepthView(vk::Result),
    #[error("could not create the raster render pass: {0}")]
    CreateRenderPass(vk::Result),
    #[error("could not read a reproducibly built raster shader artifact: {0}")]
    ReadShaderArtifact(#[from] std::io::Error),
    #[error("could not create a raster shader module: {0}")]
    CreateShaderModule(vk::Result),
    #[error("could not create the raster graphics pipeline layout: {0}")]
    CreatePipelineLayout(vk::Result),
    #[error("could not create the raster graphics pipeline: {0}")]
    CreateGraphicsPipeline(vk::Result),
    #[error("could not create a raster framebuffer: {0}")]
    CreateFramebuffer(vk::Result),
    #[error("the frame target does not belong to the configured raster presentation target")]
    StaleFrameTarget,
    #[error("the configured raster presentation target has no framebuffer for the acquired image")]
    MissingFramebuffer,
    #[error("could not configure the raster material table: {0}")]
    MaterialTable(vk::Result),
}

pub(super) struct RasterRegionGpuResources {
    pub(super) material_buffer_bytes: u64,
    pub(super) material_buffer: vk::Buffer,
    pub(super) material_memory: vk::DeviceMemory,
    pub(super) material_layout: vk::DescriptorSetLayout,
    pub(super) material_pool: vk::DescriptorPool,
    pub(super) material_set: vk::DescriptorSet,
    pub(super) transform_constants: [u32; 8],
    pub(super) identity: RasterRegionIdentity,
    pub(super) vertex_buffer: vk::Buffer,
    pub(super) vertex_memory: vk::DeviceMemory,
    pub(super) index_buffer: vk::Buffer,
    pub(super) index_memory: vk::DeviceMemory,
    pub(super) index_count: u32,
    pub(super) vertex_buffer_bytes: u64,
    pub(super) index_buffer_bytes: u64,
}

impl RasterResourceError {
    fn installation_phase(&self) -> RasterArtifactInstallationPhase {
        match self {
            Self::CreateBuffer { .. }
            | Self::MissingBufferMemory(_)
            | Self::AllocateBufferMemory { .. }
            | Self::BindBufferMemory { .. }
            | Self::UploadBuffer { .. }
            | Self::IndexCount
            | Self::VertexStride
            | Self::BufferSize(_)
            | Self::MaterialTable(_) => RasterArtifactInstallationPhase::Upload,
            Self::LifecycleControl(_) => RasterArtifactInstallationPhase::Upload,
            Self::ArtifactGate(_) => RasterArtifactInstallationPhase::Upload,
            #[cfg(any(test, feature = "qualification"))]
            Self::InjectedUploadFailure => RasterArtifactInstallationPhase::Upload,
            Self::CameraControl(_) => RasterArtifactInstallationPhase::Record,
            _ => RasterArtifactInstallationPhase::PresentationConfiguration,
        }
    }
}

impl RenderPath for RasterRenderPath {
    fn submit_edit_outcome(&mut self, outcome: VoxelEditOutcome) -> RenderPathResult<()> {
        let controller = match self.lifecycle_control.clone() {
            Some(controller) => controller,
            None => self.enable_lifecycle_control(),
        };
        controller.submit(outcome)?;
        Ok(())
    }

    fn release(&mut self, device: RenderPathDeviceContext<'_>) -> RenderPathResult<()> {
        let mut restart_error = None;
        if let Some(convergence) = &mut self.convergence {
            let hidden_release = convergence.take_hidden_resources_for_release();
            unsafe {
                hidden_release
                    .retirement
                    .release_after_gpu_completion(&device)
            };
            restart_error = hidden_release.restart_error;
        }
        self.release_resources(&device);
        let control_error = self
            .lifecycle_control
            .as_ref()
            .and_then(|controller| match controller.state.lock() {
                Ok(mut state) => {
                    state.post_upload_revision = None;
                    None
                }
                Err(_) => Some(RasterLifecycleControlError),
            });
        finish_lifecycle_operation("release", restart_error, control_error)
    }

    fn configure(
        &mut self,
        device: RenderPathDeviceContext<'_>,
        target: RenderPathTarget<'_>,
    ) -> RenderPathResult<()> {
        let source_revision = self
            .expected_source_revision
            .or_else(|| self.artifact.as_ref().map(RasterArtifact::source_revision))
            .ok_or_else(|| {
                Box::new(RasterResourceError::MissingArtifact)
                    as Box<dyn std::error::Error + Send + Sync>
            })?;
        let result = self.accept_staged_artifact();
        #[cfg(any(test, feature = "qualification"))]
        let result = result.and_then(|()| self.inject_upload_failure());
        let result = result
            .and_then(|()| self.configure_resources(&device, target))
            .and_then(|()| self.mark_artifact_installed());
        if let Err(error) = result {
            let phase = error.installation_phase();
            self.release_resources(&device);
            return Err(Box::new(RasterArtifactInstallationError::new(
                phase,
                source_revision,
                Box::new(error),
            )));
        }
        Ok(())
    }

    fn advance_frame_boundary(
        &mut self,
        device: RenderPathDeviceContext<'_>,
        target: RenderPathTarget<'_>,
    ) -> RenderPathResult<()> {
        self.advance_convergence_at_frame_boundary(Some(&device))?;
        self.update_camera_constants(target.extent())?;
        Ok(())
    }

    fn shutdown(&mut self, device: RenderPathDeviceContext<'_>) -> RenderPathResult<()> {
        let mut worker_error = None;
        if let Some(convergence) = &mut self.convergence {
            let shutdown = convergence.shutdown();
            unsafe { shutdown.retirement.release_after_gpu_completion(&device) };
            worker_error = shutdown.worker_error;
        }
        self.release_resources(&device);
        let owned_resource_count = self.region_resources.len()
            + self
                .convergence
                .as_ref()
                .and_then(|convergence| convergence.hidden_candidate.as_ref())
                .map(|candidate| candidate.successor_gpu_resources.len())
                .unwrap_or(0);
        let control_error = self
            .lifecycle_control
            .as_ref()
            .and_then(|controller| match controller.state.lock() {
                Ok(mut state) => {
                    state.pending_outcomes.clear();
                    state.post_upload_revision = None;
                    state.shutdown_owned_resource_count = Some(owned_resource_count);
                    None
                }
                Err(_) => Some(RasterLifecycleControlError),
            });
        finish_lifecycle_operation("shutdown", worker_error, control_error)
    }

    fn record(&mut self, frame: RenderPathFrameContext<'_>) -> RenderPathResult<()> {
        let source_revision = self
            .expected_source_revision
            .or_else(|| self.artifact.as_ref().map(RasterArtifact::source_revision))
            .ok_or(RasterResourceError::MissingArtifact)?;
        let result = self.record_frame(&frame);
        result.map_err(|error| {
            Box::new(RasterArtifactInstallationError::new(
                RasterArtifactInstallationPhase::Record,
                source_revision,
                Box::new(error),
            )) as Box<dyn std::error::Error + Send + Sync>
        })
    }
}

pub(super) fn finish_lifecycle_operation(
    operation: &str,
    operational_error: Option<RasterConvergenceError>,
    control_error: Option<RasterLifecycleControlError>,
) -> RenderPathResult<()> {
    if let Some(error) = operational_error {
        if let Some(control_error) = control_error {
            eprintln!(
                "additional raster lifecycle control error during {operation}: {control_error}"
            );
        }
        return Err(Box::new(error));
    }
    if let Some(error) = control_error {
        return Err(Box::new(error));
    }
    Ok(())
}

impl RasterRenderPath {
    #[cfg(any(test, feature = "qualification"))]
    fn inject_upload_failure(&self) -> Result<(), RasterResourceError> {
        if self.artifact.is_none() || self.installed_source_revision.is_some() {
            return Ok(());
        }
        let Some(installer) = &self.installation else {
            return Ok(());
        };
        let mut state = installer
            .state
            .lock()
            .map_err(|_| RasterArtifactInstallerError::StateUnavailable)?;
        if !state.inject_upload_failure {
            return Ok(());
        }
        state.inject_upload_failure = false;
        Err(RasterResourceError::InjectedUploadFailure)
    }

    fn accept_staged_artifact(&mut self) -> Result<(), RasterResourceError> {
        let Some(installer) = &self.installation else {
            return Ok(());
        };
        let mut state = installer
            .state
            .lock()
            .map_err(|_| RasterArtifactInstallerError::StateUnavailable)?;
        if let Some(artifact) = state.staged_artifact.take() {
            self.artifact = Some(artifact);
        }
        Ok(())
    }

    fn mark_artifact_installed(&mut self) -> Result<(), RasterResourceError> {
        let Some(artifact) = &self.artifact else {
            return Ok(());
        };
        let source_revision = artifact.source_revision();
        if self.expected_source_revision != Some(source_revision) {
            return Err(RasterArtifactInstallerError::RevisionMismatch {
                expected: self.expected_source_revision.unwrap_or(source_revision),
                actual: source_revision,
            }
            .into());
        }
        self.installed_source_revision = Some(source_revision);
        self.installed_regions = artifact
            .regions()
            .iter()
            .map(|region| {
                let has_gpu_resources = self.region_resources.iter().any(|resources| {
                    resources.identity == *region.identity() && resources.index_count > 0
                });
                RasterRegionInstallation::new(
                    region,
                    has_gpu_resources,
                    RasterRegionInstallationGeneration::new(1),
                )
            })
            .collect();
        if let Some(installer) = &self.installation {
            let mut state = installer
                .state
                .lock()
                .map_err(|_| RasterArtifactInstallerError::StateUnavailable)?;
            state.installed_revision = Some(source_revision);
        }
        Ok(())
    }

    fn configure_resources(
        &mut self,
        device: &RenderPathDeviceContext<'_>,
        target: RenderPathTarget<'_>,
    ) -> Result<(), RasterResourceError> {
        if let Some(artifact) = &self.artifact {
            for region in artifact.regions() {
                self.region_resources
                    .push(upload_raster_region_resources(device, region)?);
            }
        }
        self.observe_live_gpu_resources(self.region_resources.iter())?;
        self.observe_installed_gpu_resources(self.region_resources.iter())?;
        self.create_depth_resources(device, target.extent())?;
        self.create_render_pass(device, target.format())?;
        self.create_graphics_pipeline(device, target.extent())?;
        for attachment in target.attachments() {
            let framebuffer = unsafe {
                device.create_framebuffer(
                    self.render_pass,
                    &attachment,
                    self.depth_view,
                    target.extent(),
                )
            }
            .map_err(RasterResourceError::CreateFramebuffer)?;
            self.framebuffers.push(framebuffer);
            self.configured_attachments.push(attachment.identity());
        }
        self.configuration_id = Some(target.configuration_id());
        self.update_camera_constants(target.extent())?;
        Ok(())
    }

    fn update_camera_constants(&mut self, extent: vk::Extent2D) -> Result<(), RasterResourceError> {
        let state = self.camera_control.state()?;
        self.camera_constants = state.pose.view_projection([extent.width, extent.height])?;
        self.acknowledged_camera_revision = state.revision;
        Ok(())
    }

    fn create_depth_resources(
        &mut self,
        device: &RenderPathDeviceContext<'_>,
        extent: vk::Extent2D,
    ) -> Result<(), RasterResourceError> {
        let create_info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(vk::Format::D32_SFLOAT)
            .extent(vk::Extent3D {
                width: extent.width,
                height: extent.height,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        self.depth_image = unsafe { device.create_image(&create_info) }
            .map_err(RasterResourceError::CreateDepthImage)?;
        let requirements = unsafe { device.image_memory_requirements(self.depth_image) };
        let memory_type_index = device
            .memory_type_index(
                requirements.memory_type_bits,
                vk::MemoryPropertyFlags::DEVICE_LOCAL,
            )
            .ok_or(RasterResourceError::MissingDepthMemory)?;
        let allocate_info = vk::MemoryAllocateInfo::default()
            .allocation_size(requirements.size)
            .memory_type_index(memory_type_index);
        self.depth_memory = unsafe { device.allocate_memory(&allocate_info) }
            .map_err(RasterResourceError::AllocateDepthMemory)?;
        unsafe { device.bind_image_memory(self.depth_image, self.depth_memory) }
            .map_err(RasterResourceError::BindDepthMemory)?;
        let view_info = vk::ImageViewCreateInfo::default()
            .image(self.depth_image)
            .view_type(vk::ImageViewType::TYPE_2D)
            .format(vk::Format::D32_SFLOAT)
            .subresource_range(
                vk::ImageSubresourceRange::default()
                    .aspect_mask(vk::ImageAspectFlags::DEPTH)
                    .base_mip_level(0)
                    .level_count(1)
                    .base_array_layer(0)
                    .layer_count(1),
            );
        self.depth_view = unsafe { device.create_image_view(&view_info) }
            .map_err(RasterResourceError::CreateDepthView)?;
        Ok(())
    }

    fn create_render_pass(
        &mut self,
        device: &RenderPathDeviceContext<'_>,
        color_format: vk::Format,
    ) -> Result<(), RasterResourceError> {
        let color = vk::AttachmentDescription::default()
            .format(color_format)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::STORE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .final_layout(vk::ImageLayout::PRESENT_SRC_KHR);
        let depth = vk::AttachmentDescription::default()
            .format(vk::Format::D32_SFLOAT)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::DONT_CARE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .final_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL);
        let color_reference = vk::AttachmentReference::default()
            .attachment(0)
            .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);
        let depth_reference = vk::AttachmentReference::default()
            .attachment(1)
            .layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL);
        let color_references = [color_reference];
        let subpass = vk::SubpassDescription::default()
            .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
            .color_attachments(&color_references)
            .depth_stencil_attachment(&depth_reference);
        let dependency = vk::SubpassDependency::default()
            .src_subpass(vk::SUBPASS_EXTERNAL)
            .dst_subpass(0)
            .src_stage_mask(
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT
                    | vk::PipelineStageFlags::EARLY_FRAGMENT_TESTS,
            )
            .dst_stage_mask(
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT
                    | vk::PipelineStageFlags::EARLY_FRAGMENT_TESTS,
            )
            .dst_access_mask(
                vk::AccessFlags::COLOR_ATTACHMENT_WRITE
                    | vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE,
            );
        let attachments = [color, depth];
        let subpasses = [subpass];
        let dependencies = [dependency];
        let create_info = vk::RenderPassCreateInfo::default()
            .attachments(&attachments)
            .subpasses(&subpasses)
            .dependencies(&dependencies);
        self.render_pass = unsafe { device.create_render_pass(&create_info) }
            .map_err(RasterResourceError::CreateRenderPass)?;
        Ok(())
    }

    fn create_graphics_pipeline(
        &mut self,
        device: &RenderPathDeviceContext<'_>,
        extent: vk::Extent2D,
    ) -> Result<(), RasterResourceError> {
        let vertex_code = ash::util::read_spv(&mut Cursor::new(include_bytes!(concat!(
            env!("OUT_DIR"),
            "/raster.vert.spv"
        ))))?;
        let fragment_code = ash::util::read_spv(&mut Cursor::new(include_bytes!(concat!(
            env!("OUT_DIR"),
            "/raster.frag.spv"
        ))))?;
        let vertex_module = create_shader_module(device, &vertex_code)?;
        let fragment_module = match create_shader_module(device, &fragment_code) {
            Ok(module) => module,
            Err(error) => {
                unsafe { device.destroy_shader_module(vertex_module) };
                return Err(error);
            }
        };
        let result =
            self.create_pipeline_with_modules(device, extent, vertex_module, fragment_module);
        unsafe {
            device.destroy_shader_module(fragment_module);
            device.destroy_shader_module(vertex_module);
        }
        result
    }

    fn create_pipeline_with_modules(
        &mut self,
        device: &RenderPathDeviceContext<'_>,
        extent: vk::Extent2D,
        vertex_module: vk::ShaderModule,
        fragment_module: vk::ShaderModule,
    ) -> Result<(), RasterResourceError> {
        let stages = [
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::VERTEX)
                .module(vertex_module)
                .name(c"main"),
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::FRAGMENT)
                .module(fragment_module)
                .name(c"main"),
        ];
        let binding = [vk::VertexInputBindingDescription {
            binding: 0,
            stride: u32::try_from(size_of::<RasterVertex>())
                .map_err(|_| RasterResourceError::VertexStride)?,
            input_rate: vk::VertexInputRate::VERTEX,
        }];
        let attributes = [vk::VertexInputAttributeDescription {
            location: 0,
            binding: 0,
            format: vk::Format::R16G16B16A16_UINT,
            offset: 0,
        }];
        let vertex_input = vk::PipelineVertexInputStateCreateInfo::default()
            .vertex_binding_descriptions(&binding)
            .vertex_attribute_descriptions(&attributes);
        let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
        let viewports = [vk::Viewport {
            x: 0.0,
            y: 0.0,
            width: extent.width as f32,
            height: extent.height as f32,
            min_depth: 0.0,
            max_depth: 1.0,
        }];
        let scissors = [vk::Rect2D {
            offset: vk::Offset2D::default(),
            extent,
        }];
        let viewport_state = vk::PipelineViewportStateCreateInfo::default()
            .viewports(&viewports)
            .scissors(&scissors);
        let rasterization = vk::PipelineRasterizationStateCreateInfo::default()
            .polygon_mode(vk::PolygonMode::FILL)
            .cull_mode(vk::CullModeFlags::BACK)
            .front_face(Self::front_face())
            .line_width(1.0);
        let multisample = vk::PipelineMultisampleStateCreateInfo::default()
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);
        let depth_stencil = vk::PipelineDepthStencilStateCreateInfo::default()
            .depth_test_enable(true)
            .depth_write_enable(true)
            .depth_compare_op(vk::CompareOp::LESS);
        let color_attachment = [vk::PipelineColorBlendAttachmentState::default()
            .color_write_mask(vk::ColorComponentFlags::RGBA)];
        let color_blend =
            vk::PipelineColorBlendStateCreateInfo::default().attachments(&color_attachment);
        let push_constant_range = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::VERTEX)
            .offset(0)
            .size(96)];
        let material_layout = create_material_layout(device)?;
        let set_layouts = [material_layout];
        let layout_info = vk::PipelineLayoutCreateInfo::default()
            .set_layouts(&set_layouts)
            .push_constant_ranges(&push_constant_range);
        let pipeline_layout = unsafe { device.create_pipeline_layout(&layout_info) };
        unsafe { device.destroy_descriptor_set_layout(material_layout) };
        self.pipeline_layout =
            pipeline_layout.map_err(RasterResourceError::CreatePipelineLayout)?;
        let pipeline_info = vk::GraphicsPipelineCreateInfo::default()
            .stages(&stages)
            .vertex_input_state(&vertex_input)
            .input_assembly_state(&input_assembly)
            .viewport_state(&viewport_state)
            .rasterization_state(&rasterization)
            .multisample_state(&multisample)
            .depth_stencil_state(&depth_stencil)
            .color_blend_state(&color_blend)
            .layout(self.pipeline_layout)
            .render_pass(self.render_pass)
            .subpass(0);
        match unsafe {
            device.create_graphics_pipelines(vk::PipelineCache::null(), &[pipeline_info])
        } {
            Ok(mut pipelines) => {
                self.pipeline =
                    pipelines
                        .pop()
                        .ok_or(RasterResourceError::CreateGraphicsPipeline(
                            vk::Result::ERROR_UNKNOWN,
                        ))?;
                Ok(())
            }
            Err((pipelines, error)) => {
                for pipeline in pipelines {
                    unsafe { device.destroy_pipeline(pipeline) };
                }
                Err(RasterResourceError::CreateGraphicsPipeline(error))
            }
        }
    }

    fn record_frame(
        &mut self,
        frame: &RenderPathFrameContext<'_>,
    ) -> Result<(), RasterResourceError> {
        let target = frame.target();
        if self.configuration_id != Some(target.configuration_id()) {
            return Err(RasterResourceError::StaleFrameTarget);
        }
        let framebuffer_index = self
            .configured_attachments
            .iter()
            .position(|identity| *identity == target.attachment().identity())
            .ok_or(RasterResourceError::MissingFramebuffer)?;
        let framebuffer = self
            .framebuffers
            .get(framebuffer_index)
            .copied()
            .ok_or(RasterResourceError::MissingFramebuffer)?;
        let clear_values = [
            vk::ClearValue {
                color: vk::ClearColorValue {
                    float32: [0.025, 0.035, 0.06, 1.0],
                },
            },
            vk::ClearValue {
                depth_stencil: vk::ClearDepthStencilValue {
                    depth: 1.0,
                    stencil: 0,
                },
            },
        ];
        self.update_camera_constants(target.extent())?;
        // LESS keeps the first equal-depth fragment. Match the Semantic Ray oracle's
        // volume-identity tie break even after localized replacements reorder resources.
        self.region_resources.sort_unstable_by(|left, right| {
            left.identity
                .volume_identity()
                .cmp(right.identity.volume_identity())
        });
        let render_pass_info = vk::RenderPassBeginInfo::default()
            .render_pass(self.render_pass)
            .framebuffer(framebuffer)
            .render_area(vk::Rect2D {
                offset: vk::Offset2D::default(),
                extent: target.extent(),
            })
            .clear_values(&clear_values);
        unsafe {
            frame.begin_render_pass(&render_pass_info, vk::SubpassContents::INLINE);
            frame.bind_pipeline(vk::PipelineBindPoint::GRAPHICS, self.pipeline);
            for resources in &self.region_resources {
                if resources.index_count == 0 {
                    continue;
                }
                frame.bind_vertex_buffer(resources.vertex_buffer);
                frame.bind_index_buffer(resources.index_buffer);
                let mut constants = [0; 24];
                constants[..16].copy_from_slice(&self.camera_constants.map(f32::to_bits));
                constants[16..].copy_from_slice(&resources.transform_constants);
                frame.bind_descriptor_sets(
                    vk::PipelineBindPoint::GRAPHICS,
                    self.pipeline_layout,
                    &[resources.material_set],
                );
                frame.push_vertex_constants(self.pipeline_layout, u32_bytes(&constants));
                frame.draw_indexed(resources.index_count);
            }
            frame.end_render_pass();
        }
        Ok(())
    }

    fn release_resources(&mut self, device: &RenderPathDeviceContext<'_>) {
        unsafe {
            for framebuffer in self.framebuffers.drain(..) {
                device.destroy_framebuffer(framebuffer);
            }
            if self.pipeline != vk::Pipeline::null() {
                device.destroy_pipeline(self.pipeline);
                self.pipeline = vk::Pipeline::null();
            }
            if self.pipeline_layout != vk::PipelineLayout::null() {
                device.destroy_pipeline_layout(self.pipeline_layout);
                self.pipeline_layout = vk::PipelineLayout::null();
            }
            if self.render_pass != vk::RenderPass::null() {
                device.destroy_render_pass(self.render_pass);
                self.render_pass = vk::RenderPass::null();
            }
            if self.depth_view != vk::ImageView::null() {
                device.destroy_image_view(self.depth_view);
                self.depth_view = vk::ImageView::null();
            }
            if self.depth_image != vk::Image::null() {
                device.destroy_image(self.depth_image);
                self.depth_image = vk::Image::null();
            }
            if self.depth_memory != vk::DeviceMemory::null() {
                device.free_memory(self.depth_memory);
                self.depth_memory = vk::DeviceMemory::null();
            }
            for resources in self.region_resources.drain(..) {
                release_raster_region_resources(device, resources);
            }
        }
        self.configured_attachments.clear();
        self.configuration_id = None;
    }
}

pub(super) struct StaticBuffer {
    buffer: vk::Buffer,
    memory: vk::DeviceMemory,
}

pub(super) fn upload_raster_region_resources(
    device: &RenderPathDeviceContext<'_>,
    region: &RasterRegionResult,
) -> Result<RasterRegionGpuResources, RasterResourceError> {
    let index_count =
        u32::try_from(region.indices().len()).map_err(|_| RasterResourceError::IndexCount)?;
    let vertex_bytes = raster_vertex_bytes(region.vertices());
    let vertex_buffer_bytes =
        u64::try_from(vertex_bytes.len()).map_err(|_| RasterResourceError::BufferSize("vertex"))?;
    let vertex = if vertex_bytes.is_empty() {
        None
    } else {
        Some(create_static_buffer(
            device,
            vertex_bytes,
            vk::BufferUsageFlags::VERTEX_BUFFER,
            "vertex",
        )?)
    };
    let index_bytes = u32_bytes(region.indices());
    let index_buffer_bytes =
        u64::try_from(index_bytes.len()).map_err(|_| RasterResourceError::BufferSize("index"))?;
    let index = if index_bytes.is_empty() {
        None
    } else {
        match create_static_buffer(
            device,
            index_bytes,
            vk::BufferUsageFlags::INDEX_BUFFER,
            "index",
        ) {
            Ok(index) => Some(index),
            Err(error) => {
                if let Some(vertex) = vertex {
                    unsafe {
                        device.destroy_buffer(vertex.buffer);
                        device.free_memory(vertex.memory);
                    }
                }
                return Err(error);
            }
        }
    };
    let mut resources = RasterRegionGpuResources {
        material_buffer_bytes: 0,
        material_buffer: vk::Buffer::null(),
        material_memory: vk::DeviceMemory::null(),
        material_layout: vk::DescriptorSetLayout::null(),
        material_pool: vk::DescriptorPool::null(),
        material_set: vk::DescriptorSet::null(),
        transform_constants: region.transform_constants(),
        identity: region.identity().clone(),
        vertex_buffer: vertex
            .as_ref()
            .map_or(vk::Buffer::null(), |vertex| vertex.buffer),
        vertex_memory: vertex
            .as_ref()
            .map_or(vk::DeviceMemory::null(), |vertex| vertex.memory),
        index_buffer: index
            .as_ref()
            .map_or(vk::Buffer::null(), |index| index.buffer),
        index_memory: index
            .as_ref()
            .map_or(vk::DeviceMemory::null(), |index| index.memory),
        index_count,
        vertex_buffer_bytes,
        index_buffer_bytes,
    };
    if index_count > 0
        && let Err(error) = upload_material_table(device, region, &mut resources)
    {
        release_raster_region_resources(device, resources);
        return Err(error);
    }
    Ok(resources)
}

pub(super) fn release_raster_region_resources(
    device: &RenderPathDeviceContext<'_>,
    resources: RasterRegionGpuResources,
) {
    unsafe {
        if resources.material_pool != vk::DescriptorPool::null() {
            device.destroy_descriptor_pool(resources.material_pool);
        }
        if resources.material_layout != vk::DescriptorSetLayout::null() {
            device.destroy_descriptor_set_layout(resources.material_layout);
        }
        if resources.material_buffer != vk::Buffer::null() {
            device.destroy_buffer(resources.material_buffer);
        }
        if resources.material_memory != vk::DeviceMemory::null() {
            device.free_memory(resources.material_memory);
        }
        if resources.index_buffer != vk::Buffer::null() {
            device.destroy_buffer(resources.index_buffer);
        }
        if resources.index_memory != vk::DeviceMemory::null() {
            device.free_memory(resources.index_memory);
        }
        if resources.vertex_buffer != vk::Buffer::null() {
            device.destroy_buffer(resources.vertex_buffer);
        }
        if resources.vertex_memory != vk::DeviceMemory::null() {
            device.free_memory(resources.vertex_memory);
        }
    }
}

fn create_material_layout(
    device: &RenderPathDeviceContext<'_>,
) -> Result<vk::DescriptorSetLayout, RasterResourceError> {
    let bindings = [vk::DescriptorSetLayoutBinding::default()
        .binding(0)
        .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
        .descriptor_count(1)
        .stage_flags(vk::ShaderStageFlags::VERTEX)];
    unsafe {
        device.create_descriptor_set_layout(
            &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings),
        )
    }
    .map_err(RasterResourceError::MaterialTable)
}

fn upload_material_table(
    device: &RenderPathDeviceContext<'_>,
    region: &RasterRegionResult,
    resources: &mut RasterRegionGpuResources,
) -> Result<(), RasterResourceError> {
    let material = create_static_buffer(
        device,
        f32_bytes(region.material_colors().as_flattened()),
        vk::BufferUsageFlags::STORAGE_BUFFER,
        "material",
    )?;
    resources.material_buffer = material.buffer;
    resources.material_buffer_bytes = std::mem::size_of_val(region.material_colors()) as u64;
    resources.material_memory = material.memory;
    let sizes = [vk::DescriptorPoolSize {
        ty: vk::DescriptorType::STORAGE_BUFFER,
        descriptor_count: 1,
    }];
    resources.material_pool = unsafe {
        device.create_descriptor_pool(
            &vk::DescriptorPoolCreateInfo::default()
                .max_sets(1)
                .pool_sizes(&sizes),
        )
    }
    .map_err(RasterResourceError::MaterialTable)?;
    resources.material_layout = create_material_layout(device)?;
    let layouts = [resources.material_layout];
    let sets = unsafe {
        device.allocate_descriptor_sets(
            &vk::DescriptorSetAllocateInfo::default()
                .descriptor_pool(resources.material_pool)
                .set_layouts(&layouts),
        )
    };
    resources.material_set = sets
        .map_err(RasterResourceError::MaterialTable)?
        .into_iter()
        .next()
        .ok_or(RasterResourceError::MaterialTable(
            vk::Result::ERROR_UNKNOWN,
        ))?;
    let buffers = [vk::DescriptorBufferInfo::default()
        .buffer(material.buffer)
        .offset(0)
        .range(vk::WHOLE_SIZE)];
    unsafe {
        device.update_descriptor_sets(&[vk::WriteDescriptorSet::default()
            .dst_set(resources.material_set)
            .dst_binding(0)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .buffer_info(&buffers)])
    };
    Ok(())
}

fn create_static_buffer(
    device: &RenderPathDeviceContext<'_>,
    bytes: &[u8],
    usage: vk::BufferUsageFlags,
    kind: &'static str,
) -> Result<StaticBuffer, RasterResourceError> {
    let size = u64::try_from(bytes.len()).map_err(|_| RasterResourceError::BufferSize(kind))?;
    let create_info = vk::BufferCreateInfo::default()
        .size(size)
        .usage(usage)
        .sharing_mode(vk::SharingMode::EXCLUSIVE);
    let buffer = unsafe { device.create_buffer(&create_info) }
        .map_err(|source| RasterResourceError::CreateBuffer { kind, source })?;
    let requirements = unsafe { device.buffer_memory_requirements(buffer) };
    let memory_type_index = match device.memory_type_index(
        requirements.memory_type_bits,
        vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
    ) {
        Some(index) => index,
        None => {
            unsafe { device.destroy_buffer(buffer) };
            return Err(RasterResourceError::MissingBufferMemory(kind));
        }
    };
    let allocate_info = vk::MemoryAllocateInfo::default()
        .allocation_size(requirements.size)
        .memory_type_index(memory_type_index);
    let memory = match unsafe { device.allocate_memory(&allocate_info) } {
        Ok(memory) => memory,
        Err(source) => {
            unsafe { device.destroy_buffer(buffer) };
            return Err(RasterResourceError::AllocateBufferMemory { kind, source });
        }
    };
    if let Err(source) = unsafe { device.bind_buffer_memory(buffer, memory) } {
        unsafe {
            device.free_memory(memory);
            device.destroy_buffer(buffer);
        }
        return Err(RasterResourceError::BindBufferMemory { kind, source });
    }
    if let Err(source) = unsafe { device.write_memory(memory, bytes) } {
        unsafe {
            device.destroy_buffer(buffer);
            device.free_memory(memory);
        }
        return Err(RasterResourceError::UploadBuffer { kind, source });
    }
    Ok(StaticBuffer { buffer, memory })
}

fn create_shader_module(
    device: &RenderPathDeviceContext<'_>,
    code: &[u32],
) -> Result<vk::ShaderModule, RasterResourceError> {
    let create_info = vk::ShaderModuleCreateInfo::default().code(code);
    unsafe { device.create_shader_module(&create_info) }
        .map_err(RasterResourceError::CreateShaderModule)
}

fn raster_vertex_bytes(values: &[RasterVertex]) -> &[u8] {
    let byte_length = std::mem::size_of_val(values);
    // RasterVertex is repr(C), contains only u16 values, and has no padding at its checked size.
    unsafe { std::slice::from_raw_parts(values.as_ptr().cast(), byte_length) }
}

fn u32_bytes(values: &[u32]) -> &[u8] {
    let byte_length = std::mem::size_of_val(values);
    // Every u32 bit pattern is initialized data and valid to read as bytes.
    unsafe { std::slice::from_raw_parts(values.as_ptr().cast(), byte_length) }
}

fn f32_bytes(values: &[f32]) -> &[u8] {
    let byte_length = std::mem::size_of_val(values);
    // Every f32 bit pattern is initialized data and valid to read as bytes.
    unsafe { std::slice::from_raw_parts(values.as_ptr().cast(), byte_length) }
}

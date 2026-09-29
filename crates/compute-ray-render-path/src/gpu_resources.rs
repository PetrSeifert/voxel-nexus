use super::capabilities::{
    ComputeCapabilityAssessment, ComputeCapabilityRecord, ComputeDispatchConfiguration,
};
use super::compute_convergence::{ComputeConvergenceController, ComputeConvergenceWorkStamp};
use super::compute_scene::ComputeSceneBundle;
use super::observation::{
    ComputeLifecycleController, ComputeMeasurementController, ComputeTimingEvent,
    ComputeTimingPhase,
};
use super::resource_error::ComputeRenderPathError;
use super::semantic_rays::{
    ComputeSemanticRayController, camera_storage_words, decode_semantic_ray_output,
    semantic_ray_input_words,
};
use super::{
    CAMERA_BUFFER_SIZE, SEMANTIC_RAY_BUFFER_SIZE, SEMANTIC_RAY_BUFFER_WORD_COUNT,
    compute_convergence,
};
use ash::vk;
use render_backend::{
    CameraState, PresentationConfigurationId, RenderPathAttachmentIdentity,
    RenderPathDeviceContext, RenderPathFrameContext, RenderPathTarget,
};
use semantic_ray_oracle::SemanticRayProbe;
use std::io::Cursor;
use std::time::Instant;
use voxel_frontend::VoxelSceneRevision;

pub(super) struct ComputeRayRenderPath {
    pub(super) convergence: compute_convergence::ComputeConvergence,
    pub(super) camera_state: CameraState,
    pub(super) capability_assessment: Option<ComputeCapabilityAssessment>,
    pub(super) output_image: vk::Image,
    pub(super) output_memory: vk::DeviceMemory,
    pub(super) output_allocation_bytes: u64,
    pub(super) output_view: vk::ImageView,
    pub(super) sampler: vk::Sampler,
    pub(super) scene_buffer: vk::Buffer,
    pub(super) scene_memory: vk::DeviceMemory,
    pub(super) scene_allocation_bytes: u64,
    pub(super) scene_gpu_revision: VoxelSceneRevision,
    pub(super) presentation_stopped: bool,
    pub(super) hidden_scene_gpu_resources: Option<ComputeHiddenSceneGpuResources>,
    pub(super) convergence_control: Option<ComputeConvergenceController>,
    pub(super) lifecycle_controller: Option<ComputeLifecycleController>,
    pub(super) camera_buffer: vk::Buffer,
    pub(super) camera_memory: vk::DeviceMemory,
    pub(super) camera_allocation_bytes: u64,
    pub(super) semantic_ray_buffer: vk::Buffer,
    pub(super) semantic_ray_memory: vk::DeviceMemory,
    pub(super) semantic_ray_allocation_bytes: u64,
    pub(super) semantic_ray_controller: Option<ComputeSemanticRayController>,
    armed_semantic_ray_probes: Option<Vec<SemanticRayProbe>>,
    recorded_semantic_ray_frame: Option<(u64, VoxelSceneRevision)>,
    pub(super) descriptor_set_layout: vk::DescriptorSetLayout,
    pub(super) descriptor_pool: vk::DescriptorPool,
    pub(super) descriptor_set: vk::DescriptorSet,
    pub(super) compute_pipeline_layout: vk::PipelineLayout,
    pub(super) compute_pipeline: vk::Pipeline,
    pub(super) render_pass: vk::RenderPass,
    pub(super) composite_pipeline_layout: vk::PipelineLayout,
    pub(super) composite_pipeline: vk::Pipeline,
    pub(super) framebuffers: Vec<vk::Framebuffer>,
    configured_attachments: Vec<RenderPathAttachmentIdentity>,
    pub(super) configuration_id: Option<PresentationConfigurationId>,
    pub(super) output_extent: vk::Extent2D,
    dispatch_group_count: [u32; 3],
    output_initialized: bool,
    measurement_controller: Option<ComputeMeasurementController>,
}

impl ComputeRayRenderPath {
    pub(super) fn new(
        scene_bundle: ComputeSceneBundle,
        camera_state: CameraState,
        measurement_controller: Option<ComputeMeasurementController>,
    ) -> Self {
        let scene_gpu_revision = scene_bundle.revision();
        Self {
            convergence: compute_convergence::ComputeConvergence::new(scene_bundle),
            camera_state,
            capability_assessment: None,
            output_image: vk::Image::null(),
            output_memory: vk::DeviceMemory::null(),
            output_allocation_bytes: 0,
            output_view: vk::ImageView::null(),
            sampler: vk::Sampler::null(),
            scene_buffer: vk::Buffer::null(),
            scene_memory: vk::DeviceMemory::null(),
            scene_allocation_bytes: 0,
            scene_gpu_revision,
            presentation_stopped: false,
            hidden_scene_gpu_resources: None,
            convergence_control: None,
            lifecycle_controller: None,
            camera_buffer: vk::Buffer::null(),
            camera_memory: vk::DeviceMemory::null(),
            camera_allocation_bytes: 0,
            semantic_ray_buffer: vk::Buffer::null(),
            semantic_ray_memory: vk::DeviceMemory::null(),
            semantic_ray_allocation_bytes: 0,
            semantic_ray_controller: None,
            armed_semantic_ray_probes: None,
            recorded_semantic_ray_frame: None,
            descriptor_set_layout: vk::DescriptorSetLayout::null(),
            descriptor_pool: vk::DescriptorPool::null(),
            descriptor_set: vk::DescriptorSet::null(),
            compute_pipeline_layout: vk::PipelineLayout::null(),
            compute_pipeline: vk::Pipeline::null(),
            render_pass: vk::RenderPass::null(),
            composite_pipeline_layout: vk::PipelineLayout::null(),
            composite_pipeline: vk::Pipeline::null(),
            framebuffers: Vec::new(),
            configured_attachments: Vec::new(),
            configuration_id: None,
            output_extent: vk::Extent2D::default(),
            dispatch_group_count: [0; 3],
            output_initialized: false,
            measurement_controller,
        }
    }

    pub(super) fn configure_resources(
        &mut self,
        device: &RenderPathDeviceContext<'_>,
        target: RenderPathTarget<'_>,
        qualification: ComputeCapabilityRecord,
    ) -> Result<(), ComputeRenderPathError> {
        let installation_started_at = Instant::now();
        if self.scene_buffer == vk::Buffer::null() || self.scene_memory == vk::DeviceMemory::null()
        {
            self.release_scene_resources(device);
            let upload_started_at = Instant::now();
            self.create_scene_buffer(device)?;
            self.record_timing_with_bytes(
                ComputeTimingPhase::Upload,
                self.convergence.status().installed(),
                upload_started_at,
                self.convergence.installed_bundle().storage_word_count() as u64 * 4,
            )?;
        }
        if self.semantic_ray_buffer == vk::Buffer::null()
            || self.semantic_ray_memory == vk::DeviceMemory::null()
        {
            self.create_semantic_ray_buffer(device)?;
        }
        self.create_output_resources(device, qualification.dispatch())?;
        self.create_descriptor_resources(device)?;
        self.create_compute_pipeline(device)?;
        self.create_render_pass(device, target.format())?;
        self.create_composite_pipeline(device, target.extent())?;
        for attachment in target.attachments() {
            let framebuffer = unsafe {
                device.create_color_framebuffer(self.render_pass, &attachment, target.extent())
            }
            .map_err(ComputeRenderPathError::CreateFramebuffer)?;
            self.framebuffers.push(framebuffer);
            self.configured_attachments.push(attachment.identity());
        }
        self.configuration_id = Some(target.configuration_id());
        self.output_extent = target.extent();
        self.dispatch_group_count = qualification.dispatch().group_count();
        self.record_timing(
            ComputeTimingPhase::Installation,
            self.convergence.status().installed(),
            installation_started_at,
        )?;
        Ok(())
    }

    pub(super) fn record_timing(
        &self,
        phase: ComputeTimingPhase,
        stamp: ComputeConvergenceWorkStamp,
        started_at: Instant,
    ) -> Result<(), ComputeRenderPathError> {
        self.record_timing_with_bytes(phase, stamp, started_at, 0)
    }

    pub(super) fn record_timing_with_bytes(
        &self,
        phase: ComputeTimingPhase,
        stamp: ComputeConvergenceWorkStamp,
        started_at: Instant,
        uploaded_bytes: u64,
    ) -> Result<(), ComputeRenderPathError> {
        let Some(controller) = &self.measurement_controller else {
            return Ok(());
        };
        controller.record(ComputeTimingEvent {
            phase,
            uploaded_bytes,
            scene_identity: self.convergence.installed_bundle().scene_identity().clone(),
            revision: stamp.revision(),
            generation: stamp.generation().value(),
            elapsed_milliseconds: started_at.elapsed().as_secs_f64() * 1_000.0,
        })?;
        Ok(())
    }

    fn create_output_resources(
        &mut self,
        device: &RenderPathDeviceContext<'_>,
        dispatch: ComputeDispatchConfiguration,
    ) -> Result<(), ComputeRenderPathError> {
        let extent = dispatch.output_extent();
        let image_info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(dispatch.output_format())
            .extent(vk::Extent3D {
                width: extent.width,
                height: extent.height,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(vk::ImageUsageFlags::STORAGE | vk::ImageUsageFlags::SAMPLED)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        self.output_image = unsafe { device.create_image(&image_info) }
            .map_err(ComputeRenderPathError::CreateOutputImage)?;
        let requirements = unsafe { device.image_memory_requirements(self.output_image) };
        let memory_type_index = device
            .memory_type_index(
                requirements.memory_type_bits,
                vk::MemoryPropertyFlags::DEVICE_LOCAL,
            )
            .ok_or(ComputeRenderPathError::MissingOutputMemory)?;
        let allocation_info = vk::MemoryAllocateInfo::default()
            .allocation_size(requirements.size)
            .memory_type_index(memory_type_index);
        self.output_memory = unsafe { device.allocate_memory(&allocation_info) }
            .map_err(ComputeRenderPathError::AllocateOutputMemory)?;
        self.output_allocation_bytes = requirements.size;
        unsafe { device.bind_image_memory(self.output_image, self.output_memory) }
            .map_err(ComputeRenderPathError::BindOutputMemory)?;
        let view_info = vk::ImageViewCreateInfo::default()
            .image(self.output_image)
            .view_type(vk::ImageViewType::TYPE_2D)
            .format(dispatch.output_format())
            .subresource_range(output_subresource_range());
        self.output_view = unsafe { device.create_image_view(&view_info) }
            .map_err(ComputeRenderPathError::CreateOutputView)?;
        let sampler_info = vk::SamplerCreateInfo::default()
            .mag_filter(vk::Filter::NEAREST)
            .min_filter(vk::Filter::NEAREST)
            .mipmap_mode(vk::SamplerMipmapMode::NEAREST)
            .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .min_lod(0.0)
            .max_lod(0.0);
        self.sampler = unsafe { device.create_sampler(&sampler_info) }
            .map_err(ComputeRenderPathError::CreateSampler)?;
        self.create_camera_buffer(device, extent)?;
        Ok(())
    }

    fn create_scene_buffer(
        &mut self,
        device: &RenderPathDeviceContext<'_>,
    ) -> Result<(), ComputeRenderPathError> {
        let bundle = self.convergence.installed_bundle();
        let resources =
            create_scene_gpu_resources(device, bundle, |allocation_bytes, staging_bytes| {
                bundle
                    .predict_allocation_peak(0, allocation_bytes, staging_bytes)
                    .map(|_| ())
            })?;
        self.scene_buffer = resources.buffer;
        self.scene_memory = resources.memory;
        self.scene_allocation_bytes = resources.allocation_bytes;
        self.scene_gpu_revision = self.convergence.installed_bundle().revision();
        Ok(())
    }

    fn create_camera_buffer(
        &mut self,
        device: &RenderPathDeviceContext<'_>,
        extent: vk::Extent2D,
    ) -> Result<(), ComputeRenderPathError> {
        let camera_words = camera_storage_words(self.camera_state, extent);
        let buffer_info = vk::BufferCreateInfo::default()
            .size(u64::from(CAMERA_BUFFER_SIZE))
            .usage(vk::BufferUsageFlags::STORAGE_BUFFER)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        self.camera_buffer = unsafe { device.create_buffer(&buffer_info) }
            .map_err(ComputeRenderPathError::CreateCameraBuffer)?;
        let requirements = unsafe { device.buffer_memory_requirements(self.camera_buffer) };
        let memory_type_index = device
            .memory_type_index(
                requirements.memory_type_bits,
                vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
            )
            .ok_or(ComputeRenderPathError::MissingCameraMemory)?;
        let allocation_info = vk::MemoryAllocateInfo::default()
            .allocation_size(requirements.size)
            .memory_type_index(memory_type_index);
        self.camera_memory = unsafe { device.allocate_memory(&allocation_info) }
            .map_err(ComputeRenderPathError::AllocateCameraMemory)?;
        self.camera_allocation_bytes = requirements.size;
        unsafe { device.bind_buffer_memory(self.camera_buffer, self.camera_memory) }
            .map_err(ComputeRenderPathError::BindCameraMemory)?;
        unsafe { device.write_memory(self.camera_memory, f32_bytes(&camera_words)) }
            .map_err(ComputeRenderPathError::WriteCameraMemory)?;
        Ok(())
    }

    pub(super) fn write_camera_state(
        &self,
        device: &RenderPathDeviceContext<'_>,
        extent: vk::Extent2D,
    ) -> Result<(), ComputeRenderPathError> {
        if self.camera_buffer == vk::Buffer::null()
            || self.camera_memory == vk::DeviceMemory::null()
        {
            return Err(ComputeRenderPathError::CameraResourcesUnavailable);
        }
        let camera_words = camera_storage_words(self.camera_state, extent);
        unsafe { device.write_memory(self.camera_memory, f32_bytes(&camera_words)) }
            .map_err(ComputeRenderPathError::WriteCameraMemory)
    }

    fn create_semantic_ray_buffer(
        &mut self,
        device: &RenderPathDeviceContext<'_>,
    ) -> Result<(), ComputeRenderPathError> {
        let buffer_info = vk::BufferCreateInfo::default()
            .size(u64::from(SEMANTIC_RAY_BUFFER_SIZE))
            .usage(vk::BufferUsageFlags::STORAGE_BUFFER)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        self.semantic_ray_buffer = unsafe { device.create_buffer(&buffer_info) }
            .map_err(ComputeRenderPathError::CreateSemanticRayBuffer)?;
        let requirements = unsafe { device.buffer_memory_requirements(self.semantic_ray_buffer) };
        let Some(memory_type_index) = device.memory_type_index(
            requirements.memory_type_bits,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
        ) else {
            unsafe { device.destroy_buffer(self.semantic_ray_buffer) };
            self.semantic_ray_buffer = vk::Buffer::null();
            return Err(ComputeRenderPathError::MissingSemanticRayMemory);
        };
        let allocation_info = vk::MemoryAllocateInfo::default()
            .allocation_size(requirements.size)
            .memory_type_index(memory_type_index);
        self.semantic_ray_memory = match unsafe { device.allocate_memory(&allocation_info) } {
            Ok(memory) => memory,
            Err(error) => {
                unsafe { device.destroy_buffer(self.semantic_ray_buffer) };
                self.semantic_ray_buffer = vk::Buffer::null();
                return Err(ComputeRenderPathError::AllocateSemanticRayMemory(error));
            }
        };
        self.semantic_ray_allocation_bytes = requirements.size;
        if let Err(error) =
            unsafe { device.bind_buffer_memory(self.semantic_ray_buffer, self.semantic_ray_memory) }
        {
            unsafe {
                device.destroy_buffer(self.semantic_ray_buffer);
                device.free_memory(self.semantic_ray_memory);
            }
            self.semantic_ray_buffer = vk::Buffer::null();
            self.semantic_ray_memory = vk::DeviceMemory::null();
            self.semantic_ray_allocation_bytes = 0;
            return Err(ComputeRenderPathError::BindSemanticRayMemory(error));
        }
        let words = [0_u32; SEMANTIC_RAY_BUFFER_WORD_COUNT];
        unsafe { device.write_memory(self.semantic_ray_memory, u32_bytes(&words)) }
            .map_err(ComputeRenderPathError::WriteSemanticRayMemory)
    }

    pub(super) fn stage_semantic_ray_request(
        &mut self,
        device: &RenderPathDeviceContext<'_>,
    ) -> Result<(), ComputeRenderPathError> {
        if self.armed_semantic_ray_probes.is_some() || self.recorded_semantic_ray_frame.is_some() {
            return Ok(());
        }
        let Some(controller) = &self.semantic_ray_controller else {
            return Ok(());
        };
        let Some(probes) = controller.take_pending()? else {
            return Ok(());
        };
        if self.semantic_ray_memory == vk::DeviceMemory::null() {
            return Err(ComputeRenderPathError::SemanticRayResourcesUnavailable);
        }
        let words = semantic_ray_input_words(&probes);
        unsafe { device.write_memory(self.semantic_ray_memory, u32_bytes(&words)) }
            .map_err(ComputeRenderPathError::WriteSemanticRayMemory)?;
        self.armed_semantic_ray_probes = Some(probes);
        Ok(())
    }

    pub(super) fn collect_semantic_ray_observations(
        &mut self,
        device: &RenderPathDeviceContext<'_>,
    ) -> Result<(), ComputeRenderPathError> {
        let Some((frame_sequence, revision)) = self.recorded_semantic_ray_frame.take() else {
            return Ok(());
        };
        let probes = self
            .armed_semantic_ray_probes
            .take()
            .ok_or(ComputeRenderPathError::SemanticRayResourcesUnavailable)?;
        if self.semantic_ray_memory == vk::DeviceMemory::null() {
            return Err(ComputeRenderPathError::SemanticRayResourcesUnavailable);
        }
        let mut words = [0_u32; SEMANTIC_RAY_BUFFER_WORD_COUNT];
        unsafe { device.read_memory(self.semantic_ray_memory, u32_bytes_mut(&mut words)) }
            .map_err(ComputeRenderPathError::ReadSemanticRayMemory)?;
        let observations = decode_semantic_ray_output(
            &probes,
            &words,
            self.convergence.installed_bundle(),
            revision,
            frame_sequence,
        )?;
        if let Some(controller) = &self.semantic_ray_controller {
            controller.retain(observations)?;
        }
        Ok(())
    }

    fn create_descriptor_resources(
        &mut self,
        device: &RenderPathDeviceContext<'_>,
    ) -> Result<(), ComputeRenderPathError> {
        let bindings = [
            vk::DescriptorSetLayoutBinding::default()
                .binding(0)
                .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(1)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT),
            vk::DescriptorSetLayoutBinding::default()
                .binding(2)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(3)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(4)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
        ];
        let layout_info = vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings);
        self.descriptor_set_layout = unsafe { device.create_descriptor_set_layout(&layout_info) }
            .map_err(ComputeRenderPathError::CreateDescriptorSetLayout)?;

        let pool_sizes = [
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::STORAGE_IMAGE)
                .descriptor_count(1),
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(1),
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(3),
        ];
        let pool_info = vk::DescriptorPoolCreateInfo::default()
            .max_sets(1)
            .pool_sizes(&pool_sizes);
        self.descriptor_pool = unsafe { device.create_descriptor_pool(&pool_info) }
            .map_err(ComputeRenderPathError::CreateDescriptorPool)?;
        let layouts = [self.descriptor_set_layout];
        let allocation_info = vk::DescriptorSetAllocateInfo::default()
            .descriptor_pool(self.descriptor_pool)
            .set_layouts(&layouts);
        self.descriptor_set = unsafe { device.allocate_descriptor_sets(&allocation_info) }
            .map_err(ComputeRenderPathError::AllocateDescriptorSet)?
            .pop()
            .ok_or(ComputeRenderPathError::MissingDescriptorSet)?;

        let storage_image = [vk::DescriptorImageInfo::default()
            .image_view(self.output_view)
            .image_layout(vk::ImageLayout::GENERAL)];
        let sampled_image = [vk::DescriptorImageInfo::default()
            .sampler(self.sampler)
            .image_view(self.output_view)
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
        let scene_buffer = [vk::DescriptorBufferInfo::default()
            .buffer(self.scene_buffer)
            .offset(0)
            .range(scene_storage_byte_size(
                device,
                self.convergence.installed_bundle(),
            )?)];
        let camera_buffer = [vk::DescriptorBufferInfo::default()
            .buffer(self.camera_buffer)
            .offset(0)
            .range(u64::from(CAMERA_BUFFER_SIZE))];
        let semantic_ray_buffer = [vk::DescriptorBufferInfo::default()
            .buffer(self.semantic_ray_buffer)
            .offset(0)
            .range(u64::from(SEMANTIC_RAY_BUFFER_SIZE))];
        let writes = [
            vk::WriteDescriptorSet::default()
                .dst_set(self.descriptor_set)
                .dst_binding(0)
                .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
                .image_info(&storage_image),
            vk::WriteDescriptorSet::default()
                .dst_set(self.descriptor_set)
                .dst_binding(1)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .image_info(&sampled_image),
            vk::WriteDescriptorSet::default()
                .dst_set(self.descriptor_set)
                .dst_binding(2)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .buffer_info(&scene_buffer),
            vk::WriteDescriptorSet::default()
                .dst_set(self.descriptor_set)
                .dst_binding(3)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .buffer_info(&camera_buffer),
            vk::WriteDescriptorSet::default()
                .dst_set(self.descriptor_set)
                .dst_binding(4)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .buffer_info(&semantic_ray_buffer),
        ];
        unsafe { device.update_descriptor_sets(&writes) };
        Ok(())
    }

    fn create_compute_pipeline(
        &mut self,
        device: &RenderPathDeviceContext<'_>,
    ) -> Result<(), ComputeRenderPathError> {
        let shader_code = read_shader(include_bytes!(concat!(
            env!("OUT_DIR"),
            "/dense_dda.comp.spv"
        )))?;
        let shader_module = create_shader_module(device, &shader_code)?;
        let result = self.create_compute_pipeline_with_module(device, shader_module);
        unsafe { device.destroy_shader_module(shader_module) };
        result
    }

    fn create_compute_pipeline_with_module(
        &mut self,
        device: &RenderPathDeviceContext<'_>,
        shader_module: vk::ShaderModule,
    ) -> Result<(), ComputeRenderPathError> {
        let descriptor_set_layouts = [self.descriptor_set_layout];
        let layout_info =
            vk::PipelineLayoutCreateInfo::default().set_layouts(&descriptor_set_layouts);
        self.compute_pipeline_layout = unsafe { device.create_pipeline_layout(&layout_info) }
            .map_err(ComputeRenderPathError::CreatePipelineLayout)?;
        let stage = vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::COMPUTE)
            .module(shader_module)
            .name(c"main");
        let pipeline_info = vk::ComputePipelineCreateInfo::default()
            .stage(stage)
            .layout(self.compute_pipeline_layout);
        match unsafe {
            device.create_compute_pipelines(vk::PipelineCache::null(), &[pipeline_info])
        } {
            Ok(mut pipelines) => {
                self.compute_pipeline =
                    pipelines
                        .pop()
                        .ok_or(ComputeRenderPathError::CreateComputePipeline(
                            vk::Result::ERROR_UNKNOWN,
                        ))?;
                Ok(())
            }
            Err((pipelines, error)) => {
                for pipeline in pipelines {
                    unsafe { device.destroy_pipeline(pipeline) };
                }
                Err(ComputeRenderPathError::CreateComputePipeline(error))
            }
        }
    }

    fn create_render_pass(
        &mut self,
        device: &RenderPathDeviceContext<'_>,
        color_format: vk::Format,
    ) -> Result<(), ComputeRenderPathError> {
        let color_attachment = vk::AttachmentDescription::default()
            .format(color_format)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::DONT_CARE)
            .store_op(vk::AttachmentStoreOp::STORE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .final_layout(vk::ImageLayout::PRESENT_SRC_KHR);
        let color_reference = vk::AttachmentReference::default()
            .attachment(0)
            .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);
        let color_references = [color_reference];
        let subpass = vk::SubpassDescription::default()
            .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
            .color_attachments(&color_references);
        let dependency = vk::SubpassDependency::default()
            .src_subpass(vk::SUBPASS_EXTERNAL)
            .dst_subpass(0)
            .src_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
            .dst_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
            .dst_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE);
        let attachments = [color_attachment];
        let subpasses = [subpass];
        let dependencies = [dependency];
        let render_pass_info = vk::RenderPassCreateInfo::default()
            .attachments(&attachments)
            .subpasses(&subpasses)
            .dependencies(&dependencies);
        self.render_pass = unsafe { device.create_render_pass(&render_pass_info) }
            .map_err(ComputeRenderPathError::CreateRenderPass)?;
        Ok(())
    }

    fn create_composite_pipeline(
        &mut self,
        device: &RenderPathDeviceContext<'_>,
        extent: vk::Extent2D,
    ) -> Result<(), ComputeRenderPathError> {
        let vertex_code = read_shader(include_bytes!(concat!(
            env!("OUT_DIR"),
            "/composite.vert.spv"
        )))?;
        let fragment_code = read_shader(include_bytes!(concat!(
            env!("OUT_DIR"),
            "/composite.frag.spv"
        )))?;
        let vertex_module = create_shader_module(device, &vertex_code)?;
        let fragment_module = match create_shader_module(device, &fragment_code) {
            Ok(module) => module,
            Err(error) => {
                unsafe { device.destroy_shader_module(vertex_module) };
                return Err(error);
            }
        };
        let result = self.create_composite_pipeline_with_modules(
            device,
            extent,
            vertex_module,
            fragment_module,
        );
        unsafe {
            device.destroy_shader_module(fragment_module);
            device.destroy_shader_module(vertex_module);
        }
        result
    }

    fn create_composite_pipeline_with_modules(
        &mut self,
        device: &RenderPathDeviceContext<'_>,
        extent: vk::Extent2D,
        vertex_module: vk::ShaderModule,
        fragment_module: vk::ShaderModule,
    ) -> Result<(), ComputeRenderPathError> {
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
        let vertex_input = vk::PipelineVertexInputStateCreateInfo::default();
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
            .cull_mode(vk::CullModeFlags::NONE)
            .line_width(1.0);
        let multisample = vk::PipelineMultisampleStateCreateInfo::default()
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);
        let color_attachments = [vk::PipelineColorBlendAttachmentState::default()
            .color_write_mask(vk::ColorComponentFlags::RGBA)];
        let color_blend =
            vk::PipelineColorBlendStateCreateInfo::default().attachments(&color_attachments);
        let descriptor_set_layouts = [self.descriptor_set_layout];
        let layout_info =
            vk::PipelineLayoutCreateInfo::default().set_layouts(&descriptor_set_layouts);
        self.composite_pipeline_layout = unsafe { device.create_pipeline_layout(&layout_info) }
            .map_err(ComputeRenderPathError::CreatePipelineLayout)?;
        let pipeline_info = vk::GraphicsPipelineCreateInfo::default()
            .stages(&stages)
            .vertex_input_state(&vertex_input)
            .input_assembly_state(&input_assembly)
            .viewport_state(&viewport_state)
            .rasterization_state(&rasterization)
            .multisample_state(&multisample)
            .color_blend_state(&color_blend)
            .layout(self.composite_pipeline_layout)
            .render_pass(self.render_pass)
            .subpass(0);
        match unsafe {
            device.create_graphics_pipelines(vk::PipelineCache::null(), &[pipeline_info])
        } {
            Ok(mut pipelines) => {
                self.composite_pipeline =
                    pipelines
                        .pop()
                        .ok_or(ComputeRenderPathError::CreateGraphicsPipeline(
                            vk::Result::ERROR_UNKNOWN,
                        ))?;
                Ok(())
            }
            Err((pipelines, error)) => {
                for pipeline in pipelines {
                    unsafe { device.destroy_pipeline(pipeline) };
                }
                Err(ComputeRenderPathError::CreateGraphicsPipeline(error))
            }
        }
    }

    pub(super) fn record_frame(
        &mut self,
        frame: &RenderPathFrameContext<'_>,
    ) -> Result<(), ComputeRenderPathError> {
        if self.presentation_stopped {
            return Err(ComputeRenderPathError::PresentationStopped);
        }
        let target = frame.target();
        if self.configuration_id != Some(target.configuration_id())
            || self.output_extent != target.extent()
        {
            return Err(ComputeRenderPathError::StaleFrameTarget);
        }
        let framebuffer_index = self
            .configured_attachments
            .iter()
            .position(|identity| *identity == target.attachment().identity())
            .ok_or(ComputeRenderPathError::MissingFramebuffer)?;
        let framebuffer = self
            .framebuffers
            .get(framebuffer_index)
            .copied()
            .ok_or(ComputeRenderPathError::MissingFramebuffer)?;

        let output_barrier = output_barrier_plan(self.output_initialized);
        let sampling_barrier = sampling_barrier_plan();
        let prepare_for_compute = [vk::ImageMemoryBarrier::default()
            .src_access_mask(output_barrier.source_access)
            .dst_access_mask(vk::AccessFlags::SHADER_WRITE)
            .old_layout(output_barrier.old_layout)
            .new_layout(vk::ImageLayout::GENERAL)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(self.output_image)
            .subresource_range(output_subresource_range())];
        let prepare_for_sampling = [vk::ImageMemoryBarrier::default()
            .src_access_mask(sampling_barrier.source_access)
            .dst_access_mask(sampling_barrier.destination_access)
            .old_layout(sampling_barrier.old_layout)
            .new_layout(sampling_barrier.new_layout)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(self.output_image)
            .subresource_range(output_subresource_range())];
        let render_pass_info = vk::RenderPassBeginInfo::default()
            .render_pass(self.render_pass)
            .framebuffer(framebuffer)
            .render_area(vk::Rect2D {
                offset: vk::Offset2D::default(),
                extent: target.extent(),
            });
        let descriptor_sets = [self.descriptor_set];
        let semantic_ray_host_write = [vk::BufferMemoryBarrier::default()
            .src_access_mask(vk::AccessFlags::HOST_WRITE)
            .dst_access_mask(vk::AccessFlags::SHADER_READ | vk::AccessFlags::SHADER_WRITE)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .buffer(self.semantic_ray_buffer)
            .offset(0)
            .size(u64::from(SEMANTIC_RAY_BUFFER_SIZE))];
        let semantic_ray_host_read = [vk::BufferMemoryBarrier::default()
            .src_access_mask(vk::AccessFlags::SHADER_WRITE)
            .dst_access_mask(vk::AccessFlags::HOST_READ)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .buffer(self.semantic_ray_buffer)
            .offset(0)
            .size(u64::from(SEMANTIC_RAY_BUFFER_SIZE))];
        let semantic_ray_request_is_armed = self.armed_semantic_ray_probes.is_some();

        let dispatch_started_at = Instant::now();
        unsafe {
            if semantic_ray_request_is_armed {
                frame.buffer_pipeline_barrier(
                    vk::PipelineStageFlags::HOST,
                    vk::PipelineStageFlags::COMPUTE_SHADER,
                    &semantic_ray_host_write,
                );
            }
            frame.image_pipeline_barrier(
                output_barrier.source_stage,
                vk::PipelineStageFlags::COMPUTE_SHADER,
                &prepare_for_compute,
            );
            frame.bind_pipeline(vk::PipelineBindPoint::COMPUTE, self.compute_pipeline);
            frame.bind_descriptor_sets(
                vk::PipelineBindPoint::COMPUTE,
                self.compute_pipeline_layout,
                &descriptor_sets,
            );
            frame.dispatch(self.dispatch_group_count);
            if semantic_ray_request_is_armed {
                frame.buffer_pipeline_barrier(
                    vk::PipelineStageFlags::COMPUTE_SHADER,
                    vk::PipelineStageFlags::HOST,
                    &semantic_ray_host_read,
                );
            }
            frame.image_pipeline_barrier(
                sampling_barrier.source_stage,
                sampling_barrier.destination_stage,
                &prepare_for_sampling,
            );
        }
        self.record_timing(
            ComputeTimingPhase::Dispatch,
            self.convergence.status().installed(),
            dispatch_started_at,
        )?;
        let composite_started_at = Instant::now();
        unsafe {
            frame.begin_render_pass(&render_pass_info, vk::SubpassContents::INLINE);
            frame.bind_pipeline(vk::PipelineBindPoint::GRAPHICS, self.composite_pipeline);
            frame.bind_descriptor_sets(
                vk::PipelineBindPoint::GRAPHICS,
                self.composite_pipeline_layout,
                &descriptor_sets,
            );
            frame.draw(3, 1, 0, 0);
            frame.end_render_pass();
        }
        self.record_timing(
            ComputeTimingPhase::Composite,
            self.convergence.status().installed(),
            composite_started_at,
        )?;
        if semantic_ray_request_is_armed && self.recorded_semantic_ray_frame.is_none() {
            self.recorded_semantic_ray_frame = Some((
                target.frame_sequence(),
                self.convergence.installed_bundle().revision(),
            ));
        }
        self.output_initialized = true;
        Ok(())
    }

    pub(super) fn release_presentation_resources(&mut self, device: &RenderPathDeviceContext<'_>) {
        unsafe {
            for framebuffer in self.framebuffers.drain(..) {
                device.destroy_framebuffer(framebuffer);
            }
            if self.composite_pipeline != vk::Pipeline::null() {
                device.destroy_pipeline(self.composite_pipeline);
                self.composite_pipeline = vk::Pipeline::null();
            }
            if self.composite_pipeline_layout != vk::PipelineLayout::null() {
                device.destroy_pipeline_layout(self.composite_pipeline_layout);
                self.composite_pipeline_layout = vk::PipelineLayout::null();
            }
            if self.render_pass != vk::RenderPass::null() {
                device.destroy_render_pass(self.render_pass);
                self.render_pass = vk::RenderPass::null();
            }
            if self.compute_pipeline != vk::Pipeline::null() {
                device.destroy_pipeline(self.compute_pipeline);
                self.compute_pipeline = vk::Pipeline::null();
            }
            if self.compute_pipeline_layout != vk::PipelineLayout::null() {
                device.destroy_pipeline_layout(self.compute_pipeline_layout);
                self.compute_pipeline_layout = vk::PipelineLayout::null();
            }
            if self.descriptor_pool != vk::DescriptorPool::null() {
                device.destroy_descriptor_pool(self.descriptor_pool);
                self.descriptor_pool = vk::DescriptorPool::null();
                self.descriptor_set = vk::DescriptorSet::null();
            }
            if self.sampler != vk::Sampler::null() {
                device.destroy_sampler(self.sampler);
                self.sampler = vk::Sampler::null();
            }
            if self.descriptor_set_layout != vk::DescriptorSetLayout::null() {
                device.destroy_descriptor_set_layout(self.descriptor_set_layout);
                self.descriptor_set_layout = vk::DescriptorSetLayout::null();
            }
            if self.output_view != vk::ImageView::null() {
                device.destroy_image_view(self.output_view);
                self.output_view = vk::ImageView::null();
            }
            if self.output_image != vk::Image::null() {
                device.destroy_image(self.output_image);
                self.output_image = vk::Image::null();
            }
            if self.output_memory != vk::DeviceMemory::null() {
                device.free_memory(self.output_memory);
                self.output_memory = vk::DeviceMemory::null();
            }
            self.output_allocation_bytes = 0;
            if self.camera_buffer != vk::Buffer::null() {
                device.destroy_buffer(self.camera_buffer);
                self.camera_buffer = vk::Buffer::null();
            }
            if self.camera_memory != vk::DeviceMemory::null() {
                device.free_memory(self.camera_memory);
                self.camera_memory = vk::DeviceMemory::null();
            }
            self.camera_allocation_bytes = 0;
        }
        self.configured_attachments.clear();
        self.configuration_id = None;
        self.output_extent = vk::Extent2D::default();
        self.dispatch_group_count = [0; 3];
        self.output_initialized = false;
    }

    pub(super) fn release_scene_resources(&mut self, device: &RenderPathDeviceContext<'_>) {
        if let Some(candidate) = self.hidden_scene_gpu_resources.take()
            && let Some(resources) = candidate.resources
        {
            release_scene_gpu_resources(device, resources);
        }
        unsafe {
            if self.scene_buffer != vk::Buffer::null() {
                device.destroy_buffer(self.scene_buffer);
                self.scene_buffer = vk::Buffer::null();
            }
            if self.scene_memory != vk::DeviceMemory::null() {
                device.free_memory(self.scene_memory);
                self.scene_memory = vk::DeviceMemory::null();
            }
            self.scene_allocation_bytes = 0;
            if self.semantic_ray_buffer != vk::Buffer::null() {
                device.destroy_buffer(self.semantic_ray_buffer);
                self.semantic_ray_buffer = vk::Buffer::null();
            }
            if self.semantic_ray_memory != vk::DeviceMemory::null() {
                device.free_memory(self.semantic_ray_memory);
                self.semantic_ray_memory = vk::DeviceMemory::null();
            }
            self.semantic_ray_allocation_bytes = 0;
        }
        self.armed_semantic_ray_probes = None;
        self.recorded_semantic_ray_frame = None;
    }
}

pub(super) struct ComputeSceneGpuResources {
    pub(super) buffer: vk::Buffer,
    pub(super) memory: vk::DeviceMemory,
    pub(super) allocation_bytes: u64,
}

pub(super) struct ComputeHiddenSceneGpuResources {
    pub(super) stamp: ComputeConvergenceWorkStamp,
    pub(super) resources: Option<ComputeSceneGpuResources>,
    pub(super) range: u64,
}

pub(super) fn scene_storage_byte_size(
    device: &RenderPathDeviceContext<'_>,
    bundle: &ComputeSceneBundle,
) -> Result<u64, ComputeRenderPathError> {
    bundle.validate_device_limits(
        u64::from(device.capabilities().max_storage_buffer_range),
        device.capabilities().max_buffer_size,
    )?;
    let byte_size = u64::try_from(
        bundle
            .storage_word_count()
            .checked_mul(4)
            .ok_or(ComputeRenderPathError::MissingHiddenCandidate)?,
    )
    .map_err(|_| ComputeRenderPathError::SceneStorageBufferRange {
        required: u64::MAX,
        available: device.capabilities().max_storage_buffer_range,
    })?;
    if byte_size > u64::from(device.capabilities().max_storage_buffer_range) {
        return Err(ComputeRenderPathError::SceneStorageBufferRange {
            required: byte_size,
            available: device.capabilities().max_storage_buffer_range,
        });
    }
    Ok(byte_size)
}

pub(super) fn create_scene_gpu_resources(
    device: &RenderPathDeviceContext<'_>,
    bundle: &ComputeSceneBundle,
    predict_allocation: impl FnOnce(u64, u64) -> Result<(), crate::ComputeSceneBuildError>,
) -> Result<ComputeSceneGpuResources, ComputeRenderPathError> {
    let byte_size = scene_storage_byte_size(device, bundle)?;
    let buffer_info = vk::BufferCreateInfo::default()
        .size(byte_size)
        .usage(vk::BufferUsageFlags::STORAGE_BUFFER)
        .sharing_mode(vk::SharingMode::EXCLUSIVE);
    let buffer = unsafe { device.create_buffer(&buffer_info) }
        .map_err(ComputeRenderPathError::CreateSceneBuffer)?;
    let requirements = unsafe { device.buffer_memory_requirements(buffer) };
    // Vulkan may round the requested buffer size up. Check its actual allocation
    // requirement before allocating either device memory or upload staging.
    if let Err(error) = predict_allocation(requirements.size, byte_size) {
        unsafe { device.destroy_buffer(buffer) };
        return Err(error.into());
    }
    let Some(memory_type_index) = device.memory_type_index(
        requirements.memory_type_bits,
        vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
    ) else {
        unsafe { device.destroy_buffer(buffer) };
        return Err(ComputeRenderPathError::MissingSceneMemory);
    };
    let allocation_info = vk::MemoryAllocateInfo::default()
        .allocation_size(requirements.size)
        .memory_type_index(memory_type_index);
    let memory = match unsafe { device.allocate_memory(&allocation_info) } {
        Ok(memory) => memory,
        Err(error) => {
            unsafe { device.destroy_buffer(buffer) };
            return Err(ComputeRenderPathError::AllocateSceneMemory(error));
        }
    };
    if let Err(error) = unsafe { device.bind_buffer_memory(buffer, memory) } {
        release_scene_gpu_resources(
            device,
            ComputeSceneGpuResources {
                buffer,
                memory,
                allocation_bytes: requirements.size,
            },
        );
        return Err(ComputeRenderPathError::BindSceneMemory(error));
    }
    let storage_words = bundle.storage_words();
    let bytes = u32_bytes(&storage_words);
    if let Err(error) = unsafe { device.write_memory(memory, bytes) } {
        release_scene_gpu_resources(
            device,
            ComputeSceneGpuResources {
                buffer,
                memory,
                allocation_bytes: requirements.size,
            },
        );
        return Err(ComputeRenderPathError::WriteSceneMemory(error));
    }
    Ok(ComputeSceneGpuResources {
        buffer,
        memory,
        allocation_bytes: requirements.size,
    })
}

pub(super) fn release_scene_gpu_resources(
    device: &RenderPathDeviceContext<'_>,
    resources: ComputeSceneGpuResources,
) {
    unsafe {
        if resources.buffer != vk::Buffer::null() {
            device.destroy_buffer(resources.buffer);
        }
        if resources.memory != vk::DeviceMemory::null() {
            device.free_memory(resources.memory);
        }
    }
}

fn output_subresource_range() -> vk::ImageSubresourceRange {
    vk::ImageSubresourceRange::default()
        .aspect_mask(vk::ImageAspectFlags::COLOR)
        .base_mip_level(0)
        .level_count(1)
        .base_array_layer(0)
        .layer_count(1)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct OutputBarrierPlan {
    pub(super) source_stage: vk::PipelineStageFlags,
    pub(super) source_access: vk::AccessFlags,
    pub(super) old_layout: vk::ImageLayout,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct SamplingBarrierPlan {
    pub(super) source_stage: vk::PipelineStageFlags,
    pub(super) destination_stage: vk::PipelineStageFlags,
    pub(super) source_access: vk::AccessFlags,
    pub(super) destination_access: vk::AccessFlags,
    pub(super) old_layout: vk::ImageLayout,
    pub(super) new_layout: vk::ImageLayout,
}

pub(super) fn output_barrier_plan(output_initialized: bool) -> OutputBarrierPlan {
    if output_initialized {
        OutputBarrierPlan {
            source_stage: vk::PipelineStageFlags::FRAGMENT_SHADER,
            source_access: vk::AccessFlags::SHADER_READ,
            old_layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        }
    } else {
        OutputBarrierPlan {
            source_stage: vk::PipelineStageFlags::TOP_OF_PIPE,
            source_access: vk::AccessFlags::empty(),
            old_layout: vk::ImageLayout::UNDEFINED,
        }
    }
}

pub(super) fn sampling_barrier_plan() -> SamplingBarrierPlan {
    SamplingBarrierPlan {
        source_stage: vk::PipelineStageFlags::COMPUTE_SHADER,
        destination_stage: vk::PipelineStageFlags::FRAGMENT_SHADER,
        source_access: vk::AccessFlags::SHADER_WRITE,
        destination_access: vk::AccessFlags::SHADER_READ,
        old_layout: vk::ImageLayout::GENERAL,
        new_layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
    }
}

fn read_shader(bytes: &[u8]) -> Result<Vec<u32>, std::io::Error> {
    ash::util::read_spv(&mut Cursor::new(bytes))
}

fn f32_bytes(values: &[f32]) -> &[u8] {
    let byte_length = std::mem::size_of_val(values);
    // SAFETY: Every f32 bit pattern is initialized data and valid to read as bytes.
    unsafe { std::slice::from_raw_parts(values.as_ptr().cast(), byte_length) }
}

pub(super) fn u32_bytes(values: &[u32]) -> &[u8] {
    let byte_length = std::mem::size_of_val(values);
    // SAFETY: Every u32 bit pattern is initialized data and valid to read as bytes.
    unsafe { std::slice::from_raw_parts(values.as_ptr().cast(), byte_length) }
}

fn u32_bytes_mut(values: &mut [u32]) -> &mut [u8] {
    let byte_length = std::mem::size_of_val(values);
    // SAFETY: Every u32 bit pattern is valid and the mapped read initializes all requested bytes.
    unsafe { std::slice::from_raw_parts_mut(values.as_mut_ptr().cast(), byte_length) }
}

fn create_shader_module(
    device: &RenderPathDeviceContext<'_>,
    code: &[u32],
) -> Result<vk::ShaderModule, ComputeRenderPathError> {
    let create_info = vk::ShaderModuleCreateInfo::default().code(code);
    unsafe { device.create_shader_module(&create_info) }
        .map_err(ComputeRenderPathError::CreateShaderModule)
}

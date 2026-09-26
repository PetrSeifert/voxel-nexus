use super::{CAMERA_BUFFER_SIZE, OUTPUT_FORMAT, SEMANTIC_RAY_BUFFER_SIZE, WORKGROUP_SIZE};
use ash::vk;
use render_backend::RenderPathDeviceCapabilities;
use thiserror::Error;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ComputeDispatchConfiguration {
    output_extent: vk::Extent2D,
    output_format: vk::Format,
    workgroup_size: [u32; 3],
    group_count: [u32; 3],
}

impl ComputeDispatchConfiguration {
    pub fn output_extent(&self) -> vk::Extent2D {
        self.output_extent
    }

    pub fn output_format(&self) -> vk::Format {
        self.output_format
    }

    pub fn workgroup_size(&self) -> [u32; 3] {
        self.workgroup_size
    }

    pub fn group_count(&self) -> [u32; 3] {
        self.group_count
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ComputeCapabilityRecord {
    queried: RenderPathDeviceCapabilities,
    dispatch: ComputeDispatchConfiguration,
}

impl ComputeCapabilityRecord {
    pub fn queried(&self) -> RenderPathDeviceCapabilities {
        self.queried
    }

    pub fn dispatch(&self) -> ComputeDispatchConfiguration {
        self.dispatch
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ComputeDescriptorRequirement {
    BoundSet,
    StorageImagePerStage,
    StorageImagePerSet,
    StorageBufferPerStage,
    StorageBufferPerSet,
    SampledImagePerStage,
    SampledImagePerSet,
    SamplerPerStage,
    SamplerPerSet,
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum ComputeRenderPathRejection {
    #[error(
        "the device exposes Vulkan {major}.{minor}, but the compute Render Path requires Vulkan 1.3"
    )]
    VulkanApiVersion { major: u32, minor: u32 },
    #[error("the Render Backend command queue does not support graphics")]
    GraphicsQueueUnavailable,
    #[error("the Render Backend command queue does not support compute")]
    ComputeQueueUnavailable,
    #[error("the compute output extent must have nonzero width and height, got {width}x{height}")]
    EmptyOutputDimensions { width: u32, height: u32 },
    #[error(
        "the output extent {width}x{height} exceeds the maximum two-dimensional image dimension {maximum}"
    )]
    OutputDimensions {
        width: u32,
        height: u32,
        maximum: u32,
    },
    #[error(
        "the {requirement:?} descriptor limit is {available}, but {required} bindings are required"
    )]
    DescriptorBindingRange {
        requirement: ComputeDescriptorRequirement,
        required: u32,
        available: u32,
    },
    #[error(
        "the maximum storage-buffer binding range is {available} bytes, but {required} bytes are required"
    )]
    StorageBufferBindingRange { required: u32, available: u32 },
    #[error(
        "{format:?} lacks optimal-tiling storage-image and sampled-image support; available features are {available:?}"
    )]
    OutputFormat {
        format: vk::Format,
        available: vk::FormatFeatureFlags,
    },
    #[error(
        "the chosen workgroup size {chosen:?} with {chosen_invocations} invocations exceeds the device limits {maximum_size:?} and {maximum_invocations} invocations"
    )]
    WorkgroupSize {
        chosen: [u32; 3],
        chosen_invocations: u32,
        maximum_size: [u32; 3],
        maximum_invocations: u32,
    },
    #[error("the required dispatch group count {required:?} exceeds the device limit {maximum:?}")]
    DispatchGroupCount {
        required: [u32; 3],
        maximum: [u32; 3],
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ComputeCapabilityAssessment {
    Qualified(ComputeCapabilityRecord),
    Rejected {
        queried: RenderPathDeviceCapabilities,
        output_extent: vk::Extent2D,
        reason: ComputeRenderPathRejection,
    },
}

pub fn qualify_compute_render_path(
    queried: RenderPathDeviceCapabilities,
    output_extent: vk::Extent2D,
) -> Result<ComputeCapabilityRecord, ComputeRenderPathRejection> {
    if queried.api_version < vk::API_VERSION_1_3 {
        return Err(ComputeRenderPathRejection::VulkanApiVersion {
            major: vk::api_version_major(queried.api_version),
            minor: vk::api_version_minor(queried.api_version),
        });
    }
    if !queried
        .command_queue_flags
        .contains(vk::QueueFlags::GRAPHICS)
    {
        return Err(ComputeRenderPathRejection::GraphicsQueueUnavailable);
    }
    if !queried
        .command_queue_flags
        .contains(vk::QueueFlags::COMPUTE)
    {
        return Err(ComputeRenderPathRejection::ComputeQueueUnavailable);
    }
    if output_extent.width == 0 || output_extent.height == 0 {
        return Err(ComputeRenderPathRejection::EmptyOutputDimensions {
            width: output_extent.width,
            height: output_extent.height,
        });
    }
    if output_extent.width > queried.max_image_dimension_2d
        || output_extent.height > queried.max_image_dimension_2d
    {
        return Err(ComputeRenderPathRejection::OutputDimensions {
            width: output_extent.width,
            height: output_extent.height,
            maximum: queried.max_image_dimension_2d,
        });
    }

    let descriptor_limits = [
        (
            ComputeDescriptorRequirement::BoundSet,
            queried.max_bound_descriptor_sets,
            1,
        ),
        (
            ComputeDescriptorRequirement::StorageImagePerStage,
            queried.max_per_stage_descriptor_storage_images,
            1,
        ),
        (
            ComputeDescriptorRequirement::StorageImagePerSet,
            queried.max_descriptor_set_storage_images,
            1,
        ),
        (
            ComputeDescriptorRequirement::StorageBufferPerStage,
            queried.max_per_stage_descriptor_storage_buffers,
            3,
        ),
        (
            ComputeDescriptorRequirement::StorageBufferPerSet,
            queried.max_descriptor_set_storage_buffers,
            3,
        ),
        (
            ComputeDescriptorRequirement::SampledImagePerStage,
            queried.max_per_stage_descriptor_sampled_images,
            1,
        ),
        (
            ComputeDescriptorRequirement::SampledImagePerSet,
            queried.max_descriptor_set_sampled_images,
            1,
        ),
        (
            ComputeDescriptorRequirement::SamplerPerStage,
            queried.max_per_stage_descriptor_samplers,
            1,
        ),
        (
            ComputeDescriptorRequirement::SamplerPerSet,
            queried.max_descriptor_set_samplers,
            1,
        ),
    ];
    if let Some((requirement, available, required)) = descriptor_limits
        .into_iter()
        .find(|(_, available, required)| available < required)
    {
        return Err(ComputeRenderPathRejection::DescriptorBindingRange {
            requirement,
            required,
            available,
        });
    }
    let required_storage_buffer_range = CAMERA_BUFFER_SIZE.max(SEMANTIC_RAY_BUFFER_SIZE);
    if queried.max_storage_buffer_range < required_storage_buffer_range {
        return Err(ComputeRenderPathRejection::StorageBufferBindingRange {
            required: required_storage_buffer_range,
            available: queried.max_storage_buffer_range,
        });
    }

    let required_format_features =
        vk::FormatFeatureFlags::STORAGE_IMAGE | vk::FormatFeatureFlags::SAMPLED_IMAGE;
    if !queried
        .rgba8_unorm_optimal_tiling_features
        .contains(required_format_features)
    {
        return Err(ComputeRenderPathRejection::OutputFormat {
            format: OUTPUT_FORMAT,
            available: queried.rgba8_unorm_optimal_tiling_features,
        });
    }

    let chosen_invocations = WORKGROUP_SIZE.into_iter().product();
    if WORKGROUP_SIZE
        .into_iter()
        .zip(queried.max_compute_work_group_size)
        .any(|(chosen, maximum)| chosen > maximum)
        || chosen_invocations > queried.max_compute_work_group_invocations
    {
        return Err(ComputeRenderPathRejection::WorkgroupSize {
            chosen: WORKGROUP_SIZE,
            chosen_invocations,
            maximum_size: queried.max_compute_work_group_size,
            maximum_invocations: queried.max_compute_work_group_invocations,
        });
    }

    let [workgroup_width, workgroup_height, _] = WORKGROUP_SIZE;
    let group_count = [
        output_extent.width.div_ceil(workgroup_width),
        output_extent.height.div_ceil(workgroup_height),
        1,
    ];
    if group_count
        .into_iter()
        .zip(queried.max_compute_work_group_count)
        .any(|(required, maximum)| required > maximum)
    {
        return Err(ComputeRenderPathRejection::DispatchGroupCount {
            required: group_count,
            maximum: queried.max_compute_work_group_count,
        });
    }

    Ok(ComputeCapabilityRecord {
        queried,
        dispatch: ComputeDispatchConfiguration {
            output_extent,
            output_format: OUTPUT_FORMAT,
            workgroup_size: WORKGROUP_SIZE,
            group_count,
        },
    })
}

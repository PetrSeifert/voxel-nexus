use ash::vk;
use compute_ray_render_path::{
    ComputeDescriptorRequirement, ComputeRenderPathRejection, qualify_compute_render_path,
};
use render_backend::RenderPathDeviceCapabilities;

type CapabilityMutation = fn(&mut RenderPathDeviceCapabilities);

fn capable_device() -> RenderPathDeviceCapabilities {
    RenderPathDeviceCapabilities {
        api_version: vk::API_VERSION_1_3,
        command_queue_flags: vk::QueueFlags::GRAPHICS | vk::QueueFlags::COMPUTE,
        max_image_dimension_2d: 16_384,
        max_bound_descriptor_sets: 4,
        max_per_stage_descriptor_storage_images: 4,
        max_per_stage_descriptor_storage_buffers: 4,
        max_per_stage_descriptor_sampled_images: 16,
        max_per_stage_descriptor_samplers: 16,
        max_descriptor_set_storage_images: 4,
        max_descriptor_set_storage_buffers: 4,
        max_descriptor_set_sampled_images: 16,
        max_descriptor_set_samplers: 16,
        max_compute_work_group_count: [65_535, 65_535, 65_535],
        max_compute_work_group_invocations: 1_024,
        max_compute_work_group_size: [1_024, 1_024, 64],
        max_storage_buffer_range: 1 << 27,
        rgba8_unorm_optimal_tiling_features: vk::FormatFeatureFlags::STORAGE_IMAGE
            | vk::FormatFeatureFlags::SAMPLED_IMAGE,
    }
}

#[test]
fn records_queried_facts_and_a_dispatch_that_covers_the_output()
-> Result<(), Box<dyn std::error::Error>> {
    let queried = capable_device();
    let extent = vk::Extent2D {
        width: 101,
        height: 65,
    };

    let record = qualify_compute_render_path(queried, extent)?;

    assert_eq!(record.queried(), queried);
    assert_eq!(record.dispatch().output_extent(), extent);
    assert_eq!(
        record.dispatch().output_format(),
        vk::Format::R8G8B8A8_UNORM
    );
    assert_eq!(record.dispatch().workgroup_size(), [8, 8, 1]);
    assert_eq!(record.dispatch().group_count(), [13, 9, 1]);
    assert!(13 * 8 >= extent.width);
    assert!(9 * 8 >= extent.height);
    Ok(())
}

#[test]
fn rejects_a_command_queue_without_compute_support() {
    let mut queried = capable_device();
    queried.command_queue_flags = vk::QueueFlags::GRAPHICS;

    assert_eq!(
        qualify_compute_render_path(
            queried,
            vk::Extent2D {
                width: 800,
                height: 600,
            }
        ),
        Err(ComputeRenderPathRejection::ComputeQueueUnavailable)
    );
}

#[test]
fn rejects_an_old_api_or_a_command_queue_without_graphics_support() {
    let mut queried = capable_device();
    queried.api_version = vk::make_api_version(0, 1, 2, 0);
    assert_eq!(
        qualify_compute_render_path(
            queried,
            vk::Extent2D {
                width: 800,
                height: 600,
            }
        ),
        Err(ComputeRenderPathRejection::VulkanApiVersion { major: 1, minor: 2 })
    );

    let mut queried = capable_device();
    queried.command_queue_flags = vk::QueueFlags::COMPUTE;
    assert_eq!(
        qualify_compute_render_path(
            queried,
            vk::Extent2D {
                width: 800,
                height: 600,
            }
        ),
        Err(ComputeRenderPathRejection::GraphicsQueueUnavailable)
    );
}

#[test]
fn rejects_zero_or_oversized_output_dimensions() {
    let queried = capable_device();
    assert_eq!(
        qualify_compute_render_path(
            queried,
            vk::Extent2D {
                width: 0,
                height: 600,
            }
        ),
        Err(ComputeRenderPathRejection::EmptyOutputDimensions {
            width: 0,
            height: 600,
        })
    );

    let mut queried = capable_device();
    queried.max_image_dimension_2d = 1_024;
    assert_eq!(
        qualify_compute_render_path(
            queried,
            vk::Extent2D {
                width: 1_025,
                height: 600,
            }
        ),
        Err(ComputeRenderPathRejection::OutputDimensions {
            width: 1_025,
            height: 600,
            maximum: 1_024,
        })
    );
}

#[test]
fn rejects_each_descriptor_binding_limit_that_the_two_stages_use() {
    let cases: [(ComputeDescriptorRequirement, u32, CapabilityMutation); 9] = [
        (
            ComputeDescriptorRequirement::BoundSet,
            1,
            |capabilities: &mut RenderPathDeviceCapabilities| {
                capabilities.max_bound_descriptor_sets = 0;
            },
        ),
        (
            ComputeDescriptorRequirement::StorageImagePerStage,
            1,
            |capabilities: &mut RenderPathDeviceCapabilities| {
                capabilities.max_per_stage_descriptor_storage_images = 0;
            },
        ),
        (
            ComputeDescriptorRequirement::StorageImagePerSet,
            1,
            |capabilities: &mut RenderPathDeviceCapabilities| {
                capabilities.max_descriptor_set_storage_images = 0;
            },
        ),
        (
            ComputeDescriptorRequirement::StorageBufferPerStage,
            2,
            |capabilities: &mut RenderPathDeviceCapabilities| {
                capabilities.max_per_stage_descriptor_storage_buffers = 0;
            },
        ),
        (
            ComputeDescriptorRequirement::StorageBufferPerSet,
            2,
            |capabilities: &mut RenderPathDeviceCapabilities| {
                capabilities.max_descriptor_set_storage_buffers = 0;
            },
        ),
        (
            ComputeDescriptorRequirement::SampledImagePerStage,
            1,
            |capabilities: &mut RenderPathDeviceCapabilities| {
                capabilities.max_per_stage_descriptor_sampled_images = 0;
            },
        ),
        (
            ComputeDescriptorRequirement::SampledImagePerSet,
            1,
            |capabilities: &mut RenderPathDeviceCapabilities| {
                capabilities.max_descriptor_set_sampled_images = 0;
            },
        ),
        (
            ComputeDescriptorRequirement::SamplerPerStage,
            1,
            |capabilities: &mut RenderPathDeviceCapabilities| {
                capabilities.max_per_stage_descriptor_samplers = 0;
            },
        ),
        (
            ComputeDescriptorRequirement::SamplerPerSet,
            1,
            |capabilities: &mut RenderPathDeviceCapabilities| {
                capabilities.max_descriptor_set_samplers = 0;
            },
        ),
    ];

    for (requirement, required, mutate) in cases {
        let mut queried = capable_device();
        mutate(&mut queried);
        assert_eq!(
            qualify_compute_render_path(
                queried,
                vk::Extent2D {
                    width: 800,
                    height: 600,
                }
            ),
            Err(ComputeRenderPathRejection::DescriptorBindingRange {
                requirement,
                required,
                available: 0,
            })
        );
    }
}

#[test]
fn rejects_a_storage_buffer_binding_range_smaller_than_the_camera_state() {
    let mut queried = capable_device();
    queried.max_storage_buffer_range = 79;

    assert_eq!(
        qualify_compute_render_path(
            queried,
            vk::Extent2D {
                width: 800,
                height: 600,
            }
        ),
        Err(ComputeRenderPathRejection::StorageBufferBindingRange {
            required: 80,
            available: 79,
        })
    );
}

#[test]
fn rejects_an_output_format_without_both_storage_and_sampling_support() {
    let mut queried = capable_device();
    queried.rgba8_unorm_optimal_tiling_features = vk::FormatFeatureFlags::SAMPLED_IMAGE;

    assert_eq!(
        qualify_compute_render_path(
            queried,
            vk::Extent2D {
                width: 800,
                height: 600,
            }
        ),
        Err(ComputeRenderPathRejection::OutputFormat {
            format: vk::Format::R8G8B8A8_UNORM,
            available: vk::FormatFeatureFlags::SAMPLED_IMAGE,
        })
    );
}

#[test]
fn rejects_the_fixed_workgroup_or_required_dispatch_when_limits_are_too_small() {
    let mut queried = capable_device();
    queried.max_compute_work_group_invocations = 32;
    assert_eq!(
        qualify_compute_render_path(
            queried,
            vk::Extent2D {
                width: 800,
                height: 600,
            }
        ),
        Err(ComputeRenderPathRejection::WorkgroupSize {
            chosen: [8, 8, 1],
            chosen_invocations: 64,
            maximum_size: [1_024, 1_024, 64],
            maximum_invocations: 32,
        })
    );

    let mut queried = capable_device();
    queried.max_compute_work_group_count = [10, 10, 1];
    assert_eq!(
        qualify_compute_render_path(
            queried,
            vk::Extent2D {
                width: 81,
                height: 80,
            }
        ),
        Err(ComputeRenderPathRejection::DispatchGroupCount {
            required: [11, 10, 1],
            maximum: [10, 10, 1],
        })
    );
}

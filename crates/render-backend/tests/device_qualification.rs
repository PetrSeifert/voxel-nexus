use render_backend::{
    DeviceCandidate, DeviceRequirement, DeviceSelectionError, QueueFamilyCapabilities,
};

fn fully_capable_candidate() -> DeviceCandidate {
    DeviceCandidate {
        name: "Capable GPU".to_owned(),
        api_version: ash::vk::API_VERSION_1_3,
        driver_version: 1,
        supports_swapchain: true,
        has_surface_formats: true,
        has_present_modes: true,
        queue_families: vec![QueueFamilyCapabilities {
            supports_graphics: true,
            supports_presentation: true,
        }],
    }
}

fn rejection_for(
    candidate: DeviceCandidate,
) -> Result<DeviceSelectionError, Box<dyn std::error::Error>> {
    match render_backend::select_device(vec![candidate]) {
        Ok(candidate) => Err(format!("{} should have been rejected", candidate.name).into()),
        Err(error) => Ok(error),
    }
}

fn first_rejection(
    error: &DeviceSelectionError,
) -> Result<&render_backend::DeviceRejection, Box<dyn std::error::Error>> {
    error
        .candidates
        .first()
        .ok_or_else(|| "the rejected candidate should have diagnostic context".into())
}

#[test]
fn rejects_devices_below_vulkan_1_3() -> Result<(), Box<dyn std::error::Error>> {
    let candidate = DeviceCandidate {
        name: "Vulkan 1.2 GPU".to_owned(),
        api_version: ash::vk::make_api_version(0, 1, 2, 0),
        driver_version: 1,
        supports_swapchain: true,
        has_surface_formats: true,
        has_present_modes: true,
        queue_families: vec![QueueFamilyCapabilities {
            supports_graphics: true,
            supports_presentation: true,
        }],
    };

    let error = rejection_for(candidate)?;

    assert_eq!(
        error,
        DeviceSelectionError {
            candidates: vec![render_backend::DeviceRejection {
                device_name: "Vulkan 1.2 GPU".to_owned(),
                unmet_requirements: vec![DeviceRequirement::VulkanApi13 {
                    available_version: ash::vk::make_api_version(0, 1, 2, 0),
                }],
            }],
        }
    );
    assert_eq!(
        error.to_string(),
        "no suitable Vulkan device was found:\n- Vulkan 1.2 GPU: supports Vulkan 1.2.0, but Vulkan 1.3 or newer is required; update the graphics driver or use a Vulkan 1.3-capable GPU"
    );
    Ok(())
}

#[test]
fn rejects_devices_without_swapchain_support() -> Result<(), Box<dyn std::error::Error>> {
    let mut candidate = fully_capable_candidate();
    candidate.supports_swapchain = false;

    let error = rejection_for(candidate)?;

    assert_eq!(
        first_rejection(&error)?.unmet_requirements,
        vec![DeviceRequirement::SwapchainExtension]
    );
    Ok(())
}

#[test]
fn rejects_devices_without_surface_formats() -> Result<(), Box<dyn std::error::Error>> {
    let mut candidate = fully_capable_candidate();
    candidate.has_surface_formats = false;

    let error = rejection_for(candidate)?;

    assert_eq!(
        first_rejection(&error)?.unmet_requirements,
        vec![DeviceRequirement::SurfaceFormats]
    );
    Ok(())
}

#[test]
fn rejects_devices_without_present_modes() -> Result<(), Box<dyn std::error::Error>> {
    let mut candidate = fully_capable_candidate();
    candidate.has_present_modes = false;

    let error = rejection_for(candidate)?;

    assert_eq!(
        first_rejection(&error)?.unmet_requirements,
        vec![DeviceRequirement::PresentModes]
    );
    Ok(())
}

#[test]
fn rejects_devices_without_a_presentation_queue() -> Result<(), Box<dyn std::error::Error>> {
    let mut candidate = fully_capable_candidate();
    candidate.queue_families = vec![QueueFamilyCapabilities {
        supports_graphics: true,
        supports_presentation: false,
    }];

    let error = rejection_for(candidate)?;

    assert_eq!(
        first_rejection(&error)?.unmet_requirements,
        vec![DeviceRequirement::PresentationQueue]
    );
    Ok(())
}

#[test]
fn reports_every_missing_presentation_requirement() -> Result<(), Box<dyn std::error::Error>> {
    let candidate = DeviceCandidate {
        name: "Compute-only GPU".to_owned(),
        api_version: ash::vk::API_VERSION_1_3,
        driver_version: 1,
        supports_swapchain: false,
        has_surface_formats: false,
        has_present_modes: false,
        queue_families: vec![QueueFamilyCapabilities {
            supports_graphics: true,
            supports_presentation: false,
        }],
    };

    let error = rejection_for(candidate)?;

    assert_eq!(
        first_rejection(&error)?.unmet_requirements,
        vec![
            DeviceRequirement::SwapchainExtension,
            DeviceRequirement::SurfaceFormats,
            DeviceRequirement::PresentModes,
            DeviceRequirement::PresentationQueue,
        ]
    );
    assert_eq!(
        error.to_string(),
        "no suitable Vulkan device was found:\n- Compute-only GPU: the VK_KHR_swapchain device extension is unavailable; the presentation surface exposes no image formats; the presentation surface exposes no presentation modes; no queue family can present to the window surface; update the graphics driver or use a GPU and desktop session with Vulkan presentation support"
    );
    Ok(())
}

#[test]
fn accepts_separate_graphics_and_presentation_queues() {
    let mut candidate = fully_capable_candidate();
    candidate.queue_families = vec![
        QueueFamilyCapabilities {
            supports_graphics: true,
            supports_presentation: false,
        },
        QueueFamilyCapabilities {
            supports_graphics: false,
            supports_presentation: true,
        },
    ];

    let result = render_backend::select_device(vec![candidate.clone()]);

    assert_eq!(result, Ok(candidate));
}

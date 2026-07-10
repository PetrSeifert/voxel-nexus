use render_backend::{DeviceCandidate, DeviceSelectionError, QueueFamilyCapabilities};

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

#[test]
fn rejects_devices_below_vulkan_1_3() {
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

    let result = render_backend::select_device(vec![candidate]);

    assert_eq!(result, Err(DeviceSelectionError::NoSuitableDevice));
}

#[test]
fn rejects_devices_without_swapchain_support() {
    let mut candidate = fully_capable_candidate();
    candidate.supports_swapchain = false;

    let result = render_backend::select_device(vec![candidate]);

    assert_eq!(result, Err(DeviceSelectionError::NoSuitableDevice));
}

#[test]
fn rejects_devices_without_surface_formats() {
    let mut candidate = fully_capable_candidate();
    candidate.has_surface_formats = false;

    let result = render_backend::select_device(vec![candidate]);

    assert_eq!(result, Err(DeviceSelectionError::NoSuitableDevice));
}

#[test]
fn rejects_devices_without_present_modes() {
    let mut candidate = fully_capable_candidate();
    candidate.has_present_modes = false;

    let result = render_backend::select_device(vec![candidate]);

    assert_eq!(result, Err(DeviceSelectionError::NoSuitableDevice));
}

#[test]
fn rejects_devices_without_a_presentation_queue() {
    let mut candidate = fully_capable_candidate();
    candidate.queue_families = vec![QueueFamilyCapabilities {
        supports_graphics: true,
        supports_presentation: false,
    }];

    let result = render_backend::select_device(vec![candidate]);

    assert_eq!(result, Err(DeviceSelectionError::NoSuitableDevice));
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

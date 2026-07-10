use render_backend::RuntimeContext;

#[test]
fn runtime_context_reports_device_driver_and_api_versions() {
    let context = RuntimeContext {
        device_name: "Example GPU".to_owned(),
        driver_version: 0x0102_0304,
        api_version: ash::vk::make_api_version(0, 1, 3, 280),
    };

    assert_eq!(
        context.to_string(),
        "Vulkan device: Example GPU\nDriver version: 16909060 (0x01020304)\nVulkan API version: 1.3.280"
    );
}

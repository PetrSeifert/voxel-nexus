use render_backend::RuntimeContext;

#[test]
fn runtime_context_reports_device_driver_and_api_versions() {
    let context = RuntimeContext {
        device_name: "Example GPU".to_owned(),
        driver_version: 0x0102_0304,
        api_version: ash::vk::make_api_version(0, 1, 3, 280),
        validation_enabled: false,
        present_mode: ash::vk::PresentModeKHR::IMMEDIATE,
        timestamp_valid_bits: 64,
        timestamp_period_nanoseconds: 1.0,
    };

    assert_eq!(
        context.to_string(),
        "Vulkan device: Example GPU\nDriver version: 16909060 (0x01020304)\nVulkan API version: 1.3.280\nVulkan validation: disabled\nVulkan present mode: IMMEDIATE\nGPU timestamp valid bits: 64\nGPU timestamp period nanoseconds: 1"
    );
}

use ash::vk;
use render_backend::{SurfaceSupport, select_swapchain_configuration};

fn surface_support() -> SurfaceSupport {
    SurfaceSupport {
        capabilities: vk::SurfaceCapabilitiesKHR {
            min_image_count: 2,
            max_image_count: 3,
            current_extent: vk::Extent2D {
                width: u32::MAX,
                height: u32::MAX,
            },
            min_image_extent: vk::Extent2D {
                width: 320,
                height: 240,
            },
            max_image_extent: vk::Extent2D {
                width: 1920,
                height: 1080,
            },
            supported_composite_alpha: vk::CompositeAlphaFlagsKHR::OPAQUE,
            current_transform: vk::SurfaceTransformFlagsKHR::IDENTITY,
            ..Default::default()
        },
        formats: vec![
            vk::SurfaceFormatKHR {
                format: vk::Format::R8G8B8A8_UNORM,
                color_space: vk::ColorSpaceKHR::SRGB_NONLINEAR,
            },
            vk::SurfaceFormatKHR {
                format: vk::Format::B8G8R8A8_SRGB,
                color_space: vk::ColorSpaceKHR::SRGB_NONLINEAR,
            },
        ],
        present_modes: vec![vk::PresentModeKHR::IMMEDIATE, vk::PresentModeKHR::FIFO],
    }
}

#[test]
fn swapchain_uses_the_initial_drawable_extent_and_stable_presentation()
-> Result<(), Box<dyn std::error::Error>> {
    let configuration = select_swapchain_configuration(
        &surface_support(),
        vk::Extent2D {
            width: 1280,
            height: 720,
        },
    )?;

    assert_eq!(
        configuration.extent,
        vk::Extent2D {
            width: 1280,
            height: 720,
        }
    );
    assert_eq!(configuration.image_count, 3);
    assert_eq!(configuration.present_mode, vk::PresentModeKHR::FIFO);
    assert_eq!(configuration.format, vk::Format::B8G8R8A8_SRGB);
    assert_eq!(configuration.color_space, vk::ColorSpaceKHR::SRGB_NONLINEAR);
    assert_eq!(
        configuration.pre_transform,
        vk::SurfaceTransformFlagsKHR::IDENTITY
    );
    Ok(())
}

#[test]
fn swapchain_respects_a_surface_fixed_extent() -> Result<(), Box<dyn std::error::Error>> {
    let mut support = surface_support();
    support.capabilities.current_extent = vk::Extent2D {
        width: 800,
        height: 600,
    };

    let configuration = select_swapchain_configuration(
        &support,
        vk::Extent2D {
            width: 1280,
            height: 720,
        },
    )?;

    assert_eq!(
        configuration.extent,
        vk::Extent2D {
            width: 800,
            height: 600,
        }
    );
    Ok(())
}

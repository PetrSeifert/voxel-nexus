use ash::{Entry, Instance, vk};
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use render_backend::PresentationAdapter;
use std::ffi::CString;
use winit::window::Window;

pub struct WindowsPresentationAdapter<'window> {
    window: &'window Window,
}

impl<'window> WindowsPresentationAdapter<'window> {
    pub fn new(window: &'window Window) -> Self {
        Self { window }
    }
}

unsafe impl PresentationAdapter for WindowsPresentationAdapter<'_> {
    fn required_instance_extensions(&self) -> Result<Vec<CString>, String> {
        let display_handle = self
            .window
            .display_handle()
            .map_err(|error| error.to_string())?;
        let extension_name_pointers =
            ash_window::enumerate_required_extensions(display_handle.as_raw())
                .map_err(|error| error.to_string())?;
        extension_name_pointers
            .iter()
            .map(|extension_name_pointer| {
                let extension_name = unsafe { std::ffi::CStr::from_ptr(*extension_name_pointer) };
                Ok(extension_name.to_owned())
            })
            .collect()
    }

    unsafe fn create_surface(
        &self,
        entry: &Entry,
        instance: &Instance,
    ) -> Result<vk::SurfaceKHR, String> {
        let display_handle = self
            .window
            .display_handle()
            .map_err(|error| error.to_string())?;
        let window_handle = self
            .window
            .window_handle()
            .map_err(|error| error.to_string())?;
        unsafe {
            ash_window::create_surface(
                entry,
                instance,
                display_handle.as_raw(),
                window_handle.as_raw(),
                None,
            )
        }
        .map_err(|error| error.to_string())
    }
}

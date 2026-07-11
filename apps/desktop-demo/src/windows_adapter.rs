use ash::{Entry, Instance, vk};
use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawWindowHandle};
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

pub fn set_measurement_extent(window: &Window) -> Result<(), String> {
    let window_handle = window.window_handle().map_err(|error| error.to_string())?;
    let RawWindowHandle::Win32(window_handle) = window_handle.as_raw() else {
        return Err("the Windows measurement window has no Win32 handle".to_owned());
    };
    let window = window_handle.hwnd.get() as windows_sys::Win32::Foundation::HWND;
    let result = unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::SetWindowPos(
            window,
            windows_sys::Win32::UI::WindowsAndMessaging::HWND_TOP,
            0,
            0,
            1920,
            1080,
            windows_sys::Win32::UI::WindowsAndMessaging::SWP_FRAMECHANGED
                | windows_sys::Win32::UI::WindowsAndMessaging::SWP_NOACTIVATE,
        )
    };
    if result == 0 {
        return Err(format!(
            "could not set the 1920x1080 measurement window: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
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

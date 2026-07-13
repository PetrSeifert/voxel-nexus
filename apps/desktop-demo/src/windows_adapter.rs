use ash::{Entry, Instance, vk};
use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawWindowHandle};
use render_backend::PresentationAdapter;
use std::ffi::CString;
use winit::window::Window;

pub struct WindowsTextOverlay {
    window: windows_sys::Win32::Foundation::HWND,
}

impl WindowsTextOverlay {
    pub fn new(parent: &Window) -> Result<Self, String> {
        let window_handle = parent.window_handle().map_err(|error| error.to_string())?;
        let RawWindowHandle::Win32(window_handle) = window_handle.as_raw() else {
            return Err("the Windows overlay parent has no Win32 handle".to_owned());
        };
        let parent = window_handle.hwnd.get() as windows_sys::Win32::Foundation::HWND;
        let class_name = "STATIC\0".encode_utf16().collect::<Vec<_>>();
        let initial_text = "Voxel raster convergence\0"
            .encode_utf16()
            .collect::<Vec<_>>();
        let window = unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::CreateWindowExW(
                0,
                class_name.as_ptr(),
                initial_text.as_ptr(),
                windows_sys::Win32::UI::WindowsAndMessaging::WS_CHILD
                    | windows_sys::Win32::UI::WindowsAndMessaging::WS_VISIBLE
                    | windows_sys::Win32::UI::WindowsAndMessaging::WS_BORDER,
                12,
                12,
                1120,
                28,
                parent,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            )
        };
        if window.is_null() {
            return Err(format!(
                "could not create the in-client convergence overlay: {}",
                std::io::Error::last_os_error()
            ));
        }
        println!("In-client convergence overlay created");
        Ok(Self { window })
    }

    pub fn set_text(&self, text: &str) -> Result<(), String> {
        let text = text
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let result = unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::SetWindowTextW(self.window, text.as_ptr())
        };
        if result == 0 {
            return Err(format!(
                "could not update the in-client convergence overlay: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(())
    }
}

impl Drop for WindowsTextOverlay {
    fn drop(&mut self) {
        let result =
            unsafe { windows_sys::Win32::UI::WindowsAndMessaging::DestroyWindow(self.window) };
        if result == 0 {
            eprintln!(
                "could not destroy the in-client convergence overlay: {}",
                std::io::Error::last_os_error()
            );
        }
    }
}

pub struct WindowsPresentationAdapter<'window> {
    window: &'window Window,
}

impl<'window> WindowsPresentationAdapter<'window> {
    pub fn new(window: &'window Window) -> Self {
        Self { window }
    }
}

pub fn set_measurement_extent(window: &Window, extent: vk::Extent2D) -> Result<(), String> {
    let window_handle = window.window_handle().map_err(|error| error.to_string())?;
    let RawWindowHandle::Win32(window_handle) = window_handle.as_raw() else {
        return Err("the Windows measurement window has no Win32 handle".to_owned());
    };
    let window = window_handle.hwnd.get() as windows_sys::Win32::Foundation::HWND;
    let width = i32::try_from(extent.width)
        .map_err(|_| "the measurement window width cannot be represented by Win32".to_owned())?;
    let height = i32::try_from(extent.height)
        .map_err(|_| "the measurement window height cannot be represented by Win32".to_owned())?;
    let result = unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::SetWindowPos(
            window,
            windows_sys::Win32::UI::WindowsAndMessaging::HWND_TOP,
            0,
            0,
            width,
            height,
            windows_sys::Win32::UI::WindowsAndMessaging::SWP_FRAMECHANGED
                | windows_sys::Win32::UI::WindowsAndMessaging::SWP_NOACTIVATE,
        )
    };
    if result == 0 {
        return Err(format!(
            "could not set the {}x{} measurement window: {}",
            extent.width,
            extent.height,
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

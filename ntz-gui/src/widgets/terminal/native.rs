//! Win32 glue: the child windows ConEmu docks itself into.

#[cfg(windows)]
mod imp {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetFocus, SetFocus};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, GWL_STYLE, GetWindowLongPtrW, IsChild, SW_HIDE, SW_SHOWNA,
        SWP_NOACTIVATE, SWP_NOZORDER, SetWindowLongPtrW, SetWindowPos, ShowWindow, WS_CHILD,
        WS_CLIPCHILDREN, WS_CLIPSIBLINGS,
    };

    pub type Hwnd = HWND;

    pub fn hwnd_of(window: &impl HasWindowHandle) -> Option<HWND> {
        match window.window_handle().ok()?.as_raw() {
            RawWindowHandle::Win32(handle) => Some(handle.hwnd.get() as HWND),
            _ => None,
        }
    }

    pub fn focus(hwnd: HWND) {
        unsafe {
            if GetFocus() != hwnd {
                SetFocus(hwnd);
            }
        }
    }

    /// Plain child window of the egui window that ConEmu docks itself into.
    pub struct HostWindow {
        hwnd: HWND,
        /// Where the host is shown; `None` while hidden.
        rect: Option<[i32; 4]>,
    }

    impl HostWindow {
        /// Creates the host hidden, already sized to `rect` so ConEmu starts at the right size.
        pub fn create(parent: HWND, rect: [i32; 4]) -> Option<Self> {
            let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
            let [x, y, w, h] = rect;
            unsafe {
                // Keep egui's rendering from painting over the embedded windows.
                let style = GetWindowLongPtrW(parent, GWL_STYLE);
                SetWindowLongPtrW(parent, GWL_STYLE, style | WS_CLIPCHILDREN as isize);

                let hwnd = CreateWindowExW(
                    0,
                    class.as_ptr(),
                    std::ptr::null(),
                    WS_CHILD | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
                    x,
                    y,
                    w,
                    h,
                    parent,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null(),
                );
                (!hwnd.is_null()).then_some(Self { hwnd, rect: None })
            }
        }

        /// The `-InsideWnd` argument ConEmu expects.
        pub fn handle_arg(&self) -> String {
            format!("0x{:X}", self.hwnd as usize)
        }

        /// Shows the host at `[x, y, width, height]` (physical pixels, client coordinates).
        pub fn place(&mut self, rect: [i32; 4]) {
            if self.rect == Some(rect) {
                return;
            }
            let [x, y, w, h] = rect;
            // ConEmu resizes its own window to follow the host; resizing it from here as well
            // would double the resize events it has to handle.
            unsafe {
                let flags = SWP_NOZORDER | SWP_NOACTIVATE;
                SetWindowPos(self.hwnd, std::ptr::null_mut(), x, y, w, h, flags);
                ShowWindow(self.hwnd, SW_SHOWNA);
            }
            self.rect = Some(rect);
        }

        pub fn hide(&mut self) {
            if self.rect.take().is_some() {
                unsafe { ShowWindow(self.hwnd, SW_HIDE) };
            }
        }

        /// Whether keyboard focus is inside this host (i.e. in its ConEmu).
        pub fn has_focus(&self) -> bool {
            unsafe {
                let focused = GetFocus();
                !focused.is_null() && IsChild(self.hwnd, focused) != 0
            }
        }

        pub fn destroy(&mut self) {
            if !self.hwnd.is_null() {
                unsafe { DestroyWindow(self.hwnd) };
                self.hwnd = std::ptr::null_mut();
            }
        }
    }

    impl Drop for HostWindow {
        fn drop(&mut self) {
            self.destroy();
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use raw_window_handle::HasWindowHandle;

    pub type Hwnd = ();

    pub fn hwnd_of(_window: &impl HasWindowHandle) -> Option<Hwnd> {
        None
    }

    pub fn focus(_hwnd: Hwnd) {}

    pub struct HostWindow;

    impl HostWindow {
        pub fn create(_parent: Hwnd, _rect: [i32; 4]) -> Option<Self> {
            None
        }
        pub fn handle_arg(&self) -> String {
            String::new()
        }
        pub fn place(&mut self, _rect: [i32; 4]) {}
        pub fn hide(&mut self) {}
        pub fn has_focus(&self) -> bool {
            false
        }
        pub fn destroy(&mut self) {}
    }
}

pub use imp::*;

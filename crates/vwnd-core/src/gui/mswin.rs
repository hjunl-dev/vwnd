// ============================================================
// GWLP_USERDATA storage
// ============================================================

pub mod userdata {
    use windows::Win32::{
        Foundation::{HWND, LPARAM},
        UI::WindowsAndMessaging::{
            CREATESTRUCTW, GWLP_USERDATA, GetWindowLongPtrW, SetWindowLongPtrW,
        },
    };

    pub unsafe fn attach(hwnd: HWND, lp: LPARAM) {
        let cs = unsafe { &*(lp.0 as *const CREATESTRUCTW) };
        if !cs.lpCreateParams.is_null() {
            unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize) };
        }
    }

    pub unsafe fn get<T>(hwnd: HWND) -> Option<&'static T> {
        let raw: *const T = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const T;
        if raw.is_null() {
            None
        } else {
            Some(unsafe { &*raw })
        }
    }

    pub unsafe fn detach<T>(hwnd: HWND) {
        let raw = unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) } as *mut T;
        if !raw.is_null() {
            drop(unsafe { Box::from_raw(raw) });
        }
    }
}

pub mod window {
    use windows::Win32::{
        Foundation::{HINSTANCE, HWND},
        Graphics::Gdi::{COLOR_WINDOW, HBRUSH, UpdateWindow},
        System::LibraryLoader::GetModuleHandleW,
        UI::WindowsAndMessaging::{
            CS_VREDRAW, CreateWindowExW, IDC_ARROW, LoadCursorW, RegisterClassExW, SW_SHOW,
            ShowWindow, WINDOW_EX_STYLE, WNDCLASSEXW, WNDPROC, WS_OVERLAPPEDWINDOW,
        },
    };
    use windows_core::{HSTRING, PCWSTR};

    pub fn register_class(class_name: &str, wnd_proc: WNDPROC) -> windows_core::Result<()> {
        unsafe {
            // get HINSTANCE
            let instance: HINSTANCE = GetModuleHandleW(None)?.into();
            let class_name = HSTRING::from(class_name);

            // Register window class
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                style: CS_VREDRAW | CS_VREDRAW,
                lpfnWndProc: wnd_proc,
                hInstance: instance,
                hCursor: LoadCursorW(None, IDC_ARROW)?,
                hbrBackground: HBRUSH((COLOR_WINDOW.0 + 1) as _),
                lpszClassName: PCWSTR(class_name.as_ptr()),
                ..Default::default()
            };
            let atom = RegisterClassExW(&wc);
            if atom == 0 {
                return Err(windows::core::Error::from_thread());
            }
        }
        Ok(())
    }

    pub fn create(
        class_name: &str,
        wnd_name: &str,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        parent: Option<HWND>,
        show: bool,
    ) -> windows_core::Result<HWND> {
        unsafe {
            // get HINSTANCE
            let instance: HINSTANCE = GetModuleHandleW(None)?.into();
            let class_name = HSTRING::from(class_name);
            let wnd_name = HSTRING::from(wnd_name);

            // Create win32 window
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                PCWSTR(class_name.as_ptr()),
                PCWSTR(wnd_name.as_ptr()),
                WS_OVERLAPPEDWINDOW,
                x,
                y,
                w,
                h,
                parent,
                None,
                Some(instance),
                None,
            )?;

            if show {
                let _ = ShowWindow(hwnd, SW_SHOW);
                let _ = UpdateWindow(hwnd);
            }
            Ok(hwnd)
        }
    }
}

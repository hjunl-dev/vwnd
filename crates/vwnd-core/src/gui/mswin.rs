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
        Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM},
        Graphics::Gdi::{BeginPaint, COLOR_WINDOW, EndPaint, HBRUSH, PAINTSTRUCT, UpdateWindow},
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext},
            WindowsAndMessaging::{
                self, CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, DispatchMessageW,
                GetMessageW, IDC_ARROW, LoadCursorW, MSG, PM_NOREMOVE, PeekMessageW,
                PostQuitMessage, RegisterClassExW, SW_SHOW, ShowWindow, TranslateMessage,
                WINDOW_EX_STYLE, WNDCLASSEXW, WNDPROC, WS_OVERLAPPEDWINDOW,
            },
        },
    };
    use windows_core::{HSTRING, PCWSTR};

    pub extern "system" fn default_wnd_proc(
        hwnd: HWND,
        msg: u32,
        wp: WPARAM,
        lp: LPARAM,
    ) -> LRESULT {
        unsafe {
            match msg {
                WindowsAndMessaging::WM_PAINT => {
                    let mut ps = PAINTSTRUCT::default();
                    let _ = BeginPaint(hwnd, &mut ps);
                    let _ = EndPaint(hwnd, &ps);
                    LRESULT(0)
                }
                WindowsAndMessaging::WM_SIZE => LRESULT(0),
                WindowsAndMessaging::WM_DESTROY => {
                    PostQuitMessage(0);
                    LRESULT(0)
                }
                _ => DefWindowProcW(hwnd, msg, wp, lp),
            }
        }
    }

    pub fn init_dpi_awareness() -> windows_core::Result<()> {
        unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) }
    }

    pub fn register_class(class_name: &str, wnd_proc: WNDPROC) -> windows_core::Result<()> {
        unsafe {
            // get HINSTANCE
            let instance: HINSTANCE = GetModuleHandleW(None)?.into();
            let class_name = HSTRING::from(class_name);

            // Register window class
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                style: CS_HREDRAW | CS_VREDRAW,
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

    pub fn create_message_queue() {
        let mut msg = MSG::default();
        unsafe {
            let _ = PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE);
        };
    }

    pub fn pump_one() {
        let mut msg = MSG::default();
        unsafe {
            if PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE).into() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }

    pub fn run_message_loop() {
        let mut msg = MSG::default();
        unsafe {
            while GetMessageW(&mut msg, None, 0, 0).into() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}

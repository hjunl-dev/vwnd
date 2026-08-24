use windows::{
    Win32::{
        Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM},
        Graphics::Gdi::{BeginPaint, COLOR_WINDOW, EndPaint, HBRUSH, PAINTSTRUCT, UpdateWindow},
        System::{
            Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize},
            LibraryLoader::GetModuleHandleW,
        },
        UI::{
            HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext},
            WindowsAndMessaging::{
                CS_VREDRAW, CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DispatchMessageW,
                GetMessageW, IDC_ARROW, LoadCursorW, MSG, PostQuitMessage, RegisterClassExW,
                SW_SHOW, ShowWindow, TranslateMessage, WINDOW_EX_STYLE, WNDCLASSEXW,
                WS_OVERLAPPEDWINDOW,
            },
        },
    },
    core::{Result, w},
};

struct InitGuard;

impl InitGuard {
    pub fn new() -> Result<()> {
        unsafe {
            // set dpi awareness
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            // coinitialize for STA (Single-Threaded Apartment)
            CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()
        }
    }
}

impl Drop for InitGuard {
    fn drop(&mut self) {
        unsafe {
            CoUninitialize();
        }
    }
}

extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            windows::Win32::UI::WindowsAndMessaging::WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let _ = BeginPaint(hwnd, &mut ps);
                let _ = EndPaint(hwnd, &ps);
                LRESULT(0)
            }
            windows::Win32::UI::WindowsAndMessaging::WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

pub fn run() -> Result<()> {
    // init context
    InitGuard::new()?;

    unsafe {
        // get HINSTANCE
        let instance: HINSTANCE = GetModuleHandleW(None)?.into();
        let class_name = w!("WV2_Sample_Window_Class");

        // Register window class
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_VREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wnd_proc),
            hInstance: instance,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            hbrBackground: HBRUSH((COLOR_WINDOW.0 + 1) as _),
            lpszClassName: class_name,
            ..Default::default()
        };
        let atom = RegisterClassExW(&wc);
        if atom == 0 {
            return Err(windows::core::Error::from_thread());
        }

        // Create win32 window
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            class_name,
            w!("wv2-rs Win32 Basic Window"),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            1024,
            768,
            None,
            None,
            Some(instance),
            None,
        )?;

        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = UpdateWindow(hwnd);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).into() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    Ok(())
}

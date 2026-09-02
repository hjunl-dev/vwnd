use std::rc::Rc;

use windows::{
    Win32::{
        Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM},
        Graphics::Gdi::{BeginPaint, COLOR_WINDOW, EndPaint, HBRUSH, PAINTSTRUCT, UpdateWindow},
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext},
            WindowsAndMessaging::{
                self, CS_VREDRAW, CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DispatchMessageW,
                GetClientRect, GetMessageW, IDC_ARROW, LoadCursorW, MSG, PostQuitMessage,
                RegisterClassExW, SW_SHOW, ShowWindow, TranslateMessage, WINDOW_EX_STYLE,
                WNDCLASSEXW, WS_OVERLAPPEDWINDOW,
            },
        },
    },
    core::{Result, w},
};

use crate::gui::webview::Host;

mod userdata {
    use windows::Win32::{
        Foundation::HWND,
        UI::WindowsAndMessaging::{GWLP_USERDATA, GetWindowLongPtrW, SetWindowLongPtrW},
    };

    pub unsafe fn get(hwnd: HWND) -> isize {
        unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) }
    }

    pub unsafe fn set(hwnd: HWND, ud: isize) {
        unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, ud) };
    }
}

pub fn get_host_in_userdata(hwnd: HWND) -> Option<Rc<Host>> {
    let ptr = unsafe { userdata::get(hwnd) } as *const Host;
    if ptr.is_null() {
        return None;
    }
    unsafe {
        Rc::increment_strong_count(ptr);
        Some(Rc::from_raw(ptr))
    }
}

pub fn set_host_in_userdata(host: Rc<Host>) {
    let host_clone = host.clone();
    unsafe { userdata::set(host.hwnd, Rc::into_raw(host_clone) as isize) };
}

pub fn clear_host_in_userdata(hwnd: HWND) {
    let ptr = unsafe { userdata::get(hwnd) } as *const Host;
    unsafe { userdata::set(hwnd, 0) }; // 먼저 끊는다

    if !ptr.is_null() {
        drop(unsafe { Rc::from_raw(ptr) }); // into_raw로 심어둔 count 회수
    }
}

pub fn init_dpi_awareness() -> Result<()> {
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) }
}

extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WindowsAndMessaging::WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let _ = BeginPaint(hwnd, &mut ps);
                let _ = EndPaint(hwnd, &ps);
                LRESULT(0)
            }
            WindowsAndMessaging::WM_SIZE => {
                if let Some(host) = get_host_in_userdata(hwnd) {
                    if let Some(ctrl) = host.ctrl.borrow().as_ref() {
                        let mut rect = RECT::default();
                        let _ = GetClientRect(hwnd, &mut rect);
                        let _ = ctrl.SetBounds(rect);
                    }
                }
                LRESULT(0)
            }
            WindowsAndMessaging::WM_DESTROY => {
                clear_host_in_userdata(hwnd);
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

pub fn register_wnd_class() -> windows_core::Result<()> {
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
    }
    Ok(())
}

pub fn create_wnd(show_wnd: bool) -> windows_core::Result<HWND> {
    unsafe {
        // get HINSTANCE
        let instance: HINSTANCE = GetModuleHandleW(None)?.into();
        let class_name = w!("WV2_Sample_Window_Class");

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

        // show window
        if show_wnd {
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = UpdateWindow(hwnd);
        }
        Ok(hwnd)
    }
}

pub fn pump() {
    unsafe {
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).into() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

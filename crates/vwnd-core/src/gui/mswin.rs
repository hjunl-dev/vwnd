use std::{cell::Cell, marker::PhantomData, process::exit, rc::Rc};

use windows::Win32::{
    Foundation::{E_FAIL, GetLastError, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM},
    Graphics::Gdi::{COLOR_WINDOW, HBRUSH},
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext},
        WindowsAndMessaging::{
            CS_VREDRAW, DispatchMessageW, GetClientRect, GetMessageW, IDC_ARROW, LoadCursorW, MSG,
            PM_NOREMOVE, PeekMessageW, PostQuitMessage, RegisterClassExW, SW_SHOW, SetWindowTextW,
            ShowWindow, TranslateMessage, UnregisterClassW, WNDCLASSEXW,
        },
    },
};
use windows_core::{HSTRING, PCWSTR};

// ============================================================
// WindowHandler
// ============================================================

pub trait WindowHandler: 'static {
    fn on_create(self: &Rc<Self>, _hwnd: HWND) -> windows_core::Result<()> {
        Ok(())
    }

    // Calls DefWindowProcW if None is returned.
    fn on_message(
        self: &Rc<Self>,
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> Option<LRESULT>;
}

// ============================================================
// CreateContext
// - CreateWindowExW's lpParam
// ============================================================

pub struct CreateContext<H: WindowHandler> {
    handler: Cell<Option<Rc<H>>>,
    error: Cell<Option<windows_core::Error>>,
}

// ============================================================
// WindowClass
// - SetProcessDpiAwarenessContext
// - RegisterClassExW, CreateWindowExW, UnregisterClassW
// ============================================================

pub struct WindowClass<H: WindowHandler> {
    name: PCWSTR,
    hinstance: HINSTANCE,
    _handler: PhantomData<fn() -> H>,
}

impl<H: WindowHandler> WindowClass<H> {
    // SetProcessDpiAwarenessContext
    pub fn init_dpi_awareness() -> windows_core::Result<()> {
        unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) }
    }

    // RegisterClassExW
    pub fn register(name: PCWSTR) -> windows_core::Result<Self> {
        let hinstance: HINSTANCE = unsafe { GetModuleHandleW(None)? }.into();
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_VREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(WindowClass::<H>::wnd_proc),
            hInstance: hinstance,
            hCursor: unsafe { LoadCursorW(None, IDC_ARROW)? },
            hbrBackground: HBRUSH((COLOR_WINDOW.0 + 1) as _),
            lpszClassName: name,
            ..Default::default()
        };

        if unsafe { RegisterClassExW(&wc) } == 0 {
            unsafe { GetLastError() }.ok()?;
            return Err(E_FAIL.into());
        }

        Ok(Self {
            name,
            hinstance,
            _handler: PhantomData,
        })
    }

    // CreateWindowExW
    pub fn create() {}

    unsafe extern "system" fn wnd_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        LRESULT(0)
    }
}

impl<H: WindowHandler> Drop for WindowClass<H> {
    fn drop(&mut self) {
        let _ = unsafe { UnregisterClassW(self.name, Some(self.hinstance)) };
    }
}

// ============================================================
// message loop
// - GetMessageW, PeekMessageW
// ============================================================

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

pub fn run_message_loop() -> i32 {
    let mut msg = MSG::default();
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).into() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    msg.wParam.0 as i32
}

// ============================================================
// win32 window helpers
// ============================================================

pub fn show(hwnd: HWND) {
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
    }
}

pub fn client_rect(hwnd: HWND) -> windows_core::Result<RECT> {
    let mut rect = RECT::default();
    unsafe { GetClientRect(hwnd, &mut rect)? };
    Ok(rect)
}

pub fn set_title(hwnd: HWND, title: &str) -> windows_core::Result<()> {
    unsafe { SetWindowTextW(hwnd, &HSTRING::from(title)) }
}

pub fn post_quit(exit_code: i32) {
    unsafe {
        PostQuitMessage(exit_code);
    }
}

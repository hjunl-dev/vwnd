use std::{cell::Cell, ffi::c_void, marker::PhantomData, process::exit, rc::Rc};

use windows::Win32::{
    Foundation::{E_FAIL, GetLastError, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM},
    Graphics::Gdi::{COLOR_WINDOW, HBRUSH},
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext},
        WindowsAndMessaging::{
            CREATESTRUCTW, CS_VREDRAW, CW_USEDEFAULT, CreateWindowExW, DefWindowProcW,
            DispatchMessageW, GWLP_USERDATA, GetClientRect, GetMessageW, GetWindowLongPtrW,
            IDC_ARROW, LoadCursorW, MSG, PM_NOREMOVE, PeekMessageW, PostQuitMessage,
            RegisterClassExW, SW_SHOW, SetWindowLongPtrW, SetWindowTextW, ShowWindow,
            TranslateMessage, UnregisterClassW, WINDOW_EX_STYLE, WM_CREATE, WM_NCCREATE,
            WM_NCDESTROY, WNDCLASSEXW, WS_OVERLAPPEDWINDOW,
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

pub struct CreateWindowContext<H: WindowHandler> {
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
    pub fn create(
        &self,
        title: &str,
        w: i32,
        h: i32,
        handler: Rc<H>,
    ) -> windows_core::Result<HWND> {
        let context = CreateWindowContext {
            handler: Cell::new(Some(handler)),
            error: Cell::new(None),
        };
        let result = unsafe {
            let ex_style = WINDOW_EX_STYLE::default();
            let style = WS_OVERLAPPEDWINDOW;
            let lpparam = &context as *const CreateWindowContext<H> as *const c_void;
            CreateWindowExW(
                ex_style,
                self.name,
                &HSTRING::from(title),
                style,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                w,
                h,
                None,
                None,
                Some(self.hinstance),
                Some(lpparam),
            )
        };
        result.map_err(|e| context.error.take().unwrap_or(e))
    }

    unsafe extern "system" fn wnd_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        unsafe {
            // 1) handle OnNcCreate, use WindowCreateContext to save handler (GWLP_USERDATA)
            if msg == WM_NCCREATE {
                let create_ctx = get_create_window_context::<H>(lparam);
                if let Some(handler) = create_ctx.handler.take() {
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, Rc::into_raw(handler) as isize);
                }
                return DefWindowProcW(hwnd, msg, wparam, lparam);
            }
            // 2) restore USERDATA
            let handler_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const H;
            if handler_ptr.is_null() {
                return DefWindowProcW(hwnd, msg, wparam, lparam);
            }
            // 3) handle OnNcDestroy
            if msg == WM_NCDESTROY {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                drop(Rc::from_raw(handler_ptr));
                return DefWindowProcW(hwnd, msg, wparam, lparam);
            }

            // 4) use handler
            {
                Rc::increment_strong_count(handler_ptr);
                let handler = Rc::from_raw(handler_ptr);

                // call on create handler
                if msg == WM_CREATE {
                    return match handler.on_create(hwnd) {
                        Ok(()) => LRESULT(0),
                        Err(e) => {
                            get_create_window_context::<H>(lparam).error.set(Some(e));
                            LRESULT(-1)
                        }
                    };
                }
                // call on message handler
                handler
                    .on_message(hwnd, msg, wparam, lparam)
                    .unwrap_or_else(|| DefWindowProcW(hwnd, msg, wparam, lparam))
            }
        }
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

unsafe fn get_create_window_context<'a, H>(lparam: LPARAM) -> &'a CreateWindowContext<H>
where
    H: WindowHandler,
{
    unsafe {
        let cs_ref = &*(lparam.0 as *const CREATESTRUCTW);
        &*(cs_ref.lpCreateParams as *const CreateWindowContext<H>)
    }
}

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

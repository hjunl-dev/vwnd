use std::{cell::Cell, ffi::c_void, marker::PhantomData, rc::Rc};

use windows::Win32::{
    Foundation::{COLORREF, E_FAIL, GetLastError, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM},
    Graphics::Gdi::{COLOR_WINDOW, CreateSolidBrush, DeleteObject, FillRect, HBRUSH, HDC},
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        Shell::ShellExecuteW,
        HiDpi::{
            AdjustWindowRectExForDpi, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            GetDpiForSystem, GetDpiForWindow, SetProcessDpiAwarenessContext,
        },
        WindowsAndMessaging::{
            CREATESTRUCTW, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, CreateWindowExW,
            DefWindowProcW, DispatchMessageW, GWLP_USERDATA, GetClientRect, GetForegroundWindow,
            GetMessageW, GetWindowLongPtrW, IDC_ARROW, LoadCursorW, MSG, PM_NOREMOVE, PM_REMOVE,
            PeekMessageW, PostMessageW, PostQuitMessage, RegisterClassExW, SW_SHOW, SW_SHOWNORMAL,
            SWP_NOACTIVATE, SWP_NOZORDER, SetWindowLongPtrW, SetWindowPos, SetWindowTextW,
            ShowWindow, TranslateMessage, UnregisterClassW, WINDOW_EX_STYLE, WINDOW_STYLE,
            WM_CLOSE, WM_CREATE, WM_NCCREATE, WM_NCDESTROY, WNDCLASSEXW, WS_OVERLAPPEDWINDOW,
        },
    },
};
use windows_core::{HSTRING, PCWSTR};

// Styles used by WindowClass::create_wnd (also used for size calculation)
const WND_STYLE: WINDOW_STYLE = WS_OVERLAPPEDWINDOW;
const WND_EX_STYLE: WINDOW_EX_STYLE = WINDOW_EX_STYLE(0);

// 96 DPI = 100% scale
const BASE_DPI: u32 = 96;

// ============================================================
// WindowHandler
// ============================================================

pub trait WindowHandler: 'static {
    fn on_create(self: &Rc<Self>, _hwnd: HWND) -> windows_core::Result<()>;

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
// - RegisterClassExW, CreateWindowExW, UnregisterClassW
// ============================================================

pub struct WindowClass<H: WindowHandler> {
    name: PCWSTR,
    hinstance: HINSTANCE,
    _handler: PhantomData<fn() -> H>,
}

impl<H: WindowHandler> WindowClass<H> {
    // RegisterClassExW
    pub fn register(name: PCWSTR) -> windows_core::Result<Self> {
        let hinstance: HINSTANCE = unsafe { GetModuleHandleW(None)? }.into();
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
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
    // - w, h: outer window size in physical pixels
    pub fn create_wnd(
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
            let lpparam = &context as *const CreateWindowContext<H> as *const c_void;
            CreateWindowExW(
                WND_EX_STYLE,
                self.name,
                &HSTRING::from(title),
                WND_STYLE,
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
            // e.g. in c++)
            //      LPCREATESTRUCTW pCreate = reinterpret_cast<LPCREATESTRUCTW>(lParam);
            //      MyData* pMyData = reinterpret_cast<MyData*>(pCreate->lpCreateParams);
            //      SetWindowLongPtr(hWnd, GWLP_USERDATA, reinterpret_cast<LONG_PTR>(pMyData));
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

// For hosting inside an external loop (e.g. a game loop).
#[allow(dead_code)]
pub fn create_message_queue() {
    let mut msg = MSG::default();
    unsafe {
        let _ = PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE);
    };
}

// For hosting inside an external loop (e.g. a game loop).
#[allow(dead_code)]
pub fn pump_one() {
    let mut msg = MSG::default();
    unsafe {
        if PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).into() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

// GetMessageW returns -1 on error, which is "true" as a BOOL -> check the raw value.
pub fn run_message_loop() -> windows_core::Result<i32> {
    let mut msg = MSG::default();
    loop {
        match unsafe { GetMessageW(&mut msg, None, 0, 0) }.0 {
            0 => return Ok(msg.wParam.0 as i32), // WM_QUIT
            -1 => {
                unsafe { GetLastError() }.ok()?;
                return Err(E_FAIL.into());
            }
            _ => unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            },
        }
    }
}

// ============================================================
// win32 process helpers
// ============================================================

// SetProcessDpiAwarenessContext
// - must be called before any window is created
// - fails (E_ACCESSDENIED) if the awareness was already set (e.g. by the app manifest)
pub fn init_dpi_awareness() -> windows_core::Result<()> {
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) }
}

// Opens `target` (URL, mailto:, ...) with the system default handler.
// Only pass values the caller has already filtered: ShellExecute will launch
// whatever is registered for the scheme.
pub fn open_external(target: &str) -> windows_core::Result<()> {
    let result = unsafe {
        ShellExecuteW(
            None,
            windows_core::w!("open"),
            &HSTRING::from(target),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    // > 32 means success (HINSTANCE kept for 16-bit compatibility)
    if result.0 as usize > 32 {
        return Ok(());
    }
    unsafe { GetLastError() }.ok()?;
    Err(E_FAIL.into())
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

// WM_ERASEBKGND: fills the client area with a solid color (wParam = the HDC to paint).
// Returns false if nothing was painted, so the caller can fall back to DefWindowProcW.
pub fn erase_background(hwnd: HWND, wparam: WPARAM, (r, g, b): (u8, u8, u8)) -> bool {
    let hdc = HDC(wparam.0 as *mut c_void);
    let Ok(rect) = client_rect(hwnd) else {
        return false;
    };
    // COLORREF = 0x00BBGGRR
    let color = COLORREF(u32::from(r) | (u32::from(g) << 8) | (u32::from(b) << 16));
    unsafe {
        let brush = CreateSolidBrush(color);
        if brush.is_invalid() {
            return false;
        }
        FillRect(hdc, &rect, brush);
        let _ = DeleteObject(brush.into());
    }
    true
}

pub fn set_title(hwnd: HWND, title: &str) -> windows_core::Result<()> {
    unsafe { SetWindowTextW(hwnd, &HSTRING::from(title)) }
}

pub fn is_foreground(hwnd: HWND) -> bool {
    unsafe { GetForegroundWindow() == hwnd }
}

// Converts a client (content) size in DIPs to the outer window size in physical pixels
// for a window created by WindowClass::create_wnd.
// - `reference`: window whose DPI is used (e.g. the opener of a popup)
//   None -> system DPI (e.g. the first window, which has no reference yet)
pub fn outer_size_for_client_dip(
    reference: Option<HWND>,
    w: i32,
    h: i32,
) -> windows_core::Result<(i32, i32)> {
    let dpi = match reference {
        Some(hwnd) => unsafe { GetDpiForWindow(hwnd) },
        None => unsafe { GetDpiForSystem() },
    };
    let dpi = if dpi == 0 { BASE_DPI } else { dpi };
    let scale = |v: i32| ((v as i64 * dpi as i64 + (BASE_DPI / 2) as i64) / BASE_DPI as i64) as i32;

    let mut rect = RECT {
        left: 0,
        top: 0,
        right: scale(w),
        bottom: scale(h),
    };
    unsafe { AdjustWindowRectExForDpi(&mut rect, WND_STYLE, false, WND_EX_STYLE, dpi)? };
    Ok((rect.right - rect.left, rect.bottom - rect.top))
}

// WM_DPICHANGED: resize/move the window to the rect suggested by the system.
// The resulting WM_SIZE updates the content (e.g. WebView bounds).
pub fn apply_dpi_changed(hwnd: HWND, lparam: LPARAM) -> windows_core::Result<()> {
    if lparam.0 == 0 {
        return Ok(());
    }
    // lParam: *const RECT, valid for the duration of the message
    let r = unsafe { &*(lparam.0 as *const RECT) };
    unsafe {
        SetWindowPos(
            hwnd,
            None,
            r.left,
            r.top,
            r.right - r.left,
            r.bottom - r.top,
            SWP_NOZORDER | SWP_NOACTIVATE,
        )
    }
}

// Posts WM_CLOSE so the window is destroyed on the next loop iteration,
// not synchronously inside the caller (e.g. a WebView2 event callback).
pub fn post_close(hwnd: HWND) {
    unsafe {
        let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
    }
}

pub fn post_quit(exit_code: i32) {
    unsafe {
        PostQuitMessage(exit_code);
    }
}
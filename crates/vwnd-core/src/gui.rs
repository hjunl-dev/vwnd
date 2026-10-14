use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
    rc::Rc,
};

use windows::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, WPARAM},
    UI::WindowsAndMessaging::{WM_DESTROY, WM_DPICHANGED, WM_MOVE, WM_SETFOCUS, WM_SIZE},
};
use windows_core::w;

use crate::gui::{
    com::ComApartment,
    mswin::{WindowClass, WindowHandler},
    webview::{NewWindowRequest, PendingNewWindow, WebView, WebViewEnv, WebViewHost},
};

mod com;
mod mswin;
mod webview;

const DEFAULT_URL: &str = "https://www.google.com/";
// Window content sizes in DIPs (scaled to physical pixels by DPI)
// - main window: system DPI
// - popup without a window.open size: opener's DPI (same as window.open sizes)
const MAIN_SIZE: (i32, i32) = (1280, 720);
const POPUP_SIZE: (i32, i32) = (960, 640);

// ============================================================
// NewWindowPolicy
// - how the app handles NewWindowRequested
// ============================================================

#[derive(Clone, Copy, Debug)]
#[allow(dead_code)]
enum NewWindowPolicy {
    /// WebView2 default popup (not managed by the app)
    Default,
    /// Block the popup (window.open returns null)
    Block,
    /// Stage 1: navigate the current view instead
    NavigateHere,
    /// Stage 2: open in our own window (Deferral + SetNewWindow)
    OwnWindow,
}

const NEW_WINDOW_POLICY: NewWindowPolicy = NewWindowPolicy::OwnWindow;

// ============================================================
// Shared
// - state shared by every window (main + popups)
// ============================================================

struct Shared {
    wnd_class: WindowClass<App>,
    env: RefCell<Option<WebViewEnv>>,
    open_windows: Cell<usize>,
}

// What a window shows once its WebView is ready
enum Initial {
    Url(String),
    NewWindow(PendingNewWindow),
}

// ============================================================
// App (one per top-level window)
// ============================================================

pub struct App {
    shared: Rc<Shared>,
    is_popup: bool,
    hwnd: Cell<HWND>,
    host: RefCell<Option<WebViewHost>>,
    initial: RefCell<Option<Initial>>,
}

impl App {
    pub fn run() -> windows_core::Result<()> {
        // must run before any window is created
        // - not fatal: fails with E_ACCESSDENIED when already set (e.g. by the manifest)
        if let Err(e) = mswin::init_dpi_awareness() {
            eprintln!("[app] dpi awareness not applied: {e}");
        }

        // declared before Shared -> dropped after every COM object below
        let _com = ComApartment::new_sta()?;

        let shared = Rc::new(Shared {
            wnd_class: WindowClass::<App>::register(w!("vwnd_wnd_class"))?,
            env: RefCell::new(None),
            open_windows: Cell::new(0),
        });

        let main = Rc::new(App::new(shared.clone(), Initial::Url(DEFAULT_URL.to_owned())));
        let (w, h) = mswin::outer_size_for_client_dip(None, MAIN_SIZE.0, MAIN_SIZE.1)?;
        let hwnd = shared.wnd_class.create_wnd("vwnd_wnd", w, h, main)?;
        mswin::show(hwnd);

        let _exit_code = mswin::run_message_loop()?;
        Ok(())
    }

    fn new(shared: Rc<Shared>, initial: Initial) -> Self {
        Self {
            shared,
            is_popup: matches!(initial, Initial::NewWindow(_)),
            hwnd: Cell::new(HWND::default()),
            host: RefCell::new(None),
            initial: RefCell::new(Some(initial)),
        }
    }

    fn create_host(self: &Rc<Self>, env: &WebViewEnv) -> windows_core::Result<()> {
        let weak = Rc::downgrade(self);
        env.create_host(self.hwnd.get(), move |res| {
            let Some(this) = weak.upgrade() else {
                // window already gone: dropping the host closes it
                drop(res);
                return;
            };
            if let Err(e) = res.and_then(|host| this.attach_webview(host)) {
                this.on_setup_failed(e);
            }
        })
    }

    // Any `?` below drops `host`, and WebViewHost::drop closes the controller.
    fn attach_webview(&self, mut host: WebViewHost) -> windows_core::Result<()> {
        let hwnd = self.hwnd.get();
        let view = host.view();
        // set bounds to ClientRect
        view.set_bounds(mswin::client_rect(hwnd)?)?;
        // set visible
        view.set_visible(true)?;
        // set document title changed callback
        host.set_on_document_title_changed(move |title| {
            let _ = mswin::set_title(hwnd, &title);
        })?;
        // set new window requested callback (behavior selected by NEW_WINDOW_POLICY)
        let weak_shared = Rc::downgrade(&self.shared);
        host.set_on_new_window_requested(move |req| {
            let Some(shared) = weak_shared.upgrade() else {
                let _ = req.block();
                return;
            };
            if let Err(e) = App::handle_new_window(&shared, hwnd, req) {
                eprintln!("[app] failed to handle new window request: {e}");
            }
        })?;
        // set window close requested callback
        // - post WM_CLOSE instead of destroying synchronously inside the WebView2 callback
        host.set_on_window_close_requested(move || {
            mswin::post_close(hwnd);
        })?;

        *self.host.borrow_mut() = Some(host);

        // The window may have been activated before the WebView existed (WM_SETFOCUS missed).
        if mswin::is_foreground(hwnd) {
            let _ = view.focus();
        }

        // For popups, every setting/handler above must be in place before SetNewWindow.
        let initial = self.initial.borrow_mut().take();
        match initial {
            Some(Initial::Url(url)) => view.navigate(&url),
            Some(Initial::NewWindow(pending)) => pending.complete(&view),
            None => Ok(()),
        }
    }

    // Called inside the opener's NewWindowRequested callback.
    fn handle_new_window(
        shared: &Rc<Shared>,
        opener: HWND,
        req: NewWindowRequest,
    ) -> windows_core::Result<()> {
        eprintln!(
            "[app] new window requested ({:?}): {}",
            NEW_WINDOW_POLICY,
            req.uri()
        );
        match NEW_WINDOW_POLICY {
            NewWindowPolicy::Default => {
                req.allow_default();
                Ok(())
            }
            NewWindowPolicy::Block => req.block(),
            NewWindowPolicy::NavigateHere => req.navigate_here(),
            NewWindowPolicy::OwnWindow => App::open_popup(shared, opener, req),
        }
    }

    // Stage 2: open the request in our own App window.
    fn open_popup(
        shared: &Rc<Shared>,
        opener: HWND,
        req: NewWindowRequest,
    ) -> windows_core::Result<()> {
        // window.open features give a content size in DIPs
        let (cw, ch) = req
            .requested_size()
            .and_then(|(w, h)| Some((i32::try_from(w).ok()?, i32::try_from(h).ok()?)))
            .unwrap_or(POPUP_SIZE);

        // Hold the request first: from here on, any failure drops the pending request,
        // which cancels it instead of falling back to the WebView2 default popup.
        let pending = req.defer()?;

        // DIP content size -> physical outer size, using the opener's DPI
        let (w, h) = mswin::outer_size_for_client_dip(Some(opener), cw, ch)?;

        let popup = Rc::new(App::new(shared.clone(), Initial::NewWindow(pending)));
        let hwnd = shared.wnd_class.create_wnd("vwnd_popup", w, h, popup)?;
        mswin::show(hwnd);
        Ok(())
    }

    fn on_setup_failed(&self, e: windows_core::Error) {
        eprintln!("[app] failed to setup webview: {e}");
        // dropping a PendingNewWindow cancels the request (window.open returns null)
        let initial = self.initial.borrow_mut().take();
        drop(initial);
        if self.is_popup {
            mswin::post_close(self.hwnd.get());
        }
    }

    // Clones the view out so no RefCell borrow is held while calling into WebView2.
    fn view(&self) -> Option<WebView> {
        self.host.borrow().as_ref().map(WebViewHost::view)
    }

    fn resize_webview(&self) {
        let Some(view) = self.view() else {
            return;
        };
        if let Ok(rect) = mswin::client_rect(self.hwnd.get()) {
            let _ = view.set_bounds(rect);
        }
    }

    fn close_webview(&self) {
        let host = self.host.borrow_mut().take();
        if let Some(host) = host {
            host.close();
        }
    }

    fn on_destroy(&self) {
        // cancel a popup request that never completed (window closed during setup)
        let initial = self.initial.borrow_mut().take();
        drop(initial);
        self.close_webview();

        // quit when the last window (main or popup) is gone
        let remaining = self.shared.open_windows.get().saturating_sub(1);
        self.shared.open_windows.set(remaining);
        if remaining == 0 {
            mswin::post_quit(0);
        }
    }

    fn default_user_data_dir() -> PathBuf {
        std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join("vwnd_udf")
    }
}

impl WindowHandler for App {
    fn on_create(self: &Rc<Self>, hwnd: HWND) -> windows_core::Result<()> {
        self.hwnd.set(hwnd);
        // counted first, so WM_DESTROY (sent even when WM_CREATE fails) stays balanced
        self.shared
            .open_windows
            .set(self.shared.open_windows.get() + 1);

        // popups (and any later window): reuse the shared environment
        let env = self.shared.env.borrow().clone();
        if let Some(env) = env {
            return self.create_host(&env);
        }

        // first window: create the shared environment, then the controller
        let weak = Rc::downgrade(self);
        WebViewEnv::create(&App::default_user_data_dir(), move |res| {
            let Some(this) = weak.upgrade() else {
                return;
            };
            let result = res.and_then(|env| {
                *this.shared.env.borrow_mut() = Some(env.clone());
                this.create_host(&env)
            });
            if let Err(e) = result {
                this.on_setup_failed(e);
            }
        })
    }

    fn on_message(
        self: &Rc<Self>,
        hwnd: HWND,
        msg: u32,
        _wparam: WPARAM,
        lparam: LPARAM,
    ) -> Option<LRESULT> {
        match msg {
            WM_SIZE => {
                self.resize_webview();
                Some(LRESULT(0))
            }
            WM_MOVE => {
                if let Some(view) = self.view() {
                    let _ = view.notify_parent_moved();
                }
                Some(LRESULT(0))
            }
            WM_SETFOCUS => {
                if let Some(view) = self.view() {
                    let _ = view.focus();
                }
                Some(LRESULT(0))
            }
            WM_DPICHANGED => {
                if let Err(e) = mswin::apply_dpi_changed(hwnd, lparam) {
                    eprintln!("[app] failed to apply dpi change: {e}");
                }
                Some(LRESULT(0))
            }
            WM_DESTROY => {
                self.on_destroy();
                Some(LRESULT(0))
            }
            _ => None,
        }
    }
}
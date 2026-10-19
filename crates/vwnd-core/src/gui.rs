use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
    rc::Rc,
    time::{Duration, Instant},
};

use windows::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, WPARAM},
    UI::WindowsAndMessaging::{
        WM_DESTROY, WM_DPICHANGED, WM_ERASEBKGND, WM_MOVE, WM_SETFOCUS, WM_SIZE,
    },
};
use windows_core::w;

use crate::gui::{
    com::ComApartment,
    mswin::{WindowClass, WindowHandler},
    webview::{
        NewWindowRequest, PendingNewWindow, ProcessFailedEvent, ProcessFailure, WebMessage,
        WebView, WebViewBackground, WebViewEnv, WebViewHost, WebViewSettings,
    },
};

mod com;
mod demo;
mod mswin;
mod webview;

// What the main window loads first
#[derive(Clone, Copy)]
#[allow(dead_code)]
enum StartPage {
    Url(&'static str),
    /// built-in page for testing popups, close, the bridge and navigation filtering
    TestPage,
}

const START_PAGE: StartPage = StartPage::TestPage;
// const START_PAGE: StartPage = StartPage::Url("https://www.google.com/");
// Window content sizes in DIPs (scaled to physical pixels by DPI)
// - main window: system DPI
// - popup without a window.open size: opener's DPI (same as window.open sizes)
const MAIN_SIZE: (i32, i32) = (1280, 720);
const POPUP_SIZE: (i32, i32) = (960, 640);

// Renderer failure: reload, unless the last reload was this recent (crash loop -> error page)
const RELOAD_COOLDOWN: Duration = Duration::from_secs(10);

// What shows before the page paints. The window (WM_ERASEBKGND) and the WebView use the
// same color, so startup goes window -> WebView -> page without a white flash in between.
// (A page that paints its own white background still switches to white when it loads.)
const BACKGROUND_RGB: (u8, u8, u8) = (0x20, 0x20, 0x20);
const WEBVIEW_BACKGROUND: WebViewBackground =
    WebViewBackground::Rgb(BACKGROUND_RGB.0, BACKGROUND_RGB.1, BACKGROUND_RGB.2);
// const WEBVIEW_BACKGROUND: WebViewBackground = WebViewBackground::Transparent;

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
    /// Open in the system default browser
    External,
}

const NEW_WINDOW_POLICY: NewWindowPolicy = NewWindowPolicy::OwnWindow;

// ============================================================
// Navigation filter
// - applied to top-level navigations (NavigationStarting) and new-window requests
// ============================================================

// http(s) hosts that stay in the app (subdomains included).
// - None: every http(s) host stays in the app
// - Some(hosts): other hosts open in the system browser
const IN_APP_HOSTS: Option<&[&str]> = None;
// const IN_APP_HOSTS: Option<&[&str]> = Some(&["example.com", "google.com"]);

// Non-http(s) schemes handed to the system (others are blocked)
const EXTERNAL_SCHEMES: &[&str] = &["mailto", "tel"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NavDecision {
    Allow,
    External,
    Block,
}

fn classify_navigation(uri: &str) -> NavDecision {
    let Some((scheme, rest)) = uri.split_once(':') else {
        return NavDecision::Block;
    };
    let scheme = scheme.to_ascii_lowercase();
    match scheme.as_str() {
        // NavigateToString / window.open('') / blob documents
        "about" | "data" | "blob" => NavDecision::Allow,
        "http" | "https" => {
            let Some(allowed) = IN_APP_HOSTS else {
                return NavDecision::Allow;
            };
            match url_host(rest) {
                Some(host) if host_matches(&host, allowed) => NavDecision::Allow,
                Some(_) => NavDecision::External,
                None => NavDecision::Block,
            }
        }
        s if EXTERNAL_SCHEMES.contains(&s) => NavDecision::External,
        _ => NavDecision::Block,
    }
}

// "//user@Host:8080/path?q" -> "host"
fn url_host(after_scheme: &str) -> Option<String> {
    let authority = after_scheme.strip_prefix("//")?;
    let authority = authority.split(['/', '?', '#']).next()?;
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, hp)| hp);
    let host = if let Some(v6) = host_port.strip_prefix('[') {
        v6.split_once(']')?.0
    } else {
        host_port.split(':').next()?
    };
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

fn host_matches(host: &str, allowed: &[&str]) -> bool {
    allowed.iter().any(|a| {
        host == *a
            || host
                .strip_suffix(a)
                .is_some_and(|prefix| prefix.ends_with('.'))
    })
}

// Only http(s) and EXTERNAL_SCHEMES are ever handed to ShellExecute
// (about:/data:/blob: only make sense inside the app; unknown schemes may launch apps).
fn can_open_externally(uri: &str) -> bool {
    let Some((scheme, _)) = uri.split_once(':') else {
        return false;
    };
    let scheme = scheme.to_ascii_lowercase();
    matches!(scheme.as_str(), "http" | "https") || EXTERNAL_SCHEMES.contains(&scheme.as_str())
}

fn open_external(uri: &str) {
    if !can_open_externally(uri) {
        eprintln!("[app] not opening externally: {uri}");
        return;
    }
    eprintln!("[app] opening in system browser: {uri}");
    if let Err(e) = mswin::open_external(uri) {
        eprintln!("[app] failed to open externally: {e}");
    }
}

// ============================================================
// WebView settings
// - debug: developer features on / release: locked down
// ============================================================

fn webview_settings() -> WebViewSettings {
    let debug = cfg!(debug_assertions);
    WebViewSettings {
        dev_tools: debug,
        default_context_menus: debug,
        status_bar: false,
        browser_accelerator_keys: debug,
    }
}

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
    Start(StartPage),
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

        let main = Rc::new(App::new(shared.clone(), Initial::Start(START_PAGE)));
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
        // background before the first paint
        view.set_background(WEBVIEW_BACKGROUND)?;
        // set visible
        view.set_visible(true)?;
        // settings (before the first navigation / SetNewWindow)
        view.apply_settings(&webview_settings())?;
        // navigation filter
        host.set_on_navigation_starting(|uri| match classify_navigation(uri) {
            NavDecision::Allow => true,
            NavDecision::External => {
                open_external(uri);
                false
            }
            NavDecision::Block => {
                eprintln!("[app] navigation blocked: {uri}");
                false
            }
        })?;
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
        // set web message received callback (page -> host)
        host.set_on_web_message_received(App::handle_web_message)?;
        // set process failed callback (per window: its own reload history)
        let last_reload = Cell::new(None);
        host.set_on_process_failed(move |ev| App::handle_process_failed(hwnd, &last_reload, ev))?;
        // inject the bridge (window.vwnd) into every document, before the first navigation
        view.add_script_on_document_created(demo::BRIDGE_JS)?;

        *self.host.borrow_mut() = Some(host);

        // The window may have been activated before the WebView existed (WM_SETFOCUS missed).
        if mswin::is_foreground(hwnd) {
            let _ = view.focus();
        }

        // For popups, every setting/handler above must be in place before SetNewWindow.
        let initial = self.initial.borrow_mut().take();
        match initial {
            Some(Initial::Start(StartPage::Url(url))) => view.navigate(url),
            Some(Initial::Start(StartPage::TestPage)) => view.navigate_to_string(demo::TEST_HTML),
            Some(Initial::NewWindow(pending)) => pending.complete(&view),
            None => Ok(()),
        }
    }

    // Page -> host messages. Demo protocol: echo back as {"type":"echo","data":<message>}.
    // In real use, check msg.source() before trusting the content.
    // WebView2 process failures.
    // - browser exited: every WebView on the shared environment is gone -> close the window
    //   (each window gets the event, so the app quits once all are closed)
    // - renderer exited / unresponsive: reload; a second failure within RELOAD_COOLDOWN
    //   shows the crash page instead, so a page that always crashes doesn't loop forever
    // - others (GPU, utility, frame renderer): WebView2 recovers by itself
    fn handle_process_failed(
        hwnd: HWND,
        last_reload: &Cell<Option<Instant>>,
        ev: ProcessFailedEvent,
    ) {
        eprintln!("[app] webview process failed: {:?}", ev.kind());
        let result = match ev.kind() {
            ProcessFailure::BrowserExited => {
                mswin::post_close(hwnd);
                Ok(())
            }
            ProcessFailure::RendererExited | ProcessFailure::RendererUnresponsive => {
                let now = Instant::now();
                let looping = last_reload
                    .get()
                    .is_some_and(|t| now.duration_since(t) < RELOAD_COOLDOWN);
                if looping {
                    ev.show_html(demo::CRASH_HTML)
                } else {
                    last_reload.set(Some(now));
                    ev.reload()
                }
            }
            ProcessFailure::Other(_) => Ok(()),
        };
        if let Err(e) = result {
            eprintln!("[app] failed to recover from process failure: {e}");
        }
    }

    // Page -> host messages. Demo protocol:
    // - "probe" (string): run demo::PROBE_JS in the sender, reply {"type":"probe","data":<result>}
    // - anything else: echo back as {"type":"echo","data":<message>}
    // In real use, check msg.source() before trusting the content.
    fn handle_web_message(msg: WebMessage) {
        eprintln!(
            "[app] web message from {}: {}",
            msg.source(),
            msg.text().unwrap_or(msg.json())
        );
        let result = if msg.text() == Some("probe") {
            App::probe_page(&msg)
        } else {
            // msg.json() is already valid JSON, so it can be embedded as-is
            msg.reply_json(&format!(r#"{{"type":"echo","data":{}}}"#, msg.json()))
        };
        if let Err(e) = result {
            eprintln!("[app] failed to handle web message: {e}");
        }
    }

    // Host -> page script with a result: the reply goes out when the script completes.
    fn probe_page(msg: &WebMessage) -> windows_core::Result<()> {
        let reply_to = msg.clone();
        msg.execute_script(demo::PROBE_JS, move |res| {
            // the result is JSON, so it can be embedded as-is
            let reply = match res {
                Ok(json) => format!(r#"{{"type":"probe","data":{json}}}"#),
                Err(e) => {
                    eprintln!("[app] probe script failed: {e}");
                    r#"{"type":"probe","error":"script failed"}"#.to_owned()
                }
            };
            if let Err(e) = reply_to.reply_json(&reply) {
                eprintln!("[app] failed to reply probe: {e}");
            }
        })
    }

    // Called inside the opener's NewWindowRequested callback.
    fn handle_new_window(
        shared: &Rc<Shared>,
        opener: HWND,
        req: NewWindowRequest,
    ) -> windows_core::Result<()> {
        let decision = classify_navigation(req.uri());
        eprintln!(
            "[app] new window requested ({:?}, {:?}): {}",
            NEW_WINDOW_POLICY,
            decision,
            req.uri()
        );
        // the navigation filter wins over the new-window policy
        match decision {
            NavDecision::Allow => {}
            NavDecision::External => return App::open_new_window_externally(req),
            NavDecision::Block => return req.block(),
        }
        match NEW_WINDOW_POLICY {
            NewWindowPolicy::Default => {
                req.allow_default();
                Ok(())
            }
            NewWindowPolicy::Block => req.block(),
            NewWindowPolicy::NavigateHere => req.navigate_here(),
            NewWindowPolicy::OwnWindow => App::open_popup(shared, opener, req),
            NewWindowPolicy::External => App::open_new_window_externally(req),
        }
    }

    // Cancel the popup (window.open returns null) and hand the URI to the system browser.
    fn open_new_window_externally(req: NewWindowRequest) -> windows_core::Result<()> {
        let uri = req.uri().to_owned();
        req.block()?;
        open_external(&uri);
        Ok(())
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
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> Option<LRESULT> {
        match msg {
            // nonzero = erased; otherwise DefWindowProcW paints the class brush
            WM_ERASEBKGND => {
                mswin::erase_background(hwnd, wparam, BACKGROUND_RGB).then_some(LRESULT(1))
            }
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
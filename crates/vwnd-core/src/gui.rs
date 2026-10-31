use std::{
    borrow::Cow,
    cell::{Cell, RefCell},
    path::PathBuf,
    rc::Rc,
    time::{Duration, Instant},
};

use windows::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, WPARAM},
    UI::Input::KeyboardAndMouse::{VK_ESCAPE, VK_F4},
    UI::WindowsAndMessaging::{
        WM_DESTROY, WM_DPICHANGED, WM_ERASEBKGND, WM_MOVE, WM_SETFOCUS, WM_SIZE,
    },
};
use windows_core::w;

use crate::gui::{
    com::ComApartment,
    mswin::{WindowClass, WindowHandler},
    nav::{BridgePolicy, NavDecision, NavPolicy, normalize_user_url},
    webview::{
        AcceleratorKey, NavErrorKind, NavigationCompletedEvent, NewWindowRequest, PendingNewWindow,
        PermissionKind, PermissionRequest, ProcessFailedEvent, ProcessFailure, WebMessage, WebView,
        WebViewBackground, WebViewEnv, WebViewEnvOptions, WebViewHost, WebViewSettings,
    },
};

mod com;
mod demo;
mod mswin;
mod nav;
mod webview;

// What the main window loads first
#[derive(Clone)]
#[allow(dead_code)]
enum StartPage {
    Url(Cow<'static, str>),
    /// built-in page for testing popups, close, the bridge and navigation filtering
    TestPage,
}

// Used when no URL is given on the command line (`vwnd.exe [url]`)
const START_PAGE: StartPage = StartPage::TestPage;
// const START_PAGE: StartPage = StartPage::Url(Cow::Borrowed("https://www.google.com/"));

// `vwnd.exe [url]`: a URL argument overrides START_PAGE
// - "example.com" (no scheme) -> "https://example.com" (nav::normalize_user_url)
// - it still goes through NAV_POLICY like any navigation, so a blocked URL shows nothing
fn start_page_from_args() -> StartPage {
    // args_os: std::env::args() panics on an argument that isn't valid Unicode
    match std::env::args_os().nth(1) {
        Some(arg) => StartPage::Url(Cow::Owned(normalize_user_url(&arg.to_string_lossy()))),
        None => START_PAGE,
    }
}

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

// Which navigations stay in the app (see gui/nav.rs for the rules)
// - in_app_hosts: None = every http(s) host stays in the app;
//   Some(hosts) = those hosts and their subdomains stay, others open in the system browser
// - external_schemes: non-http(s) schemes handed to the system (others are blocked)
const NAV_POLICY: NavPolicy<'static> = NavPolicy {
    in_app_hosts: None,
    // in_app_hosts: Some(&["example.com", "google.com"]),
    external_schemes: &["mailto", "tel"],
};

// Who may message the host over the bridge (see gui/nav.rs)
// - trusted_hosts: https hosts (and subdomains) whose pages may call into the app
// - trust_about_blank: needed for the built-in pages (NavigateToString reports about:blank);
//   note that a script-opened blank popup reports it too
// Messages from anyone else are dropped without a reply.
const BRIDGE_POLICY: BridgePolicy<'static> = BridgePolicy {
    trusted_hosts: &[],
    // trusted_hosts: &["example.com"],
    trust_about_blank: true,
};

// Page permission requests (camera, microphone, location, notifications, clipboard read, ...)
// - kinds listed here are allowed for pages BRIDGE_POLICY trusts
// - everything else is denied: no WebView2 prompt, the page just gets "denied"
const ALLOWED_PERMISSIONS: &[PermissionKind] = &[];
// const ALLOWED_PERMISSIONS: &[PermissionKind] = &[PermissionKind::ClipboardRead];

// Keyboard shortcuts that close a popup window (the main window has none)
// - Ctrl+W / Ctrl+F4: like closing a browser tab
// - Esc: off by default, since pages use it (closing their dialogs, leaving fullscreen)
const POPUP_CLOSE_ON_ESC: bool = false;

// The main window couldn't get a WebView (sync: window creation failed; async: environment
// or controller creation failed). The usual cause is a missing WebView2 Runtime.
fn report_startup_failure(owner: Option<HWND>, e: &windows_core::Error) {
    eprintln!("[app] startup failed: {e}");
    let text = format!(
        "The web view could not be started.\n\n{e}\n\n\
         Make sure the Microsoft Edge WebView2 Runtime is installed."
    );
    mswin::show_error(owner, "vwnd", &text);
}

fn open_external(uri: &str) {
    if !NAV_POLICY.can_open_externally(uri) {
        eprintln!("[app] not opening externally: {uri}");
        return;
    }
    eprintln!("[app] opening in system browser: {uri}");
    if let Err(e) = mswin::open_external(uri) {
        eprintln!("[app] failed to open externally: {e}");
    }
}

// Environment options: fixed for the app's lifetime (all windows share one environment)
// - language: WebView2's UI and the pages' default language; None = the OS language
const ENV_LANGUAGE: Option<&str> = Some("ko-KR");

fn env_options() -> WebViewEnvOptions {
    WebViewEnvOptions {
        language: ENV_LANGUAGE.map(str::to_owned),
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
    // counted in Shared.open_windows: only once WM_CREATE succeeded
    counted: Cell<bool>,
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

        let main = Rc::new(App::new(
            shared.clone(),
            Initial::Start(start_page_from_args()),
        ));
        let (w, h) = mswin::outer_size_for_client_dip(None, MAIN_SIZE.0, MAIN_SIZE.1)?;
        // a synchronous failure (e.g. no WebView2 runtime) fails WM_CREATE -> no window
        let hwnd = shared
            .wnd_class
            .create_wnd("vwnd_wnd", None, w, h, main)
            .inspect_err(|e| report_startup_failure(None, e))?;
        mswin::show(hwnd);

        let _exit_code = mswin::run_message_loop()?;
        Ok(())
    }

    fn new(shared: Rc<Shared>, initial: Initial) -> Self {
        Self {
            shared,
            is_popup: matches!(initial, Initial::NewWindow(_)),
            hwnd: Cell::new(HWND::default()),
            counted: Cell::new(false),
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
        host.set_on_navigation_starting(|uri| match NAV_POLICY.classify(uri) {
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
        // set navigation completed callback (error page for unreachable pages)
        host.set_on_navigation_completed(App::handle_navigation_completed)?;
        // set permission requested callback (deny unless allowed)
        host.set_on_permission_requested(App::handle_permission_requested)?;
        // HTML fullscreen: the window covers its monitor while the page is fullscreen
        let windowed = Cell::new(None);
        host.set_on_fullscreen_changed(move |fullscreen| {
            App::handle_fullscreen_changed(hwnd, &windowed, fullscreen)
        })?;
        // popups only: close shortcuts (see POPUP_CLOSE_ON_ESC)
        if self.is_popup {
            host.set_on_accelerator_key(move |key| {
                if !App::is_popup_close_key(key) {
                    return;
                }
                let _ = key.set_handled();
                mswin::post_close(hwnd);
            })?;
        }
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
            Some(Initial::Start(StartPage::Url(url))) => view.navigate(&url),
            Some(Initial::Start(StartPage::TestPage)) => view.navigate_to_string(demo::TEST_HTML),
            Some(Initial::NewWindow(pending)) => pending.complete(&view),
            None => Ok(()),
        }
    }

    // Page -> host messages. Demo protocol: echo back as {"type":"echo","data":<message>}.
    // In real use, check msg.source() before trusting the content.
    // `windowed`: the frame/placement to restore; Some while this window is fullscreen
    fn handle_fullscreen_changed(
        hwnd: HWND,
        windowed: &Cell<Option<mswin::WindowedState>>,
        fullscreen: bool,
    ) {
        let result = match (fullscreen, windowed.take()) {
            (true, None) => mswin::enter_fullscreen(hwnd).map(|state| windowed.set(Some(state))),
            // already fullscreen: keep the original windowed state
            (true, Some(state)) => {
                windowed.set(Some(state));
                Ok(())
            }
            (false, Some(state)) => mswin::exit_fullscreen(hwnd, &state),
            (false, None) => Ok(()),
        };
        if let Err(e) = result {
            eprintln!("[app] failed to switch fullscreen ({fullscreen}): {e}");
        }
    }

    fn is_popup_close_key(key: &AcceleratorKey) -> bool {
        if !key.is_key_down() || key.is_repeat() {
            return false;
        }
        let vk = key.virtual_key();
        if mswin::is_ctrl_down() {
            vk == u32::from(b'W') || vk == u32::from(VK_F4.0)
        } else {
            POPUP_CLOSE_ON_ESC && vk == u32::from(VK_ESCAPE.0)
        }
    }

    // Allow only ALLOWED_PERMISSIONS, and only for pages BRIDGE_POLICY trusts; deny the rest.
    fn handle_permission_requested(req: PermissionRequest) {
        let allow = ALLOWED_PERMISSIONS.contains(&req.kind()) && BRIDGE_POLICY.trusts(req.uri());
        eprintln!(
            "[app] permission {:?} {} for {} (user initiated: {})",
            req.kind(),
            if allow { "allowed" } else { "denied" },
            req.uri(),
            req.is_user_initiated()
        );
        let result = if allow { req.allow() } else { req.deny() };
        if let Err(e) = result {
            eprintln!("[app] failed to answer permission request: {e}");
        }
    }

    // Top-level navigation results.
    // - network / certificate errors: replace WebView2's error page with the app's own
    // - canceled: normal (NAV_POLICY blocked it, a newer navigation replaced it, a download)
    // - other (auth required, bad response, ...): logged, WebView2's own handling stays
    fn handle_navigation_completed(ev: NavigationCompletedEvent) {
        let Some(err) = ev.error() else {
            return;
        };
        if err.kind == NavErrorKind::Canceled {
            return;
        }
        eprintln!(
            "[app] navigation failed ({:?}, {}): {}",
            err.kind,
            err.status,
            ev.uri()
        );
        if !matches!(err.kind, NavErrorKind::Network | NavErrorKind::Certificate) {
            return;
        }
        if let Err(e) = ev.show_html(&demo::error_page_html(ev.uri(), err.status)) {
            eprintln!("[app] failed to show error page: {e}");
        }
    }

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
    // Only sources BRIDGE_POLICY trusts get here; the rest are dropped first.
    fn handle_web_message(msg: WebMessage) {
        if !BRIDGE_POLICY.trusts(msg.source()) {
            eprintln!(
                "[app] web message dropped (untrusted source): {}",
                msg.source()
            );
            return;
        }
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
        let decision = NAV_POLICY.classify(req.uri());
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
        // window.open features give a content size and an outer position, both in DIPs
        // (read before defer(), which consumes the request)
        let (cw, ch) = req
            .requested_size()
            .and_then(|(w, h)| Some((i32::try_from(w).ok()?, i32::try_from(h).ok()?)))
            .unwrap_or(POPUP_SIZE);
        let req_pos = req.requested_position();

        // Hold the request first: from here on, any failure drops the pending request,
        // which cancels it instead of falling back to the WebView2 default popup.
        let pending = req.defer()?;

        // DIP content size -> physical outer size, using the opener's DPI
        let (w, h) = mswin::outer_size_for_client_dip(Some(opener), cw, ch)?;
        // DIP screen position -> physical; off every monitor (or none requested) -> system default
        let pos = req_pos.and_then(|(x, y)| mswin::screen_point_for_dip(opener, x, y));

        let popup = Rc::new(App::new(shared.clone(), Initial::NewWindow(pending)));
        let hwnd = shared
            .wnd_class
            .create_wnd("vwnd_popup", pos, w, h, popup)?;
        mswin::show(hwnd);
        Ok(())
    }

    fn on_setup_failed(&self, e: windows_core::Error) {
        eprintln!("[app] failed to setup webview: {e}");
        // dropping a PendingNewWindow cancels the request (window.open returns null)
        let initial = self.initial.borrow_mut().take();
        drop(initial);
        let hwnd = self.hwnd.get();
        if !self.is_popup {
            // main window: an empty window is no use -> tell the user, then close (and quit).
            // Box first: closing first would quit, and WM_QUIT ends the box's own loop at once.
            report_startup_failure(Some(hwnd), &e);
        }
        mswin::post_close(hwnd);
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

    // Popups (and any later window) reuse the shared environment; the first window creates
    // it, then its controller. Errors here are synchronous; async ones go to on_setup_failed.
    fn start_webview(self: &Rc<Self>) -> windows_core::Result<()> {
        let env = self.shared.env.borrow().clone();
        if let Some(env) = env {
            return self.create_host(&env);
        }
        let weak = Rc::downgrade(self);
        WebViewEnv::create(&App::default_user_data_dir(), &env_options(), move |res| {
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

    fn on_destroy(&self) {
        // cancel a popup request that never completed (window closed during setup)
        let initial = self.initial.borrow_mut().take();
        drop(initial);
        self.close_webview();

        // quit when the last window (main or popup) is gone
        if !self.counted.replace(false) {
            return;
        }
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
        self.start_webview()?;
        // Counted only on success. A failed WM_CREATE still gets WM_DESTROY, and counting it
        // would post WM_QUIT there, which would close the startup error box at once.
        self.counted.set(true);
        self.shared
            .open_windows
            .set(self.shared.open_windows.get() + 1);
        Ok(())
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
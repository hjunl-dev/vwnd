use std::path::Path;

use webview2_com::{
    AddScriptToExecuteOnDocumentCreatedCompletedHandler,
    CreateCoreWebView2ControllerCompletedHandler, CreateCoreWebView2EnvironmentCompletedHandler,
    DocumentTitleChangedEventHandler, ExecuteScriptCompletedHandler,
    Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC,
        COREWEBVIEW2_PROCESS_FAILED_KIND_BROWSER_PROCESS_EXITED,
        COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED,
        COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_UNRESPONSIVE,
        COREWEBVIEW2_COLOR, CreateCoreWebView2EnvironmentWithOptions, ICoreWebView2,
        ICoreWebView2Controller, ICoreWebView2Controller2, ICoreWebView2Deferral,
        ICoreWebView2Environment, ICoreWebView2NewWindowRequestedEventArgs,
        ICoreWebView2Settings3, ICoreWebView2WebMessageReceivedEventArgs,
    },
    NavigationStartingEventHandler, NewWindowRequestedEventHandler, ProcessFailedEventHandler,
    WebMessageReceivedEventHandler, WindowCloseRequestedEventHandler, take_pwstr,
};
use windows::Win32::Foundation::{E_POINTER, HWND, RECT};
use windows_core::{Error, HSTRING, Interface, PCWSTR, PWSTR};

use crate::gui::com::EventReg;

// ============================================================
// WebViewEnv
// - one environment shared by every WebView in the app
// - popups set via SetNewWindow must come from the same environment
// ============================================================

#[derive(Clone)]
pub struct WebViewEnv(ICoreWebView2Environment);

impl WebViewEnv {
    // CreateCoreWebView2EnvironmentWithOptions
    pub fn create(
        udf_path: &Path,
        on_create: impl FnOnce(windows_core::Result<WebViewEnv>) + 'static,
    ) -> windows_core::Result<()> {
        let mut on_create = Some(on_create);
        let udf_path = HSTRING::from(&*udf_path.to_string_lossy());

        let handler =
            CreateCoreWebView2EnvironmentCompletedHandler::create(Box::new(move |res, env| {
                let result = res
                    .and_then(|()| env.ok_or_else(|| Error::from(E_POINTER)))
                    .map(WebViewEnv);
                if let Some(cb) = on_create.take() {
                    cb(result);
                }
                Ok(())
            }));
        unsafe {
            CreateCoreWebView2EnvironmentWithOptions(PCWSTR::null(), &udf_path, None, &handler)
        }
    }

    // CreateCoreWebView2Controller
    pub fn create_host(
        &self,
        target: HWND,
        on_create: impl FnOnce(windows_core::Result<WebViewHost>) + 'static,
    ) -> windows_core::Result<()> {
        let mut on_create = Some(on_create);

        let handler =
            CreateCoreWebView2ControllerCompletedHandler::create(Box::new(move |res, ctrl| {
                let result = res
                    .and_then(|()| ctrl.ok_or_else(|| Error::from(E_POINTER)))
                    .and_then(WebViewHost::from_controller);
                if let Some(cb) = on_create.take() {
                    cb(result);
                }
                Ok(())
            }));
        unsafe { self.0.CreateCoreWebView2Controller(target, &handler) }
    }
}

// ============================================================
// WebViewSettings
// - applied per WebView (ICoreWebView2Settings / Settings3)
// ============================================================

#[derive(Clone, Copy, Debug)]
pub struct WebViewSettings {
    /// F12 / Inspect
    pub dev_tools: bool,
    /// Right-click menu
    pub default_context_menus: bool,
    /// Link URL shown at the bottom-left on hover
    pub status_bar: bool,
    /// Browser shortcuts such as F5, Ctrl+R, Ctrl+P, Ctrl+F
    /// (editing keys like Ctrl+C/V keep working when disabled)
    pub browser_accelerator_keys: bool,
}

// ============================================================
// WebViewBackground
// - shown where the page paints nothing: before the first paint,
//   and behind a page whose own background is transparent
// - WebView2 accepts only fully opaque or fully transparent (no partial alpha)
// ============================================================

#[derive(Clone, Copy, Debug)]
#[allow(dead_code)]
pub enum WebViewBackground {
    Rgb(u8, u8, u8),
    /// The host window shows through (for overlay-style UI)
    Transparent,
}

// ============================================================
// WebView
// ============================================================

#[derive(Clone)]
pub struct WebView {
    ctrl: ICoreWebView2Controller,
    wv: ICoreWebView2,
}

impl WebView {
    pub fn set_bounds(&self, bounds: RECT) -> windows_core::Result<()> {
        unsafe { self.ctrl.SetBounds(bounds) }
    }

    pub fn set_visible(&self, visible: bool) -> windows_core::Result<()> {
        unsafe { self.ctrl.SetIsVisible(visible) }
    }

    /// Set right after the controller is created, before the first paint.
    /// ICoreWebView2Controller2 needs a newer runtime: skipped (logged) on old runtimes.
    pub fn set_background(&self, bg: WebViewBackground) -> windows_core::Result<()> {
        let color = match bg {
            WebViewBackground::Rgb(r, g, b) => COREWEBVIEW2_COLOR { A: 255, R: r, G: g, B: b },
            WebViewBackground::Transparent => COREWEBVIEW2_COLOR { A: 0, R: 0, G: 0, B: 0 },
        };
        match self.ctrl.cast::<ICoreWebView2Controller2>() {
            Ok(ctrl2) => unsafe { ctrl2.SetDefaultBackgroundColor(color) },
            Err(e) => {
                eprintln!("[webview] ICoreWebView2Controller2 unavailable: {e}");
                Ok(())
            }
        }
    }

    pub fn navigate(&self, url: &str) -> windows_core::Result<()> {
        unsafe { self.wv.Navigate(&HSTRING::from(url)) }
    }

    /// Loads the given HTML as the document (origin is about:blank-like; max ~2 MB).
    pub fn navigate_to_string(&self, html: &str) -> windows_core::Result<()> {
        unsafe { self.wv.NavigateToString(&HSTRING::from(html)) }
    }

    /// Runs `js` in every document (top-level and iframes) before the page's own scripts.
    /// Applies to navigations started after this call, so add scripts before
    /// `navigate*` / `PendingNewWindow::complete`.
    pub fn add_script_on_document_created(&self, js: &str) -> windows_core::Result<()> {
        let handler = AddScriptToExecuteOnDocumentCreatedCompletedHandler::create(Box::new(
            |res, _id| {
                if let Err(e) = res {
                    eprintln!("[webview] failed to add document-created script: {e}");
                }
                Ok(())
            },
        ));
        unsafe {
            self.wv
                .AddScriptToExecuteOnDocumentCreated(&HSTRING::from(js), &handler)
        }
    }

    /// Sends a JSON value to the page (`chrome.webview` "message" event, `e.data` is parsed).
    /// For host-initiated pushes; replies to a page message go through `WebMessage::reply_json`.
    #[allow(dead_code)]
    pub fn post_json(&self, json: &str) -> windows_core::Result<()> {
        post_json(&self.wv, json)
    }

    /// Sends a string to the page (`e.data` is the string).
    #[allow(dead_code)]
    pub fn post_string(&self, text: &str) -> windows_core::Result<()> {
        post_string(&self.wv, text)
    }

    /// Runs `js` in the top-level document; `on_done` gets the result as JSON.
    /// See `execute_script` (helpers) for what the result looks like.
    #[allow(dead_code)]
    pub fn execute_script(
        &self,
        js: &str,
        on_done: impl FnOnce(windows_core::Result<String>) + 'static,
    ) -> windows_core::Result<()> {
        execute_script(&self.wv, js, on_done)
    }

    pub fn apply_settings(&self, s: &WebViewSettings) -> windows_core::Result<()> {
        unsafe {
            let settings = self.wv.Settings()?;
            settings.SetAreDevToolsEnabled(s.dev_tools)?;
            settings.SetAreDefaultContextMenusEnabled(s.default_context_menus)?;
            settings.SetIsStatusBarEnabled(s.status_bar)?;
            // Settings3 needs a newer runtime: skip rather than fail on old runtimes
            match settings.cast::<ICoreWebView2Settings3>() {
                Ok(s3) => s3.SetAreBrowserAcceleratorKeysEnabled(s.browser_accelerator_keys)?,
                Err(e) => eprintln!("[webview] ICoreWebView2Settings3 unavailable: {e}"),
            }
        }
        Ok(())
    }

    /// Gives keyboard focus to the WebView (call when the host window gets focus).
    pub fn focus(&self) -> windows_core::Result<()> {
        unsafe { self.ctrl.MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC) }
    }

    /// Call when the host (or an ancestor) window moves, so popups such as
    /// <select> dropdowns and IME candidate windows stay aligned.
    pub fn notify_parent_moved(&self) -> windows_core::Result<()> {
        unsafe { self.ctrl.NotifyParentWindowPositionChanged() }
    }
}

// ============================================================
// NewWindowRequest
// - passed to the NewWindowRequested callback
// - dropping it without calling anything = WebView2 default popup
// ============================================================

pub struct NewWindowRequest {
    sender: ICoreWebView2,
    args: ICoreWebView2NewWindowRequestedEventArgs,
    uri: String,
}

impl NewWindowRequest {
    pub fn uri(&self) -> &str {
        &self.uri
    }

    /// Position requested by window.open features (e.g. "left=100,top=50"):
    /// the outer window's top-left in screen DIPs (CSS px).
    /// Use mswin::screen_point_for_dip to get the position for create_wnd.
    pub fn requested_position(&self) -> Option<(i32, i32)> {
        let features = unsafe { self.args.WindowFeatures() }.ok()?;
        let mut has_position = Default::default();
        unsafe { features.HasPosition(&mut has_position) }.ok()?;
        if !has_position.as_bool() {
            return None;
        }
        let (mut x, mut y) = (0u32, 0u32);
        unsafe {
            features.Left(&mut x).ok()?;
            features.Top(&mut y).ok()?;
        }
        Some((i32::try_from(x).ok()?, i32::try_from(y).ok()?))
    }

    /// Size requested by window.open features (e.g. "width=400,height=300").
    /// Note: this is the content size in DIPs (CSS px), not the outer window size.
    /// Use mswin::outer_size_for_client_dip to get the size for create_wnd.
    pub fn requested_size(&self) -> Option<(u32, u32)> {
        let features = unsafe { self.args.WindowFeatures() }.ok()?;
        let mut has_size = Default::default();
        unsafe { features.HasSize(&mut has_size) }.ok()?;
        if !has_size.as_bool() {
            return None;
        }
        let (mut w, mut h) = (0u32, 0u32);
        unsafe {
            features.Width(&mut w).ok()?;
            features.Height(&mut h).ok()?;
        }
        (w > 0 && h > 0).then_some((w, h))
    }

    /// Stage 1: cancel the popup and navigate the current view instead.
    pub fn navigate_here(self) -> windows_core::Result<()> {
        unsafe {
            self.args.SetHandled(true)?;
            self.sender.Navigate(&HSTRING::from(self.uri.as_str()))
        }
    }

    /// Cancel the popup (window.open returns null).
    pub fn block(self) -> windows_core::Result<()> {
        unsafe { self.args.SetHandled(true) }
    }

    /// Explicitly let WebView2 open its own popup window.
    pub fn allow_default(self) {}

    /// Stage 2: hold the request until our own WebView is ready.
    /// Complete it later with `PendingNewWindow::complete`.
    pub fn defer(self) -> windows_core::Result<PendingNewWindow> {
        match unsafe { self.args.GetDeferral() } {
            Ok(deferral) => Ok(PendingNewWindow {
                inner: Some((self.args, deferral)),
            }),
            Err(e) => {
                // don't fall back to the default popup on failure
                let _ = unsafe { self.args.SetHandled(true) };
                Err(e)
            }
        }
    }
}

// ============================================================
// WebMessage
// - passed to the WebMessageReceived callback (chrome.webview.postMessage)
// ============================================================

#[derive(Clone)]
pub struct WebMessage {
    sender: ICoreWebView2,
    source: String,
    json: String,
    text: Option<String>,
}

impl WebMessage {
    /// URI of the document that posted the message. Check it before trusting the content.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// The message as JSON (always available; a posted string becomes a JSON string literal).
    pub fn json(&self) -> &str {
        &self.json
    }

    /// The message if the page posted a plain string.
    pub fn text(&self) -> Option<&str> {
        self.text.as_deref()
    }

    /// Replies to the WebView that posted the message.
    pub fn reply_json(&self, json: &str) -> windows_core::Result<()> {
        post_json(&self.sender, json)
    }

    #[allow(dead_code)]
    pub fn reply_string(&self, text: &str) -> windows_core::Result<()> {
        post_string(&self.sender, text)
    }

    /// Runs `js` in the WebView that posted the message (see `WebView::execute_script`).
    pub fn execute_script(
        &self,
        js: &str,
        on_done: impl FnOnce(windows_core::Result<String>) + 'static,
    ) -> windows_core::Result<()> {
        execute_script(&self.sender, js, on_done)
    }

    fn from_args(
        sender: ICoreWebView2,
        args: &ICoreWebView2WebMessageReceivedEventArgs,
    ) -> windows_core::Result<Self> {
        let mut raw = PWSTR::null();
        unsafe { args.Source(&mut raw)? };
        let source = take_pwstr(raw);

        let mut raw = PWSTR::null();
        unsafe { args.WebMessageAsJson(&mut raw)? };
        let json = take_pwstr(raw);

        // fails (E_INVALIDARG) when the message is not a string -> None
        let mut raw = PWSTR::null();
        let text = unsafe { args.TryGetWebMessageAsString(&mut raw) }
            .ok()
            .map(|()| take_pwstr(raw));

        Ok(Self {
            sender,
            source,
            json,
            text,
        })
    }
}

// ============================================================
// ProcessFailed
// - passed to the ProcessFailed callback
// ============================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessFailure {
    /// The browser process exited: this WebView and its environment are gone.
    /// Only closing or recreating (new environment + controller) recovers.
    BrowserExited,
    /// The page's renderer exited: the view shows an error page until reloaded.
    RendererExited,
    /// The page's renderer stopped responding (it may still recover by itself).
    RendererUnresponsive,
    /// Frame renderer, GPU, utility process, ...: WebView2 recovers by itself.
    Other(i32),
}

pub struct ProcessFailedEvent {
    sender: ICoreWebView2,
    kind: ProcessFailure,
}

impl ProcessFailedEvent {
    pub fn kind(&self) -> ProcessFailure {
        self.kind
    }

    /// Reloads the failed WebView (renderer failures only).
    pub fn reload(&self) -> windows_core::Result<()> {
        unsafe { self.sender.Reload() }
    }

    /// Replaces the page with the given HTML (renderer failures only).
    pub fn show_html(&self, html: &str) -> windows_core::Result<()> {
        unsafe { self.sender.NavigateToString(&HSTRING::from(html)) }
    }
}

// ============================================================
// PendingNewWindow
// - deferred NewWindowRequested
// - complete(): SetNewWindow (C++ put_NewWindow) + Deferral::Complete
// - drop without complete(): cancelled (Handled = true)
// ============================================================

pub struct PendingNewWindow {
    inner: Option<(ICoreWebView2NewWindowRequestedEventArgs, ICoreWebView2Deferral)>,
}

impl PendingNewWindow {
    /// `view` must be created from the same environment and must not have navigated yet.
    /// Register settings/event handlers on it before calling this.
    pub fn complete(mut self, view: &WebView) -> windows_core::Result<()> {
        let Some((args, deferral)) = self.inner.take() else {
            return Ok(());
        };
        let result = unsafe { args.SetNewWindow(&view.wv) };
        if result.is_err() {
            let _ = unsafe { args.SetHandled(true) };
        }
        let completed = unsafe { deferral.Complete() };
        result.and(completed)
    }

    #[allow(dead_code)]
    pub fn cancel(mut self) {
        self.cancel_inner();
    }

    fn cancel_inner(&mut self) {
        if let Some((args, deferral)) = self.inner.take() {
            unsafe {
                let _ = args.SetHandled(true);
                let _ = deferral.Complete();
            }
        }
    }
}

impl Drop for PendingNewWindow {
    fn drop(&mut self) {
        self.cancel_inner();
    }
}

// ============================================================
// WebViewHost
// - Drop: remove every event registration, then Controller::Close
//   (guaranteed on every path, including early `?` returns during setup)
// ============================================================

pub struct WebViewHost {
    event_regs: Vec<EventReg>,
    view: WebView,
}

impl Drop for WebViewHost {
    fn drop(&mut self) {
        // remove_Xxx...
        self.event_regs.clear();
        if let Err(e) = unsafe { self.view.ctrl.Close() } {
            eprintln!("[webview] failed to close webview2 controller: {e}");
        }
        // ICoreWebView2 / ICoreWebView2Controller are released by the field drops after this
    }
}

impl WebViewHost {
    /// Explicit close (same as drop).
    pub fn close(self) {
        drop(self);
    }

    pub fn view(&self) -> WebView {
        self.view.clone()
    }

    pub fn set_on_document_title_changed(
        &mut self,
        cb: impl Fn(String) + 'static,
    ) -> windows_core::Result<()> {
        let handler = DocumentTitleChangedEventHandler::create(Box::new(move |sender, _args| {
            let Some(sender) = sender else {
                return Ok(());
            };
            let mut raw_str = PWSTR::null();
            unsafe { sender.DocumentTitle(&mut raw_str)? };
            cb(take_pwstr(raw_str));
            Ok(())
        }));

        let mut token = Default::default();
        unsafe {
            self.view
                .wv
                .add_DocumentTitleChanged(&handler, &mut token)?
        };
        self.track(move |wv| {
            let _ = unsafe { wv.remove_DocumentTitleChanged(token) };
        });
        Ok(())
    }

    pub fn set_on_new_window_requested(
        &mut self,
        cb: impl Fn(NewWindowRequest) + 'static,
    ) -> windows_core::Result<()> {
        let handler = NewWindowRequestedEventHandler::create(Box::new(move |sender, args| {
            let (Some(sender), Some(args)) = (sender, args) else {
                return Ok(());
            };
            let mut raw = PWSTR::null();
            unsafe { args.Uri(&mut raw)? };
            let uri = take_pwstr(raw);
            cb(NewWindowRequest { sender, args, uri });
            Ok(())
        }));

        let mut token = Default::default();
        unsafe { self.view.wv.add_NewWindowRequested(&handler, &mut token)? };
        self.track(move |wv| {
            let _ = unsafe { wv.remove_NewWindowRequested(token) };
        });
        Ok(())
    }

    pub fn set_on_window_close_requested(
        &mut self,
        cb: impl Fn() + 'static,
    ) -> windows_core::Result<()> {
        let handler = WindowCloseRequestedEventHandler::create(Box::new(move |_sender, _args| {
            cb();
            Ok(())
        }));

        let mut token = Default::default();
        unsafe { self.view.wv.add_WindowCloseRequested(&handler, &mut token)? };
        self.track(move |wv| {
            let _ = unsafe { wv.remove_WindowCloseRequested(token) };
        });
        Ok(())
    }

    /// Top-level navigations (link clicks, redirects, location changes, Navigate*).
    /// Return false from `cb` to cancel the navigation.
    pub fn set_on_navigation_starting(
        &mut self,
        cb: impl Fn(&str) -> bool + 'static,
    ) -> windows_core::Result<()> {
        let handler = NavigationStartingEventHandler::create(Box::new(move |_sender, args| {
            let Some(args) = args else {
                return Ok(());
            };
            let mut raw = PWSTR::null();
            unsafe { args.Uri(&mut raw)? };
            let uri = take_pwstr(raw);
            if !cb(&uri) {
                unsafe { args.SetCancel(true)? };
            }
            Ok(())
        }));

        let mut token = Default::default();
        unsafe { self.view.wv.add_NavigationStarting(&handler, &mut token)? };
        self.track(move |wv| {
            let _ = unsafe { wv.remove_NavigationStarting(token) };
        });
        Ok(())
    }

    pub fn set_on_web_message_received(
        &mut self,
        cb: impl Fn(WebMessage) + 'static,
    ) -> windows_core::Result<()> {
        let handler = WebMessageReceivedEventHandler::create(Box::new(move |sender, args| {
            let (Some(sender), Some(args)) = (sender, args) else {
                return Ok(());
            };
            cb(WebMessage::from_args(sender, &args)?);
            Ok(())
        }));

        let mut token = Default::default();
        unsafe { self.view.wv.add_WebMessageReceived(&handler, &mut token)? };
        self.track(move |wv| {
            let _ = unsafe { wv.remove_WebMessageReceived(token) };
        });
        Ok(())
    }

    /// A WebView2 process (browser, renderer, GPU, ...) exited or hung.
    pub fn set_on_process_failed(
        &mut self,
        cb: impl Fn(ProcessFailedEvent) + 'static,
    ) -> windows_core::Result<()> {
        let handler = ProcessFailedEventHandler::create(Box::new(move |sender, args| {
            let (Some(sender), Some(args)) = (sender, args) else {
                return Ok(());
            };
            let mut raw = Default::default();
            unsafe { args.ProcessFailedKind(&mut raw)? };
            let kind = match raw {
                COREWEBVIEW2_PROCESS_FAILED_KIND_BROWSER_PROCESS_EXITED => {
                    ProcessFailure::BrowserExited
                }
                COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED => {
                    ProcessFailure::RendererExited
                }
                COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_UNRESPONSIVE => {
                    ProcessFailure::RendererUnresponsive
                }
                other => ProcessFailure::Other(other.0),
            };
            cb(ProcessFailedEvent { sender, kind });
            Ok(())
        }));

        let mut token = Default::default();
        unsafe { self.view.wv.add_ProcessFailed(&handler, &mut token)? };
        self.track(move |wv| {
            let _ = unsafe { wv.remove_ProcessFailed(token) };
        });
        Ok(())
    }

    /// Keeps the event-removal closure alive as an EventReg (runs on drop).
    fn track(&mut self, remove: impl FnOnce(&ICoreWebView2) + 'static) {
        let wv = self.view.wv.clone();
        self.event_regs.push(EventReg::new(move || remove(&wv)));
    }

    fn from_controller(ctrl: ICoreWebView2Controller) -> windows_core::Result<Self> {
        let wv = unsafe { ctrl.CoreWebView2()? };
        Ok(Self {
            event_regs: Vec::new(),
            view: WebView { ctrl, wv },
        })
    }
}

// ============================================================
// webview2 helpers
// ============================================================

fn post_json(wv: &ICoreWebView2, json: &str) -> windows_core::Result<()> {
    unsafe { wv.PostWebMessageAsJson(&HSTRING::from(json)) }
}

fn post_string(wv: &ICoreWebView2, text: &str) -> windows_core::Result<()> {
    unsafe { wv.PostWebMessageAsString(&HSTRING::from(text)) }
}

// ExecuteScript
// - runs in the top-level document only (not iframes)
// - result = the script's completion value as JSON: `"null"` for undefined,
//   `{}` for values JSON can't represent (DOM nodes, functions)
// - a thrown exception is NOT reported as an error here: the result is just `"null"`
//   (catch inside the script and return the error as a value if it matters)
// - the callback runs later on the UI thread, possibly after the page navigated away
fn execute_script(
    wv: &ICoreWebView2,
    js: &str,
    on_done: impl FnOnce(windows_core::Result<String>) + 'static,
) -> windows_core::Result<()> {
    let handler = ExecuteScriptCompletedHandler::create(Box::new(move |res, json| {
        on_done(res.map(|()| json));
        Ok(())
    }));
    unsafe { wv.ExecuteScript(&HSTRING::from(js), &handler) }
}

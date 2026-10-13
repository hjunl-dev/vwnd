use std::path::Path;

use webview2_com::{
    CreateCoreWebView2ControllerCompletedHandler, CreateCoreWebView2EnvironmentCompletedHandler,
    DocumentTitleChangedEventHandler,
    Microsoft::Web::WebView2::Win32::{
        CreateCoreWebView2EnvironmentWithOptions, ICoreWebView2, ICoreWebView2Controller,
        ICoreWebView2Deferral, ICoreWebView2Environment, ICoreWebView2NewWindowRequestedEventArgs,
    },
    NewWindowRequestedEventHandler, WindowCloseRequestedEventHandler, take_pwstr,
};
use windows::Win32::Foundation::{E_POINTER, HWND, RECT};
use windows_core::{Error, HSTRING, PCWSTR, PWSTR};

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

    pub fn navigate(&self, url: &str) -> windows_core::Result<()> {
        unsafe { self.wv.Navigate(&HSTRING::from(url)) }
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
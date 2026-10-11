use std::{cell::Cell, path::Path, rc::Rc};

use webview2_com::{
    CreateCoreWebView2ControllerCompletedHandler, CreateCoreWebView2EnvironmentCompletedHandler,
    DocumentTitleChangedEventHandler,
    Microsoft::Web::WebView2::Win32::{
        CreateCoreWebView2EnvironmentWithOptions, ICoreWebView2,
        ICoreWebView2Controller, ICoreWebView2Environment,
    },
    NewWindowRequestedEventHandler, WindowCloseRequestedEventHandler, take_pwstr,
};
use windows::Win32::Foundation::{E_POINTER, HWND, RECT};
use windows_core::{Error, HSTRING, PCWSTR, PWSTR};

use crate::gui::com::EventReg;

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

/// How to handle a NewWindowRequested event.
pub enum NewWindowPolicy {
    /// WebView2 default behavior (opens its own popup window)
    Default,
    /// Block the popup (window.open returns null)
    Block,
    /// Navigate the current view instead of opening a popup
    NavigateHere,
}

pub struct WebViewHost {
    event_regs: Vec<EventReg>,
    view: WebView,
}

type OnWebViewHostCreate = Box<dyn FnOnce(windows_core::Result<WebViewHost>)>;

#[derive(Clone)]
struct CompletionSlot(Rc<Cell<Option<OnWebViewHostCreate>>>);

impl CompletionSlot {
    fn new(on_create: OnWebViewHostCreate) -> Self {
        Self(Rc::new(Cell::new(Some(on_create))))
    }

    fn complete(&self, result: windows_core::Result<WebViewHost>) {
        if let Some(on_create) = self.0.take() {
            on_create(result);
        }
    }
}

impl WebViewHost {
    pub fn create(
        target: HWND,
        udf_path: &Path,
        on_create: impl FnOnce(windows_core::Result<WebViewHost>) + 'static,
    ) -> windows_core::Result<()> {
        let slot = CompletionSlot::new(Box::new(on_create));
        let udf_path = HSTRING::from(&*udf_path.to_string_lossy());

        let handler =
            CreateCoreWebView2EnvironmentCompletedHandler::create(Box::new(move |res, env| {
                let env = match res.and_then(|()| env.ok_or_else(|| Error::from(E_POINTER))) {
                    Ok(env) => env,
                    Err(e) => {
                        slot.complete(Err(e));
                        return Ok(());
                    }
                };
                if let Err(e) = create_controller(&env, target, slot.clone()) {
                    slot.complete(Err(e));
                }
                Ok(())
            }));
        unsafe {
            CreateCoreWebView2EnvironmentWithOptions(PCWSTR::null(), &udf_path, None, &handler)
        }
    }

    pub fn close(self) {
        let WebViewHost { event_regs, view } = self;
        drop(event_regs);
        let WebView { ctrl, wv } = view;
        drop(wv);
        if let Err(e) = unsafe { ctrl.Close() } {
            eprintln!("[webview] failed to close webview2 controller: {e}");
        }
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
        cb: impl Fn(&str) -> NewWindowPolicy + 'static,
    ) -> windows_core::Result<()> {
        let handler = NewWindowRequestedEventHandler::create(Box::new(move |sender, args| {
            let (Some(sender), Some(args)) = (sender, args) else {
                return Ok(());
            };
            let mut raw = PWSTR::null();
            unsafe { args.Uri(&mut raw)? };
            let uri = take_pwstr(raw);

            match cb(&uri) {
                NewWindowPolicy::Default => {}
                NewWindowPolicy::Block => unsafe { args.SetHandled(true)? },
                NewWindowPolicy::NavigateHere => unsafe {
                    args.SetHandled(true)?;
                    sender.Navigate(&HSTRING::from(uri.as_str()))?;
                },
            }
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

// ============================================================
// webview2 helpers
// ============================================================

fn create_controller(
    env: &ICoreWebView2Environment,
    target: HWND,
    slot: CompletionSlot,
) -> windows_core::Result<()> {
    let handler =
        CreateCoreWebView2ControllerCompletedHandler::create(Box::new(move |res, ctrl| {
            let webview_host = res
                .and_then(|()| ctrl.ok_or_else(|| Error::from(E_POINTER)))
                .and_then(WebViewHost::from_controller);
            slot.complete(webview_host);
            Ok(())
        }));
    unsafe { env.CreateCoreWebView2Controller(target, &handler) }
}
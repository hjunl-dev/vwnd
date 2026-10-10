use std::{cell::Cell, path::Path, rc::Rc};

use webview2_com::{
    CreateCoreWebView2ControllerCompletedHandler, CreateCoreWebView2EnvironmentCompletedHandler,
    DocumentTitleChangedEventHandler,
    Microsoft::Web::WebView2::Win32::{
        CreateCoreWebView2Environment, CreateCoreWebView2EnvironmentWithOptions, ICoreWebView2,
        ICoreWebView2Controller, ICoreWebView2Environment,
    },
    take_pwstr,
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

        let wv = self.view.wv.clone();
        self.event_regs.push(EventReg::new(move || {
            let _ = unsafe { wv.remove_DocumentTitleChanged(token) };
        }));
        Ok(())
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

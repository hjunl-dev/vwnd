use std::{cell::Cell, path::Path, rc::Rc};

use webview2_com::{CreateCoreWebView2EnvironmentCompletedHandler, Microsoft::Web::WebView2::Win32::{ICoreWebView2, ICoreWebView2Controller}};
use windows::Win32::Foundation::{HWND, RECT};
use windows_core::HSTRING;

use crate::gui::com::EventReg;

#[derive(Clone)]
pub struct WebView {
    controller: ICoreWebView2Controller,
    webview: ICoreWebView2,
}

impl WebView {
    pub fn set_bounds(&self, bounds: RECT) -> windows::core::Result<()> {
        unsafe { self.controller.SetBounds(bounds) }
    }

    pub fn set_visible(&self, visible: bool) -> windows::core::Result<()> {
        unsafe { self.controller.SetIsVisible(visible) }
    }

    pub fn navigate(&self, url: &str) -> windows::core::Result<()> {
        unsafe { self.webview.Navigate(&HSTRING::from(url)) }
    }
}

pub struct WebViewHost {
    event_regs: Vec<EventReg>,
    view: WebView,
}

type OnWebViewHostCreate = Box<dyn FnOnce(windows::core::Result<WebViewHost>)>;

#[derive(Clone)]
struct CompletionSlot(Rc<Cell<Option<OnWebViewHostCreate>>>);

impl CompletionSlot {
    fn new(on_create: OnWebViewHostCreate) -> Self {
        Self(Rc::new(Cell::new(Some(on_create))))
    }

    fn complete(&self, result: windows::core::Result<WebViewHost>) {
        if let Some(on_create) = self.0.take() {
            on_create(result);
        }
    }
}

impl WebViewHost {
    pub fn create(
        target: HWND,
        udf_path: &Path,
        on_create: impl FnOnce(windows::core::Result<WebViewHost>) + 'static,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    
    pub fn close(self) {

    }
}

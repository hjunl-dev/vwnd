use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
    rc::Rc,
};

use windows::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, WPARAM},
    UI::WindowsAndMessaging::{WM_DESTROY, WM_SIZE},
};
use windows_core::w;

use crate::gui::{
    com::ComApartment,
    mswin::{WindowClass, WindowHandler},
    webview::WebViewHost,
};

mod com;
mod mswin;
mod webview;

const DEFAULT_URL: &str = "https://www.google.com/";

pub struct App {
    hwnd: Cell<HWND>,
    host: RefCell<Option<WebViewHost>>,
}

impl App {
    pub fn run() -> windows::core::Result<()> {
        let _com = ComApartment::new_sta();
        let wnd_class = WindowClass::<App>::register(w!("vwnd_wnd_class"))?;
        let hwnd = wnd_class.create_wnd("vwnd_wnd", 1280, 720, Rc::new(App::new()))?;
        mswin::show(hwnd);
        let _exit_code = mswin::run_message_loop();
        Ok(())
    }

    fn new() -> Self {
        Self {
            hwnd: Cell::new(HWND::default()),
            host: RefCell::new(None),
        }
    }

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
        // navigate to default url
        view.navigate(DEFAULT_URL)?;

        *self.host.borrow_mut() = Some(host);
        Ok(())
    }

    fn resize_webview(&self) {
        let view = self.host.borrow().as_ref().map(WebViewHost::view);
        let Some(view) = view else {
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
        let weak = Rc::downgrade(self);

        WebViewHost::create(hwnd, &App::default_user_data_dir(), move |res| {
            let Some(this) = weak.upgrade() else {
                if let Ok(host) = res {
                    host.close();
                }
                return;
            };
            if let Err(e) = res.and_then(|host| this.attach_webview(host)) {
                eprintln!("[app] failed to setup webview {e}");
            }
        })
    }

    fn on_message(
        self: &std::rc::Rc<Self>,
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> Option<LRESULT> {
        match msg {
            WM_SIZE => {
                self.resize_webview();
                Some(LRESULT(0))
            }
            WM_DESTROY => {
                self.close_webview();
                mswin::post_quit(0);
                Some(LRESULT(0))
            }
            _ => None,
        }
    }
}

use std::{
    cell::{Cell, RefCell},
    mem::ManuallyDrop,
    os::raw::c_void,
    sync::OnceLock,
};

use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2, ICoreWebView2Controller, ICoreWebView2Environment,
};
use windows::Win32::Foundation::HWND;
use windows_core::Interface;

// WebView2 Environment

thread_local! {
    static WV2_ENV: Cell<*mut c_void> = const { Cell::new(std::ptr::null_mut()) };
}

pub fn set_wv2_env(env: ICoreWebView2Environment) {
    let old = WV2_ENV.replace(env.into_raw());
    debug_assert!(old.is_null(), "environment already set for this thread");
}

pub fn with_env<R>(f: impl FnOnce(&ICoreWebView2Environment) -> R) -> Option<R> {
    WV2_ENV.with(|c| {
        let p = c.get();
        unsafe { ICoreWebView2Environment::from_raw_borrowed(&p).map(f) }
    })
}

pub fn uninit_env() {
    let ptr = WV2_ENV.replace(std::ptr::null_mut());
    if !ptr.is_null() {
        drop(unsafe { ICoreWebView2Environment::from_raw(ptr) });
    }
}

pub enum HostPhase {
    Creating,
    EnvReady,
    CtrlReady,
    DomReady,
    Visible,
}

// C++ EventRegistrationToken

type Token = i64;

pub struct EventRegToken {
    token: Token,
    dtor: Option<Box<dyn FnOnce(Token)>>,
}

impl EventRegToken {
    pub fn new(token: Token, dtor: impl FnOnce(Token) + 'static) -> Self {
        Self {
            token,
            dtor: Some(Box::new(dtor)),
        }
    }
}

impl Drop for EventRegToken {
    fn drop(&mut self) {
        if let Some(dtor) = self.dtor.take() {
            dtor(self.token)
        }
    }
}

// WebView2 Host (ICoreWebView2Controller, ICoreWebView2)

pub struct Host {
    hwnd: HWND,
    phase: HostPhase,
    ctrl: Option<ICoreWebView2Controller>,
    webview: Option<ICoreWebView2>,
    event_tokens: Vec<EventRegToken>,
}

impl Host {
    pub fn new(hwnd: HWND) -> Self {
        Self {
            hwnd,
            phase: HostPhase::Creating,
            ctrl: None,
            webview: None,
            event_tokens: Vec::new(),
        }
    }
}

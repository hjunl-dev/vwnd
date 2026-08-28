use std::{
    cell::RefCell,
    ops::{Deref, DerefMut},
};

use webview2_com::Microsoft::Web::WebView2::Win32::{
    CreateCoreWebView2EnvironmentWithOptions, ICoreWebView2, ICoreWebView2Controller,
    ICoreWebView2CreateCoreWebView2ControllerCompletedHandler,
    ICoreWebView2CreateCoreWebView2EnvironmentCompletedHandler, ICoreWebView2Environment,
};
use windows::Win32::Foundation::HWND;
use windows_core::PCWSTR;

use crate::app::{handler::OnWv2EnvCreated, win::get_host_in_userdata};

// WebView2 Environment

thread_local! {
    static HOST: RefCell<Option<Host>> = const { RefCell::new(None) };
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
    pub hwnd: HWND,
    pub env: RefCell<Option<ICoreWebView2Environment>>,
    pub ctrl: RefCell<Option<ICoreWebView2Controller>>,
    pub webview: RefCell<Option<ICoreWebView2>>,
    pub phase: HostPhase,
    pub event_tokens: Vec<EventRegToken>,
}

impl Host {
    pub fn new(hwnd: HWND) -> Self {
        Self {
            hwnd,
            phase: HostPhase::Creating,
            env: RefCell::new(None),
            ctrl: RefCell::new(None),
            webview: RefCell::new(None),
            event_tokens: Vec::new(),
        }
    }
}

pub fn create(hwnd: HWND) -> windows_core::Result<()> {
    let handler: ICoreWebView2CreateCoreWebView2EnvironmentCompletedHandler =
        OnWv2EnvCreated(hwnd).into();
    unsafe {
        CreateCoreWebView2EnvironmentWithOptions(PCWSTR::null(), PCWSTR::null(), None, &handler)
    }
}
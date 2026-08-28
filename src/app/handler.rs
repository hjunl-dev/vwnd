use webview2_com::Microsoft::Web::WebView2::Win32::{
    CreateCoreWebView2EnvironmentWithOptions, ICoreWebView2, ICoreWebView2Controller,
    ICoreWebView2CreateCoreWebView2ControllerCompletedHandler,
    ICoreWebView2CreateCoreWebView2ControllerCompletedHandler_Impl,
    ICoreWebView2CreateCoreWebView2EnvironmentCompletedHandler,
    ICoreWebView2CreateCoreWebView2EnvironmentCompletedHandler_Impl, ICoreWebView2Environment,
};
use windows::Win32::{
    Foundation::{HWND, RECT},
    UI::WindowsAndMessaging::GetClientRect,
};
use windows_core::{implement, w};

use crate::app::{webview, win::get_host_in_userdata};

#[implement(ICoreWebView2CreateCoreWebView2EnvironmentCompletedHandler)]
pub struct OnWv2EnvCreated(pub HWND);

impl ICoreWebView2CreateCoreWebView2EnvironmentCompletedHandler_Impl for OnWv2EnvCreated_Impl {
    fn Invoke(
        &self,
        errorcode: windows_core::HRESULT,
        result: windows_core::Ref<ICoreWebView2Environment>,
    ) -> windows_core::Result<()> {
        errorcode.ok()?;
        let env = result.ok()?.clone();
        let handler: ICoreWebView2CreateCoreWebView2ControllerCompletedHandler =
            OnWv2CtrlCreated(self.0).into();
        unsafe {
            env.CreateCoreWebView2Controller(self.0, &handler)?;
        }

        if let Some(host) = get_host_in_userdata(self.0) {
            *host.env.borrow_mut() = Some(env);
        }
        Ok(())
    }
}

#[implement(ICoreWebView2CreateCoreWebView2ControllerCompletedHandler)]
pub struct OnWv2CtrlCreated(pub HWND);

impl ICoreWebView2CreateCoreWebView2ControllerCompletedHandler_Impl for OnWv2CtrlCreated_Impl {
    fn Invoke(
        &self,
        errorcode: windows_core::HRESULT,
        result: windows_core::Ref<ICoreWebView2Controller>,
    ) -> windows_core::Result<()> {
        errorcode.ok()?;
        let ctrl = result.ok()?.clone();
        let webview = unsafe { ctrl.CoreWebView2()? };

        unsafe {
            // set bounds to window size (ClientRect)
            let mut rect = RECT::default();
            GetClientRect(self.0, &mut rect)?;
            ctrl.SetBounds(rect)?;
            ctrl.SetIsVisible(true)?;
            webview.Navigate(w!("https://www.google.com/"))?;
        }

        if let Some(host) = get_host_in_userdata(self.0) {
            *host.ctrl.borrow_mut() = Some(ctrl);
            *host.webview.borrow_mut() = Some(webview);
        }

        Ok(())
    }
}

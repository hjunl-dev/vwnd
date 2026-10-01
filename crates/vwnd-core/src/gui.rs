mod com;
mod handler;
mod mswin;
mod webview;

use crate::gui::com::ComApartment;
use mswin::window;
use windows::core::Result;

pub fn run() -> Result<()> {
    // init dpi awareness
    window::init_dpi_awareness()?;

    // init COM apartment (STA)
    let _com_apt = ComApartment::new_sta();

    // Register window class
    let class_name = "vwnd_wnd_class";
    let wnd_name = "vwnd_wnd";
    window::register_class(class_name, Some(window::default_wnd_proc))?;

    // Create wnd
    let hwnd = window::create(class_name, wnd_name, 0, 0, 800, 600, None, true)?;

    // set webview
    webview::create(hwnd)?;

    // run message pump
    window::run_message_loop();

    Ok(())
}

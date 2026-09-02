mod com;
mod handler;
mod mswin;
mod webview;
mod win;

use crate::app::com::ComApartment;
use windows::core::Result;

pub fn run() -> Result<()> {
    // init dpi awareness
    win::init_dpi_awareness()?;

    // init COM apartment (STA)
    let _com_apt = ComApartment::new_sta();

    // Register window class
    win::register_wnd_class()?;

    // Create wnd
    let hwnd = win::create_wnd(true)?;

    // set webview
    webview::create(hwnd)?;

    // run message pump
    win::pump();

    Ok(())
}

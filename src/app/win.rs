use std::rc::Rc;

use windows::Win32::{
    Foundation::HWND,
    UI::WindowsAndMessaging::{GWLP_USERDATA, GetWindowLongPtrW},
};

use crate::app::webview::Host;

mod userdata {
    use windows::Win32::{
        Foundation::HWND,
        UI::WindowsAndMessaging::{GWLP_USERDATA, GetWindowLongPtrW, SetWindowLongPtrW},
    };

    pub unsafe fn get(hwnd: HWND) -> isize {
        unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) }
    }

    pub unsafe fn set(hwnd: HWND, ud: isize) {
        unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, ud) };
    }
}

pub fn get_host_in_userdata(hwnd: HWND) -> Option<Rc<Host>> {
    let ptr = unsafe { userdata::get(hwnd) } as *const Host;
    if ptr.is_null() {
        return None;
    }
    unsafe {
        Rc::increment_strong_count(ptr);
        Some(Rc::from_raw(ptr))
    }
}

pub fn set_host_in_userdata(host: Rc<Host>) {
    let host_clone = host.clone();
    unsafe { userdata::set(host.hwnd, Rc::into_raw(host_clone) as isize) };
}

pub fn clear_host_in_userdata(hwnd: HWND) {
    let ptr = unsafe { userdata::get(hwnd) } as *const Host;
    unsafe { userdata::set(hwnd, 0) }; // 먼저 끊는다

    if !ptr.is_null() {
        drop(unsafe { Rc::from_raw(ptr) }); // into_raw로 심어둔 count 회수
    }
}
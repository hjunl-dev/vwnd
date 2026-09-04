use windows::Win32::{
    Foundation::{HWND, LPARAM},
    UI::WindowsAndMessaging::{CREATESTRUCTW, GWLP_USERDATA, GetWindowLongPtrW, SetWindowLongPtrW},
};

// ============================================================
// GWLP_USERDATA storage
// ============================================================

pub unsafe fn attach_userdata(hwnd: HWND, lp: LPARAM) {
    let cs = unsafe { &*(lp.0 as *const CREATESTRUCTW) };
    if !cs.lpCreateParams.is_null() {
        unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize) };
    }
}

pub unsafe fn user_data<T>(hwnd: HWND) -> Option<&'static T> {
    let raw = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const T;
    if raw.is_null() {
        None
    } else {
        Some(unsafe { &*raw })
    }
}

pub unsafe fn detach_userdata<T>(hwnd: HWND) {
    let raw = unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) } as *mut T;
    if !raw.is_null() {
        drop(unsafe { Box::from_raw(raw) });
    }
}

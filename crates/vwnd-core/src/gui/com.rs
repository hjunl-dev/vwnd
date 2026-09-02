use std::marker::PhantomData;

use windows::Win32::Foundation::S_FALSE;
use windows::Win32::System::Com::{
    COINIT, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, COINIT_MULTITHREADED, CoInitializeEx,
    CoUninitialize,
};
use windows::core::Result;

// PhantomData for ComApartment
// Configure ComApartment to have the !Send + !Sync
type NotSendSync = PhantomData<*const ()>;

#[must_use]
#[derive(Debug)]
pub struct ComApartment {
    already_init: bool,
    _marker: NotSendSync,
}

impl ComApartment {
    fn new(mode: COINIT) -> Result<Self> {
        let hr = unsafe { CoInitializeEx(None, mode) };
        hr.ok()?;
        Ok(Self {
            already_init: hr == S_FALSE,
            _marker: PhantomData,
        })
    }

    pub fn new_sta() -> Result<Self> {
        Self::new(COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE)
    }

    pub fn new_mta() -> Result<Self> {
        Self::new(COINIT_MULTITHREADED | COINIT_DISABLE_OLE1DDE)
    }

    pub fn was_already_init(&self) -> bool {
        self.already_init
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        unsafe {
            CoUninitialize();
        }
    }
}

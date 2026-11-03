//! EXE-resident metadata wake. It owns only an event and never a DLL callback,
//! editor, text, context or target. A burst occupies one manual-reset event.
#![allow(non_snake_case)]
use std::cell::Cell;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::thread::{self, ThreadId};
use sumibi_tip::spike::{ISpikeInputSignal, ISpikeInputSignal_Impl};
use windows::core::{implement, ComObject, Result, HRESULT};
use windows::Win32::Foundation::{HANDLE, RPC_E_WRONG_THREAD, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::System::Threading::{CreateEventW, ResetEvent, SetEvent, WaitForSingleObject};

#[implement(ISpikeInputSignal)]
pub(crate) struct Signal {
    event: OwnedHandle,
    active: Cell<bool>,
    owner: ThreadId,
}
impl Signal {
    pub(crate) fn new() -> Result<ComObject<Self>> {
        let event = unsafe { CreateEventW(None, true, false, None)? };
        Ok(ComObject::new(Self {
            event: unsafe { OwnedHandle::from_raw_handle(event.0) },
            active: Cell::new(true),
            owner: thread::current().id(),
        }))
    }
    fn check(&self) -> Result<()> {
        if self.owner != thread::current().id() {
            return Err(RPC_E_WRONG_THREAD.into());
        }
        Ok(())
    }
    pub(crate) fn handle(&self) -> Option<HANDLE> {
        self.active
            .get()
            .then(|| HANDLE(self.event.as_raw_handle()))
    }
    pub(crate) fn notify(&self) -> Result<()> {
        self.check()?;
        if let Some(handle) = self.handle() {
            unsafe {
                SetEvent(handle)?;
            }
        }
        Ok(())
    }
    pub(crate) fn drain(&self) -> Result<bool> {
        self.check()?;
        let Some(handle) = self.handle() else {
            return Ok(false);
        };
        match unsafe { WaitForSingleObject(handle, 0) } {
            WAIT_OBJECT_0 => {
                unsafe {
                    ResetEvent(handle)?;
                }
                Ok(true)
            }
            WAIT_TIMEOUT => Ok(false),
            _ => Err(windows::core::Error::from_thread()),
        }
    }
    pub(crate) fn close(&self) -> Result<()> {
        self.check()?;
        self.active.set(false);
        unsafe { ResetEvent(HANDLE(self.event.as_raw_handle())) }
    }
}
impl ISpikeInputSignal_Impl for Signal_Impl {
    unsafe fn Changed(&self) -> HRESULT {
        self.notify().into()
    }
}

//! Independent, finite diagnostic input grant. Flat owner-STA ABI, no worker
//! callbacks or Rust allocations cross the DLL boundary.
#![allow(non_snake_case)]
use crate::com::{boundary, ObjectLease};
use crate::edit::input_composition::Tracker;
use crate::spike::Controller;
use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::rc::{Rc, Weak};
use std::thread::{self, ThreadId};
use windows::core::{
    implement, interface, IUnknown, IUnknown_Vtbl, Interface, Result, GUID, HRESULT,
};
use windows::Win32::Foundation::{E_ABORT, E_INVALIDARG, RPC_E_WRONG_THREAD};
use windows::Win32::UI::TextServices::*;

pub const FUNCTION_TEST_INPUT: GUID = GUID::from_u128(0x15f8b7b7_6ba9_49cc_b393_2c4d52389a72);
#[interface("3c2039ce-f57c-46e4-969c-a1d925dbe181")]
pub unsafe trait ISpikeInput: IUnknown {
    pub fn BeforeInput(&self, context: *mut c_void, backspace: u32) -> HRESULT;
    pub fn AfterInput(&self, text: *const u16, length: u32) -> HRESULT;
    pub fn PrepareConversion(&self) -> HRESULT;
    pub fn CheckSource(&self, text: *const u16, length: u32) -> HRESULT;
    pub fn Cancel(&self, user_boundary: u32) -> HRESULT;
}
/// Separate diagnostic vtables; never extend the existing input ABI.
#[interface("a384ee66-e5cb-4fdc-a34b-0f1528710d21")]
pub unsafe trait ISpikeInputSignal: IUnknown {
    pub fn Changed(&self) -> HRESULT;
}
#[interface("df652427-d618-466f-8221-57ee4189277a")]
pub unsafe trait ISpikeInputMonitor: IUnknown {
    pub fn SetSignal(&self, signal: *mut c_void) -> HRESULT;
    pub fn Poll(&self) -> HRESULT;
}
#[implement(ISpikeInput, ISpikeInputMonitor)]
struct Control {
    state: Weak<Controller>,
    owner: ThreadId,
    busy: Cell<bool>,
    tracker: RefCell<Option<Tracker>>,
    wake: RefCell<Option<Rc<dyn Fn()>>>,
    _lease: ObjectLease,
}
struct Busy<'a>(&'a Cell<bool>);
impl Drop for Busy<'_> {
    fn drop(&mut self) {
        self.0.set(false);
    }
}
impl Control_Impl {
    fn enter(&self) -> Result<Busy<'_>> {
        if thread::current().id() != self.owner {
            return Err(RPC_E_WRONG_THREAD.into());
        }
        if self.busy.replace(true) {
            return Err(E_ABORT.into());
        }
        Ok(Busy(&self.busy))
    }
    fn change(&self, action: impl FnOnce(&mut Tracker) -> Result<()>) -> Result<()> {
        let _busy = self.enter()?;
        self.state.upgrade().ok_or(E_ABORT)?.check_thread()?;
        let mut tracker = self.tracker.borrow_mut().take().ok_or(E_ABORT)?;
        let result = action(&mut tracker);
        if result.is_err() {
            let _ = tracker.cancel(false);
        }
        *self.tracker.borrow_mut() = Some(tracker);
        result
    }
}
impl ISpikeInput_Impl for Control_Impl {
    unsafe fn BeforeInput(&self, context: *mut c_void, backspace: u32) -> HRESULT {
        boundary(|| {
            let _busy = self.enter()?;
            let state = self.state.upgrade().ok_or(E_ABORT)?;
            let (manager, client) = state.input_owner()?;
            if context.is_null() || backspace > 1 {
                return Err(E_INVALIDARG.into());
            }
            let context = unsafe { ITfContext::from_raw_borrowed(&context) }.ok_or(E_INVALIDARG)?;
            let mut tracker = self.tracker.borrow_mut().take();
            let result = (|| {
                let incoming = context.cast::<IUnknown>()?;
                if tracker
                    .as_ref()
                    .map(|old| old.context.cast::<IUnknown>())
                    .transpose()?
                    .is_some_and(|old| old != incoming)
                {
                    tracker.as_mut().ok_or(E_ABORT)?.cancel(false)?;
                    tracker = None;
                }
                if tracker.is_none() {
                    tracker = Some(Tracker::new(&manager, context, client)?);
                    tracker
                        .as_ref()
                        .ok_or(E_ABORT)?
                        .set_wake(self.wake.borrow().clone())?;
                }
                tracker.as_mut().ok_or(E_ABORT)?.before(backspace != 0)
            })();
            if result.is_err() {
                if let Some(tracker) = &mut tracker {
                    let _ = tracker.cancel(false);
                }
            }
            *self.tracker.borrow_mut() = tracker;
            result
        })
        .into()
    }
    unsafe fn AfterInput(&self, text: *const u16, length: u32) -> HRESULT {
        boundary(|| {
            self.change(|tracker| {
                if length > 1024 || (length != 0 && text.is_null()) {
                    return Err(E_INVALIDARG.into());
                }
                let units = if length == 0 {
                    &[]
                } else {
                    unsafe { std::slice::from_raw_parts(text, length as usize) }
                };
                tracker.after(units)
            })
        })
        .into()
    }
    unsafe fn PrepareConversion(&self) -> HRESULT {
        boundary(|| self.change(Tracker::prepare)).into()
    }
    unsafe fn CheckSource(&self, text: *const u16, length: u32) -> HRESULT {
        boundary(|| {
            self.change(|tracker| {
                if text.is_null() || !(1..=1024).contains(&length) {
                    return Err(E_INVALIDARG.into());
                }
                let units = unsafe { std::slice::from_raw_parts(text, length as usize) };
                tracker.source(units)
            })
        })
        .into()
    }
    unsafe fn Cancel(&self, user_boundary: u32) -> HRESULT {
        boundary(|| {
            let _busy = self.enter()?;
            if user_boundary > 1 {
                return Err(E_INVALIDARG.into());
            }
            let mut tracker = self.tracker.borrow_mut().take();
            let result = tracker
                .as_mut()
                .map_or(Ok(()), |tracker| tracker.cancel(user_boundary != 0));
            *self.tracker.borrow_mut() = tracker;
            result
        })
        .into()
    }
}
impl ISpikeInputMonitor_Impl for Control_Impl {
    unsafe fn SetSignal(&self, signal: *mut c_void) -> HRESULT {
        boundary(|| {
            let _busy = self.enter()?;
            let wake: Option<Rc<dyn Fn()>> = if signal.is_null() {
                None
            } else {
                let unknown =
                    unsafe { IUnknown::from_raw_borrowed(&signal) }.ok_or(E_INVALIDARG)?;
                let signal: ISpikeInputSignal = unknown.cast()?;
                Some(Rc::new(move || unsafe {
                    let _ = signal.Changed();
                }))
            };
            let old = self.wake.replace(wake.clone());
            let tracker = self.tracker.borrow_mut().take();
            let result = tracker
                .as_ref()
                .map_or(Ok(()), |tracker| tracker.set_wake(wake.clone()));
            *self.tracker.borrow_mut() = tracker;
            // Release an old external sink only after restoring local state.
            drop(old);
            // Reattachment must check changes missed while detached, even if
            // no later notification arrives. Only the EXE event is signalled.
            if let Some(wake) = wake {
                wake();
            }
            result
        })
        .into()
    }
    unsafe fn Poll(&self) -> HRESULT {
        boundary(|| {
            let _busy = self.enter()?;
            let mut tracker = self.tracker.borrow_mut().take();
            let retired = self.state.upgrade().is_none();
            let result = tracker
                .as_mut()
                .map_or(Ok(()), |tracker| tracker.poll(retired));
            *self.tracker.borrow_mut() = tracker;
            result
        })
        .into()
    }
}
pub(crate) fn control(state: &Rc<Controller>) -> IUnknown {
    let control: ISpikeInput = Control {
        state: Rc::downgrade(state),
        owner: thread::current().id(),
        busy: Cell::new(false),
        tracker: RefCell::new(None),
        wake: RefCell::new(None),
        _lease: ObjectLease::new(),
    }
    .into();
    unsafe { IUnknown::from_raw(control.into_raw()) }
}

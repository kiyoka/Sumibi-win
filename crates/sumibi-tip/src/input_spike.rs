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
pub const INPUT_NOT_ATTEMPTED: u32 = 0;
pub const INPUT_APPLIED: u32 = 1;
pub const INPUT_APPLIED_UNTRACKED: u32 = 2;
pub const INPUT_UNCERTAIN: u32 = 3;
/// Synchronous insertion only. HRESULT never authorizes host fallback: only
/// INPUT_NOT_ATTEMPTED does. No Rust allocation crosses this independent ABI.
#[interface("8ceca767-ac74-49ea-9408-8e508578acda")]
pub unsafe trait ISpikeInputWriter: IUnknown {
    pub fn Insert(
        &self,
        context: *mut c_void,
        text: *const u16,
        length: u32,
        disposition: *mut u32,
    ) -> HRESULT;
}
/// Independent ABI: the original writer's vtable remains unchanged.
#[interface("80cd3b94-4b51-4a31-aaec-752b385fb120")]
pub unsafe trait ISpikeInputEraser: IUnknown {
    pub fn Backspace(&self, context: *mut c_void, count: u32, disposition: *mut u32) -> HRESULT;
}
/// Explicit nonempty selection only; never reads text to infer a forward range.
#[interface("6ac5e8a1-c126-4ec2-aad2-cea21324be31")]
pub unsafe trait ISpikeSelectionEraser: IUnknown {
    pub fn DeleteSelection(&self, context: *mut c_void, disposition: *mut u32) -> HRESULT;
}
/// Bind an explicit context before preparing a selected or recorded source.
/// This initializes metadata only; it never infers text from a collapsed caret.
#[interface("ec1aa9bd-14ef-4dcb-918e-ded188760c22")]
pub unsafe trait ISpikeInputPreparer: IUnknown {
    pub fn Prepare(&self, context: *mut c_void) -> HRESULT;
}
/// Actions 0/1 move left/right; 2 deletes forward inside recorded input only.
#[interface("e878ca43-d149-4864-9047-59ae210da34a")]
pub unsafe trait ISpikeTrackedEditor: IUnknown {
    pub fn Edit(
        &self,
        context: *mut c_void,
        action: u32,
        count: u32,
        disposition: *mut u32,
    ) -> HRESULT;
}
#[implement(
    ISpikeInput,
    ISpikeInputMonitor,
    ISpikeInputWriter,
    ISpikeInputEraser,
    ISpikeSelectionEraser,
    ISpikeInputPreparer,
    ISpikeTrackedEditor
)]
struct Control {
    state: Weak<Controller>,
    owner: ThreadId,
    busy: Cell<bool>,
    write_blocked: Cell<bool>,
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
    fn in_context(
        &self,
        context: *mut c_void,
        action: impl FnOnce(&mut Tracker) -> Result<()>,
    ) -> Result<()> {
        let _busy = self.enter()?;
        if self.write_blocked.get() {
            return Err(E_ABORT.into());
        }
        let state = self.state.upgrade().ok_or(E_ABORT)?;
        let (manager, client) = state.input_owner()?;
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
            action(tracker.as_mut().ok_or(E_ABORT)?)
        })();
        if result.is_err() {
            if let Some(tracker) = &mut tracker {
                let _ = tracker.cancel(false);
            }
        }
        *self.tracker.borrow_mut() = tracker;
        result
    }
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
        if self.write_blocked.get() {
            return Err(E_ABORT.into());
        }
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
            self.in_context(context, |tracker| {
                if backspace > 1 {
                    return Err(E_INVALIDARG.into());
                }
                tracker.before(backspace != 0)
            })
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
            if user_boundary == 1 {
                self.write_blocked.set(false);
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
impl ISpikeInputWriter_Impl for Control_Impl {
    unsafe fn Insert(
        &self,
        context: *mut c_void,
        text: *const u16,
        length: u32,
        disposition: *mut u32,
    ) -> HRESULT {
        // Keep this cell outside boundary(), so a caught panic cannot turn an
        // attempted body write into permission to replay the host input.
        let outcome = Rc::new(Cell::new(INPUT_NOT_ATTEMPTED));
        let result = boundary(|| {
            if disposition.is_null() {
                return Err(E_INVALIDARG.into());
            }
            self.in_context(context, |tracker| {
                if text.is_null() || !(1..=1024).contains(&length) {
                    return Err(E_INVALIDARG.into());
                }
                let units = unsafe { std::slice::from_raw_parts(text, length as usize) };
                tracker.insert(units, outcome.clone())
            })
        });
        if matches!(outcome.get(), INPUT_UNCERTAIN | INPUT_APPLIED_UNTRACKED) {
            // These states can only be produced after enter() on the owner.
            self.write_blocked.set(true);
        }
        if !disposition.is_null() {
            unsafe {
                disposition.write(outcome.get());
            }
        }
        result.into()
    }
}
impl ISpikeInputEraser_Impl for Control_Impl {
    unsafe fn Backspace(&self, context: *mut c_void, count: u32, disposition: *mut u32) -> HRESULT {
        let outcome = Rc::new(Cell::new(INPUT_NOT_ATTEMPTED));
        let result = boundary(|| {
            if disposition.is_null() {
                return Err(E_INVALIDARG.into());
            }
            self.in_context(context, |tracker| tracker.backspace(count, outcome.clone()))
        });
        if matches!(outcome.get(), INPUT_UNCERTAIN | INPUT_APPLIED_UNTRACKED) {
            self.write_blocked.set(true);
        }
        if !disposition.is_null() {
            unsafe {
                disposition.write(outcome.get());
            }
        }
        result.into()
    }
}
impl ISpikeSelectionEraser_Impl for Control_Impl {
    unsafe fn DeleteSelection(&self, context: *mut c_void, disposition: *mut u32) -> HRESULT {
        let outcome = Rc::new(Cell::new(INPUT_NOT_ATTEMPTED));
        let result = boundary(|| {
            if disposition.is_null() {
                return Err(E_INVALIDARG.into());
            }
            self.in_context(context, |tracker| tracker.delete_selection(outcome.clone()))
        });
        if matches!(outcome.get(), INPUT_UNCERTAIN | INPUT_APPLIED_UNTRACKED) {
            self.write_blocked.set(true);
        }
        if !disposition.is_null() {
            unsafe { disposition.write(outcome.get()) };
        }
        result.into()
    }
}
impl ISpikeTrackedEditor_Impl for Control_Impl {
    unsafe fn Edit(
        &self,
        context: *mut c_void,
        action: u32,
        count: u32,
        disposition: *mut u32,
    ) -> HRESULT {
        let outcome = Rc::new(Cell::new(INPUT_NOT_ATTEMPTED));
        let result = boundary(|| {
            if disposition.is_null() || action > 2 || !(1..=1024).contains(&count) {
                return Err(E_INVALIDARG.into());
            }
            self.in_context(context, |tracker| match action {
                0 | 1 => tracker.move_caret(action == 0, count, outcome.clone()),
                _ => tracker.forward_delete(count, outcome.clone()),
            })
        });
        if matches!(outcome.get(), INPUT_UNCERTAIN | INPUT_APPLIED_UNTRACKED) {
            self.write_blocked.set(true);
        }
        if !disposition.is_null() {
            unsafe { disposition.write(outcome.get()) };
        }
        result.into()
    }
}
impl ISpikeInputPreparer_Impl for Control_Impl {
    unsafe fn Prepare(&self, context: *mut c_void) -> HRESULT {
        boundary(|| self.in_context(context, Tracker::prepare)).into()
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
        write_blocked: Cell::new(false),
        tracker: RefCell::new(None),
        wake: RefCell::new(None),
        _lease: ObjectLease::new(),
    }
    .into();
    unsafe { IUnknown::from_raw(control.into_raw()) }
}

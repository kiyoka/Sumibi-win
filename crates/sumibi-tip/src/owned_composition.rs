//! Metadata-only ownership of one composition created by this service. Never
//! adopt a foreign view, or bypass the ordinary idle gate for body reads.
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::thread::{self, ThreadId};
use windows::core::{IUnknown, Interface, Result};
use windows::Win32::Foundation::{E_ABORT, RPC_E_WRONG_THREAD, S_FALSE, S_OK};
use windows::Win32::UI::TextServices::*;

/// Explicit development host service, never initialized by the DLL.
pub const GUID_SPIKE_COMPOSITION_SINK: windows::core::GUID =
    windows::core::GUID::from_u128(0x1aa12c58_6cbb_44c8_8aac_9a868083df09);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    New,
    Live,
    Ended,
    Terminated,
}
pub(crate) struct OwnedComposition {
    phase: Cell<Phase>,
    value: RefCell<Option<ITfComposition>>,
    owner: ThreadId,
}
impl OwnedComposition {
    pub(crate) fn new() -> Rc<Self> {
        Rc::new(Self {
            phase: Cell::new(Phase::New),
            value: RefCell::new(None),
            owner: thread::current().id(),
        })
    }
    fn check_thread(&self) -> Result<()> {
        if self.owner != thread::current().id() {
            return Err(RPC_E_WRONG_THREAD.into());
        }
        Ok(())
    }
    pub(crate) fn start(&self, context: &ITfContext, range: &ITfRange, cookie: u32) -> Result<()> {
        self.check_thread()?;
        if self.phase.get() != Phase::New {
            return Err(E_ABORT.into());
        }
        crate::composition::require_idle(context)?;
        let service: ITfContextComposition = context.cast()?;
        let compartments: ITfCompartmentMgr = context.cast()?;
        // Only the owned diagnostic host supplies this EXE-resident sink. It
        // retains no DLL reference, target, cookie, or callback into this DLL.
        let value = unsafe {
            compartments
                .GetCompartment(&GUID_SPIKE_COMPOSITION_SINK)?
                .GetValue()?
        };
        let sink: ITfCompositionSink = IUnknown::try_from(&value)?.cast()?;
        crate::composition::require_idle(context)?;
        let mut raw = std::ptr::null_mut();
        // SAFETY: validated same-context range, current write cookie and owned
        // output. This Windows rejected a null sink and retained a nonnull sink
        // after S_OK/null owner rejection. Use the host's shared EXE sink.
        let status = unsafe {
            (service.vtable().StartComposition)(
                service.as_raw(),
                cookie,
                range.as_raw(),
                sink.as_raw(),
                &mut raw,
            )
        };
        if !raw.is_null() {
            *self.value.borrow_mut() = Some(unsafe { ITfComposition::from_raw(raw) });
            self.phase.set(Phase::Live);
        }
        if status != S_OK || !self.can_commit() {
            return Err(E_ABORT.into());
        }
        if !self.present(context)? {
            self.phase.set(Phase::Terminated);
            return Err(E_ABORT.into());
        }
        Ok(())
    }
    pub(crate) fn can_commit(&self) -> bool {
        self.phase.get() == Phase::Live
    }
    /// Metadata-only exception: exactly one view and its COM identity must be
    /// ours. Never inspect owner/range/body on a foreign view.
    pub(crate) fn require_exclusive(&self, context: &ITfContext) -> Result<()> {
        self.check_thread()?;
        if !self.can_commit() {
            return Err(E_ABORT.into());
        }
        let value = self.value.borrow().clone().ok_or(E_ABORT)?;
        let expected = value.cast::<ITfCompositionView>()?.cast::<IUnknown>()?;
        let service: ITfContextComposition = context.cast()?;
        let mut raw = std::ptr::null_mut();
        let status = unsafe { (service.vtable().EnumCompositions)(service.as_raw(), &mut raw) };
        let enumerator = if raw.is_null() {
            None
        } else {
            Some(unsafe { IEnumITfCompositionView::from_raw(raw) })
        };
        if status != S_OK {
            return Err(E_ABORT.into());
        }
        let enumerator = enumerator.ok_or(E_ABORT)?;
        for index in 0..2 {
            let mut view: Option<ITfCompositionView> = None;
            let mut count = 0;
            let status = unsafe {
                (enumerator.vtable().Next)(
                    enumerator.as_raw(),
                    1,
                    (&mut view as *mut Option<ITfCompositionView>).cast(),
                    &mut count,
                )
            };
            if index == 0 {
                if status != S_OK
                    || count != 1
                    || view.ok_or(E_ABORT)?.cast::<IUnknown>()? != expected
                {
                    return Err(E_ABORT.into());
                }
            } else if status != S_FALSE || count != 0 || view.is_some() {
                return Err(E_ABORT.into());
            }
        }
        Ok(())
    }
    pub(crate) fn range(&self, context: &ITfContext) -> Result<ITfRange> {
        self.require_exclusive(context)?;
        let value = self.value.borrow().clone().ok_or(E_ABORT)?;
        unsafe { value.GetRange() }
    }
    pub(crate) fn shift_end(
        &self,
        context: &ITfContext,
        cookie: u32,
        range: &ITfRange,
    ) -> Result<()> {
        self.require_exclusive(context)?;
        if unsafe { range.GetContext()? }.cast::<IUnknown>()? != context.cast::<IUnknown>()? {
            return Err(E_ABORT.into());
        }
        let value = self.value.borrow().clone().ok_or(E_ABORT)?;
        let status = unsafe { (value.vtable().ShiftEnd)(value.as_raw(), cookie, range.as_raw()) };
        if status != S_OK {
            return Err(E_ABORT.into());
        }
        self.require_exclusive(context)
    }

    /// Match COM identity, never text. Absence requires the exact enumeration
    /// end; failure or capacity exhaustion must not masquerade as termination.
    fn present(&self, context: &ITfContext) -> Result<bool> {
        let value = self.value.borrow().clone().ok_or(E_ABORT)?;
        let expected = value.cast::<ITfCompositionView>()?.cast::<IUnknown>()?;
        let service: ITfContextComposition = context.cast()?;
        let mut raw = std::ptr::null_mut();
        // SAFETY: current owner-thread context, initialized owned output slot.
        let status = unsafe { (service.vtable().EnumCompositions)(service.as_raw(), &mut raw) };
        let enumerator = if raw.is_null() {
            None
        } else {
            Some(unsafe { IEnumITfCompositionView::from_raw(raw) })
        };
        if status != S_OK {
            return Err(E_ABORT.into());
        }
        let enumerator = enumerator.ok_or(E_ABORT)?;
        for _ in 0..16 {
            let mut view: Option<ITfCompositionView> = None;
            let mut count = 0;
            // SAFETY: exactly one initialized output, released on every branch.
            let status = unsafe {
                (enumerator.vtable().Next)(
                    enumerator.as_raw(),
                    1,
                    (&mut view as *mut Option<ITfCompositionView>).cast(),
                    &mut count,
                )
            };
            if status == S_FALSE && count == 0 && view.is_none() {
                return Ok(false);
            }
            if status != S_OK || count != 1 {
                return Err(E_ABORT.into());
            }
            if view.ok_or(E_ABORT)?.cast::<IUnknown>()? == expected {
                return Ok(true);
            }
        }
        Err(E_ABORT.into())
    }
    pub(crate) fn end(&self, context: &ITfContext, cookie: u32) -> Result<()> {
        self.check_thread()?;
        if self.phase.get() != Phase::Live {
            return Ok(());
        }
        if !self.present(context)? {
            self.phase.set(Phase::Terminated);
            self.value.borrow_mut().take();
            return Ok(());
        }
        let value = self.value.borrow().clone().ok_or(E_ABORT)?;
        // SAFETY: exact object created by this client, current write cookie;
        // no borrowed RefCell across external code. Failure retains cleanup.
        unsafe {
            value.EndComposition(cookie)?;
        }
        self.value.borrow_mut().take();
        self.phase.set(Phase::Ended);
        Ok(())
    }
    pub(crate) fn was_terminated(&self) -> bool {
        self.phase.get() == Phase::Terminated
    }
}

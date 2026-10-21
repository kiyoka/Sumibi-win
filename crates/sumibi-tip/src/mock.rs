//! Failure/reentrancy tests without installing a system text service.
#![allow(non_snake_case)]

use std::cell::{Cell, RefCell};
use windows::core::{implement, ComObject, Ref, Result, BOOL, BSTR, GUID, PCWSTR};
use windows::Win32::Foundation::{E_FAIL, E_NOTIMPL, E_UNEXPECTED, HWND, LPARAM, WPARAM};
use windows::Win32::UI::TextServices::*;

#[implement(ITfThreadMgr, ITfKeystrokeMgr)]
pub(super) struct Manager {
    sink: RefCell<Option<ITfKeyEventSink>>,
    fail_advise: Cell<bool>,
    fail_unadvise: Cell<bool>,
    reenter: RefCell<Option<ITfTextInputProcessorEx>>,
    advised: Cell<u32>,
}

impl ITfKeystrokeMgr_Impl for Manager_Impl {
    fn AdviseKeyEventSink(
        &self,
        client: u32,
        sink: Ref<ITfKeyEventSink>,
        foreground: BOOL,
    ) -> Result<()> {
        assert_eq!(client, 7);
        assert!(foreground.as_bool());
        if let Some(tip) = self.reenter.borrow().as_ref() {
            // SAFETY: same-thread live COM reference. The callback deliberately
            // reenters while the outer Activate is subscribing its key sink.
            assert_eq!(
                unsafe { tip.Deactivate() }.unwrap_err().code(),
                E_UNEXPECTED
            );
        }
        if self.fail_advise.get() {
            return Err(E_FAIL.into());
        }
        assert!(self.sink.borrow().is_none());
        *self.sink.borrow_mut() = Some(sink.ok()?.clone());
        self.advised.set(client);
        Ok(())
    }

    fn UnadviseKeyEventSink(&self, client: u32) -> Result<()> {
        assert_eq!(client, self.advised.get());
        if self.fail_unadvise.get() {
            return Err(E_FAIL.into());
        }
        assert!(self.sink.borrow_mut().take().is_some());
        Ok(())
    }

    fn GetForeground(&self) -> Result<GUID> {
        Err(E_NOTIMPL.into())
    }
    fn TestKeyDown(&self, _: WPARAM, _: LPARAM) -> Result<BOOL> {
        Err(E_NOTIMPL.into())
    }
    fn TestKeyUp(&self, _: WPARAM, _: LPARAM) -> Result<BOOL> {
        Err(E_NOTIMPL.into())
    }
    fn KeyDown(&self, _: WPARAM, _: LPARAM) -> Result<BOOL> {
        Err(E_NOTIMPL.into())
    }
    fn KeyUp(&self, _: WPARAM, _: LPARAM) -> Result<BOOL> {
        Err(E_NOTIMPL.into())
    }
    fn GetPreservedKey(&self, _: Ref<ITfContext>, _: *const TF_PRESERVEDKEY) -> Result<GUID> {
        Err(E_NOTIMPL.into())
    }
    fn IsPreservedKey(&self, _: *const GUID, _: *const TF_PRESERVEDKEY) -> Result<BOOL> {
        Err(E_NOTIMPL.into())
    }
    fn PreserveKey(
        &self,
        _: u32,
        _: *const GUID,
        _: *const TF_PRESERVEDKEY,
        _: &PCWSTR,
        _: u32,
    ) -> Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn UnpreserveKey(&self, _: *const GUID, _: *const TF_PRESERVEDKEY) -> Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn SetPreservedKeyDescription(&self, _: *const GUID, _: &PCWSTR, _: u32) -> Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn GetPreservedKeyDescription(&self, _: *const GUID) -> Result<BSTR> {
        Err(E_NOTIMPL.into())
    }
    fn SimulatePreservedKey(&self, _: Ref<ITfContext>, _: *const GUID) -> Result<BOOL> {
        Err(E_NOTIMPL.into())
    }
}

impl ITfThreadMgr_Impl for Manager_Impl {
    fn Activate(&self) -> Result<u32> {
        Err(E_NOTIMPL.into())
    }
    fn Deactivate(&self) -> Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn CreateDocumentMgr(&self) -> Result<ITfDocumentMgr> {
        Err(E_NOTIMPL.into())
    }
    fn EnumDocumentMgrs(&self) -> Result<IEnumTfDocumentMgrs> {
        Err(E_NOTIMPL.into())
    }
    fn GetFocus(&self) -> Result<ITfDocumentMgr> {
        Err(E_NOTIMPL.into())
    }
    fn SetFocus(&self, _: Ref<ITfDocumentMgr>) -> Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn AssociateFocus(&self, _: HWND, _: Ref<ITfDocumentMgr>) -> Result<ITfDocumentMgr> {
        Err(E_NOTIMPL.into())
    }
    fn IsThreadFocus(&self) -> Result<BOOL> {
        Err(E_NOTIMPL.into())
    }
    fn GetFunctionProvider(&self, _: *const GUID) -> Result<ITfFunctionProvider> {
        Err(E_NOTIMPL.into())
    }
    fn EnumFunctionProviders(&self) -> Result<IEnumTfFunctionProviders> {
        Err(E_NOTIMPL.into())
    }
    fn GetGlobalCompartment(&self) -> Result<ITfCompartmentMgr> {
        Err(E_NOTIMPL.into())
    }
}

pub(super) fn new_manager() -> ComObject<Manager> {
    ComObject::new(Manager {
        sink: RefCell::new(None),
        fail_advise: Cell::new(true),
        fail_unadvise: Cell::new(false),
        reenter: RefCell::new(None),
        advised: Cell::new(0),
    })
}

pub(super) fn exercise(
    tip: &ITfTextInputProcessorEx,
    factory: &windows::Win32::System::Com::IClassFactory,
) {
    let manager = new_manager();
    let interface = manager.to_interface::<ITfThreadMgr>();
    let state = manager.get();
    // SAFETY: local apartment-bound interfaces and valid IDs in this mock.
    unsafe {
        assert_eq!(tip.ActivateEx(&interface, 7, 0).unwrap_err().code(), E_FAIL);
        assert!(state.sink.borrow().is_none());
        state.fail_advise.set(false);
        *state.reenter.borrow_mut() = Some(tip.clone());
        tip.ActivateEx(&interface, 7, 0).unwrap();
        state.reenter.borrow_mut().take();
        let sink = state.sink.borrow().as_ref().unwrap().clone();
        assert!(!sink
            .OnKeyDown(None, WPARAM(0x0d), LPARAM(0))
            .unwrap()
            .as_bool());
        assert_eq!(
            tip.ActivateEx(&interface, 7, 0).unwrap_err().code(),
            E_UNEXPECTED
        );
        state.fail_unadvise.set(true);
        assert_eq!(tip.Deactivate().unwrap_err().code(), E_FAIL);
        assert!(state.sink.borrow().is_some());
        state.fail_unadvise.set(false);
        tip.Deactivate().unwrap();
        assert!(state.sink.borrow().is_none());
        tip.Deactivate().unwrap();
        tip.Activate(&interface, 7).unwrap();
        tip.Deactivate().unwrap();
        let dropped: ITfTextInputProcessorEx = factory.CreateInstance(None).unwrap();
        dropped.ActivateEx(&interface, 7, 0).unwrap();
        drop(dropped);
        assert!(state.sink.borrow().is_none());
        let failed_cleanup: ITfTextInputProcessorEx = factory.CreateInstance(None).unwrap();
        failed_cleanup.ActivateEx(&interface, 7, 0).unwrap();
        state.fail_unadvise.set(true);
        drop(failed_cleanup);
        assert!(state.sink.borrow().is_some());
        // No cycle: releasing the mock manager also releases the orphan sink.
        // The sink's DLL lease remains valid until this scope is torn down.
    }
}

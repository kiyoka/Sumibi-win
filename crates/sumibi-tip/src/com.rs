//! COM/FFI boundary and owner-thread TSF activation.
#![allow(non_snake_case)]

#[cfg(feature = "spike")]
use crate::spike::{Controller, ProviderSubscription};
use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;
#[cfg(feature = "spike")]
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::{self, ThreadId};
use windows::core::{implement, Error, IUnknown, Interface, Ref, Result, BOOL, GUID, HRESULT};
use windows::Win32::Foundation::{
    CLASS_E_CLASSNOTAVAILABLE, CLASS_E_NOAGGREGATION, E_INVALIDARG, E_POINTER, E_UNEXPECTED,
    RPC_E_WRONG_THREAD, S_FALSE, S_OK,
};
use windows::Win32::System::Com::{IClassFactory, IClassFactory_Impl};
use windows::Win32::UI::TextServices::{
    IEnumTfDisplayAttributeInfo, ITfDisplayAttributeInfo, ITfDisplayAttributeProvider,
    ITfDisplayAttributeProvider_Impl, ITfKeystrokeMgr, ITfTextInputProcessorEx,
    ITfTextInputProcessorEx_Impl, ITfTextInputProcessor_Impl, ITfThreadMgr,
};

pub const CLSID_SUMIBI_TIP: GUID = GUID::from_u128(0xe927d4ab_2ec9_4dd3_a921_a4620b95a410);
static OBJECTS: AtomicUsize = AtomicUsize::new(0);
static LOCKS: AtomicUsize = AtomicUsize::new(0);

pub(super) struct ObjectLease;

impl ObjectLease {
    pub(super) fn new() -> Self {
        OBJECTS.fetch_add(1, Ordering::SeqCst);
        Self
    }
}

impl Drop for ObjectLease {
    fn drop(&mut self) {
        OBJECTS.fetch_sub(1, Ordering::SeqCst);
    }
}

pub(super) fn boundary(action: impl FnOnce() -> Result<()>) -> Result<()> {
    catch_unwind(AssertUnwindSafe(action)).unwrap_or_else(|_| Err(E_UNEXPECTED.into()))
}

#[implement(IClassFactory)]
struct Factory {
    _lease: ObjectLease,
}

impl IClassFactory_Impl for Factory_Impl {
    fn CreateInstance(
        &self,
        outer: Ref<IUnknown>,
        iid: *const GUID,
        output: *mut *mut c_void,
    ) -> Result<()> {
        boundary(|| {
            if output.is_null() {
                return Err(E_POINTER.into());
            }
            // SAFETY: COM caller supplies a writable output slot for this call.
            unsafe { output.write(ptr::null_mut()) };
            if iid.is_null() {
                return Err(E_POINTER.into());
            }
            if !outer.is_null() {
                return Err(CLASS_E_NOAGGREGATION.into());
            }
            let tip: ITfTextInputProcessorEx = Tip::new().into();
            // SAFETY: iid/output are caller-owned COM arguments, checked for null.
            // QueryInterface transfers one reference on success; local tip is released.
            unsafe { tip.query(iid, output).ok() }
        })
    }

    fn LockServer(&self, lock: BOOL) -> Result<()> {
        boundary(|| {
            if lock.as_bool() {
                LOCKS.fetch_add(1, Ordering::SeqCst);
                Ok(())
            } else {
                LOCKS
                    .try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                    .map(|_| ())
                    .map_err(|_| Error::from_hresult(E_UNEXPECTED))
            }
        })
    }
}

#[implement(ITfTextInputProcessorEx, ITfDisplayAttributeProvider)]
struct Tip {
    owner: ThreadId,
    busy: Cell<bool>,
    active: RefCell<Option<Active>>,
    // Last: retained manager references are released before the object lease.
    _lease: ObjectLease,
}

struct Active {
    keys: ITfKeystrokeMgr,
    client: u32,
    #[cfg(feature = "spike")]
    controller: Rc<Controller>,
    #[cfg(feature = "spike")]
    provider: Option<ProviderSubscription>,
}

impl Active {
    fn revoke(&mut self) -> Result<()> {
        #[cfg(feature = "spike")]
        {
            self.controller.revoke()?;
            if let Some(provider) = self.provider.as_mut() {
                provider.close()?;
            }
        }
        Ok(())
    }
}

struct Operation<'a>(&'a Cell<bool>);

impl Drop for Operation<'_> {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

impl Tip {
    fn new() -> Self {
        Self {
            owner: thread::current().id(),
            busy: Cell::new(false),
            active: RefCell::new(None),
            _lease: ObjectLease::new(),
        }
    }

    fn operation(&self) -> Result<Operation<'_>> {
        if thread::current().id() != self.owner {
            return Err(RPC_E_WRONG_THREAD.into());
        }
        if self.busy.replace(true) {
            return Err(E_UNEXPECTED.into());
        }
        Ok(Operation(&self.busy))
    }
}

impl Drop for Tip {
    fn drop(&mut self) {
        // TSF normally calls Deactivate. A forgotten Deactivate must not leave
        // our sink registered. The sink has no reference back to this TIP.
        let _ = boundary(|| {
            if thread::current().id() == self.owner {
                if let Some(mut active) = self.active.get_mut().take() {
                    let _ = active.revoke();
                    // SAFETY: owner thread, matching client ID from activation,
                    // and the retained interface stays alive through the call.
                    unsafe { active.keys.UnadviseKeyEventSink(active.client)? };
                }
            }
            Ok(())
        });
    }
}

impl ITfDisplayAttributeProvider_Impl for Tip_Impl {
    fn EnumDisplayAttributeInfo(&self) -> Result<IEnumTfDisplayAttributeInfo> {
        let _operation = self.operation()?;
        Ok(crate::display::enumerate())
    }

    fn GetDisplayAttributeInfo(&self, guid: *const GUID) -> Result<ITfDisplayAttributeInfo> {
        let _operation = self.operation()?;
        crate::display::get(guid)
    }
}

impl ITfTextInputProcessor_Impl for Tip_Impl {
    fn Activate(&self, manager: Ref<ITfThreadMgr>, client: u32) -> Result<()> {
        boundary(|| {
            let _operation = self.operation()?;
            let manager = manager.ok()?;
            if client == 0 {
                return Err(E_INVALIDARG.into());
            }
            if self.active.borrow().is_some() {
                return Err(E_UNEXPECTED.into());
            }
            let keys: ITfKeystrokeMgr = manager.cast()?;
            #[cfg(feature = "spike")]
            let controller = Controller::new(manager, client);
            #[cfg(feature = "spike")]
            let provider = ProviderSubscription::register(manager, client, &controller)?;
            #[cfg(feature = "spike")]
            let sink = crate::keys::KeySink::with_controller(Rc::downgrade(&controller));
            #[cfg(not(feature = "spike"))]
            let sink = crate::keys::KeySink::create();
            // SAFETY: activation runs on the owning TSF thread; manager/client
            // are supplied by TSF. No RefCell borrow survives this external call.
            unsafe { keys.AdviseKeyEventSink(client, &sink, true)? };
            *self.active.borrow_mut() = Some(Active {
                keys,
                client,
                #[cfg(feature = "spike")]
                controller,
                #[cfg(feature = "spike")]
                provider,
            });
            Ok(())
        })
    }

    fn Deactivate(&self) -> Result<()> {
        boundary(|| {
            let _operation = self.operation()?;
            let active = self.active.borrow_mut().take();
            if let Some(mut active) = active {
                if let Err(error) = active.revoke() {
                    *self.active.borrow_mut() = Some(active);
                    return Err(error);
                }
                // SAFETY: same owner thread and retained manager/client pair.
                // Reentrant lifecycle calls are rejected by the operation guard.
                if let Err(error) = unsafe { active.keys.UnadviseKeyEventSink(active.client) } {
                    // Preserve the subscription for an explicit cleanup retry.
                    *self.active.borrow_mut() = Some(active);
                    return Err(error);
                }
            }
            Ok(())
        })
    }
}

impl ITfTextInputProcessorEx_Impl for Tip_Impl {
    fn ActivateEx(&self, manager: Ref<ITfThreadMgr>, client: u32, _flags: u32) -> Result<()> {
        self.Activate(manager, client)
    }
}

/// Return a COM class factory without registry changes.
///
/// # Safety
/// Non-null input pointers must reference valid GUIDs and output must reference
/// a writable pointer slot for the duration of the call, as required by COM.
#[no_mangle]
pub unsafe extern "system" fn DllGetClassObject(
    class: *const GUID,
    iid: *const GUID,
    output: *mut *mut c_void,
) -> HRESULT {
    boundary(|| {
        if output.is_null() {
            return Err(E_POINTER.into());
        }
        // SAFETY: writable COM output slot is part of this function's contract.
        unsafe { output.write(ptr::null_mut()) };
        if class.is_null() || iid.is_null() {
            return Err(E_POINTER.into());
        }
        // SAFETY: class points to a valid GUID per the caller contract above.
        if unsafe { *class } != CLSID_SUMIBI_TIP {
            return Err(CLASS_E_CLASSNOTAVAILABLE.into());
        }
        let factory: IClassFactory = Factory {
            _lease: ObjectLease::new(),
        }
        .into();
        // SAFETY: valid iid/output per COM contract; returns an owned reference.
        unsafe { factory.query(iid, output).ok() }
    })
    .into()
}

/// COM must serialize its unload decision against creation of new objects.
#[no_mangle]
pub extern "system" fn DllCanUnloadNow() -> HRESULT {
    if OBJECTS.load(Ordering::SeqCst) == 0 && LOCKS.load(Ordering::SeqCst) == 0 {
        S_OK
    } else {
        S_FALSE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // One test owns all process-global object and lock counters.
    #[test]
    fn com_lifetime_and_failure_contract() {
        // SAFETY: all pointers below refer to live stack slots/interfaces; no
        // registration or external host is involved. COM references stay local.
        unsafe {
            assert_eq!(DllCanUnloadNow(), S_OK);
            let sink = crate::keys::KeySink::create();
            assert_eq!(DllCanUnloadNow(), S_FALSE);
            for key in [0x08, 0x0d, 0x1b, 0x41, 0x4a, 0xff] {
                let key = windows::Win32::Foundation::WPARAM(key);
                let flags = windows::Win32::Foundation::LPARAM(0);
                assert!(!sink.OnTestKeyDown(None, key, flags).unwrap().as_bool());
                assert!(!sink.OnKeyDown(None, key, flags).unwrap().as_bool());
                assert!(!sink.OnTestKeyUp(None, key, flags).unwrap().as_bool());
                assert!(!sink.OnKeyUp(None, key, flags).unwrap().as_bool());
            }
            sink.OnSetFocus(true).unwrap();
            sink.OnSetFocus(false).unwrap();
            assert!(!sink.OnPreservedKey(None, ptr::null()).unwrap().as_bool());
            drop(sink);
            assert_eq!(DllCanUnloadNow(), S_OK);
            let mut output = ptr::null_mut();
            assert_eq!(
                DllGetClassObject(ptr::null(), &IClassFactory::IID, &mut output),
                E_POINTER
            );
            assert!(output.is_null());
            let unknown = GUID::from_u128(0xffff);
            assert_eq!(
                DllGetClassObject(&unknown, &IClassFactory::IID, &mut output),
                CLASS_E_CLASSNOTAVAILABLE
            );
            assert_eq!(
                DllGetClassObject(&CLSID_SUMIBI_TIP, &unknown, &mut output),
                windows::Win32::Foundation::E_NOINTERFACE
            );
            assert!(output.is_null());
            assert_eq!(DllCanUnloadNow(), S_OK);
            assert_eq!(
                DllGetClassObject(&CLSID_SUMIBI_TIP, &IClassFactory::IID, &mut output),
                S_OK
            );
            let factory = IClassFactory::from_raw(output);
            assert_eq!(DllCanUnloadNow(), S_FALSE);
            assert_eq!(factory.LockServer(false).unwrap_err().code(), E_UNEXPECTED);
            factory.LockServer(true).unwrap();
            let tip: ITfTextInputProcessorEx = factory.CreateInstance(None).unwrap();
            let base = tip
                .cast::<windows::Win32::UI::TextServices::ITfTextInputProcessor>()
                .unwrap();
            assert_eq!(tip.ActivateEx(None, 1, 0).unwrap_err().code(), E_POINTER);
            tip.Deactivate().unwrap();
            crate::mock::exercise(&tip, &factory);
            let outer = tip.cast::<IUnknown>().unwrap();
            assert_eq!(
                factory
                    .CreateInstance::<_, ITfTextInputProcessorEx>(&outer)
                    .unwrap_err()
                    .code(),
                CLASS_E_NOAGGREGATION
            );
            drop(outer);
            drop(base);
            drop(tip);
            drop(factory);
            // Only the server lock remains; no live COM objects mask it.
            assert_eq!(DllCanUnloadNow(), S_FALSE);
            assert_eq!(
                DllGetClassObject(&CLSID_SUMIBI_TIP, &IClassFactory::IID, &mut output),
                S_OK
            );
            let factory = IClassFactory::from_raw(output);
            factory.LockServer(false).unwrap();
            drop(factory);
            assert_eq!(DllCanUnloadNow(), S_OK);
        }
    }
}

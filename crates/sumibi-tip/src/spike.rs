//! Explicit development-only grant for the synthetic text-store harness.
//! Never enable this capability in an installed production IME.
#![allow(non_snake_case)]

pub use crate::broker_spike::{
    ISpikeBroker, ISpikeBrokerTransport, ISpikeBrokerTransport_Impl, ISpikeBrokerTransport_Vtbl,
    FUNCTION_TEST_BROKER,
};
use crate::com::{boundary, ObjectLease};
use crate::edit::preview::Preview;
use crate::edit::{capture_selection, replace_selection, EditPolicy};
pub use crate::input_spike::{
    ISpikeInput, ISpikeInputEraser, ISpikeInputMonitor, ISpikeInputPreparer, ISpikeInputSignal,
    ISpikeInputSignal_Impl, ISpikeInputWriter, ISpikeSelectionEraser, FUNCTION_TEST_INPUT,
    INPUT_APPLIED, INPUT_APPLIED_UNTRACKED, INPUT_NOT_ATTEMPTED, INPUT_UNCERTAIN,
};
pub use crate::owned_composition::GUID_SPIKE_COMPOSITION_SINK;
use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::rc::{Rc, Weak};
use std::thread::{self, ThreadId};
use std::time::{Duration, Instant};
use sumibi_core::Revision;
use windows::core::BSTR;
use windows::core::{
    implement, interface, IUnknown, IUnknown_Vtbl, Interface, Result, GUID, HRESULT,
};
use windows::Win32::Foundation::{E_ABORT, E_INVALIDARG, E_UNEXPECTED, RPC_E_WRONG_THREAD};
use windows::Win32::UI::TextServices::*;

pub const FUNCTION_TEST_CONTROL: GUID = GUID::from_u128(0xe40f959f_2d8e_44aa_988c_ef07085044a5);
pub const FUNCTION_TEST_DELAYED: GUID = GUID::from_u128(0x76389aca_c984_45ee_9f73_14bc869bbf04);
pub const FUNCTION_TEST_KEY_SINK: GUID = GUID::from_u128(0x6ae7d02b_668a_4eac_a87f_313042d4eac5);
pub const FUNCTION_TEST_MANAGER_SINK: GUID =
    GUID::from_u128(0xf0bb20a5_1805_4d16_911b_f8d24c0b3b19);
pub const COMMAND_CONVERT: GUID = GUID::from_u128(0x42e4ca10_7cae_4a74_bf57_d497c5667a2c);
const CONVERT_KEY: TF_PRESERVEDKEY = TF_PRESERVEDKEY {
    uVKey: 0x4a,
    uModifiers: TF_MOD_CONTROL,
};
/// Diagnostic counter only, absent from non-spike builds. No text or reset API.
/// The caller must retain the loaded module throughout this synchronous call.
#[no_mangle]
pub extern "system" fn SumibiSpikeBodyReadCount() -> u64 {
    crate::edit::body_read_count()
}

#[implement(ITfFunctionProvider)]
struct Provider {
    state: Weak<Controller>,
    owner: ThreadId,
    _lease: ObjectLease,
}
impl ITfFunctionProvider_Impl for Provider_Impl {
    fn GetType(&self) -> Result<GUID> {
        Ok(crate::CLSID_SUMIBI_TIP)
    }
    fn GetDescription(&self) -> Result<BSTR> {
        Ok(BSTR::from("Sumibi development controls"))
    }
    fn GetFunction(&self, function: *const GUID, iid: *const GUID) -> Result<IUnknown> {
        let mut result = None;
        boundary(|| {
            if self.owner != thread::current().id() {
                return Err(RPC_E_WRONG_THREAD.into());
            }
            if function.is_null() || iid.is_null() {
                return Err(E_INVALIDARG.into());
            }
            if unsafe { *function } == FUNCTION_TEST_INPUT && unsafe { *iid } == ISpikeInput::IID {
                result = Some(crate::input_spike::control(
                    &self.state.upgrade().ok_or(E_ABORT)?,
                ));
                return Ok(());
            }
            if unsafe { *function } == FUNCTION_TEST_BROKER && unsafe { *iid } == ISpikeBroker::IID
            {
                result = Some(crate::broker_spike::control(
                    &self.state.upgrade().ok_or(E_ABORT)?,
                ));
                return Ok(());
            }
            if unsafe { *function } == FUNCTION_TEST_DELAYED
                && unsafe { *iid } == ISpikeDelayed::IID
            {
                let state = self.state.upgrade().ok_or(E_ABORT)?;
                let delayed: ISpikeDelayed = Delayed {
                    state: Rc::downgrade(&state),
                    _lease: ObjectLease::new(),
                }
                .into();
                // SAFETY: transfer this exact interface's owned reference.
                result = Some(unsafe { IUnknown::from_raw(delayed.into_raw()) });
                return Ok(());
            }
            if unsafe { *function } == FUNCTION_TEST_MANAGER_SINK
                && unsafe { *iid } == ITfThreadMgrEventSink::IID
            {
                let state = self.state.upgrade().ok_or(E_ABORT)?;
                let sink = crate::notifications::manager_sink(&state);
                // SAFETY: transfer the requested COM interface without changing
                // its vtable, matching GetFunction's requested IID.
                result = Some(unsafe { IUnknown::from_raw(sink.into_raw()) });
                return Ok(());
            }
            // A headless TSF host does not route ordinary desktop keys. Expose
            // the same callback implementation for explicit DLL diagnostics.
            if unsafe { *function } == FUNCTION_TEST_KEY_SINK
                && unsafe { *iid } == ITfKeyEventSink::IID
            {
                let state = self.state.upgrade().ok_or(E_ABORT)?;
                let sink = crate::keys::KeySink::with_controller(Rc::downgrade(&state));
                // SAFETY: preserve the requested interface's vtable and transfer
                // its owned COM reference through GetFunction's IUnknown ABI.
                result = Some(unsafe { IUnknown::from_raw(sink.into_raw()) });
                return Ok(());
            }
            // SAFETY: COM caller supplies readable GUID values.
            if unsafe { *function } != FUNCTION_TEST_CONTROL
                || unsafe { *iid } != ISpikeControl::IID
            {
                return Err(windows::Win32::Foundation::E_NOINTERFACE.into());
            }
            result = Some(control(&self.state.upgrade().ok_or(E_ABORT)?));
            Ok(())
        })?;
        result.ok_or_else(|| E_UNEXPECTED.into())
    }
}

pub(crate) struct ProviderSubscription {
    source: ITfSourceSingle,
    client: u32,
    armed: bool,
}
impl ProviderSubscription {
    pub(crate) fn register(
        manager: &ITfThreadMgr,
        client: u32,
        state: &Rc<Controller>,
    ) -> Result<Option<Self>> {
        let source = match manager.cast::<ITfSourceSingle>() {
            Ok(source) => source,
            Err(error) if error.code() == windows::Win32::Foundation::E_NOINTERFACE => {
                return Ok(None)
            }
            Err(error) => return Err(error),
        };
        let provider: ITfFunctionProvider = Provider {
            state: Rc::downgrade(state),
            owner: state.owner,
            _lease: ObjectLease::new(),
        }
        .into();
        // SAFETY: owner thread, valid TSF client and counted provider reference.
        unsafe { source.AdviseSingleSink(client, &ITfFunctionProvider::IID, &provider)? };
        Ok(Some(Self {
            source,
            client,
            armed: true,
        }))
    }
    pub(crate) fn close(&mut self) -> Result<()> {
        if self.armed {
            // SAFETY: matches the exact client and IID used for registration.
            unsafe {
                self.source
                    .UnadviseSingleSink(self.client, &ITfFunctionProvider::IID)?
            };
            self.armed = false;
        }
        Ok(())
    }
}
impl Drop for ProviderSubscription {
    fn drop(&mut self) {
        let _ = boundary(|| self.close());
    }
}

#[interface("27fd2a99-bff8-49d0-96a5-a639946ac6f1")]
pub unsafe trait ISpikeControl: IUnknown {
    pub fn GrantContext(&self, context: *mut c_void) -> HRESULT;
    pub fn RevokeContext(&self) -> HRESULT;
    pub fn BeginPreview(&self) -> HRESULT;
    pub fn CancelPreview(&self) -> HRESULT;
    pub fn VerifyPreview(&self) -> HRESULT;
}

/// A separate development-only ABI; do not extend the existing control vtable.
#[interface("46f9336d-f31d-475e-a248-25ac7e8dec8d")]
pub unsafe trait ISpikeDelayed: IUnknown {
    pub fn BeginRequest(&self, request: u64, timeout_ms: u32) -> HRESULT;
    pub fn CompleteRequest(&self, request: u64) -> HRESULT;
}

#[implement(ISpikeDelayed)]
struct Delayed {
    state: Weak<Controller>,
    _lease: ObjectLease,
}
impl ISpikeDelayed_Impl for Delayed_Impl {
    unsafe fn BeginRequest(&self, request: u64, timeout_ms: u32) -> HRESULT {
        boundary(|| {
            let state = self.state.upgrade().ok_or(E_ABORT)?;
            state.check_thread()?;
            if !(1..=2000).contains(&timeout_ms)
                || request == 0
                || request <= state.last_request.get()
                || state.pending_request.get().is_some()
            {
                return Err(E_ABORT.into());
            }
            state.last_request.set(request);
            state.begin_preview()?;
            state.request_deadline.set(Some(
                Instant::now() + Duration::from_millis(timeout_ms.into()),
            ));
            state.pending_request.set(Some(request));
            Ok(())
        })
        .into()
    }
    unsafe fn CompleteRequest(&self, request: u64) -> HRESULT {
        boundary(|| {
            let state = self.state.upgrade().ok_or(E_ABORT)?;
            state.check_thread()?;
            if state.busy.replace(true) {
                return Err(E_UNEXPECTED.into());
            }
            let _busy = Busy(&state.busy);
            if state.pending_request.get() != Some(request) || state.invalidated.get() {
                return Err(E_ABORT.into());
            }
            state.pending_request.set(None);
            if state
                .request_deadline
                .take()
                .is_none_or(|deadline| Instant::now() >= deadline)
            {
                let _ = state.cancel_preview();
                return Err(E_ABORT.into());
            }
            // Never recapture after a delayed response. A cancelled preview is
            // not permission to replace whichever selection is current now.
            let mut pending = state.preview.borrow_mut().take().ok_or(E_ABORT)?;
            let result = pending.commit("日本語");
            if result.is_err() {
                *state.preview.borrow_mut() = Some(pending);
                let _ = state.cancel_preview();
            }
            result
        })
        .into()
    }
}

pub(crate) struct Controller {
    pub(crate) owner: ThreadId,
    manager: ITfThreadMgr,
    client: u32,
    granted: RefCell<Option<ITfContext>>,
    policy: Rc<Cell<EditPolicy>>,
    preserved: Cell<bool>,
    busy: Cell<bool>,
    preview: RefCell<Option<Preview>>,
    observers: RefCell<Vec<crate::notifications::Subscription>>,
    generation: Cell<u64>,
    invalidated: Cell<bool>,
    cleanup_queued: Cell<Option<u64>>,
    last_request: Cell<u64>,
    pending_request: Cell<Option<u64>>,
    request_deadline: Cell<Option<Instant>>,
    pub(crate) broker: RefCell<Option<Rc<crate::broker_spike::Broker>>>,
    pub(crate) broker_request_floor: Rc<Cell<u64>>,
}

struct Busy<'a>(&'a Cell<bool>);
struct RestoreBusy<'a>(&'a Cell<bool>, bool);
impl Drop for RestoreBusy<'_> {
    fn drop(&mut self) {
        self.0.set(self.1);
    }
}
impl Drop for Busy<'_> {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

impl Controller {
    pub(crate) fn input_owner(&self) -> Result<(ITfThreadMgr, u32)> {
        self.check_thread()?;
        Ok((self.manager.clone(), self.client))
    }
    pub(crate) fn new(manager: &ITfThreadMgr, client: u32) -> Rc<Self> {
        Rc::new(Self {
            owner: thread::current().id(),
            manager: manager.clone(),
            client,
            granted: RefCell::new(None),
            policy: Rc::new(Cell::new(EditPolicy {
                revision: Revision {
                    input: 0,
                    settings: 0,
                    authorization: 0,
                },
                allowed: false,
            })),
            preserved: Cell::new(false),
            busy: Cell::new(false),
            preview: RefCell::new(None),
            observers: RefCell::new(Vec::new()),
            generation: Cell::new(0),
            invalidated: Cell::new(false),
            cleanup_queued: Cell::new(None),
            last_request: Cell::new(0),
            pending_request: Cell::new(None),
            request_deadline: Cell::new(None),
            broker: RefCell::new(None),
            broker_request_floor: Rc::new(Cell::new(0)),
        })
    }
    pub(crate) fn check_thread(&self) -> Result<()> {
        if self.owner != thread::current().id() {
            Err(RPC_E_WRONG_THREAD.into())
        } else {
            Ok(())
        }
    }
    pub(crate) fn grant(self: &Rc<Self>, context: &ITfContext) -> Result<()> {
        self.check_thread()?;
        if self.busy.replace(true) {
            return Err(E_UNEXPECTED.into());
        }
        let _busy = Busy(&self.busy);
        self.revoke()?;
        let observers = crate::notifications::subscribe(self, &self.manager, context)?;
        let mut granted_policy = self.policy.get();
        granted_policy.revision.authorization = granted_policy
            .revision
            .authorization
            .checked_add(1)
            .ok_or(E_ABORT)?;
        // SAFETY: owner thread and same-context check before granting the
        // dedicated test host permission. This does not classify password fields.
        unsafe {
            if self.manager.GetFocus()?.GetTop()?.cast::<IUnknown>()?
                != context.cast::<IUnknown>()?
            {
                return Err(E_ABORT.into());
            }
            let keys: ITfKeystrokeMgr = self.manager.cast()?;
            keys.PreserveKey(
                self.client,
                &COMMAND_CONVERT,
                &CONVERT_KEY,
                &"Sumibi spike conversion".encode_utf16().collect::<Vec<_>>(),
            )?;
        }
        self.preserved.set(true);
        *self.observers.borrow_mut() = observers;
        *self.granted.borrow_mut() = Some(context.clone());
        granted_policy.allowed = true;
        self.policy.set(granted_policy);
        Ok(())
    }
    pub(crate) fn revoke(&self) -> Result<()> {
        self.check_thread()?;
        let previous_busy = self.busy.replace(true);
        let _busy = RestoreBusy(&self.busy, previous_busy);
        let mut policy = self.policy.get();
        policy.allowed = false;
        self.policy.set(policy);
        let broker = self.broker.borrow_mut().take();
        if let Some(broker) = broker {
            broker.close();
        }
        let old = self.granted.borrow_mut().take();
        drop(old);
        let cleanup = self.cancel_preview();
        if cleanup.is_ok() {
            let mut observers = std::mem::take(&mut *self.observers.borrow_mut());
            for observer in &mut observers {
                if let Err(error) = observer.close() {
                    *self.observers.borrow_mut() = observers;
                    return Err(error);
                }
            }
        }
        if self.preserved.replace(false) {
            // SAFETY: same manager and exact command/key pair used to register.
            let result = self.manager.cast::<ITfKeystrokeMgr>().and_then(|keys| {
                // SAFETY: same owner-thread manager and registered key pair.
                unsafe { keys.UnpreserveKey(&COMMAND_CONVERT, &CONVERT_KEY) }
            });
            if let Err(error) = result {
                self.preserved.set(true);
                return Err(error);
            }
        }
        cleanup
    }
    fn cancel_preview(&self) -> Result<()> {
        self.check_thread()?;
        let broker = self.broker.borrow().clone();
        if let Some(broker) = broker {
            broker.cancel(None)?;
        }
        self.pending_request.set(None);
        self.request_deadline.set(None);
        let pending = self.preview.borrow_mut().take();
        if let Some(mut pending) = pending {
            if let Err(error) = pending.cancel() {
                *self.preview.borrow_mut() = Some(pending);
                return Err(error);
            }
        }
        Ok(())
    }
    fn begin_preview(&self) -> Result<()> {
        self.check_thread()?;
        if self.busy.replace(true) {
            return Err(E_UNEXPECTED.into());
        }
        let _busy = Busy(&self.busy);
        self.cancel_preview()?;
        self.generation
            .set(self.generation.get().checked_add(1).ok_or(E_ABORT)?);
        self.invalidated.set(false);
        let granted = self.granted.borrow().clone().ok_or(E_ABORT)?;
        let snapshot =
            capture_selection(&self.manager, &granted, self.client, self.policy.clone())?;
        let pending = Preview::begin(&self.manager, self.client, snapshot)?;
        *self.preview.borrow_mut() = Some(pending);
        if !self.policy.get().allowed {
            self.cancel_preview()?;
            return Err(E_ABORT.into());
        }
        Ok(())
    }
    pub(crate) fn convert(&self, context: &ITfContext) -> Result<()> {
        self.check_thread()?;
        if self.busy.replace(true) {
            return Err(E_UNEXPECTED.into());
        }
        let _busy = Busy(&self.busy);
        self.pending_request.set(None);
        self.request_deadline.set(None);
        let granted = self.granted.borrow().clone().ok_or(E_ABORT)?;
        if granted.cast::<IUnknown>()? != context.cast::<IUnknown>()? || !self.policy.get().allowed
        {
            return Err(E_ABORT.into());
        }
        let broker = self.broker.borrow().clone();
        if let Some(broker) = broker {
            if broker.pending() {
                return Err(E_ABORT.into());
            }
            self.cancel_preview()?;
            self.invalidated.set(false);
            return broker.begin(&self.manager, self.client);
        }
        let pending = self.preview.borrow_mut().take();
        if let Some(mut pending) = pending {
            if self.invalidated.get() {
                *self.preview.borrow_mut() = Some(pending);
                let _ = self.cancel_preview();
                return Err(E_ABORT.into());
            }
            let result = pending.commit("日本語");
            if result.is_err() {
                // Keep failed work only for cleanup, never recapture a new
                // selection and apply the old result to it.
                *self.preview.borrow_mut() = Some(pending);
                let _ = self.cancel_preview();
            }
            return result;
        }
        let snapshot = capture_selection(&self.manager, context, self.client, self.policy.clone())?;
        replace_selection(&self.manager, self.client, snapshot, "日本語")
    }

    pub(crate) fn observes(&self, context: &ITfContext) -> Result<bool> {
        self.check_thread()?;
        if self.busy.get() || (self.preview.borrow().is_none() && !self.broker_pending()) {
            return Ok(false);
        }
        let granted = self.granted.borrow().clone();
        Ok(match granted {
            Some(granted) => granted.cast::<IUnknown>()? == context.cast::<IUnknown>()?,
            None => false,
        })
    }

    pub(crate) fn check_focus_notification(self: &Rc<Self>) -> Result<()> {
        self.check_thread()?;
        let granted = self.granted.borrow().clone();
        let Some(granted) = granted else {
            return Ok(());
        };
        // SAFETY: focus metadata only; no access to the new input's contents.
        let matches = unsafe {
            self.manager
                .GetFocus()
                .and_then(|doc| doc.GetTop())
                .and_then(|ctx| ctx.cast::<IUnknown>())
        }
        .map(|ctx| {
            granted
                .cast::<IUnknown>()
                .map(|old| old == ctx)
                .unwrap_or(false)
        })
        .unwrap_or(false);
        if !matches {
            self.invalidate(true)?;
        }
        Ok(())
    }

    pub(crate) fn focus_notification(self: &Rc<Self>, context: Option<&ITfContext>) -> Result<()> {
        self.check_thread()?;
        let granted = self.granted.borrow().clone();
        if let Some(granted) = granted {
            let matches = match context {
                Some(context) => granted.cast::<IUnknown>()? == context.cast::<IUnknown>()?,
                None => false,
            };
            if !matches {
                self.invalidate(true)?;
            }
        }
        Ok(())
    }

    pub(crate) fn invalidate(self: &Rc<Self>, revoke: bool) -> Result<()> {
        self.check_thread()?;
        let previous_busy = self.busy.replace(true);
        let _busy = RestoreBusy(&self.busy, previous_busy);
        self.pending_request.set(None);
        self.request_deadline.set(None);
        self.invalidated.set(true);
        if revoke {
            let mut policy = self.policy.get();
            policy.allowed = false;
            self.policy.set(policy);
        }
        let broker = self.broker.borrow().clone();
        if let Some(broker) = broker {
            if revoke {
                broker.close();
            } else {
                broker.cancel(None)?;
            }
        }
        let context = self.preview.borrow().as_ref().map(Preview::context);
        let Some(context) = context else {
            return Ok(());
        };
        let generation = self.generation.get();
        if self.cleanup_queued.get() == Some(generation) {
            return Ok(());
        }
        self.cleanup_queued.set(Some(generation));
        let weak = Rc::downgrade(self);
        let requested = crate::edit::queue_cleanup(
            &context,
            self.client,
            Box::new(move |cookie| {
                let Some(state) = weak.upgrade() else {
                    return Ok(());
                };
                state.check_thread()?;
                if state.cleanup_queued.get() == Some(generation) {
                    state.cleanup_queued.set(None);
                }
                if state.generation.get() != generation || !state.invalidated.get() {
                    return Ok(());
                }
                let pending = state.preview.borrow_mut().take();
                if let Some(mut pending) = pending {
                    if let Err(error) = pending.clear_in_session(cookie) {
                        *state.preview.borrow_mut() = Some(pending);
                        return Err(error);
                    }
                }
                Ok(())
            }),
        );
        if requested.is_err() && self.cleanup_queued.get() == Some(generation) {
            self.cleanup_queued.set(None);
        }
        requested
    }

    pub(crate) fn test_preview_key(&self, context: &ITfContext, key: usize) -> Result<bool> {
        self.check_thread()?;
        if !matches!(key, 0x0d | 0x1b)
            || self.busy.get()
            || self.invalidated.get()
            || !self.policy.get().allowed
            || (self.preview.borrow().is_none() && !self.broker_pending())
        {
            return Ok(false);
        }
        let granted = self.granted.borrow().clone().ok_or(E_ABORT)?;
        // SAFETY: current focus identity only; no contents are accessed.
        let focused = unsafe { self.manager.GetFocus()?.GetTop()? };
        Ok(granted.cast::<IUnknown>()? == context.cast::<IUnknown>()?
            && focused.cast::<IUnknown>()? == context.cast::<IUnknown>()?)
    }

    pub(crate) fn preview_key(self: &Rc<Self>, context: &ITfContext, key: usize) -> Result<bool> {
        if !self.test_preview_key(context, key)? {
            return Ok(false);
        }
        self.invalidated.set(true);
        // Key callbacks may take a synchronous write lock. If denied, keep
        // the operation cancelled and defer only its underline cleanup.
        let cancelled = {
            self.busy.set(true);
            let _busy = Busy(&self.busy);
            self.cancel_preview()
        };
        if cancelled.is_err() {
            let _ = self.invalidate(false);
        }
        // Enter confirms the original text, then goes to the host once.
        // Esc consumes the cancellation; neither key applies a conversion.
        Ok(key == 0x1b)
    }
    fn broker_pending(&self) -> bool {
        self.broker
            .borrow()
            .as_ref()
            .is_some_and(|broker| broker.pending())
    }

    pub(crate) fn complete_broker(
        &self,
        request: u64,
        revision: Revision,
        text: String,
    ) -> Result<()> {
        self.check_thread()?;
        if self.busy.replace(true) {
            return Err(E_UNEXPECTED.into());
        }
        let _busy = Busy(&self.busy);
        let broker = self.broker.borrow().clone().ok_or(E_ABORT)?;
        broker.complete(&self.manager, self.client, request, revision, text)
    }
}

impl Drop for Controller {
    fn drop(&mut self) {
        let _ = boundary(|| self.revoke());
    }
}

#[implement(ISpikeControl)]
struct Control {
    state: Weak<Controller>,
    owner: ThreadId,
    _lease: ObjectLease,
}

impl ISpikeControl_Impl for Control_Impl {
    unsafe fn BeginPreview(&self) -> HRESULT {
        boundary(|| {
            if self.owner != thread::current().id() {
                return Err(RPC_E_WRONG_THREAD.into());
            }
            self.state.upgrade().ok_or(E_ABORT)?.begin_preview()
        })
        .into()
    }
    unsafe fn CancelPreview(&self) -> HRESULT {
        boundary(|| {
            if self.owner != thread::current().id() {
                return Err(RPC_E_WRONG_THREAD.into());
            }
            let state = self.state.upgrade().ok_or(E_ABORT)?;
            if state.busy.replace(true) {
                return Err(E_UNEXPECTED.into());
            }
            let _busy = Busy(&state.busy);
            state.cancel_preview()
        })
        .into()
    }
    unsafe fn VerifyPreview(&self) -> HRESULT {
        boundary(|| {
            if self.owner != thread::current().id() {
                return Err(RPC_E_WRONG_THREAD.into());
            }
            let state = self.state.upgrade().ok_or(E_ABORT)?;
            if state.busy.replace(true) {
                return Err(E_UNEXPECTED.into());
            }
            let _busy = Busy(&state.busy);
            if state.invalidated.get() {
                return Err(E_ABORT.into());
            }
            let pending = state.preview.borrow_mut().take().ok_or(E_ABORT)?;
            let result = pending.verify_attribute(true);
            *state.preview.borrow_mut() = Some(pending);
            if !state.policy.get().allowed {
                state.cancel_preview()?;
                return Err(E_ABORT.into());
            }
            result
        })
        .into()
    }
    unsafe fn GrantContext(&self, context: *mut c_void) -> HRESULT {
        boundary(|| {
            if self.owner != thread::current().id() {
                return Err(RPC_E_WRONG_THREAD.into());
            }
            // SAFETY: COM caller supplies a valid ITfContext pointer, borrowed
            // only for this call. grant clones it when retaining the context.
            let context = unsafe { ITfContext::from_raw_borrowed(&context) }.ok_or(E_INVALIDARG)?;
            self.state.upgrade().ok_or(E_ABORT)?.grant(context)
        })
        .into()
    }
    unsafe fn RevokeContext(&self) -> HRESULT {
        boundary(|| {
            if self.owner != thread::current().id() {
                return Err(RPC_E_WRONG_THREAD.into());
            }
            // Deactivate can drop the controller before the host receives its
            // close request. With no controller, the grant is already gone.
            match self.state.upgrade() {
                Some(state) => state.revoke(),
                None => Ok(()),
            }
        })
        .into()
    }
}

pub(crate) fn control(state: &Rc<Controller>) -> IUnknown {
    let interface: ISpikeControl = Control {
        state: Rc::downgrade(state),
        owner: state.owner,
        _lease: ObjectLease::new(),
    }
    .into();
    // SAFETY: this interface derives from IUnknown. Transfer its owned reference
    // without changing the requested ISpikeControl vtable returned by GetFunction.
    unsafe { IUnknown::from_raw(interface.into_raw()) }
}

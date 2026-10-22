//! Owner-thread Core gate, immediately followed by one guarded TSF write.
//! The caller must classify input scopes before supplying an allowed policy.
//! This adapter is currently restricted to caller-owned ACP contexts. It uses
//! ITfRangeACP coordinates and is not an arbitrary application integration.
//! No communication, COM pointer, edit cookie or target coordinate leaves here.
use super::*;
use std::rc::Weak;
use std::time::{Duration, Instant};
use sumibi_core::{ManualConversion, RequestId, Target};
use windows::core::{ComObject, Ref};

struct Observation {
    owner: ThreadId,
    invalid: Cell<bool>,
    generation: Cell<Option<u64>>,
    boundary_generation: Cell<Option<u64>>,
    #[cfg(feature = "spike")]
    wake: RefCell<Option<Rc<dyn Fn()>>>,
}
#[implement(
    ITfTextEditSink,
    ITfThreadMgrEventSink,
    ITfCompartmentEventSink,
    ITfStatusSink
)]
struct Observer {
    state: Weak<Observation>,
    owner: ThreadId,
    _lease: ObjectLease,
}
impl Observer_Impl {
    fn invalidate(&self, protection_boundary: bool) -> Result<()> {
        boundary(|| {
            // Check before touching the apartment-local weak Rc.
            if self.owner != thread::current().id() {
                return Err(RPC_E_WRONG_THREAD.into());
            }
            if let Some(state) = self.state.upgrade() {
                state.invalid.set(true);
                if protection_boundary {
                    state.boundary_generation.set(
                        state
                            .boundary_generation
                            .get()
                            .and_then(|v| v.checked_add(1)),
                    );
                }
                state.generation.set(
                    state
                        .generation
                        .get()
                        .and_then(|value| value.checked_add(1)),
                );
                // Notification callbacks only signal the owner. Never edit,
                // read text, or retain a borrowed state across external code.
                #[cfg(feature = "spike")]
                {
                    let wake = state.wake.borrow().clone();
                    if let Some(wake) = wake {
                        wake();
                    }
                }
            }
            Ok(())
        })
    }
}
impl ITfTextEditSink_Impl for Observer_Impl {
    fn OnEndEdit(&self, _: Ref<ITfContext>, _: u32, record: Ref<ITfEditRecord>) -> Result<()> {
        // Conservative: any write-session notification retires the captured
        // request, including a change that later restores the original text.
        if self.owner != thread::current().id() {
            return Err(RPC_E_WRONG_THREAD.into());
        }
        // Inspect only whether INPUTSCOPE metadata changed, never a range's
        // text or property value. Such history must not be blessed as our own
        // insertion's ordinary OnEndEdit, including a change restored in-lock.
        let scope_changed = (|| -> Result<bool> {
            let ranges = unsafe {
                record.ok()?.GetTextAndPropertyUpdates(
                    GET_TEXT_AND_PROPERTY_UPDATES_FLAGS(0),
                    &[&GUID_PROP_INPUTSCOPE],
                )?
            };
            let mut slot: Option<ITfRange> = None;
            let mut count = 0;
            let status = unsafe {
                (ranges.vtable().Next)(
                    ranges.as_raw(),
                    1,
                    (&mut slot as *mut Option<ITfRange>).cast(),
                    &mut count,
                )
            };
            Ok(!(status == windows::Win32::Foundation::S_FALSE && count == 0 && slot.is_none()))
        })()
        .unwrap_or(true);
        self.invalidate(scope_changed)
    }
}
impl ITfThreadMgrEventSink_Impl for Observer_Impl {
    fn OnInitDocumentMgr(&self, _: Ref<ITfDocumentMgr>) -> Result<()> {
        self.invalidate(true)
    }
    fn OnUninitDocumentMgr(&self, _: Ref<ITfDocumentMgr>) -> Result<()> {
        self.invalidate(true)
    }
    fn OnSetFocus(&self, _: Ref<ITfDocumentMgr>, _: Ref<ITfDocumentMgr>) -> Result<()> {
        self.invalidate(true)
    }
    fn OnPushContext(&self, _: Ref<ITfContext>) -> Result<()> {
        self.invalidate(true)
    }
    fn OnPopContext(&self, _: Ref<ITfContext>) -> Result<()> {
        self.invalidate(true)
    }
}
impl ITfCompartmentEventSink_Impl for Observer_Impl {
    fn OnChange(&self, _: *const windows::core::GUID) -> Result<()> {
        self.invalidate(true)
    }
}
impl ITfStatusSink_Impl for Observer_Impl {
    fn OnStatusChange(&self, _: Ref<ITfContext>, _: u32) -> Result<()> {
        self.invalidate(true)
    }
}
struct Subscription {
    source: ITfSource,
    cookie: u32,
}
impl Drop for Subscription {
    fn drop(&mut self) {
        // SAFETY: retained source/cookie on the owner thread. A failed unadvise
        // leaves only a weak sink alive, with its own DLL lifetime lease.
        let _ = boundary(|| unsafe { self.source.UnadviseSink(self.cookie) });
    }
}
struct Guard {
    state: Rc<Observation>,
    _subscriptions: Vec<Subscription>,
}
impl Guard {
    fn new(manager: &ITfThreadMgr, context: &ITfContext) -> Result<Self> {
        let state = Rc::new(Observation {
            owner: thread::current().id(),
            invalid: Cell::new(false),
            generation: Cell::new(Some(0)),
            boundary_generation: Cell::new(Some(0)),
            #[cfg(feature = "spike")]
            wake: RefCell::new(None),
        });
        let sink = ComObject::new(Observer {
            state: Rc::downgrade(&state),
            owner: thread::current().id(),
            _lease: ObjectLease::new(),
        });
        let mut subscriptions = Vec::new();
        // SAFETY: subscriptions begin before snapshot capture; no state borrow
        // is held across COM. Failed setup unwinds subscriptions on this thread.
        unsafe {
            let source: ITfSource = context.cast()?;
            let cookie = source.AdviseSink(
                &ITfTextEditSink::IID,
                &sink.to_interface::<ITfTextEditSink>(),
            )?;
            subscriptions.push(Subscription { source, cookie });
            let source: ITfSource = context.cast()?;
            let cookie =
                source.AdviseSink(&ITfStatusSink::IID, &sink.to_interface::<ITfStatusSink>())?;
            subscriptions.push(Subscription { source, cookie });
            let compartments: ITfCompartmentMgr = context.cast()?;
            for guid in [
                GUID_COMPARTMENT_KEYBOARD_DISABLED,
                GUID_COMPARTMENT_EMPTYCONTEXT,
            ] {
                let source: ITfSource = compartments.GetCompartment(&guid)?.cast()?;
                let cookie = source.AdviseSink(
                    &ITfCompartmentEventSink::IID,
                    &sink.to_interface::<ITfCompartmentEventSink>(),
                )?;
                subscriptions.push(Subscription { source, cookie });
            }
            let source: ITfSource = manager.cast()?;
            let cookie = source.AdviseSink(
                &ITfThreadMgrEventSink::IID,
                &sink.to_interface::<ITfThreadMgrEventSink>(),
            )?;
            subscriptions.push(Subscription { source, cookie });
        }
        Ok(Self {
            state,
            _subscriptions: subscriptions,
        })
    }
    fn check(&self) -> Result<()> {
        if self.state.owner != thread::current().id() {
            return Err(RPC_E_WRONG_THREAD.into());
        }
        if self.state.invalid.get() {
            return Err(E_ABORT.into());
        }
        Ok(())
    }
}

/// Metadata-only observation generation for a cooperating host's input run.
/// This never authorizes an edit or reads text. Exhaustion remains denied.
#[derive(Clone)]
pub struct InputWatch {
    guard: Rc<Guard>,
}
impl InputWatch {
    pub fn attach(manager: &ITfThreadMgr, context: &ITfContext) -> Result<Self> {
        Ok(Self {
            guard: Rc::new(Guard::new(manager, context)?),
        })
    }
    #[cfg(feature = "spike")]
    pub(crate) fn set_wake(&self, wake: Option<Rc<dyn Fn()>>) -> Result<()> {
        if self.guard.state.owner != thread::current().id() {
            return Err(RPC_E_WRONG_THREAD.into());
        }
        *self.guard.state.wake.borrow_mut() = wake;
        Ok(())
    }
    #[cfg(feature = "spike")]
    pub(crate) fn boundary_generation(&self) -> Result<u64> {
        if self.guard.state.owner != thread::current().id() {
            return Err(RPC_E_WRONG_THREAD.into());
        }
        self.guard
            .state
            .boundary_generation
            .get()
            .ok_or_else(|| E_ABORT.into())
    }
    pub fn generation(&self) -> Result<u64> {
        if self.guard.state.owner != thread::current().id() {
            return Err(RPC_E_WRONG_THREAD.into());
        }
        self.guard
            .state
            .generation
            .get()
            .ok_or_else(|| E_ABORT.into())
    }
}

/// Single-use request, confined by Rc/COM to the capturing thread. Dropping it
/// retires the Core request; responses cannot recapture a different selection.
pub struct ManualEdit {
    manager: ITfThreadMgr,
    client: u32,
    snapshot: SelectionSnapshot,
    core: ManualConversion,
    request: RequestId,
    identity: u128,
    guard: Guard,
    deadline: Instant,
}
impl ManualEdit {
    /// Call only for an ACP context whose text store the caller owns.
    /// `identity` is a fresh opaque snapshot registration, never a COM address.
    /// It must not be reused for another context/range or later request.
    pub fn capture(
        manager: &ITfThreadMgr,
        context: &ITfContext,
        client: u32,
        policy: Rc<Cell<EditPolicy>>,
        request: RequestId,
        identity: u128,
        deadline: Instant,
    ) -> Result<Self> {
        if request.0 == 0 || identity == 0 {
            return Err(E_INVALIDARG.into());
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() || remaining > Duration::from_secs(60) {
            return Err(E_INVALIDARG.into());
        }
        if !policy.get().allowed {
            return Err(E_ACCESSDENIED.into());
        }
        let guard = Guard::new(manager, context)?;
        let snapshot = capture_selection(manager, context, client, policy)?;
        let output = Rc::new(RefCell::new(None));
        let result = output.clone();
        let manager = manager.clone();
        run(
            context,
            client,
            false,
            Box::new(move |cookie| {
                guard.check()?;
                let range = validate_snapshot(&manager, &snapshot, cookie)?;
                let target = fresh_target(&snapshot, &range, cookie, identity)?;
                validate_snapshot(&manager, &snapshot, cookie)?;
                guard.check()?;
                if Instant::now() >= deadline {
                    return Err(E_ABORT.into());
                }
                let mut core = ManualConversion::default();
                core.begin(request, target)
                    .map_err(|_| windows::core::Error::from(E_ABORT))?;
                *result.borrow_mut() = Some(Self {
                    manager,
                    client,
                    snapshot,
                    core,
                    request,
                    identity,
                    guard,
                    deadline,
                });
                Ok(())
            }),
        )?;
        let ticket = output.borrow_mut().take().ok_or(E_FAIL)?;
        Ok(ticket)
    }
    pub fn source(&self) -> Result<String> {
        self.guard.check()?;
        // A ticket can be held before transport submission. Recheck metadata
        // without rereading the body, even when a host omitted change notices.
        let manager = self.manager.clone();
        let context = self.snapshot.context.clone();
        let target = context.clone();
        let range = self.snapshot.range.clone();
        let expected = self.snapshot.protection;
        let observation = self.guard.state.clone();
        run(
            &context,
            self.client,
            false,
            Box::new(move |cookie| {
                ensure_target(&manager, &target)?;
                crate::protection::validate(&target, &range, cookie, expected)?;
                ensure_target(&manager, &target)?;
                if observation.invalid.get() {
                    return Err(E_ABORT.into());
                }
                Ok(())
            }),
        )?;
        self.guard.check()?;
        if self.snapshot.policy.get() != self.snapshot.captured_policy
            || Instant::now() >= self.deadline
        {
            return Err(E_ABORT.into());
        }
        String::from_utf16(&self.snapshot.original).map_err(|_| E_ABORT.into())
    }
    pub fn revision(&self) -> Revision {
        self.snapshot.captured_policy.revision
    }
    pub fn deadline(&self) -> Instant {
        self.deadline
    }
    pub fn request(&self) -> RequestId {
        self.request
    }
    /// A reply is applied only in this fresh, synchronous write session. The
    /// Core decision and SetText occur together, never in a later queued edit.
    pub fn finish(
        mut self,
        manager: &ITfThreadMgr,
        client: u32,
        request: RequestId,
        revision: Revision,
        replacement: String,
    ) -> Result<()> {
        self.guard.check()?;
        if request != self.request || revision != self.revision() || Instant::now() >= self.deadline
        {
            return Err(E_ABORT.into());
        }
        if replacement.is_empty()
            || replacement
                .encode_utf16()
                .take(MAX_SELECTION_UNITS + 1)
                .count()
                > MAX_SELECTION_UNITS
        {
            return Err(E_INVALIDARG.into());
        }
        let context = self.snapshot.context.clone();
        let manager = manager.clone();
        run(
            &context,
            client,
            true,
            Box::new(move |cookie| {
                self.guard.check()?;
                let range = validate_snapshot(&manager, &self.snapshot, cookie)?;
                let current = fresh_target(&self.snapshot, &range, cookie, self.identity)?;
                // Fresh reads may call external COM. Check the retained observation
                // and target again before the pure Core decision and immediate write.
                let range = validate_snapshot(&manager, &self.snapshot, cookie)?;
                self.guard.check()?;
                if Instant::now() >= self.deadline {
                    return Err(E_ABORT.into());
                }
                let text = self
                    .core
                    .finish(request, &current, &replacement)
                    .map_err(|_| windows::core::Error::from(E_ABORT))?;
                // SAFETY: unchanged original range, fresh write cookie, consumed Core
                // request. Own OnEndEdit may invalidate only after this single write.
                unsafe {
                    range.SetText(cookie, 0, &text)?;
                }
                Ok(())
            }),
        )
    }
}

fn fresh_target(
    snapshot: &SelectionSnapshot,
    range: &ITfRange,
    cookie: u32,
    identity: u128,
) -> Result<Target> {
    let range_acp: ITfRangeACP = range.cast()?;
    let mut start = 0i32;
    let mut length = 0i32;
    // SAFETY: ACP extent is queried only in the currently owning edit session.
    unsafe {
        range_acp.GetExtent(&mut start, &mut length)?;
    }
    let end = start.checked_add(length).ok_or(E_ABORT)?;
    if start < 0 || length < 0 {
        return Err(E_ABORT.into());
    }
    let policy = snapshot.policy.get();
    Ok(Target {
        context: identity,
        range: identity,
        selection: (start, end),
        text: read_all(range, cookie)?,
        revision: policy.revision,
        allowed: policy.allowed,
    })
}

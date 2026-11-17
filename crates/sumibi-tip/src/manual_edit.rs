//! Owner-thread Core gate, immediately followed by one guarded TSF write.
//! The caller must classify input scopes before supplying an allowed policy.
//! Uses generic retained TSF ranges; it never requires ITfRangeACP coordinates.
//! Compatibility has only been exercised in caller-owned diagnostic contexts.
//! No communication, COM pointer, edit cookie or target coordinate leaves here.
use super::*;
use std::rc::Weak;
use std::time::{Duration, Instant};
use sumibi_core::{ManualConversion, RequestId, Target};
use windows::core::{ComObject, Ref};

struct Observation {
    owner: ThreadId,
    invalid: Cell<Option<&'static str>>,
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
    fn invalidate(&self, protection_boundary: bool, reason: &'static str) -> Result<()> {
        boundary(|| {
            // Check before touching the apartment-local weak Rc.
            if self.owner != thread::current().id() {
                return Err(RPC_E_WRONG_THREAD.into());
            }
            if let Some(state) = self.state.upgrade() {
                if state.invalid.get().is_none() {
                    state.invalid.set(Some(reason));
                }
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
        self.invalidate(
            scope_changed,
            if scope_changed {
                "input_scope_changed"
            } else {
                "text_edit_notification"
            },
        )
    }
}
impl ITfThreadMgrEventSink_Impl for Observer_Impl {
    fn OnInitDocumentMgr(&self, _: Ref<ITfDocumentMgr>) -> Result<()> {
        self.invalidate(true, "document_initialized")
    }
    fn OnUninitDocumentMgr(&self, _: Ref<ITfDocumentMgr>) -> Result<()> {
        self.invalidate(true, "document_uninitialized")
    }
    fn OnSetFocus(&self, _: Ref<ITfDocumentMgr>, _: Ref<ITfDocumentMgr>) -> Result<()> {
        self.invalidate(true, "focus_changed")
    }
    fn OnPushContext(&self, _: Ref<ITfContext>) -> Result<()> {
        self.invalidate(true, "context_pushed")
    }
    fn OnPopContext(&self, _: Ref<ITfContext>) -> Result<()> {
        self.invalidate(true, "context_popped")
    }
}
impl ITfCompartmentEventSink_Impl for Observer_Impl {
    fn OnChange(&self, _: *const windows::core::GUID) -> Result<()> {
        self.invalidate(true, "compartment_changed")
    }
}
impl ITfStatusSink_Impl for Observer_Impl {
    fn OnStatusChange(&self, _: Ref<ITfContext>, _: u32) -> Result<()> {
        self.invalidate(true, "status_changed")
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
            invalid: Cell::new(None),
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
        if self.state.invalid.get().is_some() {
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
    pub fn boundary_generation(&self) -> Result<u64> {
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
    /// Permanently revoke this ticket when the cooperating owner observes a
    /// focus/input/lifecycle boundary. No COM call, edit session or body read.
    /// Idempotent on the capturing thread; restoring visible state cannot
    /// revive this request. Transport cancellation remains the owner's job.
    pub fn cancel(&mut self) -> Result<()> {
        if self.snapshot.owner != thread::current().id() {
            return Err(RPC_E_WRONG_THREAD.into());
        }
        self.guard.state.invalid.set(Some("cancelled"));
        self.core.cancel();
        self.snapshot.original = Vec::new();
        Ok(())
    }
    /// Call only for a diagnostic context whose text store the caller owns.
    /// The selection uses generic TSF ranges; non-ACP hosts remain unverified.
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
        Self::capture_with_run(
            manager, context, client, policy, request, identity, deadline, None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn capture_with_run(
        manager: &ITfThreadMgr,
        context: &ITfContext,
        client: u32,
        policy: Rc<Cell<EditPolicy>>,
        request: RequestId,
        identity: u128,
        deadline: Instant,
        expected_run: Option<Vec<u16>>,
    ) -> Result<Self> {
        Self::capture_target(
            manager,
            context,
            client,
            policy,
            request,
            identity,
            deadline,
            expected_run,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn capture_target(
        manager: &ITfThreadMgr,
        context: &ITfContext,
        client: u32,
        policy: Rc<Cell<EditPolicy>>,
        request: RequestId,
        identity: u128,
        deadline: Instant,
        expected_run: Option<Vec<u16>>,
        anchor: Option<ITfRange>,
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
        let snapshot =
            capture_conversion_target(manager, context, client, policy, expected_run, anchor)?;
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
        let result = self.checked_source();
        // Once this owner observes a bad selection/protection/lock/policy,
        // restoring the visible state is not permission to reuse the ticket.
        if result.is_err() {
            self.guard.state.invalid.set(Some("source_recheck_failed"));
        }
        result
    }
    fn checked_source(&self) -> Result<String> {
        // A ticket can be held before transport submission. Recheck metadata
        // without rereading the body, even when a host omitted change notices.
        let manager = self.manager.clone();
        let context = self.snapshot.context.clone();
        let target = context.clone();
        let selected = self.snapshot.selection_range.clone();
        let range = self.snapshot.range.clone();
        let style = self.snapshot.style;
        let expected = self.snapshot.protection;
        let profile = self.snapshot.captured_policy.profile;
        let observation = self.guard.state.clone();
        run(
            &context,
            self.client,
            false,
            Box::new(move |cookie| {
                ensure_target(&manager, &target)?;
                checked_selection(&target, &selected, style, cookie)?;
                crate::protection::validate_with_profile(
                    &target, &range, cookie, expected, profile,
                )?;
                ensure_target(&manager, &target)?;
                if observation.invalid.get().is_some() {
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
    pub(crate) fn invalidation_reason(&self) -> Option<&'static str> {
        self.guard.state.invalid.get()
    }

    pub(crate) fn is_current(&self) -> bool {
        Instant::now() < self.deadline && self.guard.check().is_ok()
    }

    pub(crate) fn candidate_bounds(&self) -> Result<windows::Win32::Foundation::RECT> {
        let bounds = Rc::new(Cell::new(Default::default()));
        let output = bounds.clone();
        let context = self.snapshot.context.clone();
        let manager = self.manager.clone();
        let snapshot = self.snapshot.clone();
        run(
            &context,
            self.client,
            false,
            Box::new(move |cookie| {
                let range = validate_snapshot(&manager, &snapshot, cookie)?;
                let view = unsafe { snapshot.context.GetActiveView()? };
                let mut rect = Default::default();
                unsafe { view.GetTextExt(cookie, &range, &mut rect, &mut windows::core::BOOL(0))? };
                output.set(rect);
                Ok(())
            }),
        )?;
        Ok(bounds.get())
    }

    pub(crate) fn context(&self) -> &ITfContext {
        &self.snapshot.context
    }

    /// A reply is applied only in this fresh, synchronous write session. The
    /// Core decision and SetText occur together, never in a later queued edit.
    pub fn finish(
        self,
        manager: &ITfThreadMgr,
        client: u32,
        request: RequestId,
        revision: Revision,
        replacement: String,
    ) -> Result<()> {
        self.finish_inner(manager, client, request, revision, replacement, None)
            .map(|_| ())
    }

    pub(crate) fn finish_retaining(
        self,
        manager: &ITfThreadMgr,
        client: u32,
        request: RequestId,
        revision: Revision,
        replacement: String,
        still_allowed: impl Fn() -> bool + 'static,
    ) -> Result<Option<ITfRange>> {
        self.finish_inner(
            manager,
            client,
            request,
            revision,
            replacement,
            Some(Box::new(still_allowed)),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn finish_inner(
        mut self,
        manager: &ITfThreadMgr,
        client: u32,
        request: RequestId,
        revision: Revision,
        replacement: String,
        still_allowed: Option<Box<dyn Fn() -> bool>>,
    ) -> Result<Option<ITfRange>> {
        self.guard.check()?;
        if client != self.client
            || request != self.request
            || revision != self.revision()
            || Instant::now() >= self.deadline
        {
            return Err(E_ABORT.into());
        }
        if replacement.is_empty()
            || replacement.contains('\u{fffc}')
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
        let anchor = Rc::new(RefCell::new(None));
        let output = anchor.clone();
        run(
            &context,
            client,
            true,
            Box::new(move |cookie| {
                self.guard.check()?;
                let range = validate_snapshot(&manager, &self.snapshot, cookie)?;
                let current = fresh_target(&self.snapshot, &range, cookie, self.identity)?;
                let start = if still_allowed.is_some() {
                    unsafe {
                        range
                            .Clone()
                            .and_then(|start| {
                                start.Collapse(cookie, TF_ANCHOR_START)?;
                                start.SetGravity(
                                    cookie,
                                    TF_GRAVITY_BACKWARD,
                                    TF_GRAVITY_BACKWARD,
                                )?;
                                Ok(start)
                            })
                            .ok()
                    }
                } else {
                    None
                };
                // Fresh reads may call external COM. Check the retained observation
                // and target again before the pure Core decision and immediate write.
                let range = validate_snapshot(&manager, &self.snapshot, cookie)?;
                let restore_trailing_caret = still_allowed.is_some()
                    && unsafe {
                        self.snapshot
                            .selection_range
                            .IsEmpty(cookie)
                            .is_ok_and(|empty| empty.as_bool())
                            && self
                                .snapshot
                                .selection_range
                                .IsEqualStart(cookie, &range, TF_ANCHOR_END)
                                .is_ok_and(|equal| equal.as_bool())
                    };
                self.guard.check()?;
                if Instant::now() >= self.deadline
                    || still_allowed.as_ref().is_some_and(|allowed| !allowed())
                {
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
                    if let Some(start) = start {
                        let count = text.len() as i32;
                        let mut shifted = 0;
                        if start
                            .ShiftEnd(cookie, count, &mut shifted, std::ptr::null())
                            .is_ok()
                            && shifted == count
                        {
                            if restore_trailing_caret {
                                let _ = restore_caret_after_write(
                                    &manager,
                                    &self.snapshot,
                                    cookie,
                                    &start,
                                    &text,
                                );
                            }
                            *output.borrow_mut() = Some(start);
                        }
                    }
                }
                Ok(())
            }),
        )?;
        let retained = anchor.borrow_mut().take();
        Ok(retained)
    }
}

fn restore_caret_after_write(
    manager: &ITfThreadMgr,
    snapshot: &SelectionSnapshot,
    cookie: u32,
    replacement: &ITfRange,
    expected: &[u16],
) -> Result<()> {
    ensure_target(manager, &snapshot.context)?;
    let (caret, style) = selection(&snapshot.context, cookie)?;
    // Leave applications that already placed the caret after the text alone.
    // Never move a new selection or a caret outside the converted range.
    if !unsafe {
        caret.IsEmpty(cookie)?.as_bool()
            && caret
                .IsEqualStart(cookie, replacement, TF_ANCHOR_START)?
                .as_bool()
    } || style.fInterimChar.as_bool()
    {
        return Ok(());
    }
    crate::protection::check_with_profile(
        &snapshot.context,
        replacement,
        cookie,
        snapshot.captured_policy.profile,
    )?;
    if read_all_with_profile(replacement, cookie, snapshot.captured_policy.profile)? != expected {
        return Ok(());
    }
    let end = unsafe { replacement.Clone()? };
    unsafe { end.Collapse(cookie, TF_ANCHOR_END)? };
    checked_selection(&snapshot.context, &caret, style, cookie)?;
    ensure_target(manager, &snapshot.context)?;
    let chosen = RawSelection(TF_SELECTION {
        range: ManuallyDrop::new(Some(end)),
        style: TF_SELECTIONSTYLE {
            ase: TF_AE_NONE,
            fInterimChar: false.into(),
        },
    });
    // The replacement has already succeeded. A text store may decline this
    // selection-only update without invalidating that completed write.
    unsafe {
        snapshot
            .context
            .SetSelection(cookie, std::slice::from_ref(&chosen.0))
    }
}

fn fresh_target(
    snapshot: &SelectionSnapshot,
    range: &ITfRange,
    cookie: u32,
    identity: u128,
) -> Result<Target> {
    // validate_snapshot already checked native endpoints, direction, context,
    // source and protection. Keep the native range local; Core needs only this
    // explicit retained-range stamp, never invented document coordinates.
    let text = read_all_with_profile(range, cookie, snapshot.captured_policy.profile)?;
    let policy = snapshot.policy.get();
    Ok(Target {
        context: identity,
        range: identity,
        selection: sumibi_core::Selection::RetainedRange {
            utf16_units: text.len(),
        },
        text,
        revision: policy.revision,
        allowed: policy.allowed,
    })
}

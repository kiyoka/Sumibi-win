//! Diagnostic owner-STA adapter. Semantic admission also serves waiting-demo;
//! worker I/O and recovery UI actions stay outside the window callback.
//! Each admitted event holds one existing EXE-wide recovery slot before input
//! could be consumed. No worker receives this object or any of its COM state.
use super::*;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use sumibi_core::recovery::{Metadata, Operation, Outcome, Recovery, Ticket};
use sumibi_core::waiting_input::{self as core, Boundary, Input, Target, WaitingInput};
use sumibi_core::RequestId;
use sumibi_tip::edit::{manual::InputWatch, InputMetadata};
use sumibi_tip::spike::{INPUT_APPLIED, INPUT_APPLIED_UNTRACKED, INPUT_NOT_ATTEMPTED};

#[path = "input_waiting_tests.rs"]
mod tests;
pub(crate) use tests::exercise_waiting;
pub(super) use tests::test_waiting;
#[path = "input_waiting_owner.rs"]
mod owner;
pub(super) use owner::test_waiting_demo;
pub(super) use owner::Owner;
pub(crate) use owner::{exercise_waiting_demo, run_waiting_demo};

static NEXT_SESSION: AtomicU64 = AtomicU64::new(0);
struct Slot {
    recovery: Recovery,
    ticket: Ticket,
}
struct Waiting {
    manager: ITfThreadMgr,
    context: ITfContext,
    client: u32,
    watch: InputWatch,
    notification: u64,
    boundary: u64,
    metadata: InputMetadata,
    store: ComObject<TextStore>,
    inspector: sumibi_tip::spike::ISpikeInputState,
    broker: ISpikeBroker,
    native_owned: bool,
    target: Target,
    request: u64,
    queue: Option<WaitingInput>,
    slots: VecDeque<Slot>,
    ready: Vec<Slot>,
    // Diagnostic only; production waiting/expiry policy remains undecided.
    deadline: Instant,
}
impl Waiting {
    fn begin(manager: &ITfThreadMgr, editor: &Editor, client: u32, request: u64) -> Result<Self> {
        let (_, bridge) = editor.broker.as_ref().ok_or(E_FAIL)?;
        if request == 0 || bridge.get().pending_id() != Some(request) {
            return Err(E_FAIL.into());
        }
        let watch = InputWatch::attach(manager, &editor.context)?;
        let notification = watch.generation()?;
        let boundary = watch.boundary_generation()?;
        let metadata = sumibi_tip::edit::inspect_input(manager, &editor.context, client)?;
        if watch.generation()? != notification || watch.boundary_generation()? != boundary {
            return Err(E_FAIL.into());
        }
        let identity = NEXT_SESSION
            .try_update(Ordering::AcqRel, Ordering::Acquire, |v| {
                v.checked_add(1).filter(|v| *v != u64::MAX)
            })
            .map_err(|_| E_FAIL)?
            + 1;
        let target = Target {
            context: identity as u128,
            target: identity as u128,
            generation: 0,
            settings: 1,
            authorization: boundary,
            allowed: true,
        };
        let mut slots = VecDeque::new();
        slots
            .try_reserve_exact(sumibi_core::recovery::MAX_RECORDS)
            .map_err(|_| E_FAIL)?;
        let mut ready = Vec::new();
        ready
            .try_reserve_exact(sumibi_core::recovery::MAX_RECORDS)
            .map_err(|_| E_FAIL)?;
        Ok(Self {
            manager: manager.clone(),
            context: editor.context.clone(),
            client,
            watch,
            notification,
            boundary,
            metadata,
            store: editor.store.clone(),
            inspector: editor.composing.as_ref().ok_or(E_FAIL)?.state_probe()?,
            broker: editor.broker.as_ref().ok_or(E_FAIL)?.0.clone(),
            native_owned: false,
            target,
            request,
            queue: Some(WaitingInput::new(RequestId(request as u128), target).map_err(|_| E_FAIL)?),
            slots,
            ready,
            deadline: Instant::now() + Duration::from_secs(8),
        })
    }
    fn inspect(&self) -> Result<InputMetadata> {
        if self.watch.boundary_generation()? != self.boundary {
            return Err(E_FAIL.into());
        }
        let generation = self.watch.generation()?;
        let metadata = sumibi_tip::edit::inspect_input(&self.manager, &self.context, self.client)
            .or_else(|error| {
            if !self.native_owned {
                return Err(error);
            }
            let selected = self.store.get().selection.get();
            if selected.style.ase != TS_AE_START && selected.style.ase != TS_AE_END {
                return Err(E_FAIL.into());
            }
            unsafe {
                self.inspector
                    .Validate(
                        self.context.as_raw(),
                        selected.acpStart,
                        selected.acpEnd,
                        u32::from(selected.style.ase == TS_AE_START),
                    )
                    .ok()?;
            }
            let mut metadata = self.metadata;
            metadata.selection = (selected.acpStart, selected.acpEnd);
            metadata.active_start = selected.style.ase == TS_AE_START;
            Ok(metadata)
        })?;
        if self.watch.generation()? != generation
            || self.watch.boundary_generation()? != self.boundary
            || !metadata.same_protection(&self.metadata)
        {
            return Err(E_FAIL.into());
        }
        Ok(metadata)
    }
    fn stable(&self) -> bool {
        self.watch.generation().ok() == Some(self.notification)
            && self
                .inspect()
                .is_ok_and(|m| m.same_selection(&self.metadata))
            && self.watch.generation().ok() == Some(self.notification)
    }
    // Diagnostic host only: F7 leaves the source selected while the worker is
    // pending. An unadmitted ordinary character must not replace that source.
    fn fallback_caret_end(&self) -> Option<usize> {
        if self.native_owned || !self.stable() {
            return None;
        }
        let (start, end) = self.metadata.selection;
        (start < end).then(|| usize::try_from(end).ok()).flatten()
    }
    fn refresh(&mut self) -> Result<()> {
        let notification = self.watch.generation()?;
        let metadata = self.inspect()?;
        if self.watch.generation()? != notification {
            return Err(E_FAIL.into());
        }
        self.notification = notification;
        self.metadata = metadata;
        Ok(())
    }
    fn settled_conversion(&mut self, editor: &Editor) -> Result<()> {
        self.refresh()?;
        // The existing manual edit keeps its replacement selected. This
        // cooperating store places its own caret at the confirmed result end
        // once, before draining text; never replace that result with the tail.
        let end = usize::try_from(self.metadata.selection.1).map_err(|_| E_FAIL)?;
        editor.store.get().host_selection(end, end)?;
        self.refresh()?;
        self.target.generation = self.target.generation.checked_add(1).ok_or(E_FAIL)?;
        self.queue
            .as_mut()
            .ok_or(E_FAIL)?
            .settle(RequestId(self.request as u128), self.target, true)
            .map_err(|_| E_FAIL.into())
    }
    fn admit(&mut self, editor: &Editor, input: Input<'_>) -> Result<()> {
        if Instant::now() >= self.deadline {
            self.stop(editor, Boundary::Cancelled, Outcome::Deferred);
            return Err(E_FAIL.into());
        }
        if !self.stable() {
            self.stop(editor, Boundary::ProtectionChanged, Outcome::Deferred);
            return Err(E_FAIL.into());
        }
        let operation = match &input {
            Input::Text(text) if !text.contains(&0xfffc) => Operation::Insert(text.len()),
            Input::Backspace(count) => Operation::Backspace(*count),
            Input::Enter => Operation::DeferredEnter,
            Input::Convert => Operation::DeferredConvert,
            _ => return Err(E_INVALIDARG.into()),
        };
        if self.slots.len() + self.ready.len() == sumibi_core::recovery::MAX_RECORDS {
            return Err(E_FAIL.into());
        }
        let mut recovery = Recovery::with_budget(input_recovery_host::shared_budget());
        let ticket = recovery
            .reserve(operation, Instant::now())
            .map_err(|_| E_FAIL)?;
        self.queue
            .as_mut()
            .ok_or(E_FAIL)?
            .admit(self.target, input)
            .map_err(|_| E_INVALIDARG)?;
        self.slots.push_back(Slot { recovery, ticket });
        Ok(())
    }
    fn stop(&mut self, editor: &Editor, reason: Boundary, head: Outcome) {
        if let Some((broker, _)) = &editor.broker {
            unsafe {
                let _ = broker.Cancel(self.request);
            }
        }
        let permitted = self.inspect().is_ok();
        if let Some(mut queue) = self.queue.take() {
            queue.retire(reason);
            for (index, slot) in self.slots.iter_mut().enumerate() {
                let text = match queue.retained(index) {
                    Some(Input::Text(text)) if permitted => text,
                    _ => &[],
                };
                slot.recovery.finish(
                    slot.ticket,
                    if index == 0 { head } else { Outcome::Deferred },
                    text,
                    Instant::now(),
                );
            }
            // All recovery buffers were reserved before admit. Destroy the
            // queue payload before any of its capacity can later be returned.
            drop(queue);
        }
        self.ready.extend(self.slots.drain(..));
        if !permitted {
            self.redact();
        }
    }
    fn redact(&mut self) {
        for slot in &mut self.ready {
            slot.recovery.redact(slot.ticket);
        }
    }
    /// Owner-loop only. No broker wait, window callback, or IPC runs in admit.
    fn service(&mut self, editor: &Editor) -> Result<bool> {
        if self.queue.is_none() {
            return Ok(false);
        }
        if Instant::now() >= self.deadline {
            self.stop(editor, Boundary::Cancelled, Outcome::Deferred);
            return Ok(false);
        }
        if !self.stable() {
            self.stop(editor, Boundary::ProtectionChanged, Outcome::Deferred);
            return Ok(false);
        }
        let (broker, bridge) = editor.broker.as_ref().ok_or(E_FAIL)?;
        if bridge.get().active() {
            let Some(delivery) = bridge.get().drain(broker)? else {
                return Ok(false);
            };
            if delivery != crate::broker_bridge::Delivery::Applied
                || self.settled_conversion(editor).is_err()
            {
                self.stop(editor, Boundary::Cancelled, Outcome::Deferred);
                return Ok(false);
            }
        }
        let ticket = match self.queue.as_mut().ok_or(E_FAIL)?.start_next(self.target) {
            Ok(Some(ticket)) => ticket,
            Ok(None) => {
                self.queue = None;
                return Ok(false);
            }
            Err(core::Reject::ManualEnter) => {
                self.stop(editor, Boundary::ManualEnter, Outcome::Deferred);
                return Ok(false);
            }
            Err(_) => {
                self.stop(editor, Boundary::Cancelled, Outcome::Deferred);
                return Ok(false);
            }
        };
        if matches!(
            self.queue.as_ref().and_then(|q| q.delivery(ticket)),
            Some(Input::Convert)
        ) {
            // Semantic F7 operation through the existing diagnostic helper;
            // never PostMessage/SendInput or a replayed native key event.
            if editor.input(WM_KEYDOWN, WPARAM(0x76), LPARAM(1)).is_err() {
                self.stop(editor, Boundary::Cancelled, Outcome::Uncertain);
                return Ok(false);
            }
            let Some(request) = bridge.get().pending_id() else {
                self.queue
                    .as_mut()
                    .ok_or(E_FAIL)?
                    .finish(ticket, core::Outcome::NotAttempted)
                    .map_err(|_| E_FAIL)?;
                self.stop(editor, Boundary::Cancelled, Outcome::Deferred);
                return Ok(false);
            };
            self.request = request;
            if self.refresh().is_err() {
                self.stop(editor, Boundary::ProtectionChanged, Outcome::Uncertain);
                return Ok(false);
            }
            self.queue
                .as_mut()
                .ok_or(E_FAIL)?
                .finish(
                    ticket,
                    core::Outcome::ConversionStarted(RequestId(request as u128)),
                )
                .map_err(|_| E_FAIL)?;
            self.release_head();
            return Ok(true);
        }
        let composing = editor.composing.as_ref().ok_or(E_FAIL)?;
        let disposition = match self.queue.as_ref().and_then(|q| q.delivery(ticket)) {
            Some(Input::Text(text)) => composing.insert(editor, text),
            Some(Input::Backspace(count)) => composing.backspace(editor, count),
            _ => Err(E_FAIL.into()),
        }
        .unwrap_or(sumibi_tip::spike::INPUT_UNCERTAIN);
        editor.handled_transaction(disposition);
        if disposition == INPUT_APPLIED {
            self.native_owned = true;
        }
        if disposition == INPUT_APPLIED && self.refresh().is_ok() {
            self.target.generation = self.target.generation.checked_add(1).ok_or(E_FAIL)?;
            self.queue
                .as_mut()
                .ok_or(E_FAIL)?
                .finish(ticket, core::Outcome::Applied(self.target))
                .map_err(|_| E_FAIL)?;
            self.release_head();
            return Ok(true);
        }
        // Never fall back to the host for input that was already admitted.
        let outcome = match disposition {
            INPUT_NOT_ATTEMPTED => Outcome::Deferred,
            INPUT_APPLIED | INPUT_APPLIED_UNTRACKED => Outcome::AppliedUntracked,
            _ => Outcome::Uncertain,
        };
        self.stop(
            editor,
            if outcome == Outcome::Uncertain {
                Boundary::Uncertain
            } else {
                Boundary::Cancelled
            },
            outcome,
        );
        Ok(false)
    }
    fn release_head(&mut self) {
        if let Some(mut slot) = self.slots.pop_front() {
            slot.recovery
                .finish(slot.ticket, Outcome::Applied, &[], Instant::now());
        }
    }
    fn metadata(&self, index: usize, now: Instant) -> Option<Metadata> {
        self.ready.get(index)?.recovery.metadata(now)
    }
    fn copy(
        &mut self,
        editor: &Editor,
        index: usize,
        clipboard: &dyn crate::recovery_view::Clipboard,
    ) -> Result<()> {
        self.copy_with_clock(editor, index, clipboard, &Instant::now)
    }
    fn copy_with_clock(
        &mut self,
        editor: &Editor,
        index: usize,
        clipboard: &dyn crate::recovery_view::Clipboard,
        now: &dyn Fn() -> Instant,
    ) -> Result<()> {
        if self.inspect().is_err() {
            self.redact();
            return Err(E_FAIL.into());
        }
        let slot = self.ready.get(index).ok_or(E_FAIL)?;
        let text = slot.recovery.text(slot.ticket, now()).ok_or(E_FAIL)?;
        let copied = clipboard.write(editor.store.get().window.get(), text, &|| {
            self.inspect().is_ok() && slot.recovery.text(slot.ticket, now()).is_some()
        });
        if self.inspect().is_err() {
            self.redact();
            return Err(E_FAIL.into());
        }
        copied?;
        if self.ready[index]
            .recovery
            .text(self.ready[index].ticket, now())
            .is_none()
        {
            return Err(E_FAIL.into());
        }
        // Explicit copy acknowledges this record only; never resume the queue.
        let ticket = self.ready[index].ticket;
        self.ready[index].recovery.discard(ticket);
        Ok(())
    }
    fn expire(&mut self, now: Instant) {
        for slot in &mut self.ready {
            slot.recovery.expire(now);
        }
    }
}
impl Drop for Waiting {
    fn drop(&mut self) {
        // Retire text before external COM can reenter. Cancel only this
        // session's last request, never a newer unrelated conversion.
        drop(self.queue.take());
        unsafe {
            let _ = self.broker.Cancel(self.request);
        }
    }
}

//! Experimental, bounded manual-conversion input ordering. Adapters must store
//! an event successfully BEFORE consuming its native input. No keys are replayed,
//! no document is edited, and no production keyboard routing enables this yet.
use crate::RequestId;
use std::collections::VecDeque;

pub const MAX_EVENTS: usize = 1024;
pub const MAX_TEXT_UNITS: usize = 16384;
pub const MAX_EVENT_UNITS: usize = 1024;

/// A cooperating adapter's logical input target, not a native range or cookie.
/// `target` must change on focus/document/selection boundaries, even if restored.
/// `generation` advances exactly once after each confirmed body transaction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
    pub context: u128,
    pub target: u128,
    pub generation: u64,
    pub settings: u64,
    pub authorization: u64,
    /// Fresh affirmative protection/ownership check supplied by the adapter.
    pub allowed: bool,
}
impl Target {
    fn valid(self) -> bool {
        self.allowed
            && self.context != 0
            && self.target != 0
            && [self.generation, self.settings, self.authorization]
                .iter()
                .all(|value| *value != u64::MAX)
    }
    fn follows(self, before: Self) -> bool {
        self.valid()
            && before.generation.checked_add(1) == Some(self.generation)
            && Self {
                generation: before.generation,
                ..self
            } == before
    }
}

/// Decoded text or semantic commands only. No virtual keys, native handles,
/// keyboard state, IME messages, or callbacks can enter this queue.
pub enum Input<'a> {
    Text(&'a [u16]),
    Backspace(u32),
    Enter,
    Convert,
}
enum Stored {
    Text(Vec<u16>),
    Backspace(u32),
    Enter,
    Convert,
}
impl Stored {
    fn view(&self) -> Input<'_> {
        match self {
            Self::Text(text) => Input::Text(text),
            Self::Backspace(count) => Input::Backspace(*count),
            Self::Enter => Input::Enter,
            Self::Convert => Input::Convert,
        }
    }
    fn units(&self) -> usize {
        match self {
            Self::Text(text) => text.len(),
            _ => 0,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ticket {
    session: RequestId,
    target: Target,
    sequence: u64,
}
struct Entry {
    ticket: Ticket,
    input: Stored,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reject {
    Invalid,
    Capacity,
    Allocation,
    State,
    TargetChanged,
    Ticket,
    ManualEnter,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Boundary {
    TargetChanged,
    FocusLost,
    ProtectionChanged,
    Cancelled,
    ManualEnter,
    Uncertain,
    AppliedUntracked,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Waiting(RequestId),
    Ready,
    Delivering(Ticket),
    Retired(Boundary),
}
/// A native adapter reports the result once. An attempted but unconfirmed
/// operation must NEVER be reported as NotAttempted or replayed by the host.
pub enum Outcome {
    NotAttempted,
    Applied(Target),
    AppliedUntracked,
    Uncertain,
    ConversionStarted(RequestId),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Recovery {
    pub reason: Boundary,
    pub events: usize,
    pub text_units: usize,
    /// Only the head may have been applied. Tail events were never dispatched.
    pub uncertain_head: bool,
    pub redacted: bool,
}
/// Text-bearing types intentionally omit Debug. No timer, persistence, worker,
/// automatic recovery/copy, or cross-owner/process budget is provided here.
pub struct WaitingInput {
    target: Target,
    phase: Phase,
    last_request: RequestId,
    sequence: u64,
    entries: VecDeque<Entry>,
    units: usize,
    uncertain_head: bool,
    redacted: bool,
}
impl WaitingInput {
    /// The adapter supplies globally unique request IDs; within this queue,
    /// later conversion IDs must also increase. IDs must not be reused after
    /// dropping/recreating a queue with the same target.
    pub fn new(request: RequestId, target: Target) -> Result<Self, Reject> {
        if request.0 == 0 || request.0 == u128::MAX || !target.valid() {
            return Err(Reject::Invalid);
        }
        Ok(Self {
            target,
            phase: Phase::Waiting(request),
            last_request: request,
            sequence: 0,
            entries: VecDeque::new(),
            units: 0,
            uncertain_head: false,
            redacted: false,
        })
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub fn text_units(&self) -> usize {
        self.units
    }
    fn check_target(&mut self, current: Target) -> Result<(), Reject> {
        if current != self.target || !current.valid() {
            self.changed_target(current);
            return Err(Reject::TargetChanged);
        }
        Ok(())
    }
    fn changed_target(&mut self, current: Target) {
        let reason = if !current.valid()
            || current.settings != self.target.settings
            || current.authorization != self.target.authorization
        {
            Boundary::ProtectionChanged
        } else if current.context != self.target.context || current.target != self.target.target {
            Boundary::FocusLost
        } else {
            Boundary::TargetChanged
        };
        self.retire(reason);
    }
    /// A success is the only receipt permitting the adapter to consume input.
    /// This admission receipt cannot acknowledge a later delivery attempt.
    /// Allocation/validation/capacity failure leaves all admitted events intact.
    pub fn admit(&mut self, current: Target, input: Input<'_>) -> Result<Ticket, Reject> {
        if !matches!(self.phase, Phase::Waiting(_) | Phase::Ready) {
            return Err(Reject::State);
        }
        self.check_target(current)?;
        let units = match &input {
            Input::Text(text)
                if !text.is_empty()
                    && text.len() <= MAX_EVENT_UNITS
                    && char::decode_utf16(text.iter().copied())
                        .all(|value| value.is_ok_and(|value| !value.is_control())) =>
            {
                text.len()
            }
            Input::Backspace(count) if (1..=MAX_EVENT_UNITS as u32).contains(count) => 0,
            Input::Enter | Input::Convert => 0,
            _ => return Err(Reject::Invalid),
        };
        if self.entries.len() == MAX_EVENTS || self.units + units > MAX_TEXT_UNITS {
            return Err(Reject::Capacity);
        }
        let sequence = self
            .sequence
            .checked_add(1)
            .filter(|value| *value != u64::MAX)
            .ok_or(Reject::Capacity)?;
        let stored = match input {
            Input::Text(text) => {
                let mut copy = Vec::new();
                copy.try_reserve_exact(text.len())
                    .map_err(|_| Reject::Allocation)?;
                copy.extend_from_slice(text);
                Stored::Text(copy)
            }
            Input::Backspace(count) => Stored::Backspace(count),
            Input::Enter => Stored::Enter,
            Input::Convert => Stored::Convert,
        };
        self.entries
            .try_reserve_exact(1)
            .map_err(|_| Reject::Allocation)?;
        let ticket = Ticket {
            session: self.last_request,
            target: self.target,
            sequence,
        };
        self.entries.push_back(Entry {
            ticket,
            input: stored,
        });
        self.sequence = sequence;
        self.units += units;
        Ok(ticket)
    }
    /// The adapter validates the native conversion independently. An applied
    /// result advances only the body generation, never the logical input target.
    /// A stale completion cannot cancel a newer request or drain its input.
    pub fn settle(
        &mut self,
        request: RequestId,
        current: Target,
        applied: bool,
    ) -> Result<(), Reject> {
        if self.phase != Phase::Waiting(request) {
            return Err(Reject::State);
        }
        if !(if applied {
            current.follows(self.target)
        } else {
            current == self.target
        }) {
            self.changed_target(current);
            return Err(Reject::TargetChanged);
        }
        self.target = current;
        self.phase = Phase::Ready;
        Ok(())
    }
    /// Freeze the head before an adapter attempts an operation. Enter is kept
    /// for an explicit user decision; it can never become an automatic delivery.
    pub fn start_next(&mut self, current: Target) -> Result<Option<Ticket>, Reject> {
        if self.phase != Phase::Ready {
            return Err(Reject::State);
        }
        self.check_target(current)?;
        let Some(entry) = self.entries.front() else {
            return Ok(None);
        };
        if matches!(entry.input, Stored::Enter) {
            self.retire(Boundary::ManualEnter);
            return Err(Reject::ManualEnter);
        }
        // Each attempt has a fresh ticket, including after NotAttempted. A
        // delayed acknowledgement from an earlier attempt cannot finish it.
        let sequence = self
            .sequence
            .checked_add(1)
            .filter(|value| *value != u64::MAX)
            .ok_or(Reject::Capacity)?;
        let ticket = Ticket {
            session: self.last_request,
            target: self.target,
            sequence,
        };
        self.entries.front_mut().ok_or(Reject::State)?.ticket = ticket;
        self.sequence = sequence;
        self.phase = Phase::Delivering(ticket);
        Ok(Some(ticket))
    }
    /// Borrow only; no payload is transferred out or implicitly removed.
    pub fn delivery(&self, ticket: Ticket) -> Option<Input<'_>> {
        (self.phase == Phase::Delivering(ticket))
            .then(|| self.entries.front().filter(|entry| entry.ticket == ticket))
            .flatten()
            .map(|entry| entry.input.view())
    }
    pub fn finish(&mut self, ticket: Ticket, outcome: Outcome) -> Result<(), Reject> {
        if self.phase != Phase::Delivering(ticket) {
            return Err(Reject::Ticket);
        }
        let conversion = matches!(
            self.entries.front().map(|entry| &entry.input),
            Some(Stored::Convert)
        );
        match outcome {
            Outcome::NotAttempted => self.phase = Phase::Ready,
            Outcome::Applied(current) if !conversion && current.follows(self.target) => {
                self.remove_head();
                self.target = current;
                self.phase = Phase::Ready;
            }
            Outcome::Applied(current) => {
                self.retire(Boundary::Uncertain);
                if !current.valid()
                    || current.settings != self.target.settings
                    || current.authorization != self.target.authorization
                    || current.context != self.target.context
                    || current.target != self.target.target
                {
                    self.changed_target(current);
                }
                return Err(Reject::Invalid);
            }
            Outcome::ConversionStarted(request)
                if conversion && request.0 > self.last_request.0 && request.0 != u128::MAX =>
            {
                self.remove_head();
                self.last_request = request;
                self.phase = Phase::Waiting(request);
            }
            Outcome::AppliedUntracked if !conversion => {
                self.remove_head();
                self.phase = Phase::Ready;
                self.retire(Boundary::AppliedUntracked);
            }
            Outcome::Uncertain => self.retire(Boundary::Uncertain),
            _ => {
                // A malformed completion is not proof that nothing happened.
                self.retire(Boundary::Uncertain);
                return Err(Reject::Invalid);
            }
        }
        Ok(())
    }
    fn remove_head(&mut self) {
        if let Some(entry) = self.entries.pop_front() {
            self.units -= entry.input.units();
        }
    }
    /// Retirement is irreversible. Protection/focus loss also drops payload;
    /// dropping vectors is not a guarantee of physical memory zeroization.
    pub fn retire(&mut self, reason: Boundary) {
        self.uncertain_head |= matches!(self.phase, Phase::Delivering(_));
        if !matches!(self.phase, Phase::Retired(_)) {
            self.phase = Phase::Retired(reason);
        }
        if matches!(reason, Boundary::FocusLost | Boundary::ProtectionChanged) {
            self.entries.clear();
            self.units = 0;
            self.redacted = true;
            self.uncertain_head = false;
        }
    }
    pub fn recovery(&self) -> Option<Recovery> {
        let Phase::Retired(reason) = self.phase else {
            return None;
        };
        Some(Recovery {
            reason,
            events: self.len(),
            text_units: self.units,
            uncertain_head: self.uncertain_head,
            redacted: self.redacted,
        })
    }
    /// Explicit recovery inspection only. The adapter must revalidate its
    /// protection history before any display/copy; this API authorizes no edit.
    pub fn retained(&self, index: usize) -> Option<Input<'_>> {
        self.recovery()?;
        self.entries.get(index).map(|entry| entry.input.view())
    }
}

#[cfg(test)]
#[path = "waiting_input_tests.rs"]
mod tests;

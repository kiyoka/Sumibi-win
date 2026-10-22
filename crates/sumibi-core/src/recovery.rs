//! One bounded, memory-only rescue slot. Never authorizes document editing.
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
pub const MAX_UNITS: usize = 1024;
pub const MAX_RECORDS: usize = 4;
pub const RETENTION: Duration = Duration::from_secs(600);
/// Share one budget across every owner in an EXE. Contains no text or callbacks.
/// A standalone default Recovery has its own budget for library use.
#[derive(Clone, Default)]
pub struct Budget {
    used: Arc<AtomicUsize>,
}
impl Budget {
    fn reserve(&self) -> Result<Permit, ReserveError> {
        let mut count = self.used.load(Ordering::Acquire);
        loop {
            if count >= MAX_RECORDS {
                return Err(ReserveError::Busy);
            }
            match self.used.compare_exchange_weak(
                count,
                count + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Ok(Permit(self.clone())),
                Err(actual) => count = actual,
            }
        }
    }
}
struct Permit(Budget);
impl Drop for Permit {
    fn drop(&mut self) {
        self.0.used.fetch_sub(1, Ordering::AcqRel);
    }
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Ticket(u64);
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Operation {
    Insert(usize),
    Backspace(u32),
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Outcome {
    NotAttempted,
    Applied,
    AppliedUntracked,
    Uncertain,
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ReserveError {
    Busy,
    Invalid,
    Allocation,
    Exhausted,
}
#[derive(Clone, Copy)]
pub struct Metadata {
    pub ticket: Ticket,
    pub operation: Operation,
    pub outcome: Outcome,
    pub remaining: Duration,
    pub copyable: bool,
}
struct Record {
    ticket: Ticket,
    operation: Operation,
    text: Vec<u16>,
    ready: Option<(Outcome, Instant)>,
    // Drop text before releasing capacity. Neither Record nor Permit is Clone.
    _permit: Permit,
}
/// Text-bearing state intentionally omits Debug/Clone and has no serialization.
pub struct Recovery {
    record: Option<Record>,
    sequence: u64,
    retention: Duration,
    budget: Budget,
}
impl Default for Recovery {
    fn default() -> Self {
        Self::with_budget(Budget::default())
    }
}
impl Recovery {
    pub fn with_budget(budget: Budget) -> Self {
        Self {
            record: None,
            sequence: 0,
            retention: RETENTION,
            budget,
        }
    }
    /// A shorter lifetime is useful for owner-loop tests; never permit >10 min.
    pub fn with_retention(retention: Duration) -> Option<Self> {
        Self::with_retention_and_budget(retention, Budget::default())
    }
    pub fn with_retention_and_budget(retention: Duration, budget: Budget) -> Option<Self> {
        (!retention.is_zero() && retention <= RETENTION).then_some(Self {
            retention,
            ..Self::with_budget(budget)
        })
    }
    pub fn reserve(&mut self, operation: Operation, now: Instant) -> Result<Ticket, ReserveError> {
        self.expire(now);
        if self.record.is_some() {
            return Err(ReserveError::Busy);
        }
        let length = match operation {
            Operation::Insert(length) if (1..=MAX_UNITS).contains(&length) => length,
            Operation::Backspace(count) if (1..=MAX_UNITS as u32).contains(&count) => 0,
            _ => return Err(ReserveError::Invalid),
        };
        let sequence = self
            .sequence
            .checked_add(1)
            .ok_or(ReserveError::Exhausted)?;
        // Reserve shared capacity before allocation and before input is consumed.
        let permit = self.budget.reserve()?;
        let mut text = Vec::new();
        text.try_reserve_exact(length)
            .map_err(|_| ReserveError::Allocation)?;
        let ticket = Ticket(sequence);
        self.record = Some(Record {
            ticket,
            operation,
            text,
            ready: None,
            _permit: permit,
        });
        self.sequence = sequence;
        Ok(ticket)
    }
    /// No allocation is needed after the body write. Only a failed write's
    /// supplied insertion is copied; no document text is accepted or inferred.
    pub fn finish(
        &mut self,
        ticket: Ticket,
        outcome: Outcome,
        units: &[u16],
        now: Instant,
    ) -> bool {
        let Some(record) = &mut self.record else {
            return false;
        };
        if record.ticket != ticket || record.ready.is_some() {
            return false;
        }
        if matches!(outcome, Outcome::NotAttempted | Outcome::Applied) {
            self.record = None;
            return true;
        }
        if matches!(record.operation, Operation::Insert(length) if length == units.len())
            && !units.is_empty()
            && char::decode_utf16(units.iter().copied())
                .all(|v| v.is_ok_and(|c| !c.is_control() && c != '\u{fffc}'))
        {
            record.text.extend_from_slice(units);
        }
        record.ready = Some((outcome, now));
        true
    }
    pub fn metadata(&self, now: Instant) -> Option<Metadata> {
        let record = self.record.as_ref()?;
        let (outcome, created) = record.ready?;
        let elapsed = now.checked_duration_since(created)?;
        let remaining = self
            .retention
            .checked_sub(elapsed)
            .filter(|v| !v.is_zero())?;
        Some(Metadata {
            ticket: record.ticket,
            operation: record.operation,
            outcome,
            remaining,
            copyable: !record.text.is_empty(),
        })
    }
    pub fn text(&self, ticket: Ticket, now: Instant) -> Option<&[u16]> {
        let metadata = self.metadata(now)?;
        (metadata.ticket == ticket && metadata.copyable)
            .then(|| self.record.as_ref().unwrap().text.as_slice())
    }
    pub fn discard(&mut self, ticket: Ticket) -> bool {
        if self
            .record
            .as_ref()
            .is_some_and(|r| r.ticket == ticket && r.ready.is_some())
        {
            self.record = None;
            true
        } else {
            false
        }
    }
    /// Retire private payload after a protection boundary, keeping only the
    /// operation/status warning until its original deadline. No new grant.
    pub fn redact(&mut self, ticket: Ticket) -> bool {
        if let Some(record) = &mut self.record {
            if record.ticket == ticket && record.ready.is_some() {
                record.text.fill(0);
                record.text.clear();
                return true;
            }
        }
        false
    }
    pub fn expire(&mut self, now: Instant) -> bool {
        if self.needs_expiry(now) {
            self.record = None;
            true
        } else {
            false
        }
    }
    pub fn needs_expiry(&self, now: Instant) -> bool {
        self.record.as_ref().is_some_and(|r| r.ready.is_some()) && self.metadata(now).is_none()
    }
    pub fn clear(&mut self) {
        self.record = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fill_budget(budget: &Budget, now: Instant) -> Vec<Recovery> {
        (0..MAX_RECORDS)
            .map(|_| {
                let mut owner = Recovery::with_budget(budget.clone());
                owner.reserve(Operation::Insert(MAX_UNITS), now).unwrap();
                owner
            })
            .collect()
    }
    #[test]
    fn shared_budget_counts_pending_ready_and_redacted_records() {
        let now = Instant::now();
        let budget = Budget::default();
        let mut owners = fill_budget(&budget, now);
        let mut next = Recovery::with_budget(budget);
        for owner in &mut owners {
            let ticket = owner.record.as_ref().unwrap().ticket;
            assert!(owner.finish(ticket, Outcome::Uncertain, &vec![120; MAX_UNITS], now));
            assert!(owner.redact(ticket));
            assert_eq!(
                next.reserve(Operation::Backspace(1), now),
                Err(ReserveError::Busy)
            );
            assert_eq!(owner.metadata(now).unwrap().remaining, RETENTION);
        }
        owners[0].clear();
        assert!(next.reserve(Operation::Backspace(1), now).is_ok());
        assert_eq!(
            owners[0].reserve(Operation::Insert(1), now),
            Err(ReserveError::Busy)
        );
    }
    #[test]
    fn every_retirement_releases_exactly_one_shared_slot() {
        let now = Instant::now();
        for action in 0..8 {
            let budget = Budget::default();
            let mut owners = fill_budget(&budget, now);
            let ticket = owners[0].record.as_ref().unwrap().ticket;
            match action {
                0 => {
                    assert!(owners[0].finish(ticket, Outcome::NotAttempted, &[], now));
                }
                1 => {
                    assert!(owners[0].finish(ticket, Outcome::Applied, &[], now));
                }
                2 => owners[0].clear(),
                3 => {
                    owners.remove(0);
                }
                _ => {
                    assert!(owners[0].finish(ticket, Outcome::Uncertain, &[], now));
                    match action {
                        4 => {
                            assert!(owners[0].discard(ticket));
                            assert!(!owners[0].discard(ticket));
                        }
                        5 => {
                            assert!(owners[0].expire(now + RETENTION));
                            assert!(!owners[0].expire(now + RETENTION));
                        }
                        6 => owners[0].clear(),
                        _ => {
                            owners.remove(0);
                        }
                    }
                }
            }
            let mut next = Recovery::with_budget(budget.clone());
            assert!(next.reserve(Operation::Insert(1), now).is_ok());
            let mut overflow = Recovery::with_budget(budget);
            assert_eq!(
                overflow.reserve(Operation::Insert(1), now),
                Err(ReserveError::Busy)
            );
        }
    }
    #[test]
    fn failed_reservations_do_not_consume_capacity_or_ticket_sequence() {
        let now = Instant::now();
        let budget = Budget::default();
        let mut owners = fill_budget(&budget, now);
        let mut next = Recovery::with_budget(budget.clone());
        assert_eq!(
            next.reserve(Operation::Insert(0), now),
            Err(ReserveError::Invalid)
        );
        assert_eq!(
            next.reserve(Operation::Insert(1), now),
            Err(ReserveError::Busy)
        );
        assert_eq!(next.sequence, 0);
        next.sequence = u64::MAX;
        assert_eq!(
            next.reserve(Operation::Insert(1), now),
            Err(ReserveError::Exhausted)
        );
        owners[0].clear();
        assert_eq!(
            next.reserve(Operation::Insert(1), now),
            Err(ReserveError::Exhausted)
        );
        let mut valid = Recovery::with_budget(budget);
        assert!(valid.reserve(Operation::Insert(1), now).is_ok());
    }
    #[test]
    fn budget_handles_do_not_release_live_records_or_affect_other_pools() {
        let now = Instant::now();
        let budget = Budget::default();
        let owners = fill_budget(&budget, now);
        let mut next = Recovery::with_budget(budget.clone());
        drop(budget);
        assert_eq!(
            next.reserve(Operation::Insert(1), now),
            Err(ReserveError::Busy)
        );
        let mut separate = Recovery::default();
        assert!(separate.reserve(Operation::Insert(1), now).is_ok());
        drop(owners);
        assert!(next.reserve(Operation::Insert(MAX_UNITS), now).is_ok());
    }
    #[test]
    fn concurrent_owners_share_one_bound_and_release_after_owner_drop() {
        use std::sync::Barrier;
        const OWNERS: usize = 12;
        let now = Instant::now();
        let budget = Budget::default();
        let entered = Barrier::new(OWNERS + 1);
        let release = Barrier::new(OWNERS + 1);
        let accepted = AtomicUsize::new(0);
        std::thread::scope(|scope| {
            for _ in 0..OWNERS {
                let budget = budget.clone();
                let (entered, release, accepted) = (&entered, &release, &accepted);
                scope.spawn(move || {
                    let mut owner = Recovery::with_budget(budget);
                    if let Ok(ticket) = owner.reserve(Operation::Insert(MAX_UNITS), now) {
                        assert!(owner.finish(
                            ticket,
                            Outcome::Uncertain,
                            &vec![120; MAX_UNITS],
                            now
                        ));
                        accepted.fetch_add(1, Ordering::Relaxed);
                    }
                    entered.wait();
                    release.wait();
                    drop(owner);
                });
            }
            entered.wait();
            let count = accepted.load(Ordering::Relaxed);
            release.wait();
            assert_eq!(count, MAX_RECORDS);
        });
        let owners = fill_budget(&budget, now);
        assert_eq!(owners.len(), MAX_RECORDS);
    }
    #[test]
    fn protection_redaction_preserves_only_metadata_and_the_original_deadline() {
        let now = Instant::now();
        let mut recovery = Recovery::default();
        let ticket = recovery.reserve(Operation::Insert(1), now).unwrap();
        recovery.finish(ticket, Outcome::Uncertain, &['x' as u16], now);
        assert!(recovery.redact(ticket));
        assert!(recovery.text(ticket, now).is_none());
        let metadata = recovery.metadata(now).unwrap();
        assert_eq!(metadata.ticket, ticket);
        assert!(!metadata.copyable);
        assert_eq!(metadata.remaining, RETENTION);
        assert_eq!(
            recovery.reserve(Operation::Insert(1), now),
            Err(ReserveError::Busy)
        );
        assert!(recovery.expire(now + RETENTION));
    }
    #[test]
    fn only_failed_supplied_insertions_are_exposed_after_completion() {
        let now = Instant::now();
        for outcome in [
            Outcome::NotAttempted,
            Outcome::Applied,
            Outcome::AppliedUntracked,
            Outcome::Uncertain,
        ] {
            let mut recovery = Recovery::default();
            let text: Vec<_> = "a😀b".encode_utf16().collect();
            let ticket = recovery
                .reserve(Operation::Insert(text.len()), now)
                .unwrap();
            assert!(recovery.metadata(now).is_none());
            assert!(recovery.text(ticket, now).is_none());
            assert!(recovery.finish(ticket, outcome, &text, now));
            assert!(!recovery.finish(ticket, outcome, &text, now));
            assert_eq!(
                recovery.text(ticket, now),
                matches!(outcome, Outcome::AppliedUntracked | Outcome::Uncertain)
                    .then_some(text.as_slice())
            );
        }
    }
    #[test]
    fn capacity_is_reserved_before_consumption_and_never_overwrites_an_old_record() {
        let now = Instant::now();
        let mut recovery = Recovery::default();
        let ticket = recovery.reserve(Operation::Insert(MAX_UNITS), now).unwrap();
        assert_eq!(
            recovery.reserve(Operation::Insert(1), now),
            Err(ReserveError::Busy)
        );
        assert!(!recovery.discard(ticket));
        let units = vec!['x' as u16; MAX_UNITS];
        assert!(recovery.finish(ticket, Outcome::Uncertain, &units, now));
        assert_eq!(
            recovery.reserve(Operation::Backspace(1), now),
            Err(ReserveError::Busy)
        );
        assert_eq!(recovery.text(ticket, now), Some(units.as_slice()));
        assert!(recovery.discard(ticket));
        let next = recovery.reserve(Operation::Insert(1), now).unwrap();
        assert_ne!(next, ticket);
        assert!(!recovery.finish(ticket, Outcome::Applied, &[], now));
        assert!(recovery.finish(next, Outcome::Uncertain, &['z' as u16], now));
        assert!(!recovery.discard(ticket));
    }
    #[test]
    fn expiry_denies_copy_at_the_exact_deadline_and_releases_capacity() {
        let now = Instant::now();
        let mut recovery = Recovery::default();
        let ticket = recovery.reserve(Operation::Insert(1), now).unwrap();
        recovery.finish(ticket, Outcome::Uncertain, &['x' as u16], now);
        assert!(recovery
            .text(ticket, now + RETENTION - Duration::from_nanos(1))
            .is_some());
        assert!(recovery.text(ticket, now + RETENTION).is_none());
        assert!(recovery.needs_expiry(now + RETENTION));
        assert!(recovery.expire(now + RETENTION));
        assert!(!recovery.expire(now + RETENTION));
        assert!(recovery
            .reserve(Operation::Insert(1), now + RETENTION)
            .is_ok());
    }
    #[test]
    fn deletion_has_only_operation_metadata_and_never_copies_supplied_text() {
        let now = Instant::now();
        let mut recovery = Recovery::default();
        let ticket = recovery.reserve(Operation::Backspace(2), now).unwrap();
        recovery.finish(ticket, Outcome::Uncertain, &['x' as u16], now);
        let metadata = recovery.metadata(now).unwrap();
        assert_eq!(metadata.operation, Operation::Backspace(2));
        assert!(!metadata.copyable);
        assert!(recovery.text(ticket, now).is_none());
    }
    #[test]
    fn malformed_payloads_and_invalid_capacities_never_expose_text() {
        let now = Instant::now();
        for operation in [
            Operation::Insert(0),
            Operation::Insert(1025),
            Operation::Backspace(0),
            Operation::Backspace(1025),
        ] {
            assert_eq!(
                Recovery::default().reserve(operation, now),
                Err(ReserveError::Invalid)
            );
        }
        for units in [
            vec![0xd800],
            vec![0xfffc],
            vec![0],
            vec!['a' as u16, 'b' as u16],
        ] {
            let mut recovery = Recovery::default();
            let ticket = recovery.reserve(Operation::Insert(1), now).unwrap();
            recovery.finish(ticket, Outcome::Uncertain, &units, now);
            assert!(recovery.text(ticket, now).is_none());
            assert!(recovery.metadata(now).is_some());
        }
    }
    #[test]
    fn lifetime_is_bounded_and_close_does_not_reset_ticket_sequence() {
        assert!(Recovery::with_retention(Duration::ZERO).is_none());
        assert!(Recovery::with_retention(RETENTION + Duration::from_secs(1)).is_none());
        let now = Instant::now();
        let mut recovery = Recovery::default();
        let first = recovery.reserve(Operation::Insert(1), now).unwrap();
        recovery.clear();
        let second = recovery.reserve(Operation::Insert(1), now).unwrap();
        assert_ne!(first, second);
        assert!(!recovery.finish(first, Outcome::Uncertain, &['x' as u16], now));
        recovery.finish(second, Outcome::Uncertain, &['y' as u16], now);
        assert!(recovery.metadata(now - Duration::from_millis(1)).is_none());
    }
}

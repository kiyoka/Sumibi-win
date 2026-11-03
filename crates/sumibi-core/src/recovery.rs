//! One bounded, memory-only rescue slot. Never authorizes document editing.
#[cfg(test)]
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
pub const MAX_UNITS: usize = 1024;
pub const MAX_RECORDS: usize = 4;
pub const MAX_POOL_RECORDS: usize = 1024;
pub const MAX_POOL_TEXT_UNITS: usize = 16384;
pub const RETENTION: Duration = Duration::from_secs(600);
/// Share one budget across cooperating owners. It holds counters only, never
/// text or callbacks. Separate pools do not form a cross-process budget.
#[derive(Clone)]
pub struct Budget {
    state: Arc<BudgetState>,
}
struct BudgetState {
    // One CAS reserves both a record and its maximum text length. The upper
    // and lower halves are u32 counts, bounded before arithmetic below.
    used: AtomicU64,
    max_records: u32,
    max_text_units: u32,
}
impl Default for Budget {
    fn default() -> Self {
        Self::with_limits(MAX_RECORDS, MAX_RECORDS * MAX_UNITS).unwrap()
    }
}
impl Budget {
    /// A standalone pool may choose lower or higher bounds without changing
    /// the diagnostic default. The caller must share clones with every owner
    /// that should count against the same in-process limit.
    pub fn with_limits(max_records: usize, max_text_units: usize) -> Option<Self> {
        if !(1..=MAX_POOL_RECORDS).contains(&max_records)
            || !(1..=MAX_POOL_TEXT_UNITS).contains(&max_text_units)
        {
            return None;
        }
        let max_records = u32::try_from(max_records).ok()?;
        let max_text_units = u32::try_from(max_text_units).ok()?;
        Some(Self {
            state: Arc::new(BudgetState {
                used: AtomicU64::new(0),
                max_records,
                max_text_units,
            }),
        })
    }
    fn reserve(&self, units: usize) -> Result<Permit, ReserveError> {
        let units = u32::try_from(units).map_err(|_| ReserveError::Invalid)?;
        let delta = (1u64 << 32) | u64::from(units);
        let mut current = self.state.used.load(Ordering::Acquire);
        loop {
            let records = (current >> 32) as u32;
            let text_units = current as u32;
            if records >= self.state.max_records
                || text_units
                    .checked_add(units)
                    .is_none_or(|total| total > self.state.max_text_units)
            {
                return Err(ReserveError::Busy);
            }
            match self.state.used.compare_exchange_weak(
                current,
                current + delta,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    return Ok(Permit {
                        budget: self.clone(),
                        units,
                    })
                }
                Err(actual) => current = actual,
            }
        }
    }
}
struct Permit {
    budget: Budget,
    units: u32,
}
impl Drop for Permit {
    fn drop(&mut self) {
        let delta = (1u64 << 32) | u64::from(self.units);
        self.budget.state.used.fetch_sub(delta, Ordering::AcqRel);
    }
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Ticket(u64);
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Operation {
    Insert(usize),
    Backspace(u32),
    DeleteSelection,
    DeferredEnter,
    DeferredConvert,
    ForwardDelete(u32),
    MoveCaret {
        backward: bool,
        count: u32,
    },
    ExtendSelection {
        backward: bool,
        count: u32,
    },
    MoveToEdge {
        to_end: bool,
        extend: bool,
        count: u32,
    },
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Outcome {
    NotAttempted,
    Applied,
    AppliedUntracked,
    Uncertain,
    /// Accepted while waiting, but never attempted in the native document.
    Deferred,
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
            Operation::DeleteSelection | Operation::DeferredEnter | Operation::DeferredConvert => 0,
            Operation::ForwardDelete(count)
            | Operation::MoveCaret { count, .. }
            | Operation::ExtendSelection { count, .. }
            | Operation::MoveToEdge { count, .. }
                if (1..=MAX_UNITS as u32).contains(&count) =>
            {
                0
            }
            _ => return Err(ReserveError::Invalid),
        };
        let sequence = self
            .sequence
            .checked_add(1)
            .ok_or(ReserveError::Exhausted)?;
        // Reserve shared capacity before allocation and before input is consumed.
        let permit = self.budget.reserve(length)?;
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
    /// No allocation is needed on completion. Only a reserved supplied insertion
    /// is copied for an unconfirmed/deferred result; never inferred document text.
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
    fn configurable_budget_reserves_records_and_text_together_until_release() {
        let now = Instant::now();
        let budget = Budget::with_limits(3, 2).unwrap();
        let mut text = Recovery::with_budget(budget.clone());
        let ticket = text.reserve(Operation::Insert(2), now).unwrap();
        let mut command = Recovery::with_budget(budget.clone());
        command.reserve(Operation::Backspace(1), now).unwrap();
        let mut next = Recovery::with_budget(budget.clone());
        assert_eq!(
            next.reserve(Operation::Insert(1), now),
            Err(ReserveError::Busy)
        );
        assert_eq!(next.sequence, 0);
        next.reserve(Operation::DeferredEnter, now).unwrap();
        let mut overflow = Recovery::with_budget(budget.clone());
        assert_eq!(
            overflow.reserve(Operation::Backspace(1), now),
            Err(ReserveError::Busy)
        );

        assert!(text.finish(ticket, Outcome::Deferred, &['a' as u16, 'b' as u16], now));
        assert!(text.redact(ticket));
        assert_eq!(
            overflow.reserve(Operation::Insert(1), now),
            Err(ReserveError::Busy)
        );
        assert!(text.discard(ticket));
        assert!(overflow.reserve(Operation::Insert(1), now).is_ok());
        assert!(Budget::with_limits(0, 1).is_none());
        assert!(Budget::with_limits(1, 0).is_none());
        assert!(Budget::with_limits(MAX_POOL_RECORDS + 1, 1).is_none());
        assert!(Budget::with_limits(1, MAX_POOL_TEXT_UNITS + 1).is_none());
    }
    #[test]
    fn concurrent_custom_budget_rejects_excess_text_before_consumption() {
        use std::sync::Barrier;
        const OWNERS: usize = 8;
        let now = Instant::now();
        let budget = Budget::with_limits(OWNERS, 2 * MAX_UNITS).unwrap();
        let entered = Barrier::new(OWNERS + 1);
        let release = Barrier::new(OWNERS + 1);
        let accepted = AtomicUsize::new(0);
        std::thread::scope(|scope| {
            for _ in 0..OWNERS {
                let budget = budget.clone();
                let (entered, release, accepted) = (&entered, &release, &accepted);
                scope.spawn(move || {
                    let mut owner = Recovery::with_budget(budget);
                    if owner.reserve(Operation::Insert(MAX_UNITS), now).is_ok() {
                        accepted.fetch_add(1, Ordering::Relaxed);
                    }
                    entered.wait();
                    release.wait();
                });
            }
            entered.wait();
            assert_eq!(accepted.load(Ordering::Relaxed), 2);
            release.wait();
        });
        let mut next = Recovery::with_budget(budget);
        assert!(next.reserve(Operation::Insert(MAX_UNITS), now).is_ok());
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
        for operation in [
            Operation::Backspace(2),
            Operation::DeleteSelection,
            Operation::ForwardDelete(2),
            Operation::MoveToEdge {
                to_end: true,
                extend: true,
                count: 2,
            },
            Operation::ExtendSelection {
                backward: true,
                count: 2,
            },
            Operation::MoveCaret {
                backward: true,
                count: 3,
            },
        ] {
            let ticket = recovery.reserve(operation, now).unwrap();
            recovery.finish(ticket, Outcome::Uncertain, &['x' as u16], now);
            let metadata = recovery.metadata(now).unwrap();
            assert_eq!(metadata.operation, operation);
            assert!(!metadata.copyable);
            assert!(recovery.text(ticket, now).is_none());
            recovery.clear();
        }
    }
    #[test]
    fn malformed_payloads_and_invalid_capacities_never_expose_text() {
        let now = Instant::now();
        for operation in [
            Operation::Insert(0),
            Operation::Insert(1025),
            Operation::Backspace(0),
            Operation::Backspace(1025),
            Operation::ForwardDelete(0),
            Operation::ForwardDelete(1025),
            Operation::MoveToEdge {
                to_end: false,
                extend: false,
                count: 0,
            },
            Operation::MoveToEdge {
                to_end: true,
                extend: true,
                count: 1025,
            },
            Operation::ExtendSelection {
                backward: false,
                count: 0,
            },
            Operation::ExtendSelection {
                backward: true,
                count: 1025,
            },
            Operation::MoveCaret {
                backward: false,
                count: 0,
            },
            Operation::MoveCaret {
                backward: true,
                count: 1025,
            },
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
    #[test]
    fn deferred_input_is_distinct_from_uncertain_and_cannot_finish_twice() {
        let now = Instant::now();
        let mut recovery = Recovery::default();
        let ticket = recovery.reserve(Operation::Insert(2), now).unwrap();
        assert!(recovery.finish(ticket, Outcome::Deferred, &['a' as u16, 'b' as u16], now));
        assert_eq!(recovery.metadata(now).unwrap().outcome, Outcome::Deferred);
        assert_eq!(
            recovery.text(ticket, now),
            Some(&['a' as u16, 'b' as u16][..])
        );
        assert!(!recovery.finish(ticket, Outcome::Applied, &[], now));
        assert!(recovery.metadata(now + RETENTION).is_none());
        assert!(recovery.expire(now + RETENTION));
        for operation in [Operation::DeferredEnter, Operation::DeferredConvert] {
            let ticket = recovery.reserve(operation, now).unwrap();
            assert!(recovery.finish(ticket, Outcome::Deferred, &['x' as u16], now));
            assert!(!recovery.metadata(now).unwrap().copyable);
            assert!(recovery.text(ticket, now).is_none());
            assert!(recovery.discard(ticket));
        }
    }
    #[test]
    fn deferred_waiting_and_uncertain_writes_share_capacity_until_acknowledged() {
        let now = Instant::now();
        let budget = Budget::default();
        let mut owners = Vec::new();
        for index in 0..MAX_RECORDS {
            let mut recovery = Recovery::with_budget(budget.clone());
            let ticket = recovery.reserve(Operation::Insert(1), now).unwrap();
            recovery.finish(
                ticket,
                if index == 0 {
                    Outcome::Uncertain
                } else {
                    Outcome::Deferred
                },
                &['x' as u16],
                now,
            );
            owners.push(recovery);
        }
        let mut waiting = Recovery::with_budget(budget);
        assert_eq!(
            waiting.reserve(Operation::DeferredEnter, now),
            Err(ReserveError::Busy)
        );
        let ticket = owners[1].metadata(now).unwrap().ticket;
        owners[1].redact(ticket);
        assert_eq!(
            waiting.reserve(Operation::DeferredEnter, now),
            Err(ReserveError::Busy)
        );
        owners[1].discard(ticket);
        assert!(waiting.reserve(Operation::DeferredEnter, now).is_ok());
    }
}

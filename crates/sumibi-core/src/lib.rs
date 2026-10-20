//! OS-independent gate for a single pending manual conversion.
//!
//! This does not edit text. The TSF adapter must collect a fresh snapshot inside
//! the write edit session, then apply a returned replacement before releasing
//! that session. Context/range identifiers must never be reused within a session.
#![forbid(unsafe_code)]

/// Opaque, globally unique request identity supplied by the future adapter.
/// Never reuse an identity, including after cancellation or reconnection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RequestId(pub u128);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Revision {
    pub input: u64,
    pub settings: u64,
    pub authorization: u64,
}

/// Fresh observations of the target; no COM objects or edit cookies cross here.
/// `allowed` must be false for protected, unreadable, disabled or unknown input.
#[derive(Clone, PartialEq, Eq)]
pub struct Target {
    pub context: u128,
    pub range: u128,
    pub selection: (i32, i32),
    pub text: Vec<u16>,
    pub revision: Revision,
    pub allowed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reject {
    Busy,
    InvalidTarget,
    NoPendingRequest,
    DifferentRequest,
    TargetChanged,
    InvalidResult,
}

struct Pending {
    id: RequestId,
    target: Target,
}

/// Text-bearing types deliberately omit Debug to avoid accidental body logging.
#[derive(Default)]
pub struct ManualConversion {
    pending: Option<Pending>,
}

impl ManualConversion {
    pub fn begin(&mut self, id: RequestId, target: Target) -> Result<(), Reject> {
        if self.pending.is_some() {
            return Err(Reject::Busy);
        }
        if !target.allowed
            || target.text.is_empty()
            || target.selection.0 < 0
            || target.selection.1 < target.selection.0
            || String::from_utf16(&target.text).is_err()
        {
            return Err(Reject::InvalidTarget);
        }
        self.pending = Some(Pending { id, target });
        Ok(())
    }

    /// Call on Esc, focus/input changes, disconnect or policy revocation.
    pub fn cancel(&mut self) {
        self.pending = None;
    }

    pub fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    /// Consume a matching response once, even when validation fails.
    /// An unrelated stale response must not consume the current request.
    /// Never queue the returned text for a later edit session.
    pub fn finish(
        &mut self,
        id: RequestId,
        current: &Target,
        result: &str,
    ) -> Result<Vec<u16>, Reject> {
        let pending = self.pending.as_ref().ok_or(Reject::NoPendingRequest)?;
        if pending.id != id {
            return Err(Reject::DifferentRequest);
        }
        let pending = self.pending.take().ok_or(Reject::NoPendingRequest)?;
        if !current.allowed || current != &pending.target {
            return Err(Reject::TargetChanged);
        }
        if result.is_empty() {
            return Err(Reject::InvalidResult);
        }
        Ok(result.encode_utf16().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target() -> Target {
        Target {
            context: 1,
            range: 2,
            selection: (3, 3),
            text: "nihongo".encode_utf16().collect(),
            revision: Revision {
                input: 1,
                settings: 1,
                authorization: 1,
            },
            allowed: true,
        }
    }

    #[test]
    fn matching_result_can_only_be_used_once() {
        let mut state = ManualConversion::default();
        let target = target();
        assert_eq!(state.begin(RequestId(1), target.clone()), Ok(()));
        assert_eq!(
            state.finish(RequestId(1), &target, "日本語"),
            Ok("日本語".encode_utf16().collect())
        );
        assert_eq!(
            state.finish(RequestId(1), &target, "日本語"),
            Err(Reject::NoPendingRequest)
        );
    }

    #[test]
    fn cancellation_wins_over_a_late_response() {
        let mut state = ManualConversion::default();
        let target = target();
        state.begin(RequestId(1), target.clone()).unwrap();
        state.cancel();
        assert_eq!(
            state.finish(RequestId(1), &target, "日本語"),
            Err(Reject::NoPendingRequest)
        );
    }

    #[test]
    fn stale_response_does_not_cancel_a_new_request() {
        let mut state = ManualConversion::default();
        let target = target();
        state.begin(RequestId(1), target.clone()).unwrap();
        state.cancel();
        state.begin(RequestId(2), target.clone()).unwrap();
        assert_eq!(
            state.finish(RequestId(1), &target, "古い"),
            Err(Reject::DifferentRequest)
        );
        assert!(state.finish(RequestId(2), &target, "新しい").is_ok());
    }

    #[test]
    fn every_target_and_policy_change_rejects_the_result() {
        let original = target();
        let mut changes = Vec::new();
        let mut changed = original.clone();
        changed.context += 1;
        changes.push(changed);
        let mut changed = original.clone();
        changed.range += 1;
        changes.push(changed);
        let mut changed = original.clone();
        changed.selection = (4, 4);
        changes.push(changed);
        let mut changed = original.clone();
        changed.text = "NIHONGO".encode_utf16().collect();
        changes.push(changed);
        let mut changed = original.clone();
        changed.revision.input += 1;
        changes.push(changed);
        let mut changed = original.clone();
        changed.revision.settings += 1;
        changes.push(changed);
        let mut changed = original.clone();
        changed.revision.authorization += 1;
        changes.push(changed);
        let mut changed = original.clone();
        changed.allowed = false;
        changes.push(changed);
        for changed in changes {
            let mut state = ManualConversion::default();
            state.begin(RequestId(1), original.clone()).unwrap();
            assert_eq!(
                state.finish(RequestId(1), &changed, "日本語"),
                Err(Reject::TargetChanged)
            );
            assert!(!state.is_pending());
        }
    }

    #[test]
    fn protected_empty_and_invalid_utf16_targets_are_rejected() {
        for text in [Vec::new(), vec![0xd800]] {
            let mut target = target();
            target.text = text;
            assert_eq!(
                ManualConversion::default().begin(RequestId(1), target),
                Err(Reject::InvalidTarget)
            );
        }
        let mut target = target();
        target.allowed = false;
        assert_eq!(
            ManualConversion::default().begin(RequestId(1), target),
            Err(Reject::InvalidTarget)
        );
    }

    #[test]
    fn second_request_cannot_overwrite_a_pending_request() {
        let mut state = ManualConversion::default();
        let target = target();
        state.begin(RequestId(1), target.clone()).unwrap();
        assert_eq!(state.begin(RequestId(2), target.clone()), Err(Reject::Busy));
        assert!(state.finish(RequestId(1), &target, "日本語").is_ok());
    }

    #[test]
    fn empty_result_finishes_without_replacing_text() {
        let mut state = ManualConversion::default();
        let target = target();
        state.begin(RequestId(1), target.clone()).unwrap();
        assert_eq!(
            state.finish(RequestId(1), &target, ""),
            Err(Reject::InvalidResult)
        );
        assert!(!state.is_pending());
    }
}

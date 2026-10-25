//! OS-independent gate for a single pending manual conversion.
//!
//! This does not edit text. The TSF adapter must collect a fresh snapshot inside
//! the write edit session, then apply a returned replacement before releasing
//! that session. Context/range identifiers must never be reused within a session.
#![forbid(unsafe_code)]

pub mod grapheme;
pub mod input;
pub mod recovery;
pub mod waiting_input;

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

/// Explicit coordinate model for a manual-conversion target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Selection {
    /// Absolute coordinates supplied by a cooperating ACP adapter.
    Acp(i32, i32),
    /// The adapter revalidated both endpoints and direction against its
    /// retained native range. This is a length stamp, not document coordinates.
    RetainedRange { utf16_units: usize },
}

/// Fresh observations; no COM objects or edit cookies cross here. `allowed`
/// must be false for protected, unreadable, disabled or unknown input.
#[derive(Clone, PartialEq, Eq)]
pub struct Target {
    pub context: u128,
    pub range: u128,
    pub selection: Selection,
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
            || target.text.contains(&0xfffc)
            || !match target.selection {
                Selection::Acp(start, end) => start >= 0 && end >= start,
                Selection::RetainedRange { utf16_units } => {
                    target.context != 0 && target.range != 0 && utf16_units == target.text.len()
                }
            }
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
        if result.is_empty() || result.contains('\u{fffc}') {
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
            selection: Selection::Acp(3, 3),
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
        changed.selection = Selection::Acp(4, 4);
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
    fn protected_empty_object_and_invalid_utf16_targets_are_rejected() {
        for text in [Vec::new(), vec![0xd800], vec![0xfffc]] {
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
        state.begin(RequestId(2), target.clone()).unwrap();
        assert_eq!(
            state.finish(RequestId(2), &target, "日本\u{fffc}語"),
            Err(Reject::InvalidResult)
        );
        assert!(!state.is_pending());
    }
    #[test]
    fn retained_ranges_use_explicit_lengths_and_cannot_change_coordinate_model() {
        let mut target = target();
        target.text = "a😀b".encode_utf16().collect();
        target.selection = Selection::RetainedRange { utf16_units: 4 };
        let mut core = ManualConversion::default();
        core.begin(RequestId(1), target.clone()).unwrap();
        assert_eq!(
            core.finish(RequestId(1), &target, "日本語"),
            Ok("日本語".encode_utf16().collect())
        );
        assert_eq!(
            core.finish(RequestId(1), &target, "日本語"),
            Err(Reject::NoPendingRequest)
        );
        core.begin(RequestId(2), target.clone()).unwrap();
        let mut changed = target;
        changed.selection = Selection::Acp(0, 4);
        assert_eq!(
            core.finish(RequestId(2), &changed, "日本語"),
            Err(Reject::TargetChanged)
        );
    }
    #[test]
    fn retained_range_stamps_require_matching_text_length_and_nonzero_identities() {
        for change in 0..5 {
            let mut target = target();
            target.selection = Selection::RetainedRange {
                utf16_units: target.text.len(),
            };
            match change {
                0 => target.selection = Selection::RetainedRange { utf16_units: 0 },
                1 => {
                    target.selection = Selection::RetainedRange {
                        utf16_units: target.text.len() + 1,
                    }
                }
                2 => target.context = 0,
                3 => target.range = 0,
                _ => {
                    target.text = vec![0xd800];
                    target.selection = Selection::RetainedRange { utf16_units: 1 };
                }
            }
            let mut core = ManualConversion::default();
            assert_eq!(core.begin(RequestId(1), target), Err(Reject::InvalidTarget));
            assert!(!core.is_pending());
        }
    }
}

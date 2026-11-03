use super::*;

fn target() -> Target {
    Target {
        context: 1,
        target: 2,
        generation: 3,
        settings: 4,
        authorization: 5,
        allowed: true,
    }
}
fn advanced(before: Target) -> Target {
    Target {
        generation: before.generation + 1,
        ..before
    }
}
fn queue() -> WaitingInput {
    WaitingInput::new(RequestId(10), target()).unwrap()
}
fn text(input: Option<Input<'_>>) -> Vec<u16> {
    match input {
        Some(Input::Text(value)) => value.to_vec(),
        _ => panic!("expected text"),
    }
}

#[test]
fn conversion_result_and_queued_text_are_applied_once_in_order() {
    // A cooperating adapter simulator uses the existing conversion gate. This
    // is no native edit/keyboard/IPC test and makes no such compatibility claim.
    let mut conversion = crate::ManualConversion::default();
    let source = crate::Target {
        context: 1,
        range: 9,
        selection: crate::Selection::Acp(0, 7),
        text: "nihongo".encode_utf16().collect(),
        revision: crate::Revision {
            input: 3,
            settings: 4,
            authorization: 5,
        },
        allowed: true,
    };
    conversion.begin(RequestId(10), source.clone()).unwrap();
    let mut pending = queue();
    let supplied: Vec<_> = "です".encode_utf16().collect();
    pending.admit(target(), Input::Text(&supplied)).unwrap();
    pending.admit(target(), Input::Backspace(1)).unwrap();
    assert_eq!(pending.start_next(target()), Err(Reject::State));
    let mut body = conversion.finish(RequestId(10), &source, "日本語").unwrap();
    assert!(conversion.finish(RequestId(10), &source, "重複").is_err());
    let after = advanced(target());
    pending.settle(RequestId(10), after, true).unwrap();
    let ticket = pending.start_next(after).unwrap().unwrap();
    body.extend(text(pending.delivery(ticket)));
    pending
        .finish(ticket, Outcome::Applied(advanced(after)))
        .unwrap();
    assert_eq!(
        pending.finish(ticket, Outcome::Applied(advanced(after))),
        Err(Reject::Ticket)
    );
    let ticket = pending.start_next(advanced(after)).unwrap().unwrap();
    assert!(matches!(
        pending.delivery(ticket),
        Some(Input::Backspace(1))
    ));
    body.pop(); // The simulated last character is one UTF-16 scalar.
    pending
        .finish(ticket, Outcome::Applied(advanced(advanced(after))))
        .unwrap();
    assert_eq!(String::from_utf16(&body).unwrap(), "日本語で");
    assert_eq!(pending.start_next(advanced(advanced(after))), Ok(None));
    assert_eq!((pending.len(), pending.text_units()), (0, 0));
}

#[test]
fn capacity_refusal_preserves_admitted_input_and_reuses_only_confirmed_space() {
    let mut pending = queue();
    let block = vec![b'a' as u16; MAX_EVENT_UNITS];
    for _ in 0..16 {
        pending.admit(target(), Input::Text(&block)).unwrap();
    }
    assert_eq!(pending.text_units(), MAX_TEXT_UNITS);
    assert_eq!(
        pending.admit(target(), Input::Text(&[b'b' as u16])),
        Err(Reject::Capacity)
    );
    assert_eq!((pending.len(), pending.text_units()), (16, MAX_TEXT_UNITS));
    pending.settle(RequestId(10), target(), false).unwrap();
    let ticket = pending.start_next(target()).unwrap().unwrap();
    assert_eq!(text(pending.delivery(ticket)), block);
    assert_eq!(pending.admit(target(), Input::Enter), Err(Reject::State));
    pending
        .finish(ticket, Outcome::Applied(advanced(target())))
        .unwrap();
    pending
        .admit(advanced(target()), Input::Text(&block))
        .unwrap();
    assert_eq!((pending.len(), pending.text_units()), (16, MAX_TEXT_UNITS));

    let mut commands = queue();
    for _ in 0..MAX_EVENTS {
        commands.admit(target(), Input::Backspace(1)).unwrap();
    }
    assert_eq!(
        commands.admit(target(), Input::Convert),
        Err(Reject::Capacity)
    );
    assert_eq!((commands.len(), commands.text_units()), (MAX_EVENTS, 0));
}

#[test]
fn configured_limits_reject_before_consumption_and_reopen_after_confirmed_delivery() {
    assert_eq!(Limits::new(0, 1), None);
    assert_eq!(Limits::new(1, 0), None);
    assert_eq!(Limits::new(MAX_EVENTS + 1, 1), None);
    assert_eq!(Limits::new(1, MAX_TEXT_UNITS + 1), None);

    let mut pending =
        WaitingInput::with_limits(RequestId(10), target(), Limits::new(2, 2).unwrap()).unwrap();
    let emoji: Vec<_> = "😀".encode_utf16().collect();
    pending.admit(target(), Input::Text(&emoji)).unwrap();
    pending.admit(target(), Input::Backspace(1)).unwrap();
    assert_eq!(pending.admit(target(), Input::Enter), Err(Reject::Capacity));
    assert_eq!(
        pending.admit(target(), Input::Text(&[b'a' as u16])),
        Err(Reject::Capacity)
    );
    assert_eq!((pending.len(), pending.text_units()), (2, 2));

    pending.settle(RequestId(10), target(), false).unwrap();
    let ticket = pending.start_next(target()).unwrap().unwrap();
    assert_eq!(text(pending.delivery(ticket)), emoji);
    pending
        .finish(ticket, Outcome::Applied(advanced(target())))
        .unwrap();
    pending
        .admit(advanced(target()), Input::Text(&emoji))
        .unwrap();
    assert_eq!((pending.len(), pending.text_units()), (2, 2));

    let mut text_limited =
        WaitingInput::with_limits(RequestId(20), target(), Limits::new(3, 2).unwrap()).unwrap();
    text_limited.admit(target(), Input::Text(&emoji)).unwrap();
    assert_eq!(
        text_limited.admit(target(), Input::Text(&[b'z' as u16])),
        Err(Reject::Capacity)
    );
    assert_eq!((text_limited.len(), text_limited.text_units()), (1, 2));
    text_limited.admit(target(), Input::Enter).unwrap();
    assert_eq!((text_limited.len(), text_limited.text_units()), (2, 2));
}

#[test]
fn invalid_text_repeat_and_identity_never_grant_input_consumption() {
    for request in [RequestId(0), RequestId(u128::MAX)] {
        assert!(matches!(
            WaitingInput::new(request, target()),
            Err(Reject::Invalid)
        ));
    }
    for invalid in [
        Target {
            context: 0,
            ..target()
        },
        Target {
            target: 0,
            ..target()
        },
        Target {
            generation: u64::MAX,
            ..target()
        },
        Target {
            settings: u64::MAX,
            ..target()
        },
        Target {
            authorization: u64::MAX,
            ..target()
        },
        Target {
            allowed: false,
            ..target()
        },
    ] {
        assert!(matches!(
            WaitingInput::new(RequestId(10), invalid),
            Err(Reject::Invalid)
        ));
    }
    let mut pending = queue();
    for value in [
        vec![],
        vec![0xd800],
        vec![0xdc00],
        vec![0x0a],
        vec![0x0d],
        vec![0],
        vec![0xfffc],
        vec![b'a' as u16; MAX_EVENT_UNITS + 1],
    ] {
        assert_eq!(
            pending.admit(target(), Input::Text(&value)),
            Err(Reject::Invalid)
        );
    }
    for count in [0, 1025, u32::MAX] {
        assert_eq!(
            pending.admit(target(), Input::Backspace(count)),
            Err(Reject::Invalid)
        );
    }
    assert!(pending.is_empty());
    let mut supplied: Vec<_> = "😀e\u{301}".encode_utf16().collect();
    pending.admit(target(), Input::Text(&supplied)).unwrap();
    supplied.fill(b'x' as u16);
    pending.retire(Boundary::Cancelled);
    assert_eq!(
        String::from_utf16(&text(pending.retained(0))).unwrap(),
        "😀e\u{301}"
    );
}

#[test]
fn conversion_command_pauses_fifo_until_a_new_matching_request_finishes() {
    let mut pending = queue();
    pending.admit(target(), Input::Convert).unwrap();
    pending
        .admit(target(), Input::Text(&[b'z' as u16]))
        .unwrap();
    pending.settle(RequestId(10), target(), false).unwrap();
    let ticket = pending.start_next(target()).unwrap().unwrap();
    assert!(matches!(pending.delivery(ticket), Some(Input::Convert)));
    pending
        .finish(ticket, Outcome::ConversionStarted(RequestId(11)))
        .unwrap();
    assert_eq!(pending.start_next(target()), Err(Reject::State));
    assert_eq!(
        pending.settle(RequestId(10), target(), false),
        Err(Reject::State)
    );
    assert_eq!(pending.len(), 1);
    pending.admit(target(), Input::Backspace(1)).unwrap();
    pending
        .settle(RequestId(11), advanced(target()), true)
        .unwrap();
    let ticket = pending.start_next(advanced(target())).unwrap().unwrap();
    assert_eq!(text(pending.delivery(ticket)), vec![b'z' as u16]);
}

#[test]
fn enter_stops_before_host_control_and_retains_the_undispatched_tail() {
    let mut pending = queue();
    pending.admit(target(), Input::Enter).unwrap();
    pending
        .admit(target(), Input::Text(&[b'z' as u16]))
        .unwrap();
    pending.settle(RequestId(10), target(), false).unwrap();
    assert_eq!(pending.start_next(target()), Err(Reject::ManualEnter));
    assert!(matches!(pending.retained(0), Some(Input::Enter)));
    assert_eq!(text(pending.retained(1)), vec![b'z' as u16]);
    let metadata = pending.recovery().unwrap();
    assert_eq!(metadata.reason, Boundary::ManualEnter);
    assert_eq!((metadata.events, metadata.uncertain_head), (2, false));
    assert_eq!(pending.start_next(target()), Err(Reject::State));
    assert_eq!(
        pending.settle(RequestId(10), target(), false),
        Err(Reject::State)
    );
}

#[test]
fn unknown_native_result_preserves_uncertain_head_without_automatic_retry() {
    let mut pending = queue();
    pending
        .admit(target(), Input::Text(&[b'a' as u16]))
        .unwrap();
    pending.admit(target(), Input::Backspace(1)).unwrap();
    pending.settle(RequestId(10), target(), false).unwrap();
    let ticket = pending.start_next(target()).unwrap().unwrap();
    pending.finish(ticket, Outcome::Uncertain).unwrap();
    let metadata = pending.recovery().unwrap();
    assert_eq!(
        (
            metadata.events,
            metadata.text_units,
            metadata.uncertain_head
        ),
        (2, 1, true)
    );
    assert_eq!(text(pending.retained(0)), vec![b'a' as u16]);
    assert!(pending.delivery(ticket).is_none());
    assert_eq!(pending.start_next(target()), Err(Reject::State));
    assert_eq!(
        pending.finish(ticket, Outcome::NotAttempted),
        Err(Reject::Ticket)
    );
    pending.retire(Boundary::Cancelled);
    assert!(pending.recovery().unwrap().uncertain_head);
}

#[test]
fn confirmed_but_untracked_head_is_removed_and_tail_is_never_replayed() {
    let mut pending = queue();
    pending
        .admit(target(), Input::Text(&[b'a' as u16]))
        .unwrap();
    pending
        .admit(target(), Input::Text(&[b'b' as u16]))
        .unwrap();
    pending.settle(RequestId(10), target(), false).unwrap();
    let ticket = pending.start_next(target()).unwrap().unwrap();
    pending.finish(ticket, Outcome::AppliedUntracked).unwrap();
    assert_eq!(text(pending.retained(0)), vec![b'b' as u16]);
    let metadata = pending.recovery().unwrap();
    assert_eq!(
        (
            metadata.events,
            metadata.text_units,
            metadata.uncertain_head
        ),
        (1, 1, false)
    );
    assert_eq!(metadata.reason, Boundary::AppliedUntracked);
    assert_eq!(pending.start_next(target()), Err(Reject::State));
}

#[test]
fn stale_acknowledgements_cannot_finish_another_attempt_or_owner() {
    let mut pending = queue();
    let receipt = pending
        .admit(target(), Input::Text(&[b'a' as u16]))
        .unwrap();
    pending.settle(RequestId(10), target(), false).unwrap();
    let first = pending.start_next(target()).unwrap().unwrap();
    assert_ne!(first, receipt);
    pending.finish(first, Outcome::NotAttempted).unwrap();
    let second = pending.start_next(target()).unwrap().unwrap();
    assert_ne!(first, second);
    assert_eq!(
        pending.finish(first, Outcome::Applied(advanced(target()))),
        Err(Reject::Ticket)
    );
    let mut other = WaitingInput::new(RequestId(20), target()).unwrap();
    other.admit(target(), Input::Text(&[b'a' as u16])).unwrap();
    other.settle(RequestId(20), target(), false).unwrap();
    let other_ticket = other.start_next(target()).unwrap().unwrap();
    assert_eq!(
        pending.finish(other_ticket, Outcome::NotAttempted),
        Err(Reject::Ticket)
    );
    assert_eq!(text(pending.delivery(second)), vec![b'a' as u16]);
    pending
        .finish(second, Outcome::Applied(advanced(target())))
        .unwrap();
    assert!(pending.is_empty());
}

#[test]
fn changed_target_or_protection_is_irreversible_even_when_restored() {
    for changed in [
        Target {
            context: 3,
            ..target()
        },
        Target {
            target: 3,
            ..target()
        },
        Target {
            settings: 6,
            ..target()
        },
        Target {
            authorization: 6,
            ..target()
        },
        Target {
            allowed: false,
            ..target()
        },
    ] {
        let mut pending = queue();
        pending
            .admit(target(), Input::Text(&[b'a' as u16]))
            .unwrap();
        assert_eq!(
            pending.admit(changed, Input::Enter),
            Err(Reject::TargetChanged)
        );
        assert!(pending.recovery().unwrap().redacted);
        assert_eq!((pending.len(), pending.text_units()), (0, 0));
        assert!(pending.retained(0).is_none());
        assert_eq!(
            pending.settle(RequestId(10), target(), false),
            Err(Reject::State)
        );
        assert_eq!(pending.admit(target(), Input::Enter), Err(Reject::State));
    }
    let mut pending = queue();
    pending
        .admit(target(), Input::Text(&[b'a' as u16]))
        .unwrap();
    pending.settle(RequestId(10), target(), false).unwrap();
    assert_eq!(
        pending.start_next(advanced(target())),
        Err(Reject::TargetChanged)
    );
    assert!(!pending.recovery().unwrap().uncertain_head);
    assert_eq!(text(pending.retained(0)), vec![b'a' as u16]);
    pending.retire(Boundary::ProtectionChanged);
    assert!(pending.recovery().unwrap().redacted);
    assert!(pending.retained(0).is_none());
}

#[test]
fn invalid_completion_or_boundary_during_delivery_never_becomes_a_retry() {
    for current in [
        target(),
        Target {
            generation: 9,
            ..target()
        },
        Target {
            generation: u64::MAX,
            ..target()
        },
        Target {
            authorization: 9,
            ..advanced(target())
        },
    ] {
        let mut pending = queue();
        pending
            .admit(target(), Input::Text(&[b'a' as u16]))
            .unwrap();
        pending.settle(RequestId(10), target(), false).unwrap();
        let ticket = pending.start_next(target()).unwrap().unwrap();
        assert_eq!(
            pending.finish(ticket, Outcome::Applied(current)),
            Err(Reject::Invalid)
        );
        assert_eq!(pending.start_next(target()), Err(Reject::State));
        assert_eq!(
            pending.recovery().unwrap().uncertain_head,
            current.valid() && current.authorization == target().authorization
        );
    }
    let mut pending = queue();
    pending.admit(target(), Input::Backspace(1)).unwrap();
    pending.settle(RequestId(10), target(), false).unwrap();
    let ticket = pending.start_next(target()).unwrap().unwrap();
    pending.retire(Boundary::Cancelled);
    assert_eq!(
        pending.finish(ticket, Outcome::Applied(advanced(target()))),
        Err(Reject::Ticket)
    );
    assert!(pending.recovery().unwrap().uncertain_head);
}

#[test]
fn exhausted_ids_and_reused_conversion_requests_do_not_wrap_or_drain() {
    let mut pending = queue();
    pending.admit(target(), Input::Convert).unwrap();
    pending.settle(RequestId(10), target(), false).unwrap();
    let ticket = pending.start_next(target()).unwrap().unwrap();
    assert_eq!(
        pending.finish(ticket, Outcome::ConversionStarted(RequestId(10))),
        Err(Reject::Invalid)
    );
    assert_eq!(pending.len(), 1);
    assert!(pending.recovery().unwrap().uncertain_head);

    let mut pending = queue();
    pending.admit(target(), Input::Backspace(1024)).unwrap();
    pending.sequence = u64::MAX - 1;
    assert_eq!(pending.admit(target(), Input::Enter), Err(Reject::Capacity));
    pending.settle(RequestId(10), target(), false).unwrap();
    assert_eq!(pending.start_next(target()), Err(Reject::Capacity));
    assert_eq!(pending.len(), 1);
    assert!(pending.recovery().is_none());
}

#[test]
fn failed_or_mismatched_request_completion_never_releases_input_to_another_target() {
    for (current, applied) in [
        (advanced(target()), false),
        (target(), true),
        (
            Target {
                target: 99,
                ..advanced(target())
            },
            true,
        ),
        (
            Target {
                settings: 99,
                ..advanced(target())
            },
            true,
        ),
        (
            Target {
                allowed: false,
                ..target()
            },
            false,
        ),
    ] {
        let mut pending = queue();
        pending
            .admit(target(), Input::Text(&[b'a' as u16]))
            .unwrap();
        assert_eq!(
            pending.settle(RequestId(9), target(), false),
            Err(Reject::State)
        );
        assert_eq!(pending.len(), 1);
        assert_eq!(
            pending.settle(RequestId(10), current, applied),
            Err(Reject::TargetChanged)
        );
        assert_eq!(pending.start_next(target()), Err(Reject::State));
        assert!(!pending.recovery().unwrap().uncertain_head);
        assert_eq!(
            pending.settle(RequestId(10), target(), false),
            Err(Reject::State)
        );
    }
    // A native boundary during delivery wins over the later completion and
    // permanently removes protected payload, even if permissions are restored.
    let mut pending = queue();
    pending
        .admit(target(), Input::Text(&[b'a' as u16]))
        .unwrap();
    pending.settle(RequestId(10), target(), false).unwrap();
    let ticket = pending.start_next(target()).unwrap().unwrap();
    pending.retire(Boundary::ProtectionChanged);
    assert_eq!(
        pending.finish(ticket, Outcome::Applied(advanced(target()))),
        Err(Reject::Ticket)
    );
    assert!(pending.recovery().unwrap().redacted);
    assert!(pending.retained(0).is_none());
}

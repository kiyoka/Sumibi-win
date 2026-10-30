use sumibi_protocol::{message::*, Decoder, MAX_FRAME_BYTES};

fn envelope(message: Message) -> Envelope {
    Envelope {
        version: VERSION,
        session: Id::new(1).unwrap(),
        generation: 1,
        message,
    }
}

fn revision() -> Revision {
    Revision {
        input: u64::MAX,
        settings: 2,
        authorization: 3,
    }
}

fn policy() -> Policy {
    Policy {
        settings: 2,
        authorization: 3,
        allowed: true,
    }
}

#[test]
fn shared_policy_transition_accepts_settings_only_and_requires_new_authority_for_permission() {
    let previous = policy();
    for (next, accepted) in [
        (previous, true),
        (
            Policy {
                settings: 3,
                ..previous
            },
            true,
        ),
        (
            Policy {
                authorization: 4,
                allowed: false,
                ..previous
            },
            true,
        ),
        (
            Policy {
                settings: 3,
                authorization: 4,
                allowed: false,
            },
            true,
        ),
        (
            Policy {
                settings: 1,
                ..previous
            },
            false,
        ),
        (
            Policy {
                authorization: 2,
                ..previous
            },
            false,
        ),
        (
            Policy {
                allowed: false,
                ..previous
            },
            false,
        ),
        (
            Policy {
                settings: 0,
                ..previous
            },
            false,
        ),
    ] {
        assert_eq!(next.follows(previous), accepted);
    }
}

#[test]
fn every_message_round_trips_through_the_frame_without_integer_precision_loss() {
    for message in [
        Message::Hello(Hello {
            broker: Id::new(u128::MAX).unwrap(),
        }),
        Message::Ready(Ready {
            broker: Id::new(2).unwrap(),
            policy: policy(),
        }),
        Message::Convert(Convert {
            request: u64::MAX,
            revision: revision(),
            timeout_ms: MAX_TIMEOUT_MS,
            text: "日本語😀e\u{301}".into(),
        }),
        Message::Cancel(Cancel { request: u64::MAX }),
        Message::Result(ConversionResult {
            request: 1,
            revision: revision(),
            text: "日本語".into(),
        }),
        Message::Error(ConversionError {
            request: 1,
            revision: revision(),
            code: ErrorCode::Unavailable,
        }),
        Message::PolicyChanged(policy()),
    ] {
        let original = envelope(message);
        let frame = encode(&original).unwrap();
        let mut decoder = Decoder::default();
        let decoded = decoder.decode(&frame).unwrap();
        assert_eq!(decoded.consumed, frame.len());
        assert!(decode(decoded.body.unwrap()).unwrap() == original);
    }
}

fn wire(message: &str) -> String {
    format!(
        r#"{{"version":1,"session":"00000000000000000000000000000001","generation":1,"message":{message}}}"#
    )
}

#[test]
fn unknown_missing_duplicate_fields_and_multiple_kinds_are_rejected() {
    let valid = wire(r#"{"hello":{"broker":"00000000000000000000000000000002"}}"#);
    for invalid in [
        valid.replace("\"version\":1", "\"version\":1,\"version\":1"),
        valid.replace("\"version\":1,", ""),
        valid.replace("\"version\":1", "\"version\":1,\"secret\":\"private\""),
        wire(
            r#"{"hello":{"broker":"00000000000000000000000000000002","broker":"00000000000000000000000000000002"}}"#,
        ),
        wire(r#"{"hello":{"broker":"00000000000000000000000000000002","extra":false}}"#),
        wire(r#"{"hello":{}}"#),
        wire(r#"{"cancel":{"request":1},"error":{}}"#),
        wire(r#"{"future":{}}"#),
        wire(
            r#"{"policy_changed":{"settings":1,"authorization":1,"allowed":true,"allowed":false}}"#,
        ),
        wire(
            r#"{"result":{"request":1,"revision":{"input":1,"input":2,"settings":1,"authorization":1},"text":"fixed"}}"#,
        ),
        wire(
            r#"{"result":{"request":1,"revision":{"input":1,"settings":1,"authorization":1,"extra":0},"text":"fixed"}}"#,
        ),
    ] {
        assert_eq!(decode(&invalid).err(), Some(MessageError::Malformed));
    }
}

#[test]
fn malformed_json_types_identities_and_fixed_errors_are_rejected() {
    for message in [
        r#"{"hello":{"broker":"00000000000000000000000000000000"}}"#,
        r#"{"hello":{"broker":"0000000000000000000000000000000A"}}"#,
        r#"{"hello":{"broker":2}}"#,
        r#"{"hello":{"broker":"1"}}"#,
        r#"{"cancel":{"request":-1}}"#,
        r#"{"cancel":{"request":1.0}}"#,
        r#"{"cancel":{"request":18446744073709551616}}"#,
        r#"{"cancel":{"request":"1"}}"#,
        r#"{"result":{"request":1,"revision":{"input":1,"settings":1,"authorization":1},"text":"\ud800"}}"#,
        r#"{"error":{"request":1,"revision":{"input":1,"settings":1,"authorization":1},"code":"private server body"}}"#,
    ] {
        assert_eq!(decode(&wire(message)).err(), Some(MessageError::Malformed));
    }
    let valid = wire(r#"{"cancel":{"request":1}}"#);
    for invalid in [
        format!("{valid}{{}}"),
        format!("[{valid}]"),
        format!("{valid}x"),
    ] {
        assert_eq!(decode(&invalid).err(), Some(MessageError::Malformed));
    }
    assert_eq!(
        decode(&valid.replace("\"version\":1", "\"version\":2")).err(),
        Some(MessageError::UnsupportedVersion)
    );
    assert_eq!(
        decode(&valid.replace("\"generation\":1", "\"generation\":0")).err(),
        Some(MessageError::InvalidValue)
    );
    assert_eq!(
        decode(&wire(r#"{"cancel":{"request":0}}"#)).err(),
        Some(MessageError::InvalidValue)
    );
    assert_eq!(
        decode(&wire(
            r#"{"policy_changed":{"settings":0,"authorization":1,"allowed":true}}"#
        ))
        .err(),
        Some(MessageError::InvalidValue)
    );
    assert_eq!(decode(&wire(r#"{"ready":{"broker":"00000000000000000000000000000002","policy":{"settings":1,"authorization":0,"allowed":true}}}"#)).err(), Some(MessageError::InvalidValue));
}

#[test]
fn utf16_and_encoded_size_caps_apply_in_both_directions() {
    for (text, valid) in [
        ("".to_owned(), false),
        ("x".repeat(MAX_TEXT_UNITS), true),
        ("x".repeat(MAX_TEXT_UNITS + 1), false),
        ("😀".repeat(MAX_TEXT_UNITS / 2), true),
        ("😀".repeat(MAX_TEXT_UNITS / 2 + 1), false),
        ("\0".repeat(MAX_TEXT_UNITS), true),
        ("a\u{fffc}b".to_owned(), false),
    ] {
        let message = envelope(Message::Result(ConversionResult {
            request: 1,
            revision: revision(),
            text,
        }));
        assert_eq!(encode(&message).is_ok(), valid);
        let body = serde_json::to_string(&message).unwrap();
        assert_eq!(decode(&body).is_ok(), valid);
    }
    assert_eq!(
        decode(&" ".repeat(MAX_FRAME_BYTES + 1)).err(),
        Some(MessageError::TooLarge)
    );
    let nested = format!("{}0{}", "[".repeat(256), "]".repeat(256));
    assert_eq!(decode(&wire(&nested)).err(), Some(MessageError::Malformed));
    for timeout_ms in [0, MAX_TIMEOUT_MS + 1] {
        let message = envelope(Message::Convert(Convert {
            request: 1,
            revision: revision(),
            timeout_ms,
            text: "fixed".into(),
        }));
        assert_eq!(encode(&message).err(), Some(MessageError::InvalidValue));
    }
}

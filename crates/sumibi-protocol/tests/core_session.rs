//! Protocol/Core integration with a bounded fake peer, not TSF or OS identity.
use std::sync::mpsc::sync_channel;
use std::time::{Duration, Instant};
use sumibi_core::{ManualConversion, Reject, RequestId, Target};
use sumibi_protocol::message::{self, *};
use sumibi_protocol::session::{ClientSession, Event, Outcome};
use sumibi_protocol::Decoder;

fn policy() -> Policy {
    Policy {
        settings: 1,
        authorization: 1,
        allowed: true,
    }
}

fn target() -> Target {
    Target {
        context: 10,
        range: 20,
        selection: (7, 14),
        text: "nihongo".encode_utf16().collect(),
        revision: sumibi_core::Revision {
            input: 1,
            settings: 1,
            authorization: 1,
        },
        allowed: true,
    }
}

fn decode(frame: &[u8]) -> Envelope {
    let mut decoder = Decoder::default();
    let decoded = decoder.decode(frame).unwrap();
    assert_eq!(decoded.consumed, frame.len());
    message::decode(decoded.body.unwrap()).unwrap()
}

fn peer_response(frame: &[u8]) -> Vec<u8> {
    let mut envelope = decode(frame);
    envelope.message = match envelope.message {
        Message::Hello(hello) => Message::Ready(Ready {
            broker: hello.broker,
            policy: policy(),
        }),
        Message::Convert(convert) => Message::Result(ConversionResult {
            request: convert.request,
            revision: convert.revision,
            text: "日本語".into(),
        }),
        _ => panic!("unexpected fixture request kind"),
    };
    message::encode(&envelope).unwrap()
}

fn connected(now: Instant) -> ClientSession {
    let mut client = ClientSession::new(Id::new(1).unwrap(), Id::new(2).unwrap());
    let ready = peer_response(&client.hello().unwrap());
    assert!(matches!(
        client.receive(&ready, now).unwrap().event,
        Some(Event::Ready(_))
    ));
    client
}

fn begin(
    client: &mut ClientSession,
    core: &mut ManualConversion,
    target: Target,
    now: Instant,
) -> Vec<u8> {
    let wire = client
        .begin(
            target.revision.input,
            &String::from_utf16(&target.text).unwrap(),
            100,
            now,
        )
        .unwrap();
    let Message::Convert(request) = decode(&wire).message else {
        panic!("expected fixture conversion");
    };
    core.begin(RequestId(request.request.into()), target)
        .unwrap();
    wire
}

// A fake owner-thread edit scope: collect the fresh target and consume the
// Core result immediately, with no queued edit or later target recapture.
fn apply(core: &mut ManualConversion, fresh: &Target, event: Event) -> Result<Vec<u16>, Reject> {
    if let Event::Reply {
        request,
        outcome: Outcome::Result(text),
        ..
    } = event
    {
        core.finish(RequestId(request.into()), fresh, &text)
    } else {
        panic!("expected fixture result event");
    }
}

#[test]
fn bounded_worker_round_trip_only_returns_the_current_result_once() {
    let now = Instant::now();
    let mut client = ClientSession::new(Id::new(1).unwrap(), Id::new(2).unwrap());
    let mut core = ManualConversion::default();
    std::thread::scope(|scope| {
        // Only owned bytes cross threads; each channel has capacity one. Both
        // sides use deadlines so a regression cannot hang a test indefinitely.
        let (send, peer_receive) = sync_channel::<Vec<u8>>(1);
        let (peer_send, receive) = sync_channel(1);
        let worker = scope.spawn(move || {
            for _ in 0..2 {
                let request = peer_receive.recv_timeout(Duration::from_secs(2)).unwrap();
                peer_send.try_send(peer_response(&request)).unwrap();
            }
        });
        send.try_send(client.hello().unwrap()).unwrap();
        let ready = receive.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(matches!(
            client.receive(&ready, now).unwrap().event,
            Some(Event::Ready(_))
        ));
        send.try_send(begin(&mut client, &mut core, target(), now))
            .unwrap();
        let reply = receive.recv_timeout(Duration::from_secs(2)).unwrap();
        let mut result = None;
        for part in reply.chunks(3) {
            if let Some(event) = client.receive(part, now).unwrap().event {
                result = Some(event);
            }
        }
        assert_eq!(
            apply(&mut core, &target(), result.unwrap()).unwrap(),
            "日本語".encode_utf16().collect::<Vec<_>>()
        );
        assert!(!core.is_pending());
        assert!(matches!(
            client.receive(&reply, now).unwrap().event,
            Some(Event::Ignored)
        ));
        worker.join().unwrap();
    });
}

#[test]
fn cancelled_and_disconnected_results_never_complete_the_new_core_request() {
    let now = Instant::now();
    let mut client = connected(now);
    let mut core = ManualConversion::default();
    let first = peer_response(&begin(&mut client, &mut core, target(), now));
    client.cancel().unwrap();
    core.cancel();
    let second = peer_response(&begin(&mut client, &mut core, target(), now));
    assert!(matches!(
        client.receive(&first, now).unwrap().event,
        Some(Event::Ignored)
    ));
    assert!(core.is_pending());
    client.disconnect();
    core.cancel();
    let ready = peer_response(&client.reconnect(Id::new(2).unwrap()).unwrap());
    client.receive(&ready, now).unwrap();
    let third = peer_response(&begin(&mut client, &mut core, target(), now));
    assert!(matches!(
        client.receive(&second, now).unwrap().event,
        Some(Event::Ignored)
    ));
    assert!(core.is_pending());
    let event = client.receive(&third, now).unwrap().event.unwrap();
    assert!(apply(&mut core, &target(), event).is_ok());
}

#[test]
fn core_checks_every_fresh_target_even_after_a_valid_framed_reply() {
    let mut changes = Vec::new();
    let mut changed = target();
    changed.context += 1;
    changes.push(changed);
    let mut changed = target();
    changed.range += 1;
    changes.push(changed);
    let mut changed = target();
    changed.selection = (8, 14);
    changes.push(changed);
    let mut changed = target();
    changed.text = "NIHONGO".encode_utf16().collect();
    changes.push(changed);
    let mut changed = target();
    changed.revision.input += 1;
    changes.push(changed);
    let mut changed = target();
    changed.revision.settings += 1;
    changes.push(changed);
    let mut changed = target();
    changed.revision.authorization += 1;
    changes.push(changed);
    let mut changed = target();
    changed.allowed = false;
    changes.push(changed);
    for fresh in changes {
        let now = Instant::now();
        let mut client = connected(now);
        let mut core = ManualConversion::default();
        let reply = peer_response(&begin(&mut client, &mut core, target(), now));
        let event = client.receive(&reply, now).unwrap().event.unwrap();
        assert_eq!(apply(&mut core, &fresh, event), Err(Reject::TargetChanged));
        assert!(!core.is_pending());
    }
}

#[test]
fn policy_timeout_parse_error_and_queue_full_cancel_the_owner_core_state() {
    for case in ["policy", "timeout", "malformed", "queue-full", "eof"] {
        let now = Instant::now();
        let mut client = connected(now);
        let mut core = ManualConversion::default();
        let request = begin(&mut client, &mut core, target(), now);
        let reply = peer_response(&request);
        match case {
            "policy" => {
                let mut envelope = decode(&reply);
                envelope.message = Message::PolicyChanged(Policy {
                    authorization: 2,
                    allowed: false,
                    ..policy()
                });
                assert!(matches!(
                    client
                        .receive(&message::encode(&envelope).unwrap(), now)
                        .unwrap()
                        .event,
                    Some(Event::PolicyChanged {
                        cancelled: Some(1),
                        ..
                    })
                ));
            }
            "timeout" => {
                assert!(matches!(
                    client
                        .receive(&reply, now + Duration::from_millis(100))
                        .unwrap()
                        .event,
                    Some(Event::Expired(1))
                ));
            }
            "malformed" => {
                assert!(client
                    .receive(&sumibi_protocol::encode("{bad private body").unwrap(), now)
                    .is_err());
            }
            "queue-full" => {
                let (send, _receive) = sync_channel(1);
                send.try_send(vec![0]).unwrap();
                assert!(send.try_send(request).is_err());
                client.disconnect();
            }
            "eof" => {
                client.eof().unwrap();
            }
            _ => unreachable!(),
        }
        // This wiring is required at the owner boundary, not an implicit side
        // effect of ClientSession. Production TSF integration is still pending.
        core.cancel();
        assert_eq!(
            core.finish(RequestId(1), &target(), "日本語"),
            Err(Reject::NoPendingRequest)
        );
        assert!(!core.is_pending());
    }
}

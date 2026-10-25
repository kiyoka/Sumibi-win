//! One bounded client connection; no I/O, COM, peer authentication or text edits.
//!
//! The caller must authenticate the OS peer before sending even Hello. Matching
//! IDs/Ready only establish protocol ordering, never process trust or consent.
//! Each live client needs a unique lifetime ID. Its connection generation and
//! request counter never wrap or reset on reconnect. No request is auto-retried.
use crate::message::{self, *};
use crate::{Decoder, FrameError};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Connecting,
    Ready,
    Closed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionError {
    Frame(FrameError),
    Message(MessageError),
    WrongState,
    WrongBroker,
    UnexpectedMessage,
    RevisionMismatch,
    PolicyRegression,
    Busy,
    NotAllowed,
    InvalidTimeout,
    Exhausted,
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "protocol session: {self:?}")
    }
}
impl std::error::Error for SessionError {}

pub enum Outcome {
    Result(String),
    Error(ErrorCode),
}

pub enum Event {
    Ready(Policy),
    PolicyChanged {
        policy: Policy,
        cancelled: Option<u64>,
    },
    Reply {
        request: u64,
        revision: Revision,
        outcome: Outcome,
    },
    Expired(u64),
    Ignored,
}

pub struct Received {
    pub consumed: usize,
    pub event: Option<Event>,
}

struct Pending {
    request: u64,
    revision: Revision,
    deadline: Instant,
}

/// At most one pending manual request and one decoder frame, with no output
/// queue. The transport must separately bound connections and its own buffers.
pub struct ClientSession {
    id: Id,
    generation: u64,
    broker: Id,
    status: Status,
    policy: Option<Policy>,
    last_request: u64,
    pending: Option<Pending>,
    decoder: Decoder,
}

impl ClientSession {
    pub fn new(id: Id, broker: Id) -> Self {
        Self {
            id,
            generation: 1,
            broker,
            status: Status::Connecting,
            policy: None,
            last_request: 0,
            pending: None,
            decoder: Decoder::default(),
        }
    }

    pub fn status(&self) -> Status {
        self.status
    }

    pub fn pending_request(&self) -> Option<u64> {
        self.pending.as_ref().map(|pending| pending.request)
    }

    pub fn hello(&self) -> Result<Vec<u8>, SessionError> {
        if self.status != Status::Connecting {
            return Err(SessionError::WrongState);
        }
        self.encode(Message::Hello(Hello {
            broker: self.broker,
        }))
    }

    pub fn begin(
        &mut self,
        input_revision: u64,
        text: &str,
        timeout_ms: u32,
        now: Instant,
    ) -> Result<Vec<u8>, SessionError> {
        if self.status != Status::Ready {
            return Err(SessionError::WrongState);
        }
        if self.pending.is_some() {
            return Err(SessionError::Busy);
        }
        let policy = self.policy.ok_or(SessionError::WrongState)?;
        if !policy.allowed {
            return Err(SessionError::NotAllowed);
        }
        if !(1..=MAX_TIMEOUT_MS).contains(&timeout_ms) {
            return Err(SessionError::InvalidTimeout);
        }
        if !message::valid_text(text) {
            return Err(SessionError::Message(MessageError::InvalidValue));
        }
        let request = self
            .last_request
            .checked_add(1)
            .ok_or(SessionError::Exhausted)?;
        let deadline = now
            .checked_add(Duration::from_millis(timeout_ms.into()))
            .ok_or(SessionError::InvalidTimeout)?;
        let revision = Revision {
            input: input_revision,
            settings: policy.settings,
            authorization: policy.authorization,
        };
        // Reserve the wire result before changing state. The caller must also
        // reserve transport capacity before consuming a host key or target.
        let frame = self.encode(Message::Convert(Convert {
            request,
            revision,
            timeout_ms,
            text: text.to_owned(),
        }))?;
        self.last_request = request;
        self.pending = Some(Pending {
            request,
            revision,
            deadline,
        });
        Ok(frame)
    }

    /// Local cancellation wins immediately; delivery of Cancel is best effort.
    /// A transport unable to queue it must disconnect, never restore the request.
    pub fn cancel(&mut self) -> Result<Option<Vec<u8>>, SessionError> {
        let Some(pending) = &self.pending else {
            return Ok(None);
        };
        let frame = self.encode(Message::Cancel(Cancel {
            request: pending.request,
        }))?;
        self.pending = None;
        Ok(Some(frame))
    }

    pub fn expire(&mut self, now: Instant) -> Option<u64> {
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| now >= pending.deadline)
        {
            self.pending.take().map(|pending| pending.request)
        } else {
            None
        }
    }

    pub fn disconnect(&mut self) {
        self.status = Status::Closed;
        self.pending = None;
        let _ = self.decoder.finish();
    }

    pub fn eof(&mut self) -> Result<(), SessionError> {
        let result = self.decoder.finish().map_err(SessionError::Frame);
        self.disconnect();
        result
    }

    /// Invoke only after authenticating the new transport. Preserve all counters
    /// and require a fresh Ready, even when the broker has not restarted.
    pub fn reconnect(&mut self, broker: Id) -> Result<Vec<u8>, SessionError> {
        if self.status != Status::Closed {
            return Err(SessionError::WrongState);
        }
        self.generation = self
            .generation
            .checked_add(1)
            .ok_or(SessionError::Exhausted)?;
        if broker != self.broker {
            self.policy = None;
        }
        self.broker = broker;
        self.decoder = Decoder::default();
        self.status = Status::Connecting;
        self.hello()
    }

    /// Consume at most one framed message; apply backpressure before calling
    /// again with unconsumed bytes. Every parse/ordering error is terminal.
    /// The owner must cancel its Core state on errors, disconnect and EOF.
    pub fn receive(&mut self, bytes: &[u8], now: Instant) -> Result<Received, SessionError> {
        if self.status == Status::Closed {
            return Err(SessionError::WrongState);
        }
        let result = self.receive_inner(bytes, now);
        if result.is_err() {
            self.disconnect();
        }
        result
    }

    fn receive_inner(&mut self, bytes: &[u8], now: Instant) -> Result<Received, SessionError> {
        let decoded = self.decoder.decode(bytes).map_err(SessionError::Frame)?;
        let consumed = decoded.consumed;
        let envelope = decoded
            .body
            .map(message::decode)
            .transpose()
            .map_err(SessionError::Message)?;
        let event = envelope
            .map(|envelope| self.dispatch(envelope, now))
            .transpose()?;
        Ok(Received { consumed, event })
    }

    fn encode(&self, message: Message) -> Result<Vec<u8>, SessionError> {
        message::encode(&Envelope {
            version: VERSION,
            session: self.id,
            generation: self.generation,
            message,
        })
        .map_err(SessionError::Message)
    }

    fn dispatch(&mut self, envelope: Envelope, now: Instant) -> Result<Event, SessionError> {
        if envelope.session != self.id || envelope.generation != self.generation {
            return Ok(Event::Ignored);
        }
        if let Message::Ready(ready) = envelope.message {
            if self.status != Status::Connecting {
                return Err(SessionError::UnexpectedMessage);
            }
            if ready.broker != self.broker {
                return Err(SessionError::WrongBroker);
            }
            self.check_policy(ready.policy)?;
            self.policy = Some(ready.policy);
            self.status = Status::Ready;
            return Ok(Event::Ready(ready.policy));
        }
        if self.status != Status::Ready {
            return Err(SessionError::WrongState);
        }
        match envelope.message {
            Message::PolicyChanged(policy) => {
                let previous = self.policy.ok_or(SessionError::WrongState)?;
                self.check_policy(policy)?;
                if policy == previous {
                    return Ok(Event::Ignored);
                }
                let cancelled = self.pending.take().map(|pending| pending.request);
                self.policy = Some(policy);
                Ok(Event::PolicyChanged { policy, cancelled })
            }
            Message::Result(result) => self.reply(
                result.request,
                result.revision,
                Outcome::Result(result.text),
                now,
            ),
            Message::Error(error) => self.reply(
                error.request,
                error.revision,
                Outcome::Error(error.code),
                now,
            ),
            _ => Err(SessionError::UnexpectedMessage),
        }
    }

    fn reply(
        &mut self,
        request: u64,
        revision: Revision,
        outcome: Outcome,
        now: Instant,
    ) -> Result<Event, SessionError> {
        let Some(pending) = &self.pending else {
            return if request <= self.last_request {
                Ok(Event::Ignored)
            } else {
                Err(SessionError::UnexpectedMessage)
            };
        };
        if request != pending.request {
            return if request < pending.request {
                Ok(Event::Ignored)
            } else {
                Err(SessionError::UnexpectedMessage)
            };
        }
        if revision != pending.revision {
            return Err(SessionError::RevisionMismatch);
        }
        let pending = self.pending.take().expect("matching pending request");
        if now >= pending.deadline {
            return Ok(Event::Expired(request));
        }
        Ok(Event::Reply {
            request,
            revision,
            outcome,
        })
    }

    fn check_policy(&self, next: Policy) -> Result<(), SessionError> {
        if self.policy.is_some_and(|previous| {
            next.settings < previous.settings
                || next.authorization < previous.authorization
                || (next.allowed != previous.allowed
                    && next.authorization == previous.authorization)
        }) {
            Err(SessionError::PolicyRegression)
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> Policy {
        Policy {
            settings: 2,
            authorization: 3,
            allowed: true,
        }
    }

    fn reply(request: u64) -> Message {
        Message::Result(ConversionResult {
            request,
            revision: Revision {
                input: 1,
                settings: 2,
                authorization: 3,
            },
            text: "日本語".into(),
        })
    }

    fn ready() -> Message {
        Message::Ready(Ready {
            broker: Id::new(2).unwrap(),
            policy: policy(),
        })
    }

    fn frame(message: Message, generation: u64) -> Vec<u8> {
        message::encode(&Envelope {
            version: VERSION,
            session: Id::new(1).unwrap(),
            generation,
            message,
        })
        .unwrap()
    }

    fn deliver(client: &mut ClientSession, message: Message, now: Instant) -> Event {
        let bytes = frame(message, client.generation);
        client.receive(&bytes, now).unwrap().event.unwrap()
    }

    fn connected(now: Instant) -> ClientSession {
        let mut client = ClientSession::new(Id::new(1).unwrap(), Id::new(2).unwrap());
        client.hello().unwrap();
        assert!(matches!(
            deliver(&mut client, ready(), now),
            Event::Ready(_)
        ));
        client
    }

    #[test]
    fn handshake_gates_sending_and_the_single_slot_is_reserved_after_validation() {
        let now = Instant::now();
        let mut client = ClientSession::new(Id::new(1).unwrap(), Id::new(2).unwrap());
        assert_eq!(
            client.begin(1, "fixed", 100, now).err(),
            Some(SessionError::WrongState)
        );
        client.hello().unwrap();
        deliver(&mut client, ready(), now);
        for (input, text, timeout) in [
            (0, "fixed", 100),
            (1, "", 100),
            (1, "fixed", 0),
            (1, "fixed", MAX_TIMEOUT_MS + 1),
        ] {
            assert!(client.begin(input, text, timeout, now).is_err());
            assert_eq!(client.pending_request(), None);
            assert_eq!(client.last_request, 0);
        }
        assert!(client
            .begin(1, &"x".repeat(MAX_TEXT_UNITS + 1), 100, now)
            .is_err());
        client.begin(1, "fixed", 100, now).unwrap();
        assert_eq!(
            client.begin(1, "other", 100, now).err(),
            Some(SessionError::Busy)
        );
        assert_eq!(client.pending_request(), Some(1));
        assert!(matches!(
            deliver(&mut client, reply(1), now),
            Event::Reply { request: 1, .. }
        ));
        assert!(matches!(
            deliver(&mut client, reply(1), now),
            Event::Ignored
        ));
        assert_eq!(client.pending_request(), None);
    }

    #[test]
    fn fragmented_and_coalesced_messages_never_queue_a_second_event() {
        let now = Instant::now();
        let mut client = ClientSession::new(Id::new(1).unwrap(), Id::new(2).unwrap());
        let bytes = frame(ready(), 1);
        for (index, byte) in bytes.iter().enumerate() {
            let received = client.receive(std::slice::from_ref(byte), now).unwrap();
            assert_eq!(received.consumed, 1);
            assert_eq!(received.event.is_some(), index + 1 == bytes.len());
        }
        client.begin(1, "fixed", 100, now).unwrap();
        let first = frame(reply(1), 1);
        let mut both = first.clone();
        both.extend(frame(reply(1), 1));
        let received = client.receive(&both, now).unwrap();
        assert_eq!(received.consumed, first.len());
        assert!(matches!(received.event, Some(Event::Reply { .. })));
        assert!(matches!(
            client.receive(&both[first.len()..], now).unwrap().event,
            Some(Event::Ignored)
        ));
    }

    #[test]
    fn cancellation_and_exact_deadline_win_over_late_replies() {
        let now = Instant::now();
        let mut client = connected(now);
        client.begin(1, "fixed", 100, now).unwrap();
        assert!(client.cancel().unwrap().is_some());
        assert!(client.cancel().unwrap().is_none());
        client.begin(1, "fixed", 100, now).unwrap();
        assert!(matches!(
            deliver(&mut client, reply(1), now),
            Event::Ignored
        ));
        assert_eq!(client.pending_request(), Some(2));
        assert!(matches!(
            deliver(&mut client, reply(2), now + Duration::from_millis(100)),
            Event::Expired(2)
        ));
        client.begin(1, "fixed", 100, now).unwrap();
        assert_eq!(client.expire(now + Duration::from_millis(99)), None);
        assert_eq!(client.expire(now + Duration::from_millis(100)), Some(3));
        assert!(matches!(
            deliver(&mut client, reply(3), now),
            Event::Ignored
        ));
    }

    #[test]
    fn future_reply_ids_close_the_connection_but_old_replies_are_ignored() {
        let now = Instant::now();
        let future_error = Message::Error(ConversionError {
            request: 2,
            revision: Revision {
                input: 1,
                settings: 2,
                authorization: 3,
            },
            code: ErrorCode::Unavailable,
        });
        for message in [reply(2), future_error] {
            let mut client = connected(now);
            client.begin(1, "fixed", 100, now).unwrap();
            assert_eq!(
                client.receive(&frame(message, 1), now).err(),
                Some(SessionError::UnexpectedMessage)
            );
            assert_eq!(client.status(), Status::Closed);
            assert_eq!(client.pending_request(), None);
        }

        let mut client = connected(now);
        assert_eq!(
            client.receive(&frame(reply(1), 1), now).err(),
            Some(SessionError::UnexpectedMessage)
        );
        assert_eq!(client.status(), Status::Closed);

        let mut client = connected(now);
        client.begin(1, "fixed", 100, now).unwrap();
        client.cancel().unwrap();
        assert!(matches!(
            deliver(&mut client, reply(1), now),
            Event::Ignored
        ));
        assert_eq!(
            client.receive(&frame(reply(2), 1), now).err(),
            Some(SessionError::UnexpectedMessage)
        );
        assert_eq!(client.status(), Status::Closed);
    }

    #[test]
    fn reconnect_requires_new_ready_and_keeps_generations_and_request_counters() {
        let now = Instant::now();
        let mut client = connected(now);
        client.begin(1, "fixed", 100, now).unwrap();
        assert_eq!(
            client.reconnect(Id::new(2).unwrap()).err(),
            Some(SessionError::WrongState)
        );
        client.eof().unwrap();
        assert_eq!(client.pending_request(), None);
        client.reconnect(Id::new(2).unwrap()).unwrap();
        assert_eq!(
            client.begin(1, "fixed", 100, now).err(),
            Some(SessionError::WrongState)
        );
        assert!(matches!(
            client.receive(&frame(ready(), 1), now).unwrap().event,
            Some(Event::Ignored)
        ));
        assert_eq!(client.status(), Status::Connecting);
        deliver(&mut client, ready(), now);
        client.begin(1, "fixed", 100, now).unwrap();
        assert_eq!(client.pending_request(), Some(2));
        assert!(matches!(
            client.receive(&frame(reply(2), 1), now).unwrap().event,
            Some(Event::Ignored)
        ));
        assert_eq!(client.pending_request(), Some(2));
        assert!(matches!(
            deliver(&mut client, reply(2), now),
            Event::Reply { .. }
        ));
    }

    #[test]
    fn policy_revocation_cancels_and_rollback_is_rejected_across_reconnect() {
        let now = Instant::now();
        let mut client = connected(now);
        client.begin(1, "fixed", 100, now).unwrap();
        assert!(matches!(
            deliver(&mut client, Message::PolicyChanged(policy()), now),
            Event::Ignored
        ));
        assert_eq!(client.pending_request(), Some(1));
        let revoked = Policy {
            authorization: 4,
            allowed: false,
            ..policy()
        };
        assert!(matches!(
            deliver(&mut client, Message::PolicyChanged(revoked), now),
            Event::PolicyChanged {
                cancelled: Some(1),
                ..
            }
        ));
        assert_eq!(
            client.begin(1, "fixed", 100, now).err(),
            Some(SessionError::NotAllowed)
        );
        assert!(matches!(
            deliver(&mut client, reply(1), now),
            Event::Ignored
        ));
        client.disconnect();
        client.reconnect(Id::new(2).unwrap()).unwrap();
        let bytes = frame(ready(), 2);
        assert_eq!(
            client.receive(&bytes, now).err(),
            Some(SessionError::PolicyRegression)
        );
        assert_eq!(client.status(), Status::Closed);
        // A new, separately authenticated broker boot can start a new policy epoch.
        client.reconnect(Id::new(3).unwrap()).unwrap();
        let reset = Message::Ready(Ready {
            broker: Id::new(3).unwrap(),
            policy: Policy {
                settings: 1,
                authorization: 1,
                allowed: false,
            },
        });
        assert!(matches!(deliver(&mut client, reset, now), Event::Ready(_)));
        let forged_grant = Message::PolicyChanged(Policy {
            settings: 1,
            authorization: 1,
            allowed: true,
        });
        let bytes = frame(forged_grant, 3);
        assert_eq!(
            client.receive(&bytes, now).err(),
            Some(SessionError::PolicyRegression)
        );
    }

    #[test]
    fn settings_only_policy_change_cancels_old_work_and_uses_new_revision() {
        let now = Instant::now();
        let mut client = connected(now);
        client.begin(1, "fixed", 100, now).unwrap();
        let changed = Policy {
            settings: 3,
            ..policy()
        };
        assert!(matches!(
            deliver(&mut client, Message::PolicyChanged(changed), now),
            Event::PolicyChanged {
                cancelled: Some(1),
                ..
            }
        ));
        assert!(matches!(
            deliver(&mut client, reply(1), now),
            Event::Ignored
        ));
        let bytes = client.begin(1, "fixed", 100, now).unwrap();
        let mut decoder = Decoder::default();
        let decoded = decoder.decode(&bytes).unwrap();
        let envelope = message::decode(decoded.body.unwrap()).unwrap();
        let Message::Convert(convert) = envelope.message else {
            panic!("expected conversion after settings update");
        };
        assert_eq!(convert.request, 2);
        assert_eq!(convert.revision.settings, 3);
        assert_eq!(convert.revision.authorization, 3);
    }

    #[test]
    fn wrong_broker_direction_revision_or_schema_closes_the_pending_request() {
        let now = Instant::now();
        let mut client = ClientSession::new(Id::new(1).unwrap(), Id::new(3).unwrap());
        assert_eq!(
            client.receive(&frame(ready(), 1), now).err(),
            Some(SessionError::WrongBroker)
        );
        let mut changed = match reply(1) {
            Message::Result(result) => result,
            _ => unreachable!(),
        };
        changed.revision.authorization += 1;
        for (message, error) in [
            (Message::Result(changed), SessionError::RevisionMismatch),
            (ready(), SessionError::UnexpectedMessage),
            (
                Message::Cancel(Cancel { request: 1 }),
                SessionError::UnexpectedMessage,
            ),
        ] {
            let mut client = connected(now);
            client.begin(1, "fixed", 100, now).unwrap();
            assert_eq!(client.receive(&frame(message, 1), now).err(), Some(error));
            assert_eq!(client.status(), Status::Closed);
            assert_eq!(client.pending_request(), None);
        }
        for invalid in [
            crate::encode("{private malformed body").unwrap(),
            0u32.to_le_bytes().to_vec(),
        ] {
            let mut client = connected(now);
            client.begin(1, "fixed", 100, now).unwrap();
            assert!(client.receive(&invalid, now).is_err());
            assert_eq!(client.status(), Status::Closed);
            assert_eq!(client.pending_request(), None);
        }
    }

    #[test]
    fn truncated_eof_and_fixed_server_error_are_terminal_for_the_request() {
        let now = Instant::now();
        let bytes = frame(reply(1), 1);
        for end in 1..bytes.len() {
            let mut client = connected(now);
            client.begin(1, "fixed", 100, now).unwrap();
            assert!(client.receive(&bytes[..end], now).unwrap().event.is_none());
            assert_eq!(
                client.eof(),
                Err(SessionError::Frame(FrameError::Truncated))
            );
            assert_eq!(client.status(), Status::Closed);
            assert_eq!(client.pending_request(), None);
        }
        let mut client = connected(now);
        client.begin(1, "fixed", 100, now).unwrap();
        let error = Message::Error(ConversionError {
            request: 1,
            revision: Revision {
                input: 1,
                settings: 2,
                authorization: 3,
            },
            code: ErrorCode::Timeout,
        });
        assert!(matches!(
            deliver(&mut client, error.clone(), now),
            Event::Reply {
                outcome: Outcome::Error(ErrorCode::Timeout),
                ..
            }
        ));
        assert!(matches!(deliver(&mut client, error, now), Event::Ignored));
    }

    #[test]
    fn exhausted_counters_do_not_wrap_or_reuse_ids() {
        let now = Instant::now();
        let mut client = connected(now);
        client.last_request = u64::MAX;
        assert_eq!(
            client.begin(1, "fixed", 100, now).err(),
            Some(SessionError::Exhausted)
        );
        assert_eq!(client.pending_request(), None);
        client.disconnect();
        client.generation = u64::MAX;
        assert_eq!(
            client.reconnect(Id::new(2).unwrap()).err(),
            Some(SessionError::Exhausted)
        );
        assert_eq!(client.status(), Status::Closed);
    }
}

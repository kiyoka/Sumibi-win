//! One authenticated transport's Broker-side ordering and policy state.
//! The caller must verify the OS peer before passing even Hello. This layer
//! never performs I/O, stores a COM target, or logs conversion text.
use crate::message::{self, *};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrokerError {
    InvalidMessage,
    WrongState,
    WrongBroker,
    WrongSession,
    Replay,
    Busy,
    PolicyDenied,
    PolicyRegression,
    NoticePending,
    InvalidReply,
    Expired,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Connecting,
    Ready,
    Closed,
}

pub enum Event {
    Ready(Envelope),
    Convert(Convert),
    Cancelled(u64),
    Ignored,
}

#[derive(Clone)]
pub struct PolicyNotice {
    pub frame: Envelope,
    pub cancelled: Option<u64>,
}

struct Pending {
    request: u64,
    revision: Revision,
    deadline: Instant,
}

pub struct BrokerSession {
    broker: Id,
    status: Status,
    session: Option<Id>,
    generation: u64,
    policy: Policy,
    last_request: u64,
    pending: Option<Pending>,
    undelivered_policy: Option<Policy>,
}

impl BrokerSession {
    pub fn new(broker: Id, policy: Policy) -> Result<Self, BrokerError> {
        if !policy.valid() {
            return Err(BrokerError::PolicyDenied);
        }
        Ok(Self {
            broker,
            status: Status::Connecting,
            session: None,
            generation: 0,
            policy,
            last_request: 0,
            pending: None,
            undelivered_policy: None,
        })
    }

    pub fn status(&self) -> Status {
        self.status
    }

    pub fn pending_request(&self) -> Option<u64> {
        self.pending.as_ref().map(|pending| pending.request)
    }

    /// The fanout restores a lifetime request floor before accepting a new
    /// generation's Hello. A reconnect cannot reset the wire request counter.
    pub(crate) fn resume_request_floor(&mut self, floor: u64) -> Result<(), BrokerError> {
        if self.status != Status::Connecting {
            self.disconnect();
            return Err(BrokerError::WrongState);
        }
        self.last_request = floor;
        Ok(())
    }

    pub fn disconnect(&mut self) {
        self.status = Status::Closed;
        self.pending = None;
        self.undelivered_policy = None;
    }

    /// Parse/order one complete, already authenticated frame. Every error is
    /// terminal; the transport must close and must not replay the source text.
    pub fn receive(&mut self, envelope: Envelope, now: Instant) -> Result<Event, BrokerError> {
        if self.status == Status::Closed {
            return Err(BrokerError::WrongState);
        }
        let result = self.receive_inner(envelope, now);
        if result.is_err() {
            self.disconnect();
        }
        result
    }

    fn receive_inner(&mut self, envelope: Envelope, now: Instant) -> Result<Event, BrokerError> {
        envelope
            .validate()
            .map_err(|_| BrokerError::InvalidMessage)?;
        if self.status == Status::Connecting {
            let Message::Hello(Hello { broker }) = envelope.message else {
                return Err(BrokerError::WrongState);
            };
            if broker != self.broker {
                return Err(BrokerError::WrongBroker);
            }
            self.session = Some(envelope.session);
            self.generation = envelope.generation;
            self.status = Status::Ready;
            return Ok(Event::Ready(self.frame(Message::Ready(Ready {
                broker: self.broker,
                policy: self.policy,
            }))?));
        }
        if self.session != Some(envelope.session) || self.generation != envelope.generation {
            return Err(BrokerError::WrongSession);
        }
        if self.undelivered_policy.is_some() {
            return Err(BrokerError::NoticePending);
        }
        match envelope.message {
            Message::Convert(convert) => {
                if self.pending.is_some() {
                    return Err(BrokerError::Busy);
                }
                if convert.request <= self.last_request {
                    return Err(BrokerError::Replay);
                }
                if !self.policy.allowed
                    || convert.revision.settings != self.policy.settings
                    || convert.revision.authorization != self.policy.authorization
                {
                    return Err(BrokerError::PolicyDenied);
                }
                let deadline = now
                    .checked_add(Duration::from_millis(convert.timeout_ms.into()))
                    .ok_or(BrokerError::InvalidMessage)?;
                self.last_request = convert.request;
                self.pending = Some(Pending {
                    request: convert.request,
                    revision: convert.revision,
                    deadline,
                });
                Ok(Event::Convert(convert))
            }
            Message::Cancel(cancel) => {
                if self.pending_request() == Some(cancel.request) {
                    self.pending = None;
                    Ok(Event::Cancelled(cancel.request))
                } else if cancel.request <= self.last_request {
                    Ok(Event::Ignored)
                } else {
                    Err(BrokerError::InvalidMessage)
                }
            }
            _ => Err(BrokerError::WrongState),
        }
    }

    /// Publish one monotonic policy update. The caller must deliver this frame
    /// before accepting more source text, or disconnect this session.
    pub fn update_policy(&mut self, next: Policy) -> Result<Option<PolicyNotice>, BrokerError> {
        if self.status != Status::Ready {
            return Err(BrokerError::WrongState);
        }
        if self.undelivered_policy.is_some() {
            return Err(BrokerError::NoticePending);
        }
        if !next.follows(self.policy) {
            self.disconnect();
            return Err(BrokerError::PolicyRegression);
        }
        if next == self.policy {
            return Ok(None);
        }
        let frame = self.frame(Message::PolicyChanged(next))?;
        let cancelled = self.pending.take().map(|pending| pending.request);
        self.policy = next;
        self.undelivered_policy = Some(next);
        Ok(Some(PolicyNotice { frame, cancelled }))
    }

    /// Call only after the complete PolicyChanged frame has been written.
    /// Failed writes must close the transport instead of acknowledging here.
    pub fn notice_delivered(&mut self, delivered: Policy) -> Result<(), BrokerError> {
        if self.status != Status::Ready || self.undelivered_policy != Some(delivered) {
            self.disconnect();
            return Err(BrokerError::WrongState);
        }
        self.undelivered_policy = None;
        Ok(())
    }

    /// A reply cannot outlive its original request, deadline or policy epoch.
    /// An old completion is ignored; an unissued/future completion is terminal.
    pub fn result(
        &mut self,
        request: u64,
        revision: Revision,
        text: String,
        now: Instant,
    ) -> Result<Option<Envelope>, BrokerError> {
        let Some(pending) = self.take_matching_request(request, revision)? else {
            return Ok(None);
        };
        if !message::valid_text(&text) {
            self.disconnect();
            return Err(BrokerError::InvalidReply);
        }
        if now >= pending.deadline {
            return Err(BrokerError::Expired);
        }
        if !self.policy.allowed
            || revision.settings != self.policy.settings
            || revision.authorization != self.policy.authorization
        {
            return Err(BrokerError::PolicyDenied);
        }
        self.frame(Message::Result(ConversionResult {
            request,
            revision,
            text,
        }))
        .map(Some)
    }

    /// Publish a fixed-code failure for the current request. A Timeout error
    /// may be emitted at its deadline; the client still applies its own
    /// deadline before accepting the reply.
    pub fn error(
        &mut self,
        request: u64,
        revision: Revision,
        code: ErrorCode,
        now: Instant,
    ) -> Result<Option<Envelope>, BrokerError> {
        let Some(pending) = self.take_matching_request(request, revision)? else {
            return Ok(None);
        };
        if now >= pending.deadline && code != ErrorCode::Timeout {
            return Err(BrokerError::Expired);
        }
        if !self.policy.allowed
            || revision.settings != self.policy.settings
            || revision.authorization != self.policy.authorization
        {
            return Err(BrokerError::PolicyDenied);
        }
        self.frame(Message::Error(ConversionError {
            request,
            revision,
            code,
        }))
        .map(Some)
    }

    fn take_matching_request(
        &mut self,
        request: u64,
        revision: Revision,
    ) -> Result<Option<Pending>, BrokerError> {
        if self.status != Status::Ready {
            return Err(BrokerError::WrongState);
        }
        let Some(pending) = self.pending.as_ref() else {
            return if request != 0 && request <= self.last_request {
                Ok(None)
            } else {
                self.disconnect();
                Err(BrokerError::InvalidReply)
            };
        };
        if request != pending.request {
            return if request != 0 && request < pending.request {
                Ok(None)
            } else {
                self.disconnect();
                Err(BrokerError::InvalidReply)
            };
        }
        if revision != pending.revision {
            self.disconnect();
            return Err(BrokerError::InvalidReply);
        }
        Ok(self.pending.take())
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

    fn frame(&self, message: Message) -> Result<Envelope, BrokerError> {
        let frame = Envelope {
            version: VERSION,
            session: self.session.ok_or(BrokerError::WrongState)?,
            generation: self.generation,
            message,
        };
        frame.validate().map_err(|_| BrokerError::InvalidMessage)?;
        Ok(frame)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(value: u128) -> Id {
        Id::new(value).unwrap()
    }

    fn policy() -> Policy {
        Policy {
            settings: 2,
            authorization: 3,
            allowed: true,
        }
    }

    fn revision() -> Revision {
        Revision {
            input: 4,
            settings: 2,
            authorization: 3,
        }
    }

    fn frame(message: Message, generation: u64) -> Envelope {
        Envelope {
            version: VERSION,
            session: id(10),
            generation,
            message,
        }
    }

    fn connected(now: Instant) -> BrokerSession {
        let mut session = BrokerSession::new(id(20), policy()).unwrap();
        let Event::Ready(ready) = session
            .receive(frame(Message::Hello(Hello { broker: id(20) }), 1), now)
            .unwrap()
        else {
            panic!("expected Ready");
        };
        assert!(ready.session == id(10));
        assert_eq!(ready.generation, 1);
        assert!(matches!(ready.message, Message::Ready(_)));
        session
    }

    fn convert(request: u64) -> Message {
        Message::Convert(Convert {
            request,
            revision: revision(),
            timeout_ms: 100,
            text: "nihongo".into(),
        })
    }

    #[test]
    fn hello_requires_expected_boot_and_exact_connection_epoch() {
        let now = Instant::now();
        let mut session = BrokerSession::new(id(20), policy()).unwrap();
        assert_eq!(
            session
                .receive(frame(Message::Hello(Hello { broker: id(21) }), 1), now)
                .err(),
            Some(BrokerError::WrongBroker)
        );
        assert_eq!(session.status(), Status::Closed);
        let mut session = connected(now);
        assert_eq!(
            session.receive(frame(convert(1), 2), now).err(),
            Some(BrokerError::WrongSession)
        );
        assert_eq!(session.pending_request(), None);
        assert_eq!(session.status(), Status::Closed);
    }

    #[test]
    fn cancelled_request_cannot_replay_or_replace_a_new_result() {
        let now = Instant::now();
        let mut session = connected(now);
        assert!(matches!(
            session.receive(frame(convert(1), 1), now).unwrap(),
            Event::Convert(_)
        ));
        assert_eq!(session.pending_request(), Some(1));
        assert!(matches!(
            session
                .receive(frame(Message::Cancel(Cancel { request: 1 }), 1), now)
                .unwrap(),
            Event::Cancelled(1)
        ));
        assert!(session
            .result(1, revision(), "旧".into(), now)
            .unwrap()
            .is_none());
        assert!(matches!(
            session.receive(frame(convert(2), 1), now).unwrap(),
            Event::Convert(_)
        ));
        assert!(session
            .result(1, revision(), "旧".into(), now)
            .unwrap()
            .is_none());
        let reply = session
            .result(2, revision(), "日本語".into(), now)
            .unwrap()
            .unwrap();
        assert!(matches!(reply.message, Message::Result(_)));
        assert_eq!(session.pending_request(), None);
        assert_eq!(
            session.receive(frame(convert(2), 1), now).err(),
            Some(BrokerError::Replay)
        );
        assert_eq!(session.status(), Status::Closed);
    }

    #[test]
    fn policy_change_cancels_pending_and_requires_current_revision() {
        let now = Instant::now();
        let mut session = connected(now);
        session.receive(frame(convert(1), 1), now).unwrap();
        let changed = Policy {
            settings: 3,
            ..policy()
        };
        let notice = session.update_policy(changed).unwrap().unwrap();
        assert_eq!(notice.cancelled, Some(1));
        assert!(matches!(notice.frame.message, Message::PolicyChanged(_)));
        session.notice_delivered(changed).unwrap();
        assert!(session
            .result(1, revision(), "旧".into(), now)
            .unwrap()
            .is_none());
        let mut current = match convert(2) {
            Message::Convert(convert) => convert,
            _ => unreachable!(),
        };
        current.revision.settings = 3;
        session
            .receive(frame(Message::Convert(current.clone()), 1), now)
            .unwrap();
        assert_eq!(
            session
                .result(2, current.revision, "日本語".into(), now)
                .unwrap()
                .unwrap()
                .generation,
            1
        );
        assert_eq!(
            session
                .update_policy(Policy {
                    allowed: false,
                    ..changed
                })
                .err(),
            Some(BrokerError::PolicyRegression)
        );
        assert_eq!(session.status(), Status::Closed);
    }

    #[test]
    fn exact_deadline_and_unissued_result_are_never_published() {
        let now = Instant::now();
        let mut session = connected(now);
        session.receive(frame(convert(1), 1), now).unwrap();
        assert_eq!(
            session
                .result(
                    1,
                    revision(),
                    "日本語".into(),
                    now + Duration::from_millis(100)
                )
                .err(),
            Some(BrokerError::Expired)
        );
        assert_eq!(session.pending_request(), None);
        assert_eq!(
            session.result(2, revision(), "未来".into(), now).err(),
            Some(BrokerError::InvalidReply)
        );
        assert_eq!(session.status(), Status::Closed);
    }

    #[test]
    fn invalid_result_and_busy_second_request_close_without_replay() {
        let now = Instant::now();
        let mut session = connected(now);
        session.receive(frame(convert(1), 1), now).unwrap();
        assert_eq!(
            session
                .result(1, revision(), "a\u{fffc}b".into(), now)
                .err(),
            Some(BrokerError::InvalidReply)
        );
        assert_eq!(session.status(), Status::Closed);
        let mut session = connected(now);
        session.receive(frame(convert(1), 1), now).unwrap();
        assert_eq!(
            session.receive(frame(convert(2), 1), now).err(),
            Some(BrokerError::Busy)
        );
        assert_eq!(session.status(), Status::Closed);
    }

    #[test]
    fn stale_cancel_does_not_remove_new_request_and_future_cancel_is_terminal() {
        let now = Instant::now();
        let mut session = connected(now);
        session.receive(frame(convert(1), 1), now).unwrap();
        session
            .receive(frame(Message::Cancel(Cancel { request: 1 }), 1), now)
            .unwrap();
        session.receive(frame(convert(2), 1), now).unwrap();
        assert!(matches!(
            session
                .receive(frame(Message::Cancel(Cancel { request: 1 }), 1), now)
                .unwrap(),
            Event::Ignored
        ));
        assert_eq!(session.pending_request(), Some(2));
        assert_eq!(
            session
                .receive(frame(Message::Cancel(Cancel { request: 3 }), 1), now)
                .err(),
            Some(BrokerError::InvalidMessage)
        );
        assert_eq!(session.status(), Status::Closed);
    }

    #[test]
    fn timeout_error_can_be_published_at_deadline_but_late_success_cannot() {
        let now = Instant::now();
        let mut session = connected(now);
        session.receive(frame(convert(1), 1), now).unwrap();
        let error = session
            .error(
                1,
                revision(),
                ErrorCode::Timeout,
                now + Duration::from_millis(100),
            )
            .unwrap()
            .unwrap();
        assert!(matches!(error.message, Message::Error(_)));
        assert_eq!(session.pending_request(), None);
        assert!(session
            .result(1, revision(), "遅延".into(), now)
            .unwrap()
            .is_none());
    }

    #[test]
    fn policy_notice_must_be_delivered_before_another_request() {
        let now = Instant::now();
        let changed = Policy {
            settings: 3,
            ..policy()
        };
        let mut session = connected(now);
        session.update_policy(changed).unwrap().unwrap();
        let mut current = match convert(1) {
            Message::Convert(convert) => convert,
            _ => unreachable!(),
        };
        current.revision.settings = 3;
        assert_eq!(
            session
                .receive(frame(Message::Convert(current.clone()), 1), now)
                .err(),
            Some(BrokerError::NoticePending)
        );
        assert_eq!(session.status(), Status::Closed);

        let mut session = connected(now);
        session.update_policy(changed).unwrap().unwrap();
        assert_eq!(
            session
                .update_policy(Policy {
                    settings: 4,
                    ..changed
                })
                .err(),
            Some(BrokerError::NoticePending)
        );
        session.notice_delivered(changed).unwrap();
        assert!(matches!(
            session.receive(frame(Message::Convert(current), 1), now),
            Ok(Event::Convert(_))
        ));
    }

    #[test]
    fn errors_use_the_same_old_future_and_revision_order_as_results() {
        let now = Instant::now();
        let mut session = connected(now);
        session.receive(frame(convert(1), 1), now).unwrap();
        session
            .receive(frame(Message::Cancel(Cancel { request: 1 }), 1), now)
            .unwrap();
        session.receive(frame(convert(2), 1), now).unwrap();
        assert!(session
            .error(1, revision(), ErrorCode::Unavailable, now)
            .unwrap()
            .is_none());
        assert_eq!(session.pending_request(), Some(2));
        let mut wrong = revision();
        wrong.settings += 1;
        assert_eq!(
            session.error(2, wrong, ErrorCode::Unavailable, now).err(),
            Some(BrokerError::InvalidReply)
        );
        assert_eq!(session.status(), Status::Closed);

        let mut session = connected(now);
        session.receive(frame(convert(1), 1), now).unwrap();
        assert_eq!(
            session
                .error(2, revision(), ErrorCode::Unavailable, now)
                .err(),
            Some(BrokerError::InvalidReply)
        );
        assert_eq!(session.status(), Status::Closed);
    }
}

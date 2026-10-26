//! Bounded, transport-independent policy publication across authenticated
//! Broker sessions. Callers serialize access and close retired OS transports.
use crate::broker_session::{BrokerError, BrokerSession, Event, PolicyNotice};
use crate::message::{Envelope, ErrorCode, Id, Message, Policy, Revision};
use std::time::Instant;

pub const MAX_SESSIONS: usize = 8;
/// Provisional per-Broker lifetime cap; restart with a fresh boot ID at this
/// boundary rather than evicting the replay history.
pub const MAX_ATTACHMENTS: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FanoutError {
    InvalidKey,
    Duplicate,
    StaleKey,
    StaleGeneration,
    DuplicateSession,
    Full,
    Exhausted,
    Unknown,
    InvalidPolicy,
    NoNotice,
    NoticeNotClaimed,
    StaleNotice,
    ReadyPending,
    NoReady,
    Broker(BrokerError),
}

struct Subscriber {
    key: u64,
    session_id: Id,
    session: BrokerSession,
    ready_pending: bool,
    notice: Option<PolicyNotice>,
    notice_claimed: bool,
}

struct SeenSession {
    id: Id,
    generation: u64,
    last_request: u64,
}

/// One fixed-size list with at most one unsent policy frame per connection.
/// A second change before delivery retires that connection instead of
/// retaining an unbounded notification backlog or accepting stale source.
pub struct PolicyFanout {
    broker: Id,
    policy: Policy,
    subscribers: Vec<Subscriber>,
    last_key: u64,
    seen: Vec<SeenSession>,
    attachments: usize,
}

impl PolicyFanout {
    pub fn new(broker: Id, policy: Policy) -> Result<Self, FanoutError> {
        if !policy.valid() {
            return Err(FanoutError::InvalidPolicy);
        }
        Ok(Self {
            broker,
            policy,
            subscribers: Vec::new(),
            last_key: 0,
            seen: Vec::new(),
            attachments: 0,
        })
    }

    pub fn policy(&self) -> Policy {
        self.policy
    }

    pub fn active_count(&self) -> usize {
        self.subscribers.len()
    }

    /// Snapshot the connection keys present before a policy publication.
    /// A publication receipt must wait for each of these transports to
    /// receive the new epoch or be closed, including a pending Ready write.
    pub fn subscriber_keys(&self) -> Vec<u64> {
        self.subscribers
            .iter()
            .map(|subscriber| subscriber.key)
            .collect()
    }

    /// Claim each unscheduled notice for a connection whose Ready write has
    /// completed. Release the Fanout lock before pipe I/O, then confirm the
    /// exact policy epoch after each complete write. A failed write requires
    /// detach. Call again after a pending Ready write completes.
    pub fn pending_writes(&mut self) -> Vec<(u64, PolicyNotice)> {
        self.subscribers
            .iter_mut()
            .filter_map(|subscriber| {
                if subscriber.ready_pending || subscriber.notice_claimed {
                    return None;
                }
                let notice = subscriber.notice.as_ref()?.clone();
                subscriber.notice_claimed = true;
                Some((subscriber.key, notice))
            })
            .collect()
    }

    /// Claim a notice while Ready is still pending only when both complete
    /// frames will be written in one ordered operation. After that write,
    /// acknowledge Ready first and then the notice's exact policy epoch.
    pub fn coalesced_ready_notice(&mut self, key: u64) -> Result<PolicyNotice, FanoutError> {
        let index = self.index(key)?;
        let subscriber = &mut self.subscribers[index];
        if !subscriber.ready_pending || subscriber.notice_claimed {
            return Err(FanoutError::NoNotice);
        }
        let notice = subscriber
            .notice
            .as_ref()
            .ok_or(FanoutError::NoNotice)?
            .clone();
        subscriber.notice_claimed = true;
        Ok(notice)
    }

    /// The transport must authenticate its OS peer before calling attach.
    /// Keys increase for this Broker lifetime and are never reused after
    /// detach, so delayed write callbacks cannot address a new connection.
    pub fn attach(
        &mut self,
        key: u64,
        hello: Envelope,
        now: Instant,
    ) -> Result<Envelope, FanoutError> {
        hello
            .validate()
            .map_err(|_| FanoutError::Broker(BrokerError::InvalidMessage))?;
        let Message::Hello(greeting) = &hello.message else {
            return Err(FanoutError::Broker(BrokerError::WrongState));
        };
        if greeting.broker != self.broker {
            return Err(FanoutError::Broker(BrokerError::WrongBroker));
        }
        if key == 0 {
            return Err(FanoutError::InvalidKey);
        }
        if self
            .subscribers
            .iter()
            .any(|subscriber| subscriber.key == key)
        {
            return Err(FanoutError::Duplicate);
        }
        if key <= self.last_key {
            return Err(FanoutError::StaleKey);
        }
        let session_id = hello.session;
        if self
            .subscribers
            .iter()
            .any(|subscriber| subscriber.session_id == session_id)
        {
            return Err(FanoutError::DuplicateSession);
        }
        if self.subscribers.len() == MAX_SESSIONS {
            return Err(FanoutError::Full);
        }
        if self.attachments == MAX_ATTACHMENTS {
            return Err(FanoutError::Exhausted);
        }
        if self
            .seen
            .iter()
            .any(|seen| seen.id == session_id && hello.generation <= seen.generation)
        {
            return Err(FanoutError::StaleGeneration);
        }
        let mut session =
            BrokerSession::new(self.broker, self.policy).map_err(FanoutError::Broker)?;
        if let Some(seen) = self.seen.iter().find(|seen| seen.id == session_id) {
            session
                .resume_request_floor(seen.last_request)
                .map_err(FanoutError::Broker)?;
        }
        let Event::Ready(ready) = session.receive(hello, now).map_err(FanoutError::Broker)? else {
            return Err(FanoutError::Broker(BrokerError::WrongState));
        };
        let generation = ready.generation;
        if let Some(seen) = self.seen.iter_mut().find(|seen| seen.id == session_id) {
            seen.generation = generation;
        } else {
            self.seen.push(SeenSession {
                id: session_id,
                generation,
                last_request: 0,
            });
        }
        self.subscribers.push(Subscriber {
            key,
            session_id,
            session,
            ready_pending: true,
            notice: None,
            notice_claimed: false,
        });
        self.last_key = key;
        self.attachments += 1;
        Ok(ready)
    }

    /// A newly attached transport must finish writing Ready before it can
    /// receive source text or acknowledge a later policy notice.
    pub fn ready_delivered(&mut self, key: u64) -> Result<(), FanoutError> {
        let index = self.index(key)?;
        if !self.subscribers[index].ready_pending {
            self.detach(key);
            return Err(FanoutError::NoReady);
        }
        self.subscribers[index].ready_pending = false;
        Ok(())
    }

    /// Return keys whose transports must be closed. Updates are all-or-retire:
    /// every surviving subscriber has the new policy and a pending notice.
    pub fn publish(&mut self, next: Policy) -> Result<Vec<u64>, FanoutError> {
        if !next.follows(self.policy) {
            return Err(FanoutError::InvalidPolicy);
        }
        if next == self.policy {
            return Ok(Vec::new());
        }
        let mut retired = Vec::new();
        self.subscribers.retain_mut(|subscriber| {
            if subscriber.notice.is_some() {
                subscriber.session.disconnect();
                retired.push(subscriber.key);
                return false;
            }
            match subscriber.session.update_policy(next) {
                Ok(Some(notice)) => {
                    subscriber.notice = Some(notice);
                    subscriber.notice_claimed = false;
                    true
                }
                _ => {
                    subscriber.session.disconnect();
                    retired.push(subscriber.key);
                    false
                }
            }
        });
        self.policy = next;
        Ok(retired)
    }

    /// Only acknowledge the exact epoch after a complete bounded write. On
    /// write failure, call detach and close the transport instead. A late
    /// completion for an older epoch retires this subscriber.
    pub fn notice_delivered(&mut self, key: u64, delivered: Policy) -> Result<(), FanoutError> {
        let index = self.index(key)?;
        if self.subscribers[index].ready_pending {
            self.detach(key);
            return Err(FanoutError::ReadyPending);
        }
        let Some(notice) = self.subscribers[index].notice.as_ref() else {
            self.detach(key);
            return Err(FanoutError::NoNotice);
        };
        if !self.subscribers[index].notice_claimed {
            self.detach(key);
            return Err(FanoutError::NoticeNotClaimed);
        }
        if !matches!(notice.frame.message, Message::PolicyChanged(policy) if policy == delivered) {
            self.detach(key);
            return Err(FanoutError::StaleNotice);
        }
        if let Err(error) = self.subscribers[index].session.notice_delivered(delivered) {
            self.detach(key);
            return Err(FanoutError::Broker(error));
        }
        self.subscribers[index].notice = None;
        self.subscribers[index].notice_claimed = false;
        Ok(())
    }

    pub fn receive(
        &mut self,
        key: u64,
        frame: Envelope,
        now: Instant,
    ) -> Result<Event, FanoutError> {
        let index = self.index(key)?;
        if self.subscribers[index].ready_pending {
            self.detach(key);
            return Err(FanoutError::ReadyPending);
        }
        let result = self.subscribers[index].session.receive(frame, now);
        if let Ok(Event::Convert(convert)) = &result {
            let session_id = self.subscribers[index].session_id;
            let Some(seen) = self.seen.iter_mut().find(|seen| seen.id == session_id) else {
                self.subscribers.remove(index);
                return Err(FanoutError::Broker(BrokerError::WrongState));
            };
            seen.last_request = convert.request;
        }
        if result.is_err() {
            self.subscribers.remove(index);
        }
        result.map_err(FanoutError::Broker)
    }

    pub fn result(
        &mut self,
        key: u64,
        request: u64,
        revision: Revision,
        text: String,
        now: Instant,
    ) -> Result<Option<Envelope>, FanoutError> {
        let index = self.index(key)?;
        let result = self.subscribers[index]
            .session
            .result(request, revision, text, now);
        match result {
            // A worker can complete after the deadline but before the timer
            // calls expire. The request is consumed; the session is still
            // valid and can accept its next monotonically numbered request.
            Err(BrokerError::Expired) => Ok(None),
            Err(error) => {
                self.subscribers.remove(index);
                Err(FanoutError::Broker(error))
            }
            Ok(reply) => Ok(reply),
        }
    }

    pub fn error(
        &mut self,
        key: u64,
        request: u64,
        revision: Revision,
        code: ErrorCode,
        now: Instant,
    ) -> Result<Option<Envelope>, FanoutError> {
        let index = self.index(key)?;
        let result = self.subscribers[index]
            .session
            .error(request, revision, code, now);
        match result {
            Err(BrokerError::Expired) => Ok(None),
            Err(error) => {
                self.subscribers.remove(index);
                Err(FanoutError::Broker(error))
            }
            Ok(reply) => Ok(reply),
        }
    }

    /// Retire one request silently when its local deadline passes. The caller
    /// owns the timer. To send a protocol Timeout response, call error before
    /// expire consumes the request. A later worker completion cannot publish
    /// the expired result.
    pub fn expire(&mut self, key: u64, now: Instant) -> Result<Option<u64>, FanoutError> {
        let index = self.index(key)?;
        Ok(self.subscribers[index].session.expire(now))
    }

    pub fn detach(&mut self, key: u64) -> bool {
        if let Ok(index) = self.index(key) {
            let mut subscriber = self.subscribers.remove(index);
            subscriber.session.disconnect();
            true
        } else {
            false
        }
    }

    fn index(&self, key: u64) -> Result<usize, FanoutError> {
        self.subscribers
            .iter()
            .position(|subscriber| subscriber.key == key)
            .ok_or(FanoutError::Unknown)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::broker_session::Event;
    use crate::message::{Convert, Hello, VERSION};
    use crate::session::{ClientSession, Event as ClientEvent, Outcome};
    use crate::Decoder;

    fn id(value: u128) -> Id {
        Id::new(value).unwrap()
    }
    fn policy() -> Policy {
        Policy {
            settings: 1,
            authorization: 1,
            allowed: true,
        }
    }
    fn hello(client: u128) -> Envelope {
        hello_generation(client, 1)
    }
    fn hello_generation(client: u128, generation: u64) -> Envelope {
        Envelope {
            version: VERSION,
            session: id(client),
            generation,
            message: Message::Hello(Hello { broker: id(90) }),
        }
    }
    fn convert(client: u128, request: u64, policy: Policy) -> Envelope {
        convert_generation(client, 1, request, policy)
    }
    fn convert_generation(client: u128, generation: u64, request: u64, policy: Policy) -> Envelope {
        Envelope {
            version: VERSION,
            session: id(client),
            generation,
            message: Message::Convert(Convert {
                request,
                revision: Revision {
                    input: 1,
                    settings: policy.settings,
                    authorization: policy.authorization,
                },
                timeout_ms: 1000,
                text: "nihongo".into(),
            }),
        }
    }
    fn attach_ready(fanout: &mut PolicyFanout, key: u64, client: u128, now: Instant) -> Envelope {
        let ready = fanout.attach(key, hello(client), now).unwrap();
        fanout.ready_delivered(key).unwrap();
        ready
    }
    fn deliver(fanout: &mut PolicyFanout, key: u64, policy: Policy) -> Result<(), FanoutError> {
        let _ = fanout.pending_writes();
        fanout.notice_delivered(key, policy)
    }

    fn decode_frame(bytes: &[u8]) -> Envelope {
        let mut decoder = Decoder::default();
        let decoded = decoder.decode(bytes).unwrap();
        assert_eq!(decoded.consumed, bytes.len());
        crate::message::decode(decoded.body.unwrap()).unwrap()
    }

    #[test]
    fn revocation_reaches_each_connection_before_another_request() {
        let now = Instant::now();
        let mut fanout = PolicyFanout::new(id(90), policy()).unwrap();
        attach_ready(&mut fanout, 1, 1, now);
        attach_ready(&mut fanout, 2, 2, now);
        fanout.receive(1, convert(1, 1, policy()), now).unwrap();
        let revoked = Policy {
            authorization: 2,
            allowed: false,
            ..policy()
        };
        assert!(fanout.publish(revoked).unwrap().is_empty());
        let writes = fanout.pending_writes();
        assert_eq!(writes.len(), 2);
        assert_eq!(writes[0].1.cancelled, Some(1));
        assert_eq!(writes[1].1.cancelled, None);
        assert!(fanout
            .result(
                1,
                1,
                Revision {
                    input: 1,
                    settings: 1,
                    authorization: 1,
                },
                "遅延結果".into(),
                now,
            )
            .unwrap()
            .is_none());
        deliver(&mut fanout, 1, revoked).unwrap();
        deliver(&mut fanout, 2, revoked).unwrap();
        assert!(matches!(
            fanout.receive(1, convert(1, 2, revoked), now),
            Err(FanoutError::Broker(BrokerError::PolicyDenied))
        ));
        let ready = attach_ready(&mut fanout, 3, 3, now);
        assert!(matches!(ready.message, Message::Ready(ready) if !ready.policy.allowed));
    }

    #[test]
    fn second_change_retires_only_lagging_connection_and_capacity_is_bounded() {
        let now = Instant::now();
        let mut fanout = PolicyFanout::new(id(90), policy()).unwrap();
        for key in 1..=MAX_SESSIONS as u64 {
            attach_ready(&mut fanout, key, key.into(), now);
        }
        assert_eq!(fanout.active_count(), MAX_SESSIONS);
        assert!(matches!(
            fanout.attach(9, hello(9), now),
            Err(FanoutError::Full)
        ));
        let first = Policy {
            settings: 2,
            ..policy()
        };
        fanout.publish(first).unwrap();
        deliver(&mut fanout, 1, first).unwrap();
        let second = Policy {
            settings: 3,
            ..policy()
        };
        assert_eq!(fanout.publish(second).unwrap(), (2..=8).collect::<Vec<_>>());
        assert_eq!(fanout.active_count(), 1);
        let writes = fanout.pending_writes();
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].0, 1);
        assert_eq!(
            fanout.notice_delivered(2, second),
            Err(FanoutError::Unknown)
        );
        let ready = attach_ready(&mut fanout, 9, 9, now);
        assert!(matches!(ready.message, Message::Ready(ready) if ready.policy.settings == 3));
    }

    #[test]
    fn live_connections_cannot_claim_the_same_session_identity() {
        let now = Instant::now();
        let mut fanout = PolicyFanout::new(id(90), policy()).unwrap();
        attach_ready(&mut fanout, 1, 1, now);
        assert!(matches!(
            fanout.attach(2, hello(1), now),
            Err(FanoutError::DuplicateSession)
        ));
        assert_eq!(fanout.active_count(), 1);
        assert!(fanout.detach(1));
        assert!(matches!(
            fanout.attach(2, hello(1), now),
            Err(FanoutError::StaleGeneration)
        ));
        assert!(fanout.attach(2, hello_generation(1, 2), now).is_ok());
    }

    #[test]
    fn lifetime_attachment_cap_keeps_reconnect_history_bounded() {
        let now = Instant::now();
        let mut fanout = PolicyFanout::new(id(90), policy()).unwrap();
        for key in 1..=MAX_ATTACHMENTS as u64 {
            attach_ready(&mut fanout, key, key.into(), now);
            assert!(fanout.detach(key));
        }
        assert_eq!(fanout.active_count(), 0);
        assert!(matches!(
            fanout.attach(65, hello(65), now),
            Err(FanoutError::Exhausted)
        ));
    }

    #[test]
    fn reconnect_preserves_the_last_accepted_request_number() {
        let now = Instant::now();
        let mut fanout = PolicyFanout::new(id(90), policy()).unwrap();
        attach_ready(&mut fanout, 1, 1, now);
        assert!(matches!(
            fanout.receive(1, convert(1, 5, policy()), now),
            Ok(Event::Convert(_))
        ));
        assert!(fanout.detach(1));
        fanout.attach(2, hello_generation(1, 2), now).unwrap();
        fanout.ready_delivered(2).unwrap();
        assert!(matches!(
            fanout.receive(2, convert_generation(1, 2, 5, policy()), now),
            Err(FanoutError::Broker(BrokerError::Replay))
        ));
        fanout.attach(3, hello_generation(1, 3), now).unwrap();
        fanout.ready_delivered(3).unwrap();
        assert!(matches!(
            fanout.receive(3, convert_generation(1, 3, 6, policy()), now),
            Ok(Event::Convert(_))
        ));
    }

    #[test]
    fn retired_transport_key_cannot_name_a_new_connection() {
        let now = Instant::now();
        let mut fanout = PolicyFanout::new(id(90), policy()).unwrap();
        attach_ready(&mut fanout, 1, 1, now);
        assert!(fanout.detach(1));
        assert!(matches!(
            fanout.attach(1, hello(2), now),
            Err(FanoutError::StaleKey)
        ));
        assert!(fanout.attach(2, hello(2), now).is_ok());
    }

    #[test]
    fn invalid_initial_frames_are_rejected_before_session_membership_checks() {
        let now = Instant::now();
        let mut fanout = PolicyFanout::new(id(90), policy()).unwrap();
        attach_ready(&mut fanout, 1, 1, now);
        let mut invalid = hello(1);
        invalid.version = 0;
        assert!(matches!(
            fanout.attach(2, invalid, now),
            Err(FanoutError::Broker(BrokerError::InvalidMessage))
        ));
        let mut wrong_broker = hello(1);
        if let Message::Hello(greeting) = &mut wrong_broker.message {
            greeting.broker = id(91);
        }
        assert!(matches!(
            fanout.attach(2, wrong_broker, now),
            Err(FanoutError::Broker(BrokerError::WrongBroker))
        ));
        assert!(matches!(
            fanout.attach(2, convert(1, 1, policy()), now),
            Err(FanoutError::Broker(BrokerError::WrongState))
        ));
        assert_eq!(fanout.active_count(), 1);
        attach_ready(&mut fanout, 2, 2, now);
    }

    #[test]
    fn failed_publication_leaves_every_session_at_previous_epoch() {
        let now = Instant::now();
        let mut fanout = PolicyFanout::new(id(90), policy()).unwrap();
        attach_ready(&mut fanout, 1, 1, now);
        assert!(matches!(
            fanout.publish(Policy {
                allowed: false,
                ..policy()
            }),
            Err(FanoutError::InvalidPolicy)
        ));
        assert_eq!(fanout.active_count(), 1);
        assert!(fanout.pending_writes().is_empty());
        assert!(matches!(
            fanout.receive(1, convert(1, 1, policy()), now),
            Ok(Event::Convert(_))
        ));
    }

    #[test]
    fn input_before_notice_delivery_retires_only_that_connection() {
        let now = Instant::now();
        let mut fanout = PolicyFanout::new(id(90), policy()).unwrap();
        attach_ready(&mut fanout, 1, 1, now);
        attach_ready(&mut fanout, 2, 2, now);
        let changed = Policy {
            settings: 2,
            ..policy()
        };
        fanout.publish(changed).unwrap();
        assert!(matches!(
            fanout.receive(1, convert(1, 1, changed), now),
            Err(FanoutError::Broker(BrokerError::NoticePending))
        ));
        assert_eq!(fanout.active_count(), 1);
        deliver(&mut fanout, 2, changed).unwrap();
        assert!(matches!(
            fanout.receive(2, convert(2, 1, changed), now),
            Ok(Event::Convert(_))
        ));
    }

    #[test]
    fn late_write_completion_cannot_acknowledge_a_newer_notice() {
        let now = Instant::now();
        let mut fanout = PolicyFanout::new(id(90), policy()).unwrap();
        attach_ready(&mut fanout, 1, 1, now);
        attach_ready(&mut fanout, 2, 2, now);
        let first = Policy {
            settings: 2,
            ..policy()
        };
        fanout.publish(first).unwrap();
        let first_writes = fanout.pending_writes();
        assert_eq!(
            first_writes.iter().map(|(key, _)| *key).collect::<Vec<_>>(),
            [1, 2]
        );
        deliver(&mut fanout, 1, first).unwrap();
        deliver(&mut fanout, 2, first).unwrap();
        let second = Policy {
            settings: 3,
            ..policy()
        };
        fanout.publish(second).unwrap();
        let second_writes = fanout.pending_writes();
        assert_eq!(
            second_writes
                .iter()
                .map(|(key, _)| *key)
                .collect::<Vec<_>>(),
            [1, 2]
        );
        assert!(matches!(
            first_writes[0].1.frame.message,
            Message::PolicyChanged(policy) if policy == first
        ));
        assert_eq!(
            fanout.notice_delivered(first_writes[0].0, first),
            Err(FanoutError::StaleNotice)
        );
        assert_eq!(fanout.active_count(), 1);
        assert_eq!(
            fanout.notice_delivered(1, second),
            Err(FanoutError::Unknown)
        );
        assert!(fanout.pending_writes().is_empty());
        fanout.notice_delivered(2, second).unwrap();
        assert!(matches!(
            fanout.receive(2, convert(2, 1, second), now),
            Ok(Event::Convert(_))
        ));
    }

    #[test]
    fn duplicate_write_completion_retires_that_connection() {
        let now = Instant::now();
        let mut fanout = PolicyFanout::new(id(90), policy()).unwrap();
        attach_ready(&mut fanout, 1, 1, now);
        attach_ready(&mut fanout, 2, 2, now);
        let changed = Policy {
            settings: 2,
            ..policy()
        };
        fanout.publish(changed).unwrap();
        deliver(&mut fanout, 1, changed).unwrap();
        assert_eq!(deliver(&mut fanout, 1, changed), Err(FanoutError::NoNotice));
        assert_eq!(fanout.active_count(), 1);
        deliver(&mut fanout, 2, changed).unwrap();
        assert!(matches!(
            fanout.receive(2, convert(2, 1, changed), now),
            Ok(Event::Convert(_))
        ));
    }

    #[test]
    fn each_notice_is_claimed_once_before_write_completion() {
        let now = Instant::now();
        let mut fanout = PolicyFanout::new(id(90), policy()).unwrap();
        attach_ready(&mut fanout, 1, 1, now);
        attach_ready(&mut fanout, 2, 2, now);
        let changed = Policy {
            settings: 2,
            ..policy()
        };
        fanout.publish(changed).unwrap();
        assert_eq!(
            fanout.notice_delivered(1, changed),
            Err(FanoutError::NoticeNotClaimed)
        );
        let writes = fanout.pending_writes();
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].0, 2);
        assert!(fanout.pending_writes().is_empty());
        fanout.notice_delivered(2, changed).unwrap();
        assert_eq!(fanout.active_count(), 1);
    }

    #[test]
    fn notice_waits_for_ready_unless_both_frames_share_one_write() {
        let now = Instant::now();
        let mut fanout = PolicyFanout::new(id(90), policy()).unwrap();
        fanout.attach(1, hello(1), now).unwrap();
        fanout.attach(2, hello(2), now).unwrap();
        let changed = Policy {
            settings: 2,
            ..policy()
        };
        fanout.publish(changed).unwrap();
        assert!(fanout.pending_writes().is_empty());

        fanout.ready_delivered(1).unwrap();
        let writes = fanout.pending_writes();
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].0, 1);
        let coalesced = fanout.coalesced_ready_notice(2).unwrap();
        assert!(
            matches!(coalesced.frame.message, Message::PolicyChanged(value) if value == changed)
        );
        assert_eq!(
            fanout.coalesced_ready_notice(2).err(),
            Some(FanoutError::NoNotice)
        );
        assert!(fanout.pending_writes().is_empty());
        fanout.ready_delivered(2).unwrap();
        fanout.notice_delivered(1, changed).unwrap();
        fanout.notice_delivered(2, changed).unwrap();
        assert!(fanout.pending_writes().is_empty());
    }

    #[test]
    fn coalesced_ready_and_notice_decode_in_wire_order() {
        let now = Instant::now();
        let mut client = ClientSession::new(id(1), id(90));
        let mut fanout = PolicyFanout::new(id(90), policy()).unwrap();
        let ready = fanout
            .attach(1, decode_frame(&client.hello().unwrap()), now)
            .unwrap();
        let revoked = Policy {
            authorization: 2,
            allowed: false,
            ..policy()
        };
        fanout.publish(revoked).unwrap();
        let notice = fanout.coalesced_ready_notice(1).unwrap();
        let ready_bytes = crate::message::encode(&ready).unwrap();
        let mut wire = ready_bytes.clone();
        wire.extend(crate::message::encode(&notice.frame).unwrap());

        let first = client.receive(&wire, now).unwrap();
        assert_eq!(first.consumed, ready_bytes.len());
        assert!(matches!(first.event, Some(ClientEvent::Ready(value)) if value == policy()));
        let second = client.receive(&wire[first.consumed..], now).unwrap();
        assert!(
            matches!(second.event, Some(ClientEvent::PolicyChanged { policy, cancelled: None }) if policy == revoked)
        );
        assert_eq!(first.consumed + second.consumed, wire.len());
        fanout.ready_delivered(1).unwrap();
        fanout.notice_delivered(1, revoked).unwrap();
        assert!(matches!(
            client.begin(1, "nihongo", 1000, now),
            Err(crate::session::SessionError::NotAllowed)
        ));
    }

    #[test]
    fn source_before_ready_write_completion_retires_only_its_connection() {
        let now = Instant::now();
        let mut fanout = PolicyFanout::new(id(90), policy()).unwrap();
        fanout.attach(1, hello(1), now).unwrap();
        fanout.attach(2, hello(2), now).unwrap();
        assert!(matches!(
            fanout.receive(1, convert(1, 1, policy()), now),
            Err(FanoutError::ReadyPending)
        ));
        assert_eq!(fanout.active_count(), 1);
        fanout.ready_delivered(2).unwrap();
        assert!(matches!(
            fanout.receive(2, convert(2, 1, policy()), now),
            Ok(Event::Convert(_))
        ));
        assert_eq!(fanout.ready_delivered(2), Err(FanoutError::NoReady));
        assert_eq!(fanout.active_count(), 0);
    }

    #[test]
    fn coalesced_notice_still_waits_for_complete_ready_write() {
        let now = Instant::now();
        let changed = Policy {
            authorization: 2,
            allowed: false,
            ..policy()
        };
        let mut fanout = PolicyFanout::new(id(90), policy()).unwrap();
        fanout.attach(1, hello(1), now).unwrap();
        fanout.attach(2, hello(2), now).unwrap();
        fanout.publish(changed).unwrap();
        assert_eq!(
            deliver(&mut fanout, 1, changed),
            Err(FanoutError::ReadyPending)
        );
        assert_eq!(fanout.active_count(), 1);
        fanout.ready_delivered(2).unwrap();
        deliver(&mut fanout, 2, changed).unwrap();
        assert!(fanout.pending_writes().is_empty());
    }

    #[test]
    fn expiry_is_scoped_to_one_connection_and_discards_late_completion() {
        let now = Instant::now();
        let mut fanout = PolicyFanout::new(id(90), policy()).unwrap();
        attach_ready(&mut fanout, 1, 1, now);
        attach_ready(&mut fanout, 2, 2, now);
        fanout.receive(1, convert(1, 1, policy()), now).unwrap();
        fanout.receive(2, convert(2, 1, policy()), now).unwrap();
        let deadline = now + std::time::Duration::from_millis(1000);
        assert_eq!(fanout.expire(1, deadline).unwrap(), Some(1));
        assert_eq!(fanout.expire(1, deadline).unwrap(), None);
        let revision = Revision {
            input: 1,
            settings: 1,
            authorization: 1,
        };
        assert!(fanout
            .result(1, 1, revision, "古い".into(), deadline)
            .unwrap()
            .is_none());
        assert!(fanout
            .error(1, 1, revision, ErrorCode::Timeout, deadline)
            .unwrap()
            .is_none());
        assert!(fanout
            .result(2, 1, revision, "日本語".into(), now)
            .unwrap()
            .is_some());
        fanout.receive(2, convert(2, 2, policy()), now).unwrap();
        assert!(matches!(
            fanout
                .error(2, 2, revision, ErrorCode::Timeout, deadline)
                .unwrap()
                .unwrap()
                .message,
            Message::Error(_)
        ));
        assert!(matches!(
            fanout.receive(1, convert(1, 2, policy()), deadline),
            Ok(Event::Convert(_))
        ));
    }

    #[test]
    fn late_worker_completion_before_timer_keeps_connection_reusable() {
        let now = Instant::now();
        let mut fanout = PolicyFanout::new(id(90), policy()).unwrap();
        attach_ready(&mut fanout, 1, 1, now);
        attach_ready(&mut fanout, 2, 2, now);
        fanout.receive(1, convert(1, 1, policy()), now).unwrap();
        fanout.receive(2, convert(2, 1, policy()), now).unwrap();
        let deadline = now + std::time::Duration::from_millis(1000);
        let revision = Revision {
            input: 1,
            settings: 1,
            authorization: 1,
        };
        // The worker wins the scheduler race with expire, but its success is
        // already too late for the client. It must not retire this transport.
        assert!(fanout
            .result(1, 1, revision, "遅延".into(), deadline)
            .unwrap()
            .is_none());
        assert_eq!(fanout.active_count(), 2);
        assert_eq!(fanout.expire(1, deadline).unwrap(), None);
        assert!(fanout
            .result(2, 1, revision, "継続".into(), now)
            .unwrap()
            .is_some());
        fanout
            .receive(1, convert(1, 2, policy()), deadline)
            .unwrap();
        let second_deadline = deadline + std::time::Duration::from_millis(1000);
        assert!(fanout
            .error(1, 2, revision, ErrorCode::Unavailable, second_deadline)
            .unwrap()
            .is_none());
        assert_eq!(fanout.active_count(), 2);
        assert!(matches!(
            fanout.receive(1, convert(1, 3, policy()), second_deadline),
            Ok(Event::Convert(_))
        ));
    }

    #[test]
    fn client_reconnect_and_broker_replay_floor_agree_on_real_frames() {
        let now = Instant::now();
        let mut client = ClientSession::new(id(1), id(90));
        let mut fanout = PolicyFanout::new(id(90), policy()).unwrap();
        let ready = fanout
            .attach(1, decode_frame(&client.hello().unwrap()), now)
            .unwrap();
        let ready_bytes = crate::message::encode(&ready).unwrap();
        assert!(matches!(
            client.receive(&ready_bytes, now).unwrap().event,
            Some(ClientEvent::Ready(_))
        ));
        fanout.ready_delivered(1).unwrap();

        let first = client.begin(1, "nihongo", 1000, now).unwrap();
        assert!(matches!(
            fanout.receive(1, decode_frame(&first), now),
            Ok(Event::Convert(convert)) if convert.request == 1
        ));
        assert!(fanout.detach(1));
        client.disconnect();

        let reconnect = client.reconnect(id(90)).unwrap();
        let ready = fanout.attach(2, decode_frame(&reconnect), now).unwrap();
        let ready_bytes = crate::message::encode(&ready).unwrap();
        assert!(matches!(
            client.receive(&ready_bytes, now).unwrap().event,
            Some(ClientEvent::Ready(_))
        ));
        fanout.ready_delivered(2).unwrap();
        let second = client.begin(2, "nihongo", 1000, now).unwrap();
        let Event::Convert(convert) = fanout.receive(2, decode_frame(&second), now).unwrap() else {
            panic!("expected Convert");
        };
        assert_eq!(convert.request, 2);
        assert!(fanout
            .result(2, 1, convert.revision, "古い".into(), now)
            .unwrap()
            .is_none());
        let result = fanout
            .result(2, 2, convert.revision, "日本語".into(), now)
            .unwrap()
            .unwrap();
        assert!(matches!(
            client
                .receive(&crate::message::encode(&result).unwrap(), now)
                .unwrap()
                .event,
            Some(ClientEvent::Reply {
                request: 2,
                outcome: Outcome::Result(ref text),
                ..
            }) if text == "日本語"
        ));
    }
}

//! Bounded, transport-independent policy publication across authenticated
//! Broker sessions. Callers serialize access and close retired OS transports.
use crate::broker_session::{BrokerError, BrokerSession, Event, PolicyNotice};
use crate::message::{Envelope, ErrorCode, Id, Message, Policy, Revision};
use std::time::Instant;

pub const MAX_SESSIONS: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FanoutError {
    InvalidKey,
    Duplicate,
    Full,
    Unknown,
    InvalidPolicy,
    NoNotice,
    Broker(BrokerError),
}

struct Subscriber {
    key: u64,
    session: BrokerSession,
    notice: Option<PolicyNotice>,
}

/// One fixed-size list with at most one unsent policy frame per connection.
/// A second change before delivery retires that connection instead of
/// retaining an unbounded notification backlog or accepting stale source.
pub struct PolicyFanout {
    broker: Id,
    policy: Policy,
    subscribers: Vec<Subscriber>,
}

impl PolicyFanout {
    pub fn new(broker: Id, policy: Policy) -> Result<Self, FanoutError> {
        BrokerSession::new(broker, policy).map_err(FanoutError::Broker)?;
        Ok(Self {
            broker,
            policy,
            subscribers: Vec::new(),
        })
    }

    pub fn policy(&self) -> Policy {
        self.policy
    }

    pub fn active_count(&self) -> usize {
        self.subscribers.len()
    }

    /// The transport must authenticate its OS peer before calling attach.
    pub fn attach(
        &mut self,
        key: u64,
        hello: Envelope,
        now: Instant,
    ) -> Result<Envelope, FanoutError> {
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
        if self.subscribers.len() == MAX_SESSIONS {
            return Err(FanoutError::Full);
        }
        let mut session =
            BrokerSession::new(self.broker, self.policy).map_err(FanoutError::Broker)?;
        let Event::Ready(ready) = session.receive(hello, now).map_err(FanoutError::Broker)? else {
            return Err(FanoutError::Broker(BrokerError::WrongState));
        };
        self.subscribers.push(Subscriber {
            key,
            session,
            notice: None,
        });
        Ok(ready)
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

    pub fn pending_notice(&self, key: u64) -> Result<Option<&PolicyNotice>, FanoutError> {
        let subscriber = self
            .subscribers
            .iter()
            .find(|subscriber| subscriber.key == key)
            .ok_or(FanoutError::Unknown)?;
        Ok(subscriber.notice.as_ref())
    }

    /// Only acknowledge after a complete bounded write. On write failure,
    /// call detach and close the transport instead.
    pub fn notice_delivered(&mut self, key: u64) -> Result<(), FanoutError> {
        let subscriber = self
            .subscribers
            .iter_mut()
            .find(|subscriber| subscriber.key == key)
            .ok_or(FanoutError::Unknown)?;
        let notice = subscriber.notice.as_ref().ok_or(FanoutError::NoNotice)?;
        let Message::PolicyChanged(policy) = notice.frame.message else {
            return Err(FanoutError::InvalidPolicy);
        };
        subscriber
            .session
            .notice_delivered(policy)
            .map_err(FanoutError::Broker)?;
        subscriber.notice = None;
        Ok(())
    }

    pub fn receive(
        &mut self,
        key: u64,
        frame: Envelope,
        now: Instant,
    ) -> Result<Event, FanoutError> {
        let index = self.index(key)?;
        let result = self.subscribers[index].session.receive(frame, now);
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
        if result.is_err() {
            self.subscribers.remove(index);
        }
        result.map_err(FanoutError::Broker)
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
        if result.is_err() {
            self.subscribers.remove(index);
        }
        result.map_err(FanoutError::Broker)
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
        Envelope {
            version: VERSION,
            session: id(client),
            generation: 1,
            message: Message::Hello(Hello { broker: id(90) }),
        }
    }
    fn convert(client: u128, request: u64, policy: Policy) -> Envelope {
        Envelope {
            version: VERSION,
            session: id(client),
            generation: 1,
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

    #[test]
    fn revocation_reaches_each_connection_before_another_request() {
        let now = Instant::now();
        let mut fanout = PolicyFanout::new(id(90), policy()).unwrap();
        fanout.attach(1, hello(1), now).unwrap();
        fanout.attach(2, hello(2), now).unwrap();
        fanout.receive(1, convert(1, 1, policy()), now).unwrap();
        let revoked = Policy {
            authorization: 2,
            allowed: false,
            ..policy()
        };
        assert!(fanout.publish(revoked).unwrap().is_empty());
        assert_eq!(
            fanout.pending_notice(1).unwrap().unwrap().cancelled,
            Some(1)
        );
        assert_eq!(fanout.pending_notice(2).unwrap().unwrap().cancelled, None);
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
        fanout.notice_delivered(1).unwrap();
        fanout.notice_delivered(2).unwrap();
        assert!(matches!(
            fanout.receive(1, convert(1, 2, revoked), now),
            Err(FanoutError::Broker(BrokerError::PolicyDenied))
        ));
        let ready = fanout.attach(3, hello(3), now).unwrap();
        assert!(matches!(ready.message, Message::Ready(ready) if !ready.policy.allowed));
    }

    #[test]
    fn second_change_retires_only_lagging_connection_and_capacity_is_bounded() {
        let now = Instant::now();
        let mut fanout = PolicyFanout::new(id(90), policy()).unwrap();
        for key in 1..=MAX_SESSIONS as u64 {
            fanout.attach(key, hello(key.into()), now).unwrap();
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
        fanout.notice_delivered(1).unwrap();
        let second = Policy {
            settings: 3,
            ..policy()
        };
        assert_eq!(fanout.publish(second).unwrap(), (2..=8).collect::<Vec<_>>());
        assert_eq!(fanout.active_count(), 1);
        assert!(fanout.pending_notice(1).unwrap().is_some());
        assert!(matches!(
            fanout.pending_notice(2),
            Err(FanoutError::Unknown)
        ));
        let ready = fanout.attach(9, hello(9), now).unwrap();
        assert!(matches!(ready.message, Message::Ready(ready) if ready.policy.settings == 3));
    }

    #[test]
    fn failed_publication_leaves_every_session_at_previous_epoch() {
        let now = Instant::now();
        let mut fanout = PolicyFanout::new(id(90), policy()).unwrap();
        fanout.attach(1, hello(1), now).unwrap();
        assert!(matches!(
            fanout.publish(Policy {
                allowed: false,
                ..policy()
            }),
            Err(FanoutError::InvalidPolicy)
        ));
        assert_eq!(fanout.active_count(), 1);
        assert!(fanout.pending_notice(1).unwrap().is_none());
        assert!(matches!(
            fanout.receive(1, convert(1, 1, policy()), now),
            Ok(Event::Convert(_))
        ));
    }

    #[test]
    fn input_before_notice_delivery_retires_only_that_connection() {
        let now = Instant::now();
        let mut fanout = PolicyFanout::new(id(90), policy()).unwrap();
        fanout.attach(1, hello(1), now).unwrap();
        fanout.attach(2, hello(2), now).unwrap();
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
        fanout.notice_delivered(2).unwrap();
        assert!(matches!(
            fanout.receive(2, convert(2, 1, changed), now),
            Ok(Event::Convert(_))
        ));
    }
}

//! Bounded bridge from Host settings/consent authority to Broker admission
//! and per-connection notices. The caller owns the I/O loop and must close
//! retired transports and finish notice writes before accepting source text.
use crate::policy::{ChangeError, PolicyAuthority, PolicyVersion};
use std::time::Instant;
use sumibi_ipc::{broker::BrokerPolicyGate, Error as IpcError};
use sumibi_protocol::{message::Policy, policy_fanout::PolicyFanout};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelayError {
    Source(ChangeError),
    Broker(IpcError),
    Diverged,
}

pub struct Published {
    pub policy: Policy,
    pub retired: Vec<u64>,
}

/// One serialized publisher per Broker. Keep the cursor only after the
/// admission gate and every surviving session have accepted the same epoch.
pub struct BrokerPolicyRelay<'a> {
    source: &'a PolicyAuthority,
    cursor: PolicyVersion,
}

impl<'a> BrokerPolicyRelay<'a> {
    pub fn new(
        source: &'a PolicyAuthority,
        gate: &BrokerPolicyGate,
        fanout: &PolicyFanout,
    ) -> Result<Self, RelayError> {
        let policy = source.wire_policy().map_err(RelayError::Source)?;
        if gate.snapshot().map_err(RelayError::Broker)? != policy || fanout.policy() != policy {
            return Err(RelayError::Diverged);
        }
        Ok(Self {
            source,
            cursor: version(policy),
        })
    }

    /// Run on a Broker worker, never the TSF owner thread. Do not lock the
    /// Fanout while waiting; other connections must keep making progress.
    pub fn wait_for_change(&self, deadline: Instant) -> Result<bool, RelayError> {
        self.source
            .wait_for_change(self.cursor, deadline)
            .map(|change| change.is_some())
            .map_err(RelayError::Source)
    }

    /// After a wake, acquire the shared Fanout lock and publish the latest
    /// source snapshot. A failed publication leaves the cursor old. The caller
    /// closes retired transports and completes each remaining notice write.
    pub fn publish_current(
        &mut self,
        gate: &BrokerPolicyGate,
        fanout: &mut PolicyFanout,
    ) -> Result<Option<Published>, RelayError> {
        let Some(policy) = self
            .source
            .wire_change_after(self.cursor)
            .map_err(RelayError::Source)?
        else {
            return Ok(None);
        };
        let retired = gate.publish(fanout, policy).map_err(RelayError::Broker)?;
        self.cursor = version(policy);
        Ok(Some(Published { policy, retired }))
    }
}

fn version(policy: Policy) -> PolicyVersion {
    PolicyVersion {
        settings: policy.settings,
        authorization: policy.authorization,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manual::{Credential, Destination, Scheme};
    use sumibi_protocol::message::{Envelope, Hello, Id, Message, VERSION};

    fn id(value: u128) -> Id {
        Id::new(value).unwrap()
    }

    #[test]
    fn host_consent_changes_publish_to_two_broker_sessions() {
        let destination = Destination::new(Scheme::Https, "example.org", 443, "/").unwrap();
        let source = PolicyAuthority::new(destination.clone());
        let initial = source.wire_policy().unwrap();
        let gate = BrokerPolicyGate::new(initial).unwrap();
        let mut fanout = PolicyFanout::new(id(90), initial).unwrap();
        let now = Instant::now();
        for key in 1..=2 {
            let hello = Envelope {
                version: VERSION,
                session: id(key.into()),
                generation: 1,
                message: Message::Hello(Hello { broker: id(90) }),
            };
            fanout.attach(key, hello, now).unwrap();
            fanout.ready_delivered(key).unwrap();
        }
        let mut relay = BrokerPolicyRelay::new(&source, &gate, &fanout).unwrap();
        assert!(!relay.wait_for_change(now).unwrap());
        assert!(relay.publish_current(&gate, &mut fanout).unwrap().is_none());

        source
            .replace_credential(Credential::new("fixture".into()))
            .unwrap();
        source.grant_manual_consent(&destination).unwrap();
        assert!(relay.wait_for_change(now).unwrap());
        let grant = relay.publish_current(&gate, &mut fanout).unwrap().unwrap();
        assert!(grant.policy.allowed);
        assert!(grant.retired.is_empty());
        assert!(gate.snapshot().unwrap() == grant.policy);
        for key in 1..=2 {
            assert!(fanout.pending_notice(key).unwrap().is_some());
            fanout.notice_delivered(key, grant.policy).unwrap();
        }

        source.revoke_manual_consent().unwrap();
        assert!(relay.wait_for_change(now).unwrap());
        let revoke = relay.publish_current(&gate, &mut fanout).unwrap().unwrap();
        assert!(!revoke.policy.allowed);
        assert!(revoke.retired.is_empty());
        assert!(gate.snapshot().unwrap() == revoke.policy);
        for key in 1..=2 {
            assert!(fanout.pending_notice(key).unwrap().is_some());
        }
    }

    #[test]
    fn divergence_rejects_publication_without_advancing_the_cursor() {
        let destination = Destination::new(Scheme::Https, "example.org", 443, "/").unwrap();
        let source = PolicyAuthority::new(destination);
        let initial = source.wire_policy().unwrap();
        let gate = BrokerPolicyGate::new(initial).unwrap();
        let mut fanout = PolicyFanout::new(id(90), initial).unwrap();
        let ahead = Policy {
            authorization: initial.authorization + 1,
            allowed: false,
            ..initial
        };
        fanout.publish(ahead).unwrap();
        assert!(matches!(
            BrokerPolicyRelay::new(&source, &gate, &fanout),
            Err(RelayError::Diverged)
        ));

        let mut matching = PolicyFanout::new(id(90), initial).unwrap();
        let mut relay = BrokerPolicyRelay::new(&source, &gate, &matching).unwrap();
        let first_cursor = relay.cursor;
        source.revoke_manual_consent().unwrap();
        matching.publish(ahead).unwrap();
        assert_eq!(
            relay.publish_current(&gate, &mut matching).err(),
            Some(RelayError::Broker(IpcError::PolicyDenied))
        );
        assert_eq!(relay.cursor, first_cursor);
        assert!(gate.snapshot().unwrap() == initial);
    }

    #[test]
    fn source_change_between_wake_and_publication_uses_latest_epoch() {
        let destination = Destination::new(Scheme::Https, "example.org", 443, "/").unwrap();
        let source = PolicyAuthority::new(destination.clone());
        let initial = source.wire_policy().unwrap();
        let gate = BrokerPolicyGate::new(initial).unwrap();
        let mut fanout = PolicyFanout::new(id(90), initial).unwrap();
        let mut relay = BrokerPolicyRelay::new(&source, &gate, &fanout).unwrap();
        source
            .replace_credential(Credential::new("fixture".into()))
            .unwrap();
        source.grant_manual_consent(&destination).unwrap();
        assert!(relay.wait_for_change(Instant::now()).unwrap());
        source.revoke_manual_consent().unwrap();
        let published = relay.publish_current(&gate, &mut fanout).unwrap().unwrap();
        assert!(!published.policy.allowed);
        assert!(published.policy == source.wire_policy().unwrap());
        assert!(gate.snapshot().unwrap() == published.policy);
        assert!(!relay.wait_for_change(Instant::now()).unwrap());
    }
}

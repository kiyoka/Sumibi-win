//! Serialized Host-side policy delivery over authenticated Broker pipes.
//! The caller owns accept/read scheduling; this layer keeps connection keys,
//! policy publication, bounded writes, and physical close receipts together.
use crate::broker_policy::{BrokerPolicyRelay, DeliveryCompletion, DeliveryError, RelayError};
use crate::policy::{ChangeError, PolicyAuthority};
use std::time::Instant;
use sumibi_ipc::{
    broker::BrokerPolicyGate, Error as IpcError, Pipe, Stop, WriteBatch, WriteRequest,
};
use sumibi_protocol::{
    broker_session::Event,
    message::{self, Envelope, Id, Policy},
    policy_fanout::{FanoutError, PolicyFanout},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeError {
    Source(ChangeError),
    Relay(RelayError),
    Transport(IpcError),
    Fanout(FanoutError),
    Delivery(DeliveryError),
    Exhausted,
    MissingPipe,
    Incomplete,
    Retired,
    PolicyPending,
}

pub struct PublicationOutcome {
    pub policy: Policy,
    pub delivered: Vec<u64>,
    pub closed: Vec<u64>,
    /// Requests invalidated by this epoch. The caller cancels their worker
    /// jobs by connection key; closed connections retire every remaining job.
    pub cancelled: Vec<(u64, u64)>,
    pub completion: DeliveryCompletion,
}

struct Connection {
    key: u64,
    pipe: Pipe,
}

/// One Broker boot's serialized policy path. The caller must use the same
/// worker for `attach` and `publish_current`, and must not accept source text
/// until pending source changes have been published.
pub struct BrokerPolicyRuntime<'a> {
    source: &'a PolicyAuthority,
    gate: BrokerPolicyGate,
    fanout: PolicyFanout,
    relay: BrokerPolicyRelay<'a>,
    connections: Vec<Connection>,
    last_key: u64,
    failed: bool,
}

impl<'a> BrokerPolicyRuntime<'a> {
    pub fn new(source: &'a PolicyAuthority, broker: Id) -> Result<Self, RuntimeError> {
        let initial = source.wire_policy().map_err(RuntimeError::Source)?;
        let gate = BrokerPolicyGate::new(initial).map_err(RuntimeError::Transport)?;
        let fanout = PolicyFanout::new(broker, initial).map_err(RuntimeError::Fanout)?;
        let relay = BrokerPolicyRelay::new(source, &gate, &fanout).map_err(RuntimeError::Relay)?;
        Ok(Self {
            source,
            gate,
            fanout,
            relay,
            connections: Vec::new(),
            last_key: 0,
            failed: false,
        })
    }

    pub fn policy(&self) -> Policy {
        self.fanout.policy()
    }

    pub fn active_count(&self) -> usize {
        self.connections.len()
    }

    /// Close a disconnected OS endpoint and retire only its session. The
    /// caller never reuses its connection key for another accepted pipe.
    pub fn disconnect(&mut self, key: u64) -> bool {
        self.close(key)
    }

    pub fn wait_for_change(&mut self, deadline: Instant) -> Result<bool, RuntimeError> {
        if self.failed {
            return Err(RuntimeError::Retired);
        }
        let result = self
            .relay
            .wait_for_change(deadline)
            .map_err(RuntimeError::Relay);
        if result.is_err() {
            self.failed = true;
            self.close_all();
        }
        result
    }

    /// `pipe` has already passed OS peer authentication. Consume a key even
    /// when Hello or Ready fails; no later callback may address a new pipe.
    pub fn attach(
        &mut self,
        mut pipe: Pipe,
        hello: Envelope,
        stop: &Stop,
        deadline: Instant,
    ) -> Result<u64, RuntimeError> {
        if self.failed {
            return Err(RuntimeError::Retired);
        }
        let key = self
            .last_key
            .checked_add(1)
            .ok_or(RuntimeError::Exhausted)?;
        self.last_key = key;
        self.require_current_source()?;
        let ready = self
            .fanout
            .attach(key, hello, Instant::now())
            .map_err(RuntimeError::Fanout)?;
        let frame = match message::encode(&ready) {
            Ok(frame) => frame,
            Err(_) => {
                self.fanout.detach(key);
                return Err(RuntimeError::Incomplete);
            }
        };
        if let Err(error) = pipe.write_all(frame, stop, deadline) {
            self.fanout.detach(key);
            return Err(RuntimeError::Transport(error));
        }
        if let Err(error) = self.fanout.ready_delivered(key) {
            self.fanout.detach(key);
            return Err(RuntimeError::Fanout(error));
        }
        self.connections.push(Connection { key, pipe });
        Ok(key)
    }

    /// Admit a frame only against the latest Host source epoch. A source
    /// change that has not reached Fanout closes this pipe without forwarding
    /// its text or automatically retrying it under the next epoch.
    pub fn receive(
        &mut self,
        key: u64,
        frame: Envelope,
        now: Instant,
    ) -> Result<Event, RuntimeError> {
        if self.failed {
            return Err(RuntimeError::Retired);
        }
        if !self
            .connections
            .iter()
            .any(|connection| connection.key == key)
        {
            return Err(RuntimeError::MissingPipe);
        }
        if let Err(error) = self.require_current_source() {
            self.close(key);
            return Err(error);
        }
        let event = match self.fanout.receive(key, frame, now) {
            Ok(event) => event,
            Err(error) => {
                self.close(key);
                return Err(RuntimeError::Fanout(error));
            }
        };
        if let Event::Convert(convert) = &event {
            if let Err(error) = self.gate.authorize(convert.revision) {
                self.failed = true;
                self.close_all();
                return Err(RuntimeError::Transport(error));
            }
            if let Err(error) = self.require_current_source() {
                self.close(key);
                return Err(error);
            }
        }
        Ok(event)
    }

    /// Publish the latest Host epoch, then write or close every connection
    /// that existed before publication. Return only after its receipt is
    /// complete. A failed endpoint does not block the others.
    pub fn publish_current(
        &mut self,
        stop: &Stop,
        deadline: Instant,
    ) -> Result<Option<PublicationOutcome>, RuntimeError> {
        if self.failed {
            return Err(RuntimeError::Retired);
        }
        let result = self.publish_current_inner(stop, deadline);
        if result.is_err() {
            self.failed = true;
            self.close_all();
        }
        result
    }

    fn publish_current_inner(
        &mut self,
        stop: &Stop,
        deadline: Instant,
    ) -> Result<Option<PublicationOutcome>, RuntimeError> {
        let Some(mut published) = self
            .relay
            .publish_current(&self.gate, &mut self.fanout)
            .map_err(RuntimeError::Relay)?
        else {
            return Ok(None);
        };
        let broker = published.delivery.broker_id();
        let mut closed = Vec::new();
        for key in published.retired {
            if !self.close(key) {
                return Err(RuntimeError::MissingPipe);
            }
            published
                .delivery
                .transport_closed(broker, key)
                .map_err(RuntimeError::Delivery)?;
            closed.push(key);
        }
        let mut frames = Vec::with_capacity(published.notices.len());
        let mut cancelled = Vec::new();
        for (key, notice) in published.notices {
            if let Some(request) = notice.cancelled {
                cancelled.push((key, request));
            }
            let frame = message::encode(&notice.frame).map_err(|_| RuntimeError::Incomplete)?;
            frames.push((key, frame));
        }
        if frames.iter().any(|(key, _)| {
            !self
                .connections
                .iter()
                .any(|connection| connection.key == *key)
        }) {
            return Err(RuntimeError::MissingPipe);
        }
        let mut delivered = Vec::new();
        let mut failed = Vec::new();
        if !frames.is_empty() {
            let mut keys = Vec::with_capacity(frames.len());
            let mut requests = Vec::with_capacity(frames.len());
            for connection in &mut self.connections {
                if let Some((_, bytes)) = frames.iter_mut().find(|(key, _)| *key == connection.key)
                {
                    keys.push(connection.key);
                    requests.push(WriteRequest {
                        pipe: &mut connection.pipe,
                        bytes: std::mem::take(bytes),
                        stop,
                        deadline,
                    });
                }
            }
            let mut batch = WriteBatch::new(requests).map_err(RuntimeError::Transport)?;
            while let Some((index, result)) = batch.wait_next().map_err(RuntimeError::Transport)? {
                let key = keys[index];
                if result.is_ok() && self.fanout.notice_delivered(key, published.policy).is_ok() {
                    published
                        .delivery
                        .notice_delivered(broker, key, published.policy)
                        .map_err(RuntimeError::Delivery)?;
                    delivered.push(key);
                } else {
                    self.fanout.detach(key);
                    failed.push(key);
                }
            }
            drop(batch);
        }
        for key in failed {
            if !self.close(key) {
                return Err(RuntimeError::MissingPipe);
            }
            published
                .delivery
                .transport_closed(broker, key)
                .map_err(RuntimeError::Delivery)?;
            closed.push(key);
        }
        let Some(completion) = published.delivery.completion() else {
            return Err(RuntimeError::Incomplete);
        };
        Ok(Some(PublicationOutcome {
            policy: published.policy,
            delivered,
            closed,
            cancelled,
            completion,
        }))
    }

    fn close(&mut self, key: u64) -> bool {
        let Some(index) = self
            .connections
            .iter()
            .position(|connection| connection.key == key)
        else {
            return false;
        };
        self.fanout.detach(key);
        drop(self.connections.remove(index));
        true
    }

    fn require_current_source(&mut self) -> Result<(), RuntimeError> {
        match self.source.wire_policy() {
            Ok(policy) if policy == self.fanout.policy() => Ok(()),
            Ok(_) => Err(RuntimeError::PolicyPending),
            Err(error) => {
                self.failed = true;
                self.close_all();
                Err(RuntimeError::Source(error))
            }
        }
    }

    fn close_all(&mut self) {
        for connection in self.connections.drain(..) {
            self.fanout.detach(connection.key);
            drop(connection);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manual::{Credential, Destination, Scheme};
    use std::time::Duration;
    use sumibi_ipc::{Nonce, PinnedProcess, Server};
    use sumibi_protocol::{
        broker_session::{BrokerError, Event},
        message::{Cancel, Convert, Hello, Revision},
        Decoder,
    };

    fn id(value: u128) -> Id {
        Id::new(value).unwrap()
    }

    fn supported() -> bool {
        match PinnedProcess::current() {
            Ok(_) => true,
            Err(IpcError::UnsupportedElevation | IpcError::UnsupportedAppContainer) => false,
            Err(error) => panic!("current process identity failed: {error:?}"),
        }
    }

    fn connected(stop: &Stop, deadline: Instant) -> (Pipe, Pipe) {
        let name = Nonce::random().unwrap();
        let server = Server::create(&name).unwrap();
        std::thread::scope(|scope| {
            let server_stop = stop.clone();
            let accepted = scope.spawn(move || {
                server.accept(PinnedProcess::current().unwrap(), &server_stop, deadline)
            });
            let client = Pipe::connect(&name, PinnedProcess::current().unwrap()).unwrap();
            (accepted.join().unwrap().unwrap(), client)
        })
    }

    fn hello(client: u128, broker: Id) -> Envelope {
        Envelope {
            version: message::VERSION,
            session: id(client),
            generation: 1,
            message: message::Message::Hello(Hello { broker }),
        }
    }

    fn convert(client: u128, generation: u64, request: u64, policy: Policy) -> Envelope {
        Envelope {
            version: message::VERSION,
            session: id(client),
            generation,
            message: message::Message::Convert(Convert {
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

    fn read_frame(pipe: &mut Pipe, stop: &Stop, deadline: Instant) -> Envelope {
        let mut decoder = Decoder::default();
        loop {
            let read = pipe.start_read(stop, deadline).unwrap().wait().unwrap();
            let decoded = decoder.decode(&read.bytes).unwrap();
            assert_eq!(decoded.consumed, read.bytes.len());
            if let Some(body) = decoded.body {
                return message::decode(body).unwrap();
            }
        }
    }

    fn source() -> (PolicyAuthority, Destination) {
        let destination = Destination::new(Scheme::Https, "example.org", 443, "/").unwrap();
        (PolicyAuthority::new(destination.clone()), destination)
    }

    #[test]
    fn runtime_publishes_grant_and_revoke_to_two_pipes() {
        let _guard = crate::HOST_PIPE_TEST_LOCK.lock().unwrap();
        if !supported() {
            return;
        }
        let (source, destination) = source();
        let mut runtime = BrokerPolicyRuntime::new(&source, id(90)).unwrap();
        let stop = Stop::new().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let (server_one, mut client_one) = connected(&stop, deadline);
        let (server_two, mut client_two) = connected(&stop, deadline);
        assert_eq!(
            runtime.attach(server_one, hello(1, id(90)), &stop, deadline),
            Ok(1)
        );
        assert!(matches!(
            read_frame(&mut client_one, &stop, deadline).message,
            message::Message::Ready(_)
        ));
        assert_eq!(
            runtime.attach(server_two, hello(2, id(90)), &stop, deadline),
            Ok(2)
        );
        assert!(matches!(
            read_frame(&mut client_two, &stop, deadline).message,
            message::Message::Ready(_)
        ));
        source
            .replace_credential(Credential::new("fixture".into()))
            .unwrap();
        source.grant_manual_consent(&destination).unwrap();
        assert!(runtime.wait_for_change(deadline).unwrap());
        let grant = runtime.publish_current(&stop, deadline).unwrap().unwrap();
        assert!(grant.policy.allowed);
        assert!(grant.closed.is_empty());
        assert_eq!(grant.delivered.len(), 2);
        assert_eq!(grant.completion, DeliveryCompletion::ExactOrClosed);
        for client in [&mut client_one, &mut client_two] {
            assert!(matches!(
                read_frame(client, &stop, deadline).message,
                message::Message::PolicyChanged(policy) if policy == grant.policy
            ));
        }
        let waiting = Envelope {
            version: message::VERSION,
            session: id(1),
            generation: 1,
            message: message::Message::Convert(Convert {
                request: 1,
                revision: Revision {
                    input: 1,
                    settings: grant.policy.settings,
                    authorization: grant.policy.authorization,
                },
                timeout_ms: 1000,
                text: "nihongo".into(),
            }),
        };
        assert!(matches!(
            runtime.receive(1, waiting, Instant::now()),
            Ok(Event::Convert(_))
        ));
        source.revoke_manual_consent().unwrap();
        let revoke = runtime.publish_current(&stop, deadline).unwrap().unwrap();
        assert!(!revoke.policy.allowed);
        assert_eq!(revoke.cancelled, [(1, 1)]);
        assert!(revoke.closed.is_empty());
        assert_eq!(revoke.delivered.len(), 2);
        assert_eq!(runtime.active_count(), 2);
        for client in [&mut client_one, &mut client_two] {
            assert!(matches!(
                read_frame(client, &stop, deadline).message,
                message::Message::PolicyChanged(policy) if policy == revoke.policy
            ));
        }
    }

    #[test]
    fn runtime_closes_failed_notice_and_keeps_other_connection() {
        let _guard = crate::HOST_PIPE_TEST_LOCK.lock().unwrap();
        if !supported() {
            return;
        }
        let (source, destination) = source();
        let mut runtime = BrokerPolicyRuntime::new(&source, id(90)).unwrap();
        let stop = Stop::new().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let (server_one, mut client_one) = connected(&stop, deadline);
        let (server_two, mut client_two) = connected(&stop, deadline);
        runtime
            .attach(server_one, hello(1, id(90)), &stop, deadline)
            .unwrap();
        read_frame(&mut client_one, &stop, deadline);
        runtime
            .attach(server_two, hello(2, id(90)), &stop, deadline)
            .unwrap();
        read_frame(&mut client_two, &stop, deadline);
        drop(client_two);
        source
            .replace_credential(Credential::new("fixture".into()))
            .unwrap();
        source.grant_manual_consent(&destination).unwrap();
        let grant = runtime.publish_current(&stop, deadline).unwrap().unwrap();
        assert_eq!(grant.delivered, [1]);
        assert_eq!(grant.closed, [2]);
        assert_eq!(runtime.active_count(), 1);
        assert_eq!(grant.completion, DeliveryCompletion::ExactOrClosed);
        assert!(matches!(
            read_frame(&mut client_one, &stop, deadline).message,
            message::Message::PolicyChanged(policy) if policy == grant.policy
        ));
        source.revoke_manual_consent().unwrap();
        let revoke = runtime.publish_current(&stop, deadline).unwrap().unwrap();
        assert_eq!(revoke.delivered, [1]);
        assert!(revoke.closed.is_empty());
        assert!(matches!(
            read_frame(&mut client_one, &stop, deadline).message,
            message::Message::PolicyChanged(policy) if policy == revoke.policy
        ));
    }

    #[test]
    fn rejected_hello_consumes_connection_key() {
        let _guard = crate::HOST_PIPE_TEST_LOCK.lock().unwrap();
        if !supported() {
            return;
        }
        let (source, _) = source();
        let mut runtime = BrokerPolicyRuntime::new(&source, id(90)).unwrap();
        let stop = Stop::new().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let (bad_server, _bad_client) = connected(&stop, deadline);
        assert!(matches!(
            runtime.attach(bad_server, hello(1, id(91)), &stop, deadline),
            Err(RuntimeError::Fanout(FanoutError::Broker(_)))
        ));
        let (good_server, mut good_client) = connected(&stop, deadline);
        assert_eq!(
            runtime.attach(good_server, hello(2, id(90)), &stop, deadline),
            Ok(2)
        );
        assert!(matches!(
            read_frame(&mut good_client, &stop, deadline).message,
            message::Message::Ready(_)
        ));
    }

    #[test]
    fn failed_ready_write_consumes_key_and_allows_next_pipe() {
        let _guard = crate::HOST_PIPE_TEST_LOCK.lock().unwrap();
        if !supported() {
            return;
        }
        let (source, _) = source();
        let mut runtime = BrokerPolicyRuntime::new(&source, id(90)).unwrap();
        let stop = Stop::new().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let (failed_server, failed_client) = connected(&stop, deadline);
        drop(failed_client);
        assert!(matches!(
            runtime.attach(failed_server, hello(1, id(90)), &stop, deadline),
            Err(RuntimeError::Transport(IpcError::Disconnected))
        ));
        assert_eq!(runtime.active_count(), 0);
        let (live_server, mut live_client) = connected(&stop, deadline);
        assert_eq!(
            runtime.attach(live_server, hello(2, id(90)), &stop, deadline),
            Ok(2)
        );
        assert!(matches!(
            read_frame(&mut live_client, &stop, deadline).message,
            message::Message::Ready(_)
        ));
    }

    #[test]
    fn unpublished_source_change_rejects_text_and_new_ready() {
        let _guard = crate::HOST_PIPE_TEST_LOCK.lock().unwrap();
        if !supported() {
            return;
        }
        let (source, destination) = source();
        source
            .replace_credential(Credential::new("fixture".into()))
            .unwrap();
        source.grant_manual_consent(&destination).unwrap();
        let mut runtime = BrokerPolicyRuntime::new(&source, id(90)).unwrap();
        let granted = runtime.policy();
        let stop = Stop::new().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let (first_server, mut first_client) = connected(&stop, deadline);
        assert_eq!(
            runtime.attach(first_server, hello(1, id(90)), &stop, deadline),
            Ok(1)
        );
        read_frame(&mut first_client, &stop, deadline);
        source.revoke_manual_consent().unwrap();
        let old_request = Envelope {
            version: message::VERSION,
            session: id(1),
            generation: 1,
            message: message::Message::Convert(Convert {
                request: 1,
                revision: Revision {
                    input: 1,
                    settings: granted.settings,
                    authorization: granted.authorization,
                },
                timeout_ms: 1000,
                text: "nihongo".into(),
            }),
        };
        assert!(matches!(
            runtime.receive(1, old_request, Instant::now()),
            Err(RuntimeError::PolicyPending)
        ));
        assert_eq!(runtime.active_count(), 0);
        drop(first_client);
        let (unpublished_server, unpublished_client) = connected(&stop, deadline);
        assert!(matches!(
            runtime.attach(unpublished_server, hello(2, id(90)), &stop, deadline),
            Err(RuntimeError::PolicyPending)
        ));
        drop(unpublished_client);
        let revoked = runtime.publish_current(&stop, deadline).unwrap().unwrap();
        assert!(!revoked.policy.allowed);
        assert!(revoked.delivered.is_empty());
        let (current_server, mut current_client) = connected(&stop, deadline);
        assert_eq!(
            runtime.attach(current_server, hello(3, id(90)), &stop, deadline),
            Ok(3)
        );
        assert!(matches!(
            read_frame(&mut current_client, &stop, deadline).message,
            message::Message::Ready(ready) if ready.policy == revoked.policy
        ));
    }

    #[test]
    fn reconnect_keeps_request_floor_and_rejects_old_generation() {
        let _guard = crate::HOST_PIPE_TEST_LOCK.lock().unwrap();
        if !supported() {
            return;
        }
        let (source, destination) = source();
        source
            .replace_credential(Credential::new("fixture".into()))
            .unwrap();
        source.grant_manual_consent(&destination).unwrap();
        let mut runtime = BrokerPolicyRuntime::new(&source, id(90)).unwrap();
        let policy = runtime.policy();
        let stop = Stop::new().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let (first_server, mut first_client) = connected(&stop, deadline);
        assert_eq!(
            runtime.attach(first_server, hello(1, id(90)), &stop, deadline),
            Ok(1)
        );
        read_frame(&mut first_client, &stop, deadline);
        assert!(matches!(
            runtime.receive(1, convert(1, 1, 1, policy), Instant::now()),
            Ok(Event::Convert(_))
        ));
        assert!(runtime.disconnect(1));
        drop(first_client);
        let (second_server, mut second_client) = connected(&stop, deadline);
        let mut second_hello = hello(1, id(90));
        second_hello.generation = 2;
        assert_eq!(
            runtime.attach(second_server, second_hello, &stop, deadline),
            Ok(2)
        );
        read_frame(&mut second_client, &stop, deadline);
        assert!(matches!(
            runtime.receive(2, convert(1, 2, 1, policy), Instant::now()),
            Err(RuntimeError::Fanout(FanoutError::Broker(
                BrokerError::Replay
            )))
        ));
        drop(second_client);
        let (stale_server, stale_client) = connected(&stop, deadline);
        let mut stale_hello = hello(1, id(90));
        stale_hello.generation = 2;
        assert!(matches!(
            runtime.attach(stale_server, stale_hello, &stop, deadline),
            Err(RuntimeError::Fanout(FanoutError::StaleGeneration))
        ));
        drop(stale_client);
        let (third_server, mut third_client) = connected(&stop, deadline);
        let mut third_hello = hello(1, id(90));
        third_hello.generation = 3;
        assert_eq!(
            runtime.attach(third_server, third_hello, &stop, deadline),
            Ok(4)
        );
        read_frame(&mut third_client, &stop, deadline);
        assert!(matches!(
            runtime.receive(4, convert(1, 3, 2, policy), Instant::now()),
            Ok(Event::Convert(_))
        ));
    }

    #[test]
    fn invalid_cancel_retires_only_its_connection() {
        let _guard = crate::HOST_PIPE_TEST_LOCK.lock().unwrap();
        if !supported() {
            return;
        }
        let (source, destination) = source();
        source
            .replace_credential(Credential::new("fixture".into()))
            .unwrap();
        source.grant_manual_consent(&destination).unwrap();
        let mut runtime = BrokerPolicyRuntime::new(&source, id(90)).unwrap();
        let policy = runtime.policy();
        let stop = Stop::new().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let (first_server, mut first_client) = connected(&stop, deadline);
        let (second_server, mut second_client) = connected(&stop, deadline);
        runtime
            .attach(first_server, hello(1, id(90)), &stop, deadline)
            .unwrap();
        runtime
            .attach(second_server, hello(2, id(90)), &stop, deadline)
            .unwrap();
        read_frame(&mut first_client, &stop, deadline);
        read_frame(&mut second_client, &stop, deadline);
        assert!(matches!(
            runtime.receive(1, convert(1, 1, 1, policy), Instant::now()),
            Ok(Event::Convert(_))
        ));
        assert!(matches!(
            runtime.receive(2, convert(2, 1, 1, policy), Instant::now()),
            Ok(Event::Convert(_))
        ));
        let future_cancel = Envelope {
            version: message::VERSION,
            session: id(2),
            generation: 1,
            message: message::Message::Cancel(Cancel { request: 2 }),
        };
        assert!(matches!(
            runtime.receive(2, future_cancel, Instant::now()),
            Err(RuntimeError::Fanout(FanoutError::Broker(
                BrokerError::InvalidMessage
            )))
        ));
        assert_eq!(runtime.active_count(), 1);
        let valid_cancel = Envelope {
            version: message::VERSION,
            session: id(1),
            generation: 1,
            message: message::Message::Cancel(Cancel { request: 1 }),
        };
        assert!(matches!(
            runtime.receive(1, valid_cancel, Instant::now()),
            Ok(Event::Cancelled(1))
        ));
        assert!(matches!(
            runtime.receive(1, convert(1, 1, 2, policy), Instant::now()),
            Ok(Event::Convert(_))
        ));
    }

    #[test]
    fn divergent_publication_retires_runtime_and_closes_live_pipe() {
        let _guard = crate::HOST_PIPE_TEST_LOCK.lock().unwrap();
        if !supported() {
            return;
        }
        let (source, _) = source();
        let mut runtime = BrokerPolicyRuntime::new(&source, id(90)).unwrap();
        let stop = Stop::new().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let (server, mut client) = connected(&stop, deadline);
        runtime
            .attach(server, hello(1, id(90)), &stop, deadline)
            .unwrap();
        read_frame(&mut client, &stop, deadline);
        let diverged = Policy {
            authorization: runtime.policy().authorization + 1,
            ..runtime.policy()
        };
        runtime.gate.publish(&mut runtime.fanout, diverged).unwrap();
        assert!(matches!(
            runtime.publish_current(&stop, deadline),
            Err(RuntimeError::Relay(RelayError::Diverged))
        ));
        assert_eq!(runtime.active_count(), 0);
        assert!(matches!(
            runtime.publish_current(&stop, deadline),
            Err(RuntimeError::Retired)
        ));
        assert!(matches!(
            runtime.wait_for_change(deadline),
            Err(RuntimeError::Retired)
        ));
    }
}

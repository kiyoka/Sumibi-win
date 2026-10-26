//! Serialized Host-side policy delivery over authenticated Broker pipes.
//! The caller owns accept/read scheduling; this layer keeps connection keys,
//! policy publication, bounded writes, and physical close receipts together.
use crate::broker_policy::{BrokerPolicyRelay, DeliveryCompletion, DeliveryError, RelayError};
use crate::manual::{SendAdmission, SendError, TransportError};
use crate::policy::{ChangeError, PolicyAuthority, PolicyVersion};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};
use sumibi_ipc::{
    broker::BrokerPolicyGate, framed::FrameInbox, Error as IpcError, Pipe, ReadBatch, ReadRequest,
    Stop, WriteBatch, WriteRequest,
};
use sumibi_protocol::{
    broker_session::Event,
    message::{self, Envelope, ErrorCode, Id, Policy, Revision},
    policy_fanout::{FanoutError, PolicyFanout, MAX_SESSIONS},
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
    WrongBroker,
    InvalidTarget,
}

/// Bind a worker completion to the Broker boot and original connection.
/// A later Broker may reuse a numeric key but has a different boot identity.
#[derive(Clone, Copy)]
pub struct ReplyContext {
    pub broker: Id,
    pub key: u64,
    pub request: u64,
    pub revision: Revision,
    pub completed_at: Instant,
}

/// A worker's admission belongs to exactly one Broker boot and connection.
/// Clones observe every retirement path without carrying source text.
#[derive(Clone)]
pub struct JobTicket {
    broker: Id,
    key: u64,
    request: u64,
    revision: Revision,
    deadline: Instant,
    cancelled: Arc<AtomicBool>,
    stop: Stop,
    send_lock: Arc<Mutex<()>>,
}

impl JobTicket {
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    /// A worker may include this event in its bounded I/O wait. It belongs
    /// only to this request, so retiring another connection cannot signal it.
    pub fn stop(&self) -> Stop {
        self.stop.clone()
    }

    pub fn reply_context(&self, completed_at: Instant) -> ReplyContext {
        ReplyContext {
            broker: self.broker,
            key: self.key,
            request: self.request,
            revision: self.revision,
            completed_at,
        }
    }

    pub fn identity(&self) -> (Id, u64, u64) {
        (self.broker, self.key, self.request)
    }

    pub fn deadline(&self) -> Instant {
        self.deadline
    }

    pub(crate) fn cancel(&self) {
        // Wake any cancellable I/O immediately. The mutex below fixes the
        // cancellation order against a request write already in progress.
        let _ = self.stop.cancel();
        let _guard = self
            .send_lock
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        self.cancelled.store(true, Ordering::Release);
    }
}

impl SendAdmission for JobTicket {
    fn is_cancelled(&self) -> bool {
        JobTicket::is_cancelled(self)
    }

    fn request_deadline(&self) -> Option<Instant> {
        Some(self.deadline)
    }

    fn write_if_active(
        &self,
        write: &mut dyn FnMut() -> Result<(), TransportError>,
    ) -> Result<(), SendError> {
        let _guard = self.send_lock.lock().map_err(|_| SendError::PolicyDenied)?;
        if self.is_cancelled() || Instant::now() >= self.deadline {
            return Err(SendError::PolicyDenied);
        }
        write().map_err(SendError::Transport)
    }
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

/// Settings-side completion after the requested Host version has reached the
/// admission gate and every prior endpoint has received it or been closed.
pub struct PolicyAcknowledgment {
    pub requested: PolicyVersion,
    pub current: Policy,
    pub completion: DeliveryCompletion,
}

/// One bounded unit of progress from already authenticated connections.
/// The owner interleaves this with OS accept and worker completions.
pub enum PollStep {
    Published(PublicationOutcome),
    Activated(u64),
    Received { key: u64, event: Event },
    Expired { key: u64, request: u64 },
    Closed { key: u64, error: RuntimeError },
    Idle,
}

struct Connection {
    key: u64,
    pipe: Pipe,
    inbox: FrameInbox,
}

/// One Broker boot's serialized policy path. The caller must use the same
/// worker for `attach` and `publish_current`, and must not accept source text
/// until pending source changes have been published.
pub struct BrokerPolicyRuntime<'a> {
    source: &'a PolicyAuthority,
    gate: BrokerPolicyGate,
    fanout: PolicyFanout,
    relay: BrokerPolicyRelay<'a>,
    pending: Vec<Connection>,
    connections: Vec<Connection>,
    jobs: Vec<JobTicket>,
    last_key: u64,
    last_polled_key: u64,
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
            pending: Vec::new(),
            connections: Vec::new(),
            jobs: Vec::new(),
            last_key: 0,
            last_polled_key: 0,
            failed: false,
        })
    }

    pub fn policy(&self) -> Policy {
        self.fanout.policy()
    }

    pub(crate) fn uses_source(&self, source: &PolicyAuthority) -> bool {
        std::ptr::eq(self.source, source)
    }

    /// Complete a finished settings mutation. The caller supplies the version
    /// returned by PolicyAuthority and reads that authority again for display
    /// after this receipt. Coalescing to a newer version reports Superseded.
    pub fn acknowledge_version(
        &mut self,
        requested: PolicyVersion,
        stop: &Stop,
        deadline: Instant,
    ) -> Result<PolicyAcknowledgment, RuntimeError> {
        if self.failed {
            return Err(RuntimeError::Retired);
        }
        if requested.settings == 0 || requested.authorization == 0 {
            return Err(RuntimeError::InvalidTarget);
        }
        if Instant::now() >= deadline {
            return Err(RuntimeError::Transport(IpcError::Timeout));
        }
        let source = match self.source.wire_policy() {
            Ok(policy) => policy,
            Err(error) => {
                self.failed = true;
                self.close_all();
                return Err(RuntimeError::Source(error));
            }
        };
        if requested.settings > source.settings || requested.authorization > source.authorization {
            return Err(RuntimeError::InvalidTarget);
        }
        let _ = self.publish_current(stop, deadline)?;
        let current = self.policy();
        if current.settings < requested.settings || current.authorization < requested.authorization
        {
            return Err(RuntimeError::Incomplete);
        }
        let completion = if current.settings == requested.settings
            && current.authorization == requested.authorization
        {
            DeliveryCompletion::ExactOrClosed
        } else {
            DeliveryCompletion::Superseded
        };
        Ok(PolicyAcknowledgment {
            requested,
            current,
            completion,
        })
    }

    pub fn active_count(&self) -> usize {
        self.connections.len()
    }

    /// Snapshot keys for a bounded round-robin read pass. Keys never repeat
    /// within this Broker boot, so a later close cannot retarget a result.
    pub fn active_keys(&self) -> Vec<u64> {
        self.connections.iter().map(|entry| entry.key).collect()
    }

    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    pub fn pending_keys(&self) -> Vec<u64> {
        self.pending.iter().map(|entry| entry.key).collect()
    }

    /// Clone the admission token after `receive` returns Convert. A worker
    /// checks it before sending and again before handing back its completion.
    pub fn job_ticket(&self, key: u64, request: u64) -> Option<JobTicket> {
        self.jobs
            .iter()
            .find(|job| job.key == key && job.request == request)
            .cloned()
    }

    fn remove_job(&mut self, key: u64, request: u64, cancel: bool) {
        if let Some(index) = self
            .jobs
            .iter()
            .position(|job| job.key == key && job.request == request)
        {
            let job = self.jobs.remove(index);
            if cancel {
                job.cancel();
            }
        }
    }

    fn cancel_connection_jobs(&mut self, key: u64) {
        self.jobs.retain(|job| {
            if job.key == key {
                job.cancel();
                false
            } else {
                true
            }
        });
    }

    /// Reserve a never-reused key at OS accept, before reading Hello. Pending
    /// endpoints cannot receive requests and are closed before a publication
    /// can be acknowledged.
    pub fn reserve(&mut self, pipe: Pipe) -> Result<u64, RuntimeError> {
        if self.failed {
            return Err(RuntimeError::Retired);
        }
        if self.pending.len() + self.connections.len() >= MAX_SESSIONS {
            return Err(RuntimeError::Fanout(FanoutError::Full));
        }
        let key = self
            .last_key
            .checked_add(1)
            .ok_or(RuntimeError::Exhausted)?;
        self.fanout.reserve_key(key).map_err(RuntimeError::Fanout)?;
        self.last_key = key;
        self.pending.push(Connection {
            key,
            pipe,
            inbox: FrameInbox::default(),
        });
        Ok(key)
    }

    /// Publish first, then expire jobs and poll one frame per connection in
    /// round-robin order. A peer error closes only that endpoint. Source or
    /// runtime failure remains terminal and is returned to the owner.
    pub fn poll_once(&mut self, stop: &Stop, deadline: Instant) -> Result<PollStep, RuntimeError> {
        if let Some(outcome) = self.publish_current(stop, deadline)? {
            return Ok(PollStep::Published(outcome));
        }
        for key in self.active_keys() {
            match self.expire(key, Instant::now()) {
                Ok(Some(request)) => return Ok(PollStep::Expired { key, request }),
                Ok(None) => {}
                Err(error) if self.failed => return Err(error),
                Err(error) => return Ok(PollStep::Closed { key, error }),
            }
        }
        for key in self.pending_keys() {
            match self.poll_reserved_hello(key, stop, deadline) {
                Ok(Some(hello)) => match self.activate(key, hello, stop, deadline) {
                    Ok(()) => return Ok(PollStep::Activated(key)),
                    Err(error) if self.failed => return Err(error),
                    Err(error) => return Ok(PollStep::Closed { key, error }),
                },
                Ok(None) => {}
                Err(error) if self.failed => return Err(error),
                Err(error) => return Ok(PollStep::Closed { key, error }),
            }
        }
        let mut keys = self.active_keys();
        keys.sort_unstable();
        let split = keys.partition_point(|key| *key <= self.last_polled_key);
        keys.rotate_left(split);
        for key in keys {
            match self.poll_active_message(key, stop, deadline) {
                Ok(Some(frame)) => {
                    self.last_polled_key = key;
                    return match self.receive(key, frame, Instant::now()) {
                        Ok(event) => Ok(PollStep::Received { key, event }),
                        Err(error) if self.failed => Err(error),
                        Err(error) => Ok(PollStep::Closed { key, error }),
                    };
                }
                Ok(None) => {}
                Err(error) if self.failed => return Err(error),
                Err(error) => return Ok(PollStep::Closed { key, error }),
            }
        }
        Ok(PollStep::Idle)
    }

    /// Lockstep Hello reader for the launched diagnostic. A product loop may
    /// instead schedule reads independently and pass the frame to `activate`.
    pub fn read_reserved_hello(
        &mut self,
        key: u64,
        stop: &Stop,
        deadline: Instant,
    ) -> Result<Envelope, RuntimeError> {
        let Some(connection) = self.pending.iter_mut().find(|entry| entry.key == key) else {
            return Err(RuntimeError::MissingPipe);
        };
        let result = connection.inbox.read(&mut connection.pipe, stop, deadline);
        if result.is_err() {
            self.close(key);
        }
        result.map_err(RuntimeError::Transport)
    }

    /// Poll an incomplete Hello without blocking the policy publisher. A
    /// source change may close this key between chunks before Ready is sent.
    pub fn poll_reserved_hello(
        &mut self,
        key: u64,
        stop: &Stop,
        deadline: Instant,
    ) -> Result<Option<Envelope>, RuntimeError> {
        let Some(connection) = self.pending.iter_mut().find(|entry| entry.key == key) else {
            return Err(RuntimeError::MissingPipe);
        };
        let result = connection
            .inbox
            .read_queued(&mut connection.pipe, stop, deadline);
        if result.is_err() {
            self.close(key);
        }
        result.map_err(RuntimeError::Transport)
    }

    /// Close a disconnected OS endpoint and retire only its session. The
    /// caller never reuses its connection key for another accepted pipe.
    pub fn disconnect(&mut self, key: u64) -> bool {
        self.close(key)
    }

    /// Close one OS endpoint and return its pending request for worker
    /// cancellation. Unknown/stale keys cannot address a later connection.
    pub fn disconnect_with_pending(&mut self, key: u64) -> Result<Option<u64>, RuntimeError> {
        if self.failed {
            return Err(RuntimeError::Retired);
        }
        if self.pending.iter().any(|entry| entry.key == key) {
            self.close(key);
            return Ok(None);
        }
        if !self.connections.iter().any(|entry| entry.key == key) {
            return Err(RuntimeError::MissingPipe);
        }
        let pending = match self.fanout.pending_request(key) {
            Ok(pending) => pending,
            Err(error) => {
                self.failed = true;
                self.close_all();
                return Err(RuntimeError::Fanout(error));
            }
        };
        if !self.close(key) {
            self.failed = true;
            self.close_all();
            return Err(RuntimeError::MissingPipe);
        }
        Ok(pending)
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
        pipe: Pipe,
        hello: Envelope,
        stop: &Stop,
        deadline: Instant,
    ) -> Result<u64, RuntimeError> {
        let key = self.reserve(pipe)?;
        self.activate(key, hello, stop, deadline)?;
        Ok(key)
    }

    /// Finish a reserved handshake. Any failure consumes the key and drops
    /// its OS pipe. A source change before Ready rejects this old attempt.
    pub fn activate(
        &mut self,
        key: u64,
        hello: Envelope,
        stop: &Stop,
        deadline: Instant,
    ) -> Result<(), RuntimeError> {
        if self.failed {
            return Err(RuntimeError::Retired);
        }
        let Some(index) = self.pending.iter().position(|entry| entry.key == key) else {
            return Err(RuntimeError::MissingPipe);
        };
        let mut connection = self.pending.remove(index);
        let _source = match self.source.authorize_ready(self.fanout.policy()) {
            Ok(guard) => guard,
            Err(ChangeError::StalePolicy) => {
                self.fanout.abandon_key(key);
                return Err(RuntimeError::PolicyPending);
            }
            Err(error) => {
                self.fanout.abandon_key(key);
                self.failed = true;
                self.close_all();
                return Err(RuntimeError::Source(error));
            }
        };
        let ready_deadline = deadline.min(Instant::now() + Duration::from_millis(500));
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
        if let Err(error) = connection.pipe.write_all(frame, stop, ready_deadline) {
            self.fanout.detach(key);
            return Err(RuntimeError::Transport(error));
        }
        if let Err(error) = self.fanout.ready_delivered(key) {
            self.fanout.detach(key);
            return Err(RuntimeError::Fanout(error));
        }
        self.connections.push(connection);
        Ok(())
    }

    /// Read the next active frame, preserving any tail from a previous Read.
    /// On transport or framing failure retire this endpoint and its job.
    pub fn read_active_message(
        &mut self,
        key: u64,
        stop: &Stop,
        deadline: Instant,
    ) -> Result<Envelope, RuntimeError> {
        let Some(connection) = self.connections.iter_mut().find(|entry| entry.key == key) else {
            return Err(RuntimeError::MissingPipe);
        };
        let result = connection.inbox.read(&mut connection.pipe, stop, deadline);
        if result.is_err() {
            self.close(key);
        }
        result.map_err(RuntimeError::Transport)
    }

    /// Read at most one queued frame without waiting on an idle connection.
    /// The caller still bounds partial-frame age and interleaves publication,
    /// worker completion, expiry, and new accepts between read passes.
    pub fn poll_active_message(
        &mut self,
        key: u64,
        stop: &Stop,
        deadline: Instant,
    ) -> Result<Option<Envelope>, RuntimeError> {
        let Some(connection) = self.connections.iter_mut().find(|entry| entry.key == key) else {
            return Err(RuntimeError::MissingPipe);
        };
        let result = connection
            .inbox
            .read_queued(&mut connection.pipe, stop, deadline);
        if result.is_err() {
            self.close(key);
        }
        result.map_err(RuntimeError::Transport)
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
            if self.jobs.iter().any(|job| job.key == key) {
                self.close(key);
                return Err(RuntimeError::Incomplete);
            }
            let stop = match Stop::new() {
                Ok(stop) => stop,
                Err(error) => {
                    self.close(key);
                    return Err(RuntimeError::Transport(error));
                }
            };
            let Some(deadline) = now.checked_add(Duration::from_millis(convert.timeout_ms.into()))
            else {
                self.close(key);
                return Err(RuntimeError::Incomplete);
            };
            self.jobs.push(JobTicket {
                broker: self.fanout.broker_id(),
                key,
                request: convert.request,
                revision: convert.revision,
                deadline,
                cancelled: Arc::new(AtomicBool::new(false)),
                stop,
                send_lock: Arc::new(Mutex::new(())),
            });
        } else if let Event::Cancelled(request) = &event {
            self.remove_job(key, *request, true);
        }
        Ok(event)
    }

    /// Retire only this connection's expired request. The caller uses the
    /// returned ID to cancel its worker; a late completion publishes nothing.
    pub fn expire(&mut self, key: u64, now: Instant) -> Result<Option<u64>, RuntimeError> {
        if self.failed {
            return Err(RuntimeError::Retired);
        }
        if !self.connections.iter().any(|entry| entry.key == key) {
            return Err(RuntimeError::MissingPipe);
        }
        match self.fanout.expire(key, now) {
            Ok(request) => {
                if let Some(request) = request {
                    self.remove_job(key, request, true);
                }
                Ok(request)
            }
            Err(error) => {
                self.close(key);
                Err(RuntimeError::Fanout(error))
            }
        }
    }

    /// Publish one worker result to its original authenticated connection.
    /// The Host source and Broker gate are held through the bounded write, so
    /// a simultaneous revocation cannot complete before this result's order
    /// is fixed. A failed write retires only this connection.
    pub fn publish_result(
        &mut self,
        reply: ReplyContext,
        text: String,
        stop: &Stop,
        deadline: Instant,
    ) -> Result<bool, RuntimeError> {
        self.publish_reply(reply, stop, deadline, |fanout| {
            fanout.result(
                reply.key,
                reply.request,
                reply.revision,
                text,
                reply.completed_at,
            )
        })
    }

    /// Publish a fixed worker failure under the same policy and transport
    /// ordering as a successful result.
    pub fn publish_error(
        &mut self,
        reply: ReplyContext,
        code: ErrorCode,
        stop: &Stop,
        deadline: Instant,
    ) -> Result<bool, RuntimeError> {
        self.publish_reply(reply, stop, deadline, |fanout| {
            fanout.error(
                reply.key,
                reply.request,
                reply.revision,
                code,
                reply.completed_at,
            )
        })
    }

    fn publish_reply(
        &mut self,
        reply: ReplyContext,
        stop: &Stop,
        deadline: Instant,
        prepare: impl FnOnce(&mut PolicyFanout) -> Result<Option<Envelope>, FanoutError>,
    ) -> Result<bool, RuntimeError> {
        if self.failed {
            return Err(RuntimeError::Retired);
        }
        if reply.broker != self.fanout.broker_id() {
            return Err(RuntimeError::WrongBroker);
        }
        if !self.connections.iter().any(|entry| entry.key == reply.key) {
            return Err(RuntimeError::MissingPipe);
        }
        let started = Instant::now();
        if deadline <= started || deadline > started + Duration::from_millis(500) {
            return Err(RuntimeError::Transport(IpcError::InvalidArgument));
        }
        let ticket = self.job_ticket(reply.key, reply.request);
        if ticket
            .as_ref()
            .is_some_and(|ticket| ticket.revision != reply.revision)
        {
            self.close(reply.key);
            return Err(RuntimeError::Incomplete);
        }
        let ticket_deadline = ticket.map(|ticket| ticket.deadline());
        if ticket_deadline.is_some_and(|limit| started >= limit) {
            self.expire(reply.key, started)?;
            self.remove_job(reply.key, reply.request, true);
            return Ok(false);
        }
        let result = (|| {
            let _source = self
                .source
                .authorize_result(reply.revision)
                .map_err(RuntimeError::Source)?;
            let _gate = self
                .gate
                .authorize_for_publish(reply.revision)
                .map_err(RuntimeError::Transport)?;
            if ticket_deadline.is_some_and(|limit| Instant::now() >= limit) {
                return Ok(false);
            }
            let Some(prepared) = prepare(&mut self.fanout).map_err(RuntimeError::Fanout)? else {
                return Ok(false);
            };
            let frame = message::encode(&prepared).map_err(|_| RuntimeError::Incomplete)?;
            let connection = self
                .connections
                .iter_mut()
                .find(|entry| entry.key == reply.key)
                .ok_or(RuntimeError::MissingPipe)?;
            connection
                .pipe
                .write_all(
                    frame,
                    stop,
                    ticket_deadline.map_or(deadline, |limit| deadline.min(limit)),
                )
                .map_err(RuntimeError::Transport)?;
            Ok(true)
        })();
        match result {
            Ok(delivered) => {
                if !delivered && ticket_deadline.is_some_and(|limit| Instant::now() >= limit) {
                    self.expire(reply.key, Instant::now())?;
                }
                self.remove_job(reply.key, reply.request, !delivered);
            }
            Err(
                RuntimeError::Source(ChangeError::Unavailable)
                | RuntimeError::Transport(IpcError::PolicyDenied),
            ) => {
                self.failed = true;
                self.close_all();
            }
            Err(
                RuntimeError::Fanout(_) | RuntimeError::Transport(_) | RuntimeError::Incomplete,
            ) => {
                self.close(reply.key);
            }
            _ => {}
        }
        result
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
        for connection in self.pending.drain(..) {
            self.fanout.abandon_key(connection.key);
            closed.push(connection.key);
            drop(connection);
        }
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
                self.remove_job(key, request, true);
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
        self.cancel_connection_jobs(key);
        if let Some(index) = self.pending.iter().position(|entry| entry.key == key) {
            self.fanout.abandon_key(key);
            drop(self.pending.remove(index));
            return true;
        }
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
        for job in self.jobs.drain(..) {
            job.cancel();
        }
        for connection in self.pending.drain(..) {
            self.fanout.abandon_key(connection.key);
        }
        for connection in self.connections.drain(..) {
            self.fanout.detach(connection.key);
            drop(connection);
        }
    }
}

impl Drop for BrokerPolicyRuntime<'_> {
    fn drop(&mut self) {
        self.close_all();
    }
}

/// The disposable two-client fixture reads one frame from each pipe concurrently.
/// Complete each batch before touching either pipe again; a partially decoded
/// frame stays in its connection's inbox for the next bounded read.
fn read_frame_pair(
    first: &mut Connection,
    second: &mut Connection,
    stop: &Stop,
    deadline: Instant,
) -> sumibi_ipc::Result<(Envelope, Envelope)> {
    let mut first_hello = None;
    let mut second_hello = None;
    loop {
        if first_hello.is_none() {
            first_hello = first.inbox.next_frame()?;
        }
        if second_hello.is_none() {
            second_hello = second.inbox.next_frame()?;
        }
        if let Some(first_frame) = first_hello.take() {
            if let Some(second_frame) = second_hello.take() {
                return Ok((first_frame, second_frame));
            }
            first_hello = Some(first_frame);
        }
        let mut slots = Vec::new();
        let mut requests = Vec::new();
        if first_hello.is_none() {
            slots.push(0);
            requests.push(ReadRequest {
                pipe: &mut first.pipe,
                stop,
                deadline,
            });
        }
        if second_hello.is_none() {
            slots.push(1);
            requests.push(ReadRequest {
                pipe: &mut second.pipe,
                stop,
                deadline,
            });
        }
        let mut batch = ReadBatch::new(requests)?;
        let mut chunks = Vec::new();
        while let Some((index, result)) = batch.wait_next()? {
            chunks.push((slots[index], result?));
        }
        drop(batch);
        for (slot, bytes) in chunks {
            if slot == 0 {
                first.inbox.push(bytes)?;
            } else {
                second.inbox.push(bytes)?;
            }
        }
    }
}

/// Child-process fixture for the explicit spike build. It uses the same Host
/// runtime and authenticated pipes as the product path, but no network, store,
/// registration or persistent credential. The parent owns the stop event.
pub fn serve_diagnostic(
    bootstrap: &sumibi_ipc::bootstrap::Bootstrap,
    diagnostic_enabled: bool,
) -> sumibi_ipc::Result<()> {
    use crate::broker_workers::{BrokerWorkLoop, BrokerWorkers, OwnerStep};
    use crate::manual::{Credential, Destination, Scheme};
    use std::time::Duration;

    if !diagnostic_enabled || !bootstrap.is_policy_runtime_diagnostic() {
        return Err(IpcError::PolicyDenied);
    }
    let destination = Destination::new(Scheme::Https, "example.test", 443, "/")
        .ok_or(IpcError::InvalidArgument)?;
    let source = PolicyAuthority::new(destination.clone());
    let mut runtime = BrokerPolicyRuntime::new(&source, bootstrap.boot_id())
        .map_err(|_| IpcError::PolicyDenied)?;
    let mut owner = BrokerWorkLoop::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    let (first_key, second_key) = bootstrap.accept_policy_runtime_pair(deadline, |pipe| {
        let (key, published) = owner
            .accept(&mut runtime, pipe, bootstrap.stop(), deadline)
            .map_err(|_| IpcError::PolicyDenied)?;
        if published.is_some() {
            return Err(IpcError::InvalidMessage);
        }
        Ok(key)
    })?;
    let (first_hello, second_hello) = {
        let [first, second] = runtime.pending.as_mut_slice() else {
            return Err(IpcError::InvalidMessage);
        };
        if first.key != first_key || second.key != second_key {
            return Err(IpcError::InvalidMessage);
        }
        read_frame_pair(first, second, bootstrap.stop(), deadline)?
    };
    let first_session = first_hello.session;
    let second_session = second_hello.session;
    runtime
        .activate(first_key, first_hello, bootstrap.stop(), deadline)
        .map_err(|_| IpcError::InvalidMessage)?;
    runtime
        .activate(second_key, second_hello, bootstrap.stop(), deadline)
        .map_err(|_| IpcError::InvalidMessage)?;
    let session_one = Id::new(1).ok_or(IpcError::InvalidMessage)?;
    let session_two = Id::new(2).ok_or(IpcError::InvalidMessage)?;
    let (request_key, result_key) = match (first_session, second_session) {
        (first, second) if first == session_one && second == session_two => (first_key, second_key),
        (first, second) if first == session_two && second == session_one => (second_key, first_key),
        _ => return Err(IpcError::InvalidMessage),
    };
    let credential = Credential::new("diagnostic-only".into()).ok_or(IpcError::InvalidArgument)?;
    source
        .replace_credential(Some(credential))
        .map_err(|_| IpcError::PolicyDenied)?;
    publish_diagnostic(&mut runtime, bootstrap.stop(), deadline, 2, false, None)?;
    source
        .grant_manual_consent(&destination)
        .map_err(|_| IpcError::PolicyDenied)?;
    publish_diagnostic(&mut runtime, bootstrap.stop(), deadline, 3, true, None)?;
    let (request, second_request) = {
        let [first, second] = runtime.connections.as_mut_slice() else {
            return Err(IpcError::InvalidMessage);
        };
        let (first_frame, second_frame) =
            read_frame_pair(first, second, bootstrap.stop(), deadline)?;
        if first.key == request_key && second.key == result_key {
            (first_frame, second_frame)
        } else if first.key == result_key && second.key == request_key {
            (second_frame, first_frame)
        } else {
            return Err(IpcError::InvalidMessage);
        }
    };
    let Event::Convert(convert) = runtime
        .receive(request_key, request, Instant::now())
        .map_err(|_| IpcError::PolicyDenied)?
    else {
        return Err(IpcError::InvalidMessage);
    };
    if convert.request != 1 || convert.text != "nihongo" {
        return Err(IpcError::InvalidMessage);
    }
    let Event::Convert(second) = runtime
        .receive(result_key, second_request, Instant::now())
        .map_err(|_| IpcError::PolicyDenied)?
    else {
        return Err(IpcError::InvalidMessage);
    };
    if second.request != 1 || second.text != "second" {
        return Err(IpcError::InvalidMessage);
    }
    let result_ticket = runtime
        .job_ticket(result_key, second.request)
        .ok_or(IpcError::InvalidMessage)?;
    let cancelled_ticket = runtime
        .job_ticket(request_key, convert.request)
        .ok_or(IpcError::InvalidMessage)?;
    let mut workers = BrokerWorkers::new();
    workers
        .submit(cancelled_ticket.clone(), |ticket| {
            if ticket
                .stop()
                .wait_until(Instant::now() + Duration::from_secs(5))
                .unwrap_or(false)
            {
                Err(ErrorCode::Cancelled)
            } else {
                Err(ErrorCode::Unavailable)
            }
        })
        .map_err(|_| IpcError::WorkerFailed)?;
    workers
        .submit(result_ticket.clone(), |ticket| {
            if ticket
                .stop()
                .wait_until(Instant::now() + Duration::from_millis(10))
                .unwrap_or(true)
                || ticket.is_cancelled()
            {
                Err(ErrorCode::Cancelled)
            } else {
                Ok("完了".into())
            }
        })
        .map_err(|_| IpcError::WorkerFailed)?;
    let completion = loop {
        if let Some(completion) = workers.try_completion() {
            break completion;
        }
        if Instant::now() >= deadline {
            return Err(IpcError::Timeout);
        }
        std::thread::sleep(Duration::from_millis(1));
    };
    if completion.reply.key != result_key
        || completion.reply.request != second.request
        || !matches!(completion.outcome.as_deref(), Ok("完了"))
    {
        return Err(IpcError::InvalidMessage);
    }
    let now = Instant::now();
    if !completion
        .publish(
            &mut runtime,
            bootstrap.stop(),
            now + Duration::from_millis(500),
        )
        .map_err(|_| IpcError::PolicyDenied)?
    {
        return Err(IpcError::InvalidMessage);
    }
    source
        .revoke_manual_consent()
        .map_err(|_| IpcError::PolicyDenied)?;
    publish_diagnostic(
        &mut runtime,
        bootstrap.stop(),
        deadline,
        4,
        false,
        Some((request_key, convert.request)),
    )?;
    while workers.active_count() != 0 {
        if workers.try_completion().is_some() {
            return Err(IpcError::InvalidMessage);
        }
        if Instant::now() >= deadline {
            return Err(IpcError::Timeout);
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    if !cancelled_ticket.is_cancelled() || result_ticket.is_cancelled() {
        return Err(IpcError::InvalidMessage);
    }
    source
        .grant_manual_consent(&destination)
        .map_err(|_| IpcError::PolicyDenied)?;
    publish_diagnostic(&mut runtime, bootstrap.stop(), deadline, 5, true, None)?;
    let mut resumed = false;
    let mut awaiting_resume = false;
    while !bootstrap.stop().is_cancelled()? && Instant::now() < deadline {
        match owner
            .advance(&mut runtime, bootstrap.stop(), deadline, |request| {
                move |ticket: JobTicket| {
                    if request.text != "again" {
                        Err(ErrorCode::Rejected)
                    } else if ticket.is_cancelled() {
                        Err(ErrorCode::Cancelled)
                    } else {
                        Ok("再開".into())
                    }
                }
            })
            .map_err(|_| IpcError::InvalidMessage)?
        {
            OwnerStep::Broker(PollStep::Idle) => std::thread::sleep(Duration::from_millis(1)),
            OwnerStep::Submitted { key, request } => {
                if awaiting_resume || key != request_key || request != 2 {
                    return Err(IpcError::InvalidMessage);
                }
                awaiting_resume = true;
            }
            OwnerStep::Completed {
                key,
                request,
                delivered: true,
                succeeded: true,
            } if awaiting_resume && key == request_key && request == 2 => {
                resumed = true;
                break;
            }
            _ => return Err(IpcError::InvalidMessage),
        }
    }
    if !resumed {
        return Err(IpcError::Timeout);
    }
    if !owner.stop_and_reap(deadline) {
        return Err(IpcError::WorkerFailed);
    }
    while !bootstrap.stop().is_cancelled()? && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}

fn publish_diagnostic(
    runtime: &mut BrokerPolicyRuntime<'_>,
    stop: &Stop,
    deadline: Instant,
    authorization: u64,
    allowed: bool,
    expected_cancelled: Option<(u64, u64)>,
) -> sumibi_ipc::Result<()> {
    let outcome = runtime
        .publish_current(stop, deadline)
        .map_err(|_| IpcError::PolicyDenied)?
        .ok_or(IpcError::InvalidMessage)?;
    if outcome.policy.authorization != authorization
        || outcome.policy.allowed != allowed
        || outcome.delivered.len() != 2
        || !outcome.closed.is_empty()
        || outcome.cancelled != expected_cancelled.into_iter().collect::<Vec<_>>()
    {
        return Err(IpcError::InvalidMessage);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::broker_workers::{BrokerWorkLoop, BrokerWorkers, OwnerStep, WorkerPoolError};
    use crate::diagnostic_http::DiagnosticLoopbackHttp;
    use crate::manual::{
        self, Credential, Destination, ManualRequest, OrderedConversionTransport, Scheme, SendGate,
    };
    use std::cell::Cell;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
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

    fn read_http_request(stream: &mut TcpStream) -> Vec<u8> {
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut request = Vec::new();
        let mut chunk = [0u8; 1024];
        let header_end = loop {
            let size = stream.read(&mut chunk).unwrap();
            assert!(size > 0);
            request.extend_from_slice(&chunk[..size]);
            if let Some(index) = request.windows(4).position(|part| part == b"\r\n\r\n") {
                break index + 4;
            }
        };
        let headers = std::str::from_utf8(&request[..header_end]).unwrap();
        let length: usize = headers
            .lines()
            .find_map(|line| line.strip_prefix("Content-Length: "))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        while request.len() - header_end < length {
            let size = stream.read(&mut chunk).unwrap();
            assert!(size > 0);
            request.extend_from_slice(&chunk[..size]);
        }
        request
    }

    fn source() -> (PolicyAuthority, Destination) {
        let destination = Destination::new(Scheme::Https, "example.org", 443, "/").unwrap();
        (PolicyAuthority::new(destination.clone()), destination)
    }

    struct CountingTransport(Cell<usize>);
    impl OrderedConversionTransport for CountingTransport {
        fn convert_ordered(
            &self,
            _: &Destination,
            _: &str,
            _: &str,
            _: Instant,
            gate: &mut dyn SendGate,
        ) -> Result<String, SendError> {
            gate.write_authorized(&mut || {
                self.0.set(self.0.get() + 1);
                Ok(())
            })?;
            Ok("日本語".into())
        }
    }

    struct BlockingTransport {
        entered: std::sync::mpsc::Sender<()>,
        release: Mutex<std::sync::mpsc::Receiver<()>>,
        wrote: AtomicBool,
    }
    impl OrderedConversionTransport for BlockingTransport {
        fn convert_ordered(
            &self,
            _: &Destination,
            _: &str,
            _: &str,
            _: Instant,
            gate: &mut dyn SendGate,
        ) -> Result<String, SendError> {
            gate.write_authorized(&mut || {
                self.entered
                    .send(())
                    .map_err(|_| TransportError::Connection)?;
                self.release
                    .lock()
                    .map_err(|_| TransportError::Connection)?
                    .recv()
                    .map_err(|_| TransportError::Connection)?;
                self.wrote.store(true, Ordering::Release);
                Ok(())
            })?;
            Ok("日本語".into())
        }
    }

    #[test]
    fn disconnect_waits_for_an_already_started_bounded_send() {
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
        let (server, mut client) = connected(&stop, deadline);
        runtime
            .attach(server, hello(1, id(90)), &stop, deadline)
            .unwrap();
        read_frame(&mut client, &stop, deadline);
        runtime
            .receive(1, convert(1, 1, 1, policy), Instant::now())
            .unwrap();
        let ticket = runtime.job_ticket(1, 1).unwrap();
        let (entered_send, entered_receive) = std::sync::mpsc::channel();
        let (release_send, release_receive) = std::sync::mpsc::channel();
        let transport = BlockingTransport {
            entered: entered_send,
            release: Mutex::new(release_receive),
            wrote: AtomicBool::new(false),
        };
        std::thread::scope(|scope| {
            let worker_ticket = ticket.clone();
            let source_ref = &source;
            let destination_ref = &destination;
            let transport_ref = &transport;
            let worker = scope.spawn(move || {
                manual::convert_ordered(
                    ManualRequest {
                        revision: Revision {
                            input: 1,
                            settings: policy.settings,
                            authorization: policy.authorization,
                        },
                        destination: destination_ref,
                        source: "nihongo",
                        deadline: worker_ticket.deadline(),
                    },
                    source_ref,
                    &worker_ticket,
                    transport_ref,
                )
            });
            entered_receive
                .recv_timeout(Duration::from_secs(1))
                .unwrap();
            let (started_send, started_receive) = std::sync::mpsc::channel();
            let (closed_send, closed_receive) = std::sync::mpsc::channel();
            let runtime_ref = &mut runtime;
            scope.spawn(move || {
                started_send.send(()).unwrap();
                closed_send
                    .send(runtime_ref.disconnect_with_pending(1))
                    .unwrap();
            });
            started_receive
                .recv_timeout(Duration::from_secs(1))
                .unwrap();
            assert!(matches!(
                closed_receive.recv_timeout(Duration::from_millis(20)),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout)
            ));
            release_send.send(()).unwrap();
            assert_eq!(
                closed_receive.recv_timeout(Duration::from_secs(1)).unwrap(),
                Ok(Some(1))
            );
            let _ = worker.join().unwrap();
        });
        assert!(transport.wrote.load(Ordering::Acquire));
        assert!(ticket.is_cancelled());
        assert_eq!(runtime.active_count(), 0);
    }

    #[test]
    fn reserved_hello_keeps_coalesced_request_for_active_connection() {
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
        let (server, mut client) = connected(&stop, deadline);
        let key = runtime.reserve(server).unwrap();
        let mut combined = message::encode(&hello(1, id(90))).unwrap();
        combined.extend(message::encode(&convert(1, 1, 1, policy)).unwrap());
        client.write_all(combined, &stop, deadline).unwrap();
        let received = runtime.read_reserved_hello(key, &stop, deadline).unwrap();
        runtime.activate(key, received, &stop, deadline).unwrap();
        assert!(matches!(
            read_frame(&mut client, &stop, deadline).message,
            message::Message::Ready(_)
        ));
        let request = runtime.read_active_message(key, &stop, deadline).unwrap();
        assert!(matches!(
            runtime.receive(key, request, Instant::now()),
            Ok(Event::Convert(convert)) if convert.request == 1
        ));
        assert!(runtime.job_ticket(key, 1).is_some());
    }

    #[test]
    fn queued_poll_advances_complete_peer_while_other_frame_is_partial() {
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
        assert_eq!(runtime.active_keys(), [1, 2]);
        assert!(runtime
            .poll_active_message(1, &stop, deadline)
            .unwrap()
            .is_none());
        let first_frame = message::encode(&convert(1, 1, 1, policy)).unwrap();
        first_client
            .write_all(first_frame[..6].to_vec(), &stop, deadline)
            .unwrap();
        assert!(runtime
            .poll_active_message(1, &stop, deadline)
            .unwrap()
            .is_none());
        second_client
            .write_all(
                message::encode(&convert(2, 1, 1, policy)).unwrap(),
                &stop,
                deadline,
            )
            .unwrap();
        assert!(runtime
            .poll_active_message(1, &stop, deadline)
            .unwrap()
            .is_none());
        let second = runtime
            .poll_active_message(2, &stop, deadline)
            .unwrap()
            .unwrap();
        assert!(matches!(
            runtime.receive(2, second, Instant::now()),
            Ok(Event::Convert(convert)) if convert.text == "nihongo"
        ));
        first_client
            .write_all(first_frame[6..].to_vec(), &stop, deadline)
            .unwrap();
        let first = runtime
            .poll_active_message(1, &stop, deadline)
            .unwrap()
            .unwrap();
        assert!(matches!(
            runtime.receive(1, first, Instant::now()),
            Ok(Event::Convert(convert)) if convert.request == 1
        ));
        assert!(runtime.job_ticket(1, 1).is_some());
        assert!(runtime.job_ticket(2, 1).is_some());
    }

    #[test]
    fn bounded_worker_shutdown_wakes_and_reaps_a_pending_job() {
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
        let (server, mut client) = connected(&stop, deadline);
        runtime
            .attach(server, hello(1, id(90)), &stop, deadline)
            .unwrap();
        read_frame(&mut client, &stop, deadline);
        runtime
            .receive(1, convert(1, 1, 1, policy), Instant::now())
            .unwrap();
        let ticket = runtime.job_ticket(1, 1).unwrap();
        let mut workers = BrokerWorkers::new();
        workers
            .submit(ticket.clone(), |ticket| {
                if ticket
                    .stop()
                    .wait_until(Instant::now() + Duration::from_secs(2))
                    .unwrap()
                {
                    Err(ErrorCode::Cancelled)
                } else {
                    Err(ErrorCode::Unavailable)
                }
            })
            .unwrap();
        assert!(workers.stop_and_reap(Instant::now() + Duration::from_millis(500)));
        assert_eq!(workers.active_count(), 0);
        assert!(ticket.is_cancelled());
        assert!(ticket.stop().is_cancelled().unwrap());
    }

    #[test]
    fn accepted_request_deadline_cannot_be_extended_by_worker() {
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
        let (server, mut client) = connected(&stop, deadline);
        runtime
            .attach(server, hello(1, id(90)), &stop, deadline)
            .unwrap();
        read_frame(&mut client, &stop, deadline);
        runtime
            .receive(1, convert(1, 1, 1, policy), Instant::now())
            .unwrap();
        let ticket = runtime.job_ticket(1, 1).unwrap();
        let transport = CountingTransport(Cell::new(0));
        assert_eq!(
            manual::convert_ordered(
                ManualRequest {
                    revision: Revision {
                        input: 1,
                        settings: policy.settings,
                        authorization: policy.authorization,
                    },
                    destination: &destination,
                    source: "nihongo",
                    deadline: ticket.deadline() + Duration::from_millis(1),
                },
                &source,
                &ticket,
                &transport,
            ),
            Err(SendError::InvalidRequest)
        );
        assert_eq!(transport.0.get(), 0);
        let expired_now = Instant::now() + Duration::from_secs(2);
        assert_eq!(runtime.expire(1, expired_now), Ok(Some(1)));
        assert!(ticket.is_cancelled());
        let mut workers = BrokerWorkers::new();
        assert_eq!(
            workers.submit(ticket, |_| Ok("遅延".into())),
            Err(WorkerPoolError::Cancelled)
        );
        runtime
            .receive(
                1,
                convert(1, 1, 2, policy),
                Instant::now() - Duration::from_secs(2),
            )
            .unwrap();
        assert_eq!(
            workers.submit(runtime.job_ticket(1, 2).unwrap(), |_| Ok("遅延".into())),
            Err(WorkerPoolError::Expired)
        );
    }

    #[test]
    fn result_completed_before_deadline_is_not_written_after_owner_delay() {
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
        let (server, mut client) = connected(&stop, deadline);
        runtime
            .attach(server, hello(1, id(90)), &stop, deadline)
            .unwrap();
        read_frame(&mut client, &stop, deadline);
        let accepted_at = Instant::now() - Duration::from_secs(2);
        runtime
            .receive(1, convert(1, 1, 1, policy), accepted_at)
            .unwrap();
        let ticket = runtime.job_ticket(1, 1).unwrap();
        let finished_early = ticket.reply_context(accepted_at + Duration::from_millis(100));
        let now = Instant::now();
        assert_eq!(
            runtime.publish_result(
                finished_early,
                "古い結果".into(),
                &stop,
                now + Duration::from_millis(500)
            ),
            Ok(false)
        );
        assert!(ticket.is_cancelled());
        assert_eq!(runtime.active_keys(), [1]);
        client
            .write_all(
                message::encode(&convert(1, 1, 2, policy)).unwrap(),
                &stop,
                deadline,
            )
            .unwrap();
        assert!(matches!(
            runtime.poll_once(&stop, deadline),
            Ok(PollStep::Received {
                key: 1,
                event: Event::Convert(request)
            }) if request.request == 2
        ));
        let mut forged = runtime
            .job_ticket(1, 2)
            .unwrap()
            .reply_context(Instant::now());
        forged.revision.authorization += 1;
        let now = Instant::now();
        assert_eq!(
            runtime.publish_result(
                forged,
                "不一致".into(),
                &stop,
                now + Duration::from_millis(500)
            ),
            Err(RuntimeError::Incomplete)
        );
        assert_eq!(runtime.active_count(), 0);
    }

    #[test]
    fn owner_loop_dispatches_and_returns_a_worker_result() {
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
        let deadline = Instant::now() + Duration::from_secs(3);
        let (server, mut client) = connected(&stop, deadline);
        runtime
            .attach(server, hello(1, id(90)), &stop, deadline)
            .unwrap();
        read_frame(&mut client, &stop, deadline);
        client
            .write_all(
                message::encode(&convert(1, 1, 1, policy)).unwrap(),
                &stop,
                deadline,
            )
            .unwrap();
        let mut owner = BrokerWorkLoop::new();
        assert!(matches!(
            owner.advance(&mut runtime, &stop, deadline, |request| {
                move |ticket: JobTicket| {
                    assert_eq!(request.text, "nihongo");
                    assert!(!ticket.is_cancelled());
                    Ok("日本語".into())
                }
            }),
            Ok(OwnerStep::Submitted { key: 1, request: 1 })
        ));
        loop {
            match owner
                .advance(&mut runtime, &stop, deadline, |_| {
                    |_: JobTicket| Err(ErrorCode::Unavailable)
                })
                .unwrap()
            {
                OwnerStep::Completed {
                    key: 1,
                    request: 1,
                    delivered: true,
                    succeeded: true,
                } => break,
                OwnerStep::Broker(PollStep::Idle) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(1));
                }
                _ => panic!("unexpected owner step"),
            }
        }
        assert!(matches!(
            read_frame(&mut client, &stop, deadline).message,
            message::Message::Result(result) if result.request == 1 && result.text == "日本語"
        ));
        assert_eq!(owner.active_count(), 0);
    }

    #[test]
    fn owner_loop_publishes_pending_policy_before_reserving_new_pipe() {
        let _guard = crate::HOST_PIPE_TEST_LOCK.lock().unwrap();
        if !supported() {
            return;
        }
        let (source, destination) = source();
        let mut runtime = BrokerPolicyRuntime::new(&source, id(90)).unwrap();
        let old = runtime.policy();
        assert!(!old.allowed);
        let stop = Stop::new().unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        let (server, mut client) = connected(&stop, deadline);
        source
            .replace_credential(Credential::new("fixture".into()))
            .unwrap();
        source.grant_manual_consent(&destination).unwrap();
        let mut owner = BrokerWorkLoop::new();
        let (key, published) = owner.accept(&mut runtime, server, &stop, deadline).unwrap();
        assert_eq!(key, 1);
        let published = published.expect("pending policy must publish before reservation");
        assert!(published.policy.allowed);
        assert!(published.delivered.is_empty());
        assert_eq!(runtime.pending_keys(), [1]);
        runtime
            .activate(key, hello(1, id(90)), &stop, deadline)
            .unwrap();
        assert!(matches!(
            read_frame(&mut client, &stop, deadline).message,
            message::Message::Ready(ready) if ready.policy == published.policy
        ));
    }

    #[test]
    fn manual_owner_loop_rejects_a_different_policy_authority() {
        let (source, destination) = source();
        let source = Arc::new(source);
        let other = Arc::new(PolicyAuthority::new(destination));
        let mut runtime = BrokerPolicyRuntime::new(&source, id(90)).unwrap();
        let mut owner = BrokerWorkLoop::new();
        let stop = Stop::new().unwrap();
        assert!(matches!(
            owner.advance_manual(
                &mut runtime,
                other,
                Arc::new(DiagnosticLoopbackHttp),
                &stop,
                Instant::now() + Duration::from_secs(1)
            ),
            Err(RuntimeError::InvalidTarget)
        ));
        assert_eq!(runtime.active_count(), 0);
    }

    #[test]
    fn manual_owner_loop_uses_the_destination_after_settings_change() {
        let _guard = crate::HOST_PIPE_TEST_LOCK.lock().unwrap();
        if !supported() {
            return;
        }
        let old_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let old_port = old_listener.local_addr().unwrap().port();
        let next_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let next_port = next_listener.local_addr().unwrap().port();
        let old = Destination::new(Scheme::HttpLoopback, "127.0.0.1", old_port, "/old").unwrap();
        let next = Destination::new(Scheme::HttpLoopback, "127.0.0.1", next_port, "/new").unwrap();
        let source = Arc::new(PolicyAuthority::new(old.clone()));
        source
            .replace_credential(Credential::new("fixture".into()))
            .unwrap();
        source.grant_manual_consent(&old).unwrap();
        let mut runtime = BrokerPolicyRuntime::new(&source, id(90)).unwrap();
        let stop = Stop::new().unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        let (server, mut client) = connected(&stop, deadline);
        runtime
            .attach(server, hello(1, id(90)), &stop, deadline)
            .unwrap();
        read_frame(&mut client, &stop, deadline);
        source.set_destination(next.clone()).unwrap();
        source.grant_manual_consent(&next).unwrap();
        drop(old_listener);
        let server = std::thread::spawn(move || {
            let (mut stream, _) = next_listener.accept().unwrap();
            let request = read_http_request(&mut stream);
            let body = r#"{"text":"新しい宛先"}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).unwrap();
            request
        });
        let mut owner = BrokerWorkLoop::new();
        assert!(matches!(
            owner.advance_manual(
                &mut runtime,
                source.clone(),
                Arc::new(DiagnosticLoopbackHttp),
                &stop,
                deadline
            ),
            Ok(OwnerStep::Published(outcome)) if outcome.policy.allowed
        ));
        let current = runtime.policy();
        assert!(matches!(
            read_frame(&mut client, &stop, deadline).message,
            message::Message::PolicyChanged(policy) if policy == current
        ));
        client
            .write_all(
                message::encode(&convert(1, 1, 1, current)).unwrap(),
                &stop,
                deadline,
            )
            .unwrap();
        assert!(matches!(
            owner.advance_manual(
                &mut runtime,
                source.clone(),
                Arc::new(DiagnosticLoopbackHttp),
                &stop,
                deadline
            ),
            Ok(OwnerStep::Submitted { key: 1, request: 1 })
        ));
        loop {
            match owner
                .advance_manual(
                    &mut runtime,
                    source.clone(),
                    Arc::new(DiagnosticLoopbackHttp),
                    &stop,
                    deadline,
                )
                .unwrap()
            {
                OwnerStep::Completed {
                    key: 1,
                    request: 1,
                    delivered: true,
                    succeeded: true,
                } => break,
                OwnerStep::Broker(PollStep::Idle) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(1));
                }
                _ => panic!("updated destination did not complete"),
            }
        }
        assert!(matches!(
            read_frame(&mut client, &stop, deadline).message,
            message::Message::Result(result) if result.text == "新しい宛先"
        ));
        let request = server.join().unwrap();
        assert!(request
            .windows(b"POST /new".len())
            .any(|part| part == b"POST /new"));
    }

    #[test]
    fn owner_loop_publishes_revoke_before_a_queued_worker_completion() {
        use std::sync::atomic::{AtomicBool, Ordering};

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
        let deadline = Instant::now() + Duration::from_secs(3);
        let (server, mut client) = connected(&stop, deadline);
        runtime
            .attach(server, hello(1, id(90)), &stop, deadline)
            .unwrap();
        read_frame(&mut client, &stop, deadline);
        client
            .write_all(
                message::encode(&convert(1, 1, 1, policy)).unwrap(),
                &stop,
                deadline,
            )
            .unwrap();
        let done = Arc::new(AtomicBool::new(false));
        let worker_done = done.clone();
        let mut owner = BrokerWorkLoop::new();
        assert!(matches!(
            owner.advance(&mut runtime, &stop, deadline, |_| {
                move |_: JobTicket| {
                    worker_done.store(true, Ordering::Release);
                    Ok("旧結果".into())
                }
            }),
            Ok(OwnerStep::Submitted { key: 1, request: 1 })
        ));
        while !done.load(Ordering::Acquire) {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        source.revoke_manual_consent().unwrap();
        let ticket = runtime.job_ticket(1, 1).unwrap();
        assert!(matches!(
            owner.advance(&mut runtime, &stop, deadline, |_| {
                |_: JobTicket| Err(ErrorCode::Unavailable)
            }),
            Ok(OwnerStep::Published(outcome))
                if outcome.cancelled == [(1, 1)] && outcome.delivered == [1]
        ));
        assert!(ticket.is_cancelled());
        assert!(matches!(
            read_frame(&mut client, &stop, deadline).message,
            message::Message::PolicyChanged(next) if !next.allowed
        ));
        assert!(owner.stop_and_reap(deadline));
        assert!(runtime.job_ticket(1, 1).is_none());
    }

    #[test]
    fn owner_loop_returns_a_worker_result_while_another_hello_is_partial() {
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
        let deadline = Instant::now() + Duration::from_secs(3);
        let (first_server, mut first_client) = connected(&stop, deadline);
        runtime
            .attach(first_server, hello(1, id(90)), &stop, deadline)
            .unwrap();
        read_frame(&mut first_client, &stop, deadline);
        let (second_server, mut second_client) = connected(&stop, deadline);
        assert_eq!(runtime.reserve(second_server), Ok(2));
        let second_hello = message::encode(&hello(2, id(90))).unwrap();
        second_client
            .write_all(second_hello[..5].to_vec(), &stop, deadline)
            .unwrap();
        first_client
            .write_all(
                message::encode(&convert(1, 1, 1, policy)).unwrap(),
                &stop,
                deadline,
            )
            .unwrap();
        let mut owner = BrokerWorkLoop::new();
        assert!(matches!(
            owner.advance(&mut runtime, &stop, deadline, |_| {
                |_: JobTicket| Ok("完了".into())
            }),
            Ok(OwnerStep::Submitted { key: 1, request: 1 })
        ));
        loop {
            match owner
                .advance(&mut runtime, &stop, deadline, |_| {
                    |_: JobTicket| Err(ErrorCode::Unavailable)
                })
                .unwrap()
            {
                OwnerStep::Completed {
                    key: 1,
                    request: 1,
                    delivered: true,
                    succeeded: true,
                } => break,
                OwnerStep::Broker(PollStep::Idle) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(1));
                }
                _ => panic!("partial Hello blocked or changed another connection"),
            }
        }
        assert_eq!(runtime.pending_keys(), [2]);
        assert!(matches!(
            read_frame(&mut first_client, &stop, deadline).message,
            message::Message::Result(result) if result.text == "完了"
        ));
        second_client
            .write_all(second_hello[5..].to_vec(), &stop, deadline)
            .unwrap();
        assert!(matches!(
            owner.advance(&mut runtime, &stop, deadline, |_| {
                |_: JobTicket| Err(ErrorCode::Unavailable)
            }),
            Ok(OwnerStep::Broker(PollStep::Activated(2)))
        ));
        assert!(matches!(
            read_frame(&mut second_client, &stop, deadline).message,
            message::Message::Ready(ready) if ready.policy == policy
        ));
    }

    #[test]
    fn owner_loop_rejects_new_request_when_cancelled_workers_fill_pool() {
        use std::sync::atomic::{AtomicBool, Ordering};

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
        let deadline = Instant::now() + Duration::from_secs(10);
        let (server, mut client) = connected(&stop, deadline);
        runtime
            .attach(server, hello(1, id(90)), &stop, deadline)
            .unwrap();
        read_frame(&mut client, &stop, deadline);
        let mut owner = BrokerWorkLoop::new();
        let release = Arc::new(AtomicBool::new(false));
        for request in 1..=MAX_SESSIONS as u64 {
            let mut frame = convert(1, 1, request, policy);
            let message::Message::Convert(convert) = &mut frame.message else {
                unreachable!();
            };
            convert.timeout_ms = 5_000;
            client
                .write_all(message::encode(&frame).unwrap(), &stop, deadline)
                .unwrap();
            let worker_release = release.clone();
            assert!(matches!(
                owner.advance(&mut runtime, &stop, deadline, move |_| {
                    move |ticket: JobTicket| {
                        while !worker_release.load(Ordering::Acquire)
                            && Instant::now() < ticket.deadline()
                        {
                            std::thread::sleep(Duration::from_millis(1));
                        }
                        Err(ErrorCode::Cancelled)
                    }
                }),
                Ok(OwnerStep::Submitted { key: 1, request: accepted }) if accepted == request
            ));
            client
                .write_all(
                    message::encode(&Envelope {
                        version: message::VERSION,
                        session: id(1),
                        generation: 1,
                        message: message::Message::Cancel(Cancel { request }),
                    })
                    .unwrap(),
                    &stop,
                    deadline,
                )
                .unwrap();
            assert!(matches!(
                owner.advance(&mut runtime, &stop, deadline, |_| {
                    |_: JobTicket| Err(ErrorCode::Unavailable)
                }),
                Ok(OwnerStep::Broker(PollStep::Received {
                    key: 1,
                    event: Event::Cancelled(cancelled)
                })) if cancelled == request
            ));
        }
        assert_eq!(owner.active_count(), MAX_SESSIONS);
        client
            .write_all(
                message::encode(&convert(1, 1, 9, policy)).unwrap(),
                &stop,
                deadline,
            )
            .unwrap();
        assert!(matches!(
            owner.advance(&mut runtime, &stop, deadline, |_| {
                |_: JobTicket| Err(ErrorCode::Unavailable)
            }),
            Ok(OwnerStep::Rejected {
                key: 1,
                request: 9,
                cause: WorkerPoolError::Full,
                delivered: true
            })
        ));
        assert!(matches!(
            read_frame(&mut client, &stop, deadline).message,
            message::Message::Error(error)
                if error.request == 9 && error.code == ErrorCode::Unavailable
        ));
        assert!(runtime.job_ticket(1, 9).is_none());
        release.store(true, Ordering::Release);
        assert!(owner.stop_and_reap(deadline));
        client
            .write_all(
                message::encode(&convert(1, 1, 10, policy)).unwrap(),
                &stop,
                deadline,
            )
            .unwrap();
        assert!(matches!(
            owner.advance(&mut runtime, &stop, deadline, |_| {
                |_: JobTicket| Ok("再開".into())
            }),
            Ok(OwnerStep::Submitted {
                key: 1,
                request: 10
            })
        ));
        loop {
            match owner
                .advance(&mut runtime, &stop, deadline, |_| {
                    |_: JobTicket| Err(ErrorCode::Unavailable)
                })
                .unwrap()
            {
                OwnerStep::Completed {
                    key: 1,
                    request: 10,
                    delivered: true,
                    succeeded: true,
                } => break,
                OwnerStep::Broker(PollStep::Idle) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(1));
                }
                _ => panic!("worker capacity did not recover"),
            }
        }
        assert!(matches!(
            read_frame(&mut client, &stop, deadline).message,
            message::Message::Result(result) if result.request == 10 && result.text == "再開"
        ));
    }

    #[test]
    fn authenticated_pipe_worker_and_loopback_http_return_result_and_error() {
        let _guard = crate::HOST_PIPE_TEST_LOCK.lock().unwrap();
        if !supported() {
            return;
        }
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut first, _) = listener.accept().unwrap();
            let request = read_http_request(&mut first);
            let body = r#"{"text":"日本語"}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            first.write_all(response.as_bytes()).unwrap();
            let (mut second, _) = listener.accept().unwrap();
            let failed_request = read_http_request(&mut second);
            second
                .write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .unwrap();
            (request, failed_request)
        });
        let destination =
            Destination::new(Scheme::HttpLoopback, "127.0.0.1", port, "/convert").unwrap();
        let source = Arc::new(PolicyAuthority::new(destination.clone()));
        source
            .replace_credential(Credential::new("fixture".into()))
            .unwrap();
        source.grant_manual_consent(&destination).unwrap();
        let mut runtime = BrokerPolicyRuntime::new(&source, id(90)).unwrap();
        let policy = runtime.policy();
        let stop = Stop::new().unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        let (pipe, mut client) = connected(&stop, deadline);
        runtime
            .attach(pipe, hello(1, id(90)), &stop, deadline)
            .unwrap();
        read_frame(&mut client, &stop, deadline);
        client
            .write_all(
                message::encode(&convert(1, 1, 1, policy)).unwrap(),
                &stop,
                deadline,
            )
            .unwrap();
        let mut owner = BrokerWorkLoop::new();
        assert!(matches!(
            owner.advance_manual(
                &mut runtime,
                source.clone(),
                Arc::new(DiagnosticLoopbackHttp),
                &stop,
                deadline
            ),
            Ok(OwnerStep::Submitted { key: 1, request: 1 })
        ));
        loop {
            match owner
                .advance(&mut runtime, &stop, deadline, |_| {
                    |_: JobTicket| Err(ErrorCode::Unavailable)
                })
                .unwrap()
            {
                OwnerStep::Completed {
                    key: 1,
                    request: 1,
                    delivered: true,
                    succeeded: true,
                } => break,
                OwnerStep::Broker(PollStep::Idle) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(1));
                }
                _ => panic!("unexpected owner step"),
            }
        }
        assert!(matches!(
            read_frame(&mut client, &stop, deadline).message,
            message::Message::Result(result) if result.text == "日本語"
        ));
        client
            .write_all(
                message::encode(&convert(1, 1, 2, policy)).unwrap(),
                &stop,
                deadline,
            )
            .unwrap();
        assert!(matches!(
            owner.advance_manual(
                &mut runtime,
                source.clone(),
                Arc::new(DiagnosticLoopbackHttp),
                &stop,
                deadline
            ),
            Ok(OwnerStep::Submitted { key: 1, request: 2 })
        ));
        loop {
            match owner
                .advance(&mut runtime, &stop, deadline, |_| {
                    |_: JobTicket| Err(ErrorCode::Unavailable)
                })
                .unwrap()
            {
                OwnerStep::Completed {
                    key: 1,
                    request: 2,
                    delivered: true,
                    succeeded: false,
                } => break,
                OwnerStep::Broker(PollStep::Idle) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(1));
                }
                _ => panic!("unexpected owner step"),
            }
        }
        assert!(matches!(
            read_frame(&mut client, &stop, deadline).message,
            message::Message::Error(error)
                if error.request == 2 && error.code == ErrorCode::Unavailable
        ));
        let (first_request, second_request) = server.join().unwrap();
        for request in [&first_request, &second_request] {
            assert!(request
                .windows(b"Authorization: Bearer fixture".len())
                .any(|part| part == b"Authorization: Bearer fixture"));
            assert!(request
                .windows(b"nihongo".len())
                .any(|part| part == b"nihongo"));
        }
        assert_eq!(owner.active_count(), 0);
    }

    #[test]
    fn policy_revoke_drops_a_worker_waiting_for_http_response() {
        let _guard = crate::HOST_PIPE_TEST_LOCK.lock().unwrap();
        if !supported() {
            return;
        }
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (received_send, received_receive) = std::sync::mpsc::channel();
        let (release_send, release_receive) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let request = read_http_request(&mut stream);
            received_send.send(()).unwrap();
            release_receive.recv().unwrap();
            request
        });
        let destination =
            Destination::new(Scheme::HttpLoopback, "127.0.0.1", port, "/convert").unwrap();
        let source = Arc::new(PolicyAuthority::new(destination.clone()));
        source
            .replace_credential(Credential::new("fixture".into()))
            .unwrap();
        source.grant_manual_consent(&destination).unwrap();
        let mut runtime = BrokerPolicyRuntime::new(&source, id(90)).unwrap();
        let policy = runtime.policy();
        let stop = Stop::new().unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        let (pipe, mut client) = connected(&stop, deadline);
        runtime
            .attach(pipe, hello(1, id(90)), &stop, deadline)
            .unwrap();
        read_frame(&mut client, &stop, deadline);
        client
            .write_all(
                message::encode(&convert(1, 1, 1, policy)).unwrap(),
                &stop,
                deadline,
            )
            .unwrap();
        let mut owner = BrokerWorkLoop::new();
        assert!(matches!(
            owner.advance_manual(
                &mut runtime,
                source.clone(),
                Arc::new(DiagnosticLoopbackHttp),
                &stop,
                deadline
            ),
            Ok(OwnerStep::Submitted { key: 1, request: 1 })
        ));
        let ticket = runtime.job_ticket(1, 1).unwrap();
        received_receive
            .recv_timeout(Duration::from_secs(1))
            .unwrap();
        source.revoke_manual_consent().unwrap();
        let OwnerStep::Published(outcome) = owner
            .advance(&mut runtime, &stop, deadline, |_| {
                |_: JobTicket| Err(ErrorCode::Unavailable)
            })
            .unwrap()
        else {
            panic!("policy must be published before the worker completion");
        };
        assert_eq!(outcome.cancelled, [(1, 1)]);
        assert_eq!(outcome.delivered, [1]);
        assert!(ticket.is_cancelled());
        assert!(matches!(
            read_frame(&mut client, &stop, deadline).message,
            message::Message::PolicyChanged(next) if !next.allowed
        ));
        let waiting_started = Instant::now();
        while owner.active_count() != 0 {
            assert!(matches!(
                owner.advance(&mut runtime, &stop, deadline, |_| {
                    |_: JobTicket| Err(ErrorCode::Unavailable)
                }),
                Ok(OwnerStep::Broker(PollStep::Idle))
            ));
            assert!(waiting_started.elapsed() < Duration::from_millis(500));
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(runtime.active_keys(), [1]);
        release_send.send(()).unwrap();
        let request = server.join().unwrap();
        assert!(request
            .windows(b"nihongo".len())
            .any(|part| part == b"nihongo"));
    }

    #[test]
    fn poll_once_interleaves_hello_partial_requests_and_policy_publication() {
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
        assert_eq!(runtime.reserve(first_server), Ok(1));
        assert_eq!(runtime.reserve(second_server), Ok(2));
        let first_hello = message::encode(&hello(1, id(90))).unwrap();
        first_client
            .write_all(first_hello[..5].to_vec(), &stop, deadline)
            .unwrap();
        assert!(matches!(
            runtime.poll_once(&stop, deadline),
            Ok(PollStep::Idle)
        ));
        second_client
            .write_all(message::encode(&hello(2, id(90))).unwrap(), &stop, deadline)
            .unwrap();
        assert!(matches!(
            runtime.poll_once(&stop, deadline),
            Ok(PollStep::Activated(2))
        ));
        read_frame(&mut second_client, &stop, deadline);
        first_client
            .write_all(first_hello[5..].to_vec(), &stop, deadline)
            .unwrap();
        assert!(matches!(
            runtime.poll_once(&stop, deadline),
            Ok(PollStep::Activated(1))
        ));
        read_frame(&mut first_client, &stop, deadline);

        let first_request = message::encode(&convert(1, 1, 1, policy)).unwrap();
        first_client
            .write_all(first_request[..7].to_vec(), &stop, deadline)
            .unwrap();
        second_client
            .write_all(
                message::encode(&convert(2, 1, 1, policy)).unwrap(),
                &stop,
                deadline,
            )
            .unwrap();
        assert!(matches!(
            runtime.poll_once(&stop, deadline),
            Ok(PollStep::Received {
                key: 2,
                event: Event::Convert(_)
            })
        ));
        first_client
            .write_all(first_request[7..].to_vec(), &stop, deadline)
            .unwrap();
        assert!(matches!(
            runtime.poll_once(&stop, deadline),
            Ok(PollStep::Received {
                key: 1,
                event: Event::Convert(_)
            })
        ));
        let (pending_server, _pending_client) = connected(&stop, deadline);
        assert_eq!(runtime.reserve(pending_server), Ok(3));
        source.revoke_manual_consent().unwrap();
        let PollStep::Published(outcome) = runtime.poll_once(&stop, deadline).unwrap() else {
            panic!("policy publication must lead the poll pass");
        };
        assert_eq!(outcome.closed, [3]);
        assert_eq!(outcome.cancelled.len(), 2);
        assert_eq!(runtime.pending_count(), 0);
        assert_eq!(runtime.active_count(), 2);
        assert!(matches!(
            read_frame(&mut first_client, &stop, deadline).message,
            message::Message::PolicyChanged(policy) if !policy.allowed
        ));
        assert!(matches!(
            read_frame(&mut second_client, &stop, deadline).message,
            message::Message::PolicyChanged(policy) if !policy.allowed
        ));
    }

    #[test]
    fn poll_once_rotates_away_from_a_coalesced_connection() {
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
        runtime
            .receive(1, convert(1, 1, 1, policy), Instant::now())
            .unwrap();
        let cancelled = runtime.job_ticket(1, 1).unwrap();
        let cancel = Envelope {
            version: message::VERSION,
            session: id(1),
            generation: 1,
            message: message::Message::Cancel(Cancel { request: 1 }),
        };
        let mut coalesced = message::encode(&cancel).unwrap();
        coalesced.extend(message::encode(&convert(1, 1, 2, policy)).unwrap());
        first_client.write_all(coalesced, &stop, deadline).unwrap();
        second_client
            .write_all(
                message::encode(&convert(2, 1, 1, policy)).unwrap(),
                &stop,
                deadline,
            )
            .unwrap();
        assert!(matches!(
            runtime.poll_once(&stop, deadline),
            Ok(PollStep::Received {
                key: 1,
                event: Event::Cancelled(1)
            })
        ));
        assert!(cancelled.is_cancelled());
        assert!(matches!(
            runtime.poll_once(&stop, deadline),
            Ok(PollStep::Received {
                key: 2,
                event: Event::Convert(_)
            })
        ));
        assert!(matches!(
            runtime.poll_once(&stop, deadline),
            Ok(PollStep::Received {
                key: 1,
                event: Event::Convert(_)
            })
        ));
        assert!(runtime.job_ticket(1, 2).is_some());
        assert!(runtime.job_ticket(2, 1).is_some());
    }

    #[test]
    fn publication_closes_two_partial_hellos_before_acknowledging_change() {
        let _guard = crate::HOST_PIPE_TEST_LOCK.lock().unwrap();
        if !supported() {
            return;
        }
        let (source, destination) = source();
        let mut runtime = BrokerPolicyRuntime::new(&source, id(90)).unwrap();
        let stop = Stop::new().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let (active_server, mut active_client) = connected(&stop, deadline);
        runtime
            .attach(active_server, hello(1, id(90)), &stop, deadline)
            .unwrap();
        read_frame(&mut active_client, &stop, deadline);
        let (first_server, mut first_client) = connected(&stop, deadline);
        let (second_server, mut second_client) = connected(&stop, deadline);
        assert_eq!(runtime.reserve(first_server), Ok(2));
        assert_eq!(runtime.reserve(second_server), Ok(3));
        let first = message::encode(&hello(2, id(90))).unwrap();
        let second = message::encode(&hello(3, id(90))).unwrap();
        first_client
            .write_all(first[..3].to_vec(), &stop, deadline)
            .unwrap();
        second_client
            .write_all(second[..7].to_vec(), &stop, deadline)
            .unwrap();
        assert_eq!(runtime.pending_keys(), [2, 3]);
        assert!(runtime
            .poll_reserved_hello(2, &stop, deadline)
            .unwrap()
            .is_none());
        assert!(runtime
            .poll_reserved_hello(3, &stop, deadline)
            .unwrap()
            .is_none());
        source
            .replace_credential(Credential::new("fixture".into()))
            .unwrap();
        source.grant_manual_consent(&destination).unwrap();
        let outcome = runtime.publish_current(&stop, deadline).unwrap().unwrap();
        assert_eq!(outcome.closed, [2, 3]);
        assert_eq!(outcome.delivered, [1]);
        assert_eq!(runtime.pending_count(), 0);
        assert!(matches!(
            runtime.poll_reserved_hello(2, &stop, deadline),
            Err(RuntimeError::MissingPipe)
        ));
        assert!(matches!(
            read_frame(&mut active_client, &stop, deadline).message,
            message::Message::PolicyChanged(policy) if policy == outcome.policy
        ));
        let (new_server, mut new_client) = connected(&stop, deadline);
        assert_eq!(
            runtime.attach(new_server, hello(4, id(90)), &stop, deadline),
            Ok(4)
        );
        assert!(matches!(
            read_frame(&mut new_client, &stop, deadline).message,
            message::Message::Ready(ready) if ready.policy == outcome.policy
        ));
    }

    #[test]
    fn settings_acknowledgment_waits_for_pipe_delivery_and_reports_coalescing() {
        let _guard = crate::HOST_PIPE_TEST_LOCK.lock().unwrap();
        if !supported() {
            return;
        }
        let (source, destination) = source();
        let mut runtime = BrokerPolicyRuntime::new(&source, id(90)).unwrap();
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
        let (pending_server, _pending_client) = connected(&stop, deadline);
        assert_eq!(runtime.reserve(pending_server), Ok(3));
        source
            .replace_credential(Credential::new("fixture".into()))
            .unwrap();
        let granted = source.grant_manual_consent(&destination).unwrap();
        let receipt = runtime
            .acknowledge_version(granted, &stop, deadline)
            .unwrap();
        assert_eq!(receipt.requested, granted);
        assert_eq!(receipt.completion, DeliveryCompletion::ExactOrClosed);
        assert!(receipt.current.allowed);
        assert_eq!(runtime.pending_count(), 0);
        for client in [&mut first_client, &mut second_client] {
            assert!(matches!(
                read_frame(client, &stop, deadline).message,
                message::Message::PolicyChanged(policy) if policy == receipt.current
            ));
        }
        let revoked = source.revoke_manual_consent().unwrap();
        source.grant_manual_consent(&destination).unwrap();
        let superseded = runtime
            .acknowledge_version(revoked, &stop, deadline)
            .unwrap();
        assert_eq!(superseded.completion, DeliveryCompletion::Superseded);
        assert!(superseded.current.allowed);
        assert!(superseded.current.authorization > revoked.authorization);
        for client in [&mut first_client, &mut second_client] {
            assert!(matches!(
                read_frame(client, &stop, deadline).message,
                message::Message::PolicyChanged(policy) if policy == superseded.current
            ));
        }
        assert!(matches!(
            runtime.acknowledge_version(
                PolicyVersion {
                    settings: 1,
                    authorization: superseded.current.authorization + 1,
                },
                &stop,
                deadline,
            ),
            Err(RuntimeError::InvalidTarget)
        ));
        assert_eq!(runtime.active_count(), 2);
    }

    #[test]
    fn source_change_between_request_chunks_rejects_old_text_only_on_that_pipe() {
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
        let (old_server, mut old_client) = connected(&stop, deadline);
        let (live_server, mut live_client) = connected(&stop, deadline);
        runtime
            .attach(old_server, hello(1, id(90)), &stop, deadline)
            .unwrap();
        runtime
            .attach(live_server, hello(2, id(90)), &stop, deadline)
            .unwrap();
        read_frame(&mut old_client, &stop, deadline);
        read_frame(&mut live_client, &stop, deadline);
        let frame = message::encode(&convert(1, 1, 1, granted)).unwrap();
        old_client
            .write_all(frame[..8].to_vec(), &stop, deadline)
            .unwrap();
        assert!(runtime
            .poll_active_message(1, &stop, deadline)
            .unwrap()
            .is_none());
        source.revoke_manual_consent().unwrap();
        old_client
            .write_all(frame[8..].to_vec(), &stop, deadline)
            .unwrap();
        let completed = runtime
            .poll_active_message(1, &stop, deadline)
            .unwrap()
            .unwrap();
        assert!(matches!(
            runtime.receive(1, completed, Instant::now()),
            Err(RuntimeError::PolicyPending)
        ));
        assert!(runtime.job_ticket(1, 1).is_none());
        assert_eq!(runtime.active_keys(), [2]);
        let outcome = runtime.publish_current(&stop, deadline).unwrap().unwrap();
        assert_eq!(outcome.delivered, [2]);
        assert!(matches!(
            read_frame(&mut live_client, &stop, deadline).message,
            message::Message::PolicyChanged(policy) if policy == outcome.policy && !policy.allowed
        ));
    }

    #[test]
    fn malformed_active_frame_cancels_only_its_worker() {
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
        let (bad_server, mut bad_client) = connected(&stop, deadline);
        let (good_server, mut good_client) = connected(&stop, deadline);
        runtime
            .attach(bad_server, hello(1, id(90)), &stop, deadline)
            .unwrap();
        runtime
            .attach(good_server, hello(2, id(90)), &stop, deadline)
            .unwrap();
        read_frame(&mut bad_client, &stop, deadline);
        read_frame(&mut good_client, &stop, deadline);
        runtime
            .receive(1, convert(1, 1, 1, policy), Instant::now())
            .unwrap();
        runtime
            .receive(2, convert(2, 1, 1, policy), Instant::now())
            .unwrap();
        let bad_job = runtime.job_ticket(1, 1).unwrap();
        let good_job = runtime.job_ticket(2, 1).unwrap();
        bad_client
            .write_all(sumibi_protocol::encode("{").unwrap(), &stop, deadline)
            .unwrap();
        assert!(matches!(
            runtime.read_active_message(1, &stop, deadline),
            Err(RuntimeError::Transport(IpcError::InvalidMessage))
        ));
        assert!(bad_job.is_cancelled());
        assert!(!good_job.is_cancelled());
        assert_eq!(runtime.active_count(), 1);
        let now = Instant::now();
        assert_eq!(
            runtime.publish_result(
                good_job.reply_context(now),
                "日本語".into(),
                &stop,
                now + Duration::from_millis(500)
            ),
            Ok(true)
        );
        assert!(matches!(
            read_frame(&mut good_client, &stop, deadline).message,
            message::Message::Result(result) if result.text == "日本語"
        ));
    }

    #[test]
    fn pending_hello_closes_before_publication_completion_and_key_is_not_reused() {
        let _guard = crate::HOST_PIPE_TEST_LOCK.lock().unwrap();
        if !supported() {
            return;
        }
        let (source, destination) = source();
        let mut runtime = BrokerPolicyRuntime::new(&source, id(90)).unwrap();
        let stop = Stop::new().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let (active_server, mut active_client) = connected(&stop, deadline);
        assert_eq!(
            runtime.attach(active_server, hello(1, id(90)), &stop, deadline),
            Ok(1)
        );
        read_frame(&mut active_client, &stop, deadline);
        let (pending_server, _pending_client) = connected(&stop, deadline);
        assert_eq!(runtime.reserve(pending_server), Ok(2));
        assert_eq!(runtime.pending_count(), 1);
        source
            .replace_credential(Credential::new("fixture".into()))
            .unwrap();
        source.grant_manual_consent(&destination).unwrap();
        let outcome = runtime.publish_current(&stop, deadline).unwrap().unwrap();
        assert_eq!(outcome.delivered, [1]);
        assert_eq!(outcome.closed, [2]);
        assert_eq!(outcome.completion, DeliveryCompletion::ExactOrClosed);
        assert_eq!(runtime.pending_count(), 0);
        assert_eq!(runtime.active_count(), 1);
        assert!(matches!(
            read_frame(&mut active_client, &stop, deadline).message,
            message::Message::PolicyChanged(policy) if policy == outcome.policy
        ));
        assert!(matches!(
            runtime.activate(2, hello(2, id(90)), &stop, deadline),
            Err(RuntimeError::MissingPipe)
        ));
        let (next_server, mut next_client) = connected(&stop, deadline);
        assert_eq!(
            runtime.attach(next_server, hello(2, id(90)), &stop, deadline),
            Ok(3)
        );
        assert!(matches!(
            read_frame(&mut next_client, &stop, deadline).message,
            message::Message::Ready(ready) if ready.policy == outcome.policy
        ));
    }

    #[test]
    fn source_change_before_hello_rejects_old_pipe_and_new_ready_uses_latest_epoch() {
        let _guard = crate::HOST_PIPE_TEST_LOCK.lock().unwrap();
        if !supported() {
            return;
        }
        let (source, destination) = source();
        let mut runtime = BrokerPolicyRuntime::new(&source, id(90)).unwrap();
        let stop = Stop::new().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let (old_server, _old_client) = connected(&stop, deadline);
        assert_eq!(runtime.reserve(old_server), Ok(1));
        source
            .replace_credential(Credential::new("fixture".into()))
            .unwrap();
        source.grant_manual_consent(&destination).unwrap();
        assert_eq!(
            runtime.activate(1, hello(1, id(90)), &stop, deadline),
            Err(RuntimeError::PolicyPending)
        );
        assert_eq!(runtime.pending_count(), 0);
        assert_eq!(runtime.active_count(), 0);
        let publication = runtime.publish_current(&stop, deadline).unwrap().unwrap();
        assert!(publication.policy.allowed);
        assert!(publication.delivered.is_empty());
        assert!(publication.closed.is_empty());
        let (new_server, mut new_client) = connected(&stop, deadline);
        assert_eq!(
            runtime.attach(new_server, hello(2, id(90)), &stop, deadline),
            Ok(2)
        );
        assert!(matches!(
            read_frame(&mut new_client, &stop, deadline).message,
            message::Message::Ready(ready) if ready.policy == publication.policy
        ));
    }

    #[test]
    fn result_write_uses_host_epoch_and_revocation_cancels_unsent_result() {
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
        let stop = Stop::new().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let (server, mut client) = connected(&stop, deadline);
        assert_eq!(
            runtime.attach(server, hello(1, id(90)), &stop, deadline),
            Ok(1)
        );
        let policy = runtime.policy();
        read_frame(&mut client, &stop, deadline);
        let revision = Revision {
            input: 1,
            settings: policy.settings,
            authorization: policy.authorization,
        };
        let reply = |request, completed_at| ReplyContext {
            broker: id(90),
            key: 1,
            request,
            revision,
            completed_at,
        };
        assert!(matches!(
            runtime.receive(1, convert(1, 1, 1, policy), Instant::now()),
            Ok(Event::Convert(_))
        ));
        let first_job = runtime.job_ticket(1, 1).unwrap();
        assert!(first_job.reply_context(Instant::now()).broker == id(90));
        assert!(!first_job.is_cancelled());
        let now = Instant::now();
        assert_eq!(
            runtime.publish_result(
                ReplyContext {
                    broker: id(91),
                    ..reply(1, now)
                },
                "別Broker".into(),
                &stop,
                now + Duration::from_millis(500)
            ),
            Err(RuntimeError::WrongBroker)
        );
        assert_eq!(
            runtime.publish_result(
                reply(1, now),
                "日本語".into(),
                &stop,
                now + Duration::from_millis(500)
            ),
            Ok(true)
        );
        assert!(!first_job.is_cancelled());
        assert!(runtime.job_ticket(1, 1).is_none());
        assert!(matches!(
            read_frame(&mut client, &stop, deadline).message,
            message::Message::Result(result) if result.request == 1 && result.text == "日本語"
        ));
        assert!(matches!(
            runtime.receive(1, convert(1, 1, 2, policy), Instant::now()),
            Ok(Event::Convert(_))
        ));
        let now = Instant::now();
        assert_eq!(
            runtime.publish_error(
                reply(2, now),
                ErrorCode::Unavailable,
                &stop,
                now + Duration::from_millis(500)
            ),
            Ok(true)
        );
        assert!(matches!(
            read_frame(&mut client, &stop, deadline).message,
            message::Message::Error(error)
                if error.request == 2 && error.code == ErrorCode::Unavailable
        ));
        assert!(matches!(
            runtime.receive(1, convert(1, 1, 3, policy), Instant::now()),
            Ok(Event::Convert(_))
        ));
        let revoked_job = runtime.job_ticket(1, 3).unwrap();
        source.revoke_manual_consent().unwrap();
        let now = Instant::now();
        assert_eq!(
            runtime.publish_result(
                reply(3, now),
                "古い結果".into(),
                &stop,
                now + Duration::from_millis(500)
            ),
            Err(RuntimeError::Source(ChangeError::StalePolicy))
        );
        let revoked = runtime.publish_current(&stop, deadline).unwrap().unwrap();
        assert_eq!(revoked.cancelled, [(1, 3)]);
        assert!(revoked_job.is_cancelled());
        assert!(runtime.job_ticket(1, 3).is_none());
        assert!(matches!(
            read_frame(&mut client, &stop, deadline).message,
            message::Message::PolicyChanged(policy) if policy == revoked.policy
        ));
    }

    #[test]
    fn expired_worker_result_is_silent_and_next_request_survives() {
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
        let stop = Stop::new().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let (server, mut client) = connected(&stop, deadline);
        assert_eq!(
            runtime.attach(server, hello(1, id(90)), &stop, deadline),
            Ok(1)
        );
        read_frame(&mut client, &stop, deadline);
        let policy = runtime.policy();
        let mut timed = convert(1, 1, 1, policy);
        if let message::Message::Convert(request) = &mut timed.message {
            request.timeout_ms = 1;
        }
        let accepted = Instant::now();
        assert!(matches!(
            runtime.receive(1, timed, accepted),
            Ok(Event::Convert(_))
        ));
        let expired_job = runtime.job_ticket(1, 1).unwrap();
        let expired = accepted + Duration::from_millis(2);
        assert_eq!(runtime.expire(1, expired), Ok(Some(1)));
        assert!(expired_job.is_cancelled());
        assert!(runtime.job_ticket(1, 1).is_none());
        let revision = Revision {
            input: 1,
            settings: policy.settings,
            authorization: policy.authorization,
        };
        let now = Instant::now();
        assert_eq!(
            runtime.publish_result(
                ReplyContext {
                    broker: id(90),
                    key: 1,
                    request: 1,
                    revision,
                    completed_at: expired,
                },
                "期限切れ".into(),
                &stop,
                now + Duration::from_millis(500)
            ),
            Ok(false)
        );
        assert!(matches!(
            runtime.receive(1, convert(1, 1, 2, policy), Instant::now()),
            Ok(Event::Convert(_))
        ));
        let now = Instant::now();
        assert_eq!(
            runtime.publish_result(
                ReplyContext {
                    broker: id(90),
                    key: 1,
                    request: 2,
                    revision,
                    completed_at: now,
                },
                "次の結果".into(),
                &stop,
                now + Duration::from_millis(500)
            ),
            Ok(true)
        );
        assert!(matches!(
            read_frame(&mut client, &stop, deadline).message,
            message::Message::Result(result)
                if result.request == 2 && result.text == "次の結果"
        ));
    }

    #[test]
    fn disconnect_returns_only_its_pending_worker_and_keeps_other_pipe() {
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
        let stop = Stop::new().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let (first_server, mut first_client) = connected(&stop, deadline);
        let (second_server, mut second_client) = connected(&stop, deadline);
        assert_eq!(
            runtime.attach(first_server, hello(1, id(90)), &stop, deadline),
            Ok(1)
        );
        assert_eq!(
            runtime.attach(second_server, hello(2, id(90)), &stop, deadline),
            Ok(2)
        );
        read_frame(&mut first_client, &stop, deadline);
        read_frame(&mut second_client, &stop, deadline);
        let policy = runtime.policy();
        assert!(matches!(
            runtime.receive(1, convert(1, 1, 1, policy), Instant::now()),
            Ok(Event::Convert(_))
        ));
        assert_eq!(runtime.disconnect_with_pending(1), Ok(Some(1)));
        assert_eq!(
            runtime.disconnect_with_pending(1),
            Err(RuntimeError::MissingPipe)
        );
        assert_eq!(runtime.active_count(), 1);
        let revision = Revision {
            input: 1,
            settings: policy.settings,
            authorization: policy.authorization,
        };
        let now = Instant::now();
        assert_eq!(
            runtime.publish_result(
                ReplyContext {
                    broker: id(90),
                    key: 1,
                    request: 1,
                    revision,
                    completed_at: now,
                },
                "古い結果".into(),
                &stop,
                now + Duration::from_millis(500)
            ),
            Err(RuntimeError::MissingPipe)
        );
        assert!(matches!(
            runtime.receive(2, convert(2, 1, 1, policy), Instant::now()),
            Ok(Event::Convert(_))
        ));
        let now = Instant::now();
        assert_eq!(
            runtime.publish_result(
                ReplyContext {
                    broker: id(90),
                    key: 2,
                    request: 1,
                    revision,
                    completed_at: now,
                },
                "生存".into(),
                &stop,
                now + Duration::from_millis(500)
            ),
            Ok(true)
        );
        assert!(matches!(
            read_frame(&mut second_client, &stop, deadline).message,
            message::Message::Result(result) if result.text == "生存"
        ));
    }

    #[test]
    fn failed_result_write_retires_only_target_pipe() {
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
        let policy = runtime.policy();
        for (key, session) in [(1, 1), (2, 2)] {
            assert!(matches!(
                runtime.receive(key, convert(session, 1, 1, policy), Instant::now()),
                Ok(Event::Convert(_))
            ));
        }
        drop(first_client);
        let revision = Revision {
            input: 1,
            settings: policy.settings,
            authorization: policy.authorization,
        };
        let now = Instant::now();
        assert!(matches!(
            runtime.publish_result(
                ReplyContext {
                    broker: id(90),
                    key: 1,
                    request: 1,
                    revision,
                    completed_at: now,
                },
                "送信失敗".into(),
                &stop,
                now + Duration::from_millis(500)
            ),
            Err(RuntimeError::Transport(_))
        ));
        assert_eq!(runtime.active_count(), 1);
        let now = Instant::now();
        assert_eq!(
            runtime.publish_result(
                ReplyContext {
                    broker: id(90),
                    key: 2,
                    request: 1,
                    revision,
                    completed_at: now,
                },
                "生存".into(),
                &stop,
                now + Duration::from_millis(500)
            ),
            Ok(true)
        );
        assert!(matches!(
            read_frame(&mut second_client, &stop, deadline).message,
            message::Message::Result(result) if result.text == "生存"
        ));
    }

    #[test]
    fn overlong_result_write_budget_does_not_consume_request() {
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
        let stop = Stop::new().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let (server, mut client) = connected(&stop, deadline);
        runtime
            .attach(server, hello(1, id(90)), &stop, deadline)
            .unwrap();
        read_frame(&mut client, &stop, deadline);
        let policy = runtime.policy();
        let Event::Convert(convert) = runtime
            .receive(1, convert(1, 1, 1, policy), Instant::now())
            .unwrap()
        else {
            panic!("expected convert");
        };
        let now = Instant::now();
        let reply = ReplyContext {
            broker: id(90),
            key: 1,
            request: convert.request,
            revision: convert.revision,
            completed_at: now,
        };
        assert_eq!(
            runtime.publish_result(
                reply,
                "too long".into(),
                &stop,
                now + Duration::from_secs(1)
            ),
            Err(RuntimeError::Transport(IpcError::InvalidArgument))
        );
        assert_eq!(runtime.active_count(), 1);
        let now = Instant::now();
        assert_eq!(
            runtime.publish_result(
                ReplyContext {
                    completed_at: now,
                    ..reply
                },
                "成功".into(),
                &stop,
                now + Duration::from_millis(500)
            ),
            Ok(true)
        );
        assert!(matches!(
            read_frame(&mut client, &stop, deadline).message,
            message::Message::Result(result) if result.text == "成功"
        ));
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
        let disconnected_job = runtime.job_ticket(1, 1).unwrap();
        assert!(runtime.disconnect(1));
        assert!(disconnected_job.is_cancelled());
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
        let shutdown_job = runtime.job_ticket(4, 2).unwrap();
        drop(runtime);
        assert!(shutdown_job.is_cancelled());
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
        let first_job = runtime.job_ticket(1, 1).unwrap();
        let second_job = runtime.job_ticket(2, 1).unwrap();
        let first_stop = first_job.stop();
        let second_stop = second_job.stop();
        assert!(!first_stop.is_cancelled().unwrap());
        assert!(!second_stop.is_cancelled().unwrap());
        let waiting_worker = {
            let stop = second_stop.clone();
            std::thread::spawn(move || stop.wait_until(Instant::now() + Duration::from_secs(2)))
        };
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
        assert!(!first_job.is_cancelled());
        assert!(second_job.is_cancelled());
        assert!(!first_stop.is_cancelled().unwrap());
        assert!(second_stop.is_cancelled().unwrap());
        assert!(waiting_worker.join().unwrap().unwrap());
        let transport = CountingTransport(Cell::new(0));
        assert_eq!(
            manual::convert_ordered(
                ManualRequest {
                    revision: Revision {
                        input: 1,
                        settings: policy.settings,
                        authorization: policy.authorization,
                    },
                    destination: &destination,
                    source: "nihongo",
                    deadline: second_job.deadline(),
                },
                &source,
                &second_job,
                &transport,
            ),
            Err(SendError::PolicyDenied)
        );
        assert_eq!(transport.0.get(), 0);
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
        assert!(first_job.is_cancelled());
        assert!(first_stop.is_cancelled().unwrap());
        assert!(runtime.job_ticket(1, 1).is_none());
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

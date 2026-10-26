//! Bounded EXE-side worker handoff. Workers own text and a cancellation ticket;
//! only the Broker owner may touch policy fanout or authenticated pipes.
use crate::broker_runtime::{
    BrokerPolicyRuntime, JobTicket, PolicyAcknowledgment, PollStep, PublicationOutcome,
    ReplyContext, RuntimeError,
};
use crate::manual::{
    self, ManualRequest, OrderedConversionTransport, PolicySource, SendError, TransportError,
};
use crate::policy::ChangeError;
use crate::policy::{PolicyAuthority, PolicyVersion};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use sumibi_ipc::{Error as IpcError, Pipe, Stop};
use sumibi_protocol::broker_session::Event;
use sumibi_protocol::message::{Convert, ErrorCode, Id};
use sumibi_protocol::policy_fanout::MAX_SESSIONS;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkerPoolError {
    Full,
    Duplicate,
    Cancelled,
    Expired,
    Spawn,
}

pub struct WorkerCompletion {
    pub reply: ReplyContext,
    pub outcome: Result<String, ErrorCode>,
}

impl WorkerCompletion {
    /// Route either successful text or a fixed failure through the same
    /// Broker policy, identity, expiry and bounded pipe-write checks.
    pub fn publish(
        self,
        runtime: &mut BrokerPolicyRuntime<'_>,
        stop: &Stop,
        deadline: Instant,
    ) -> Result<bool, RuntimeError> {
        match self.outcome {
            Ok(text) => runtime.publish_result(self.reply, text, stop, deadline),
            Err(code) => runtime.publish_error(self.reply, code, stop, deadline),
        }
    }
}

struct Slot {
    identity: (Id, u64, u64),
    ticket: JobTicket,
    thread: JoinHandle<()>,
}

/// At most eight workers and eight queued completions. A cancelled worker
/// retains its slot until it exits; its completion is discarded. The work
/// closure must obey its finite deadline and the ticket's Stop event.
pub struct BrokerWorkers {
    send: SyncSender<WorkerCompletion>,
    receive: Receiver<WorkerCompletion>,
    slots: Vec<Slot>,
}

/// One owner-thread step. The owner interleaves this with authenticated OS
/// accept; only this thread may read the policy source or touch a pipe.
/// Every completed publication uses `Published`, including one observed by
/// `poll_once` after this step's initial source check.
pub enum OwnerStep {
    Published(PublicationOutcome),
    Completed {
        key: u64,
        request: u64,
        delivered: bool,
        succeeded: bool,
    },
    Submitted {
        key: u64,
        request: u64,
    },
    Rejected {
        key: u64,
        request: u64,
        cause: WorkerPoolError,
        delivered: bool,
    },
    Broker(PollStep),
}

/// Serialize source publication, worker completion and the next pipe frame.
/// A failed worker admission consumes the accepted request with a fixed error
/// or closes its connection; no request is left waiting for a worker that was
/// never started. The work factory receives text only after policy admission.
pub struct BrokerWorkLoop {
    workers: BrokerWorkers,
}

impl Default for BrokerWorkLoop {
    fn default() -> Self {
        Self::new()
    }
}

impl BrokerWorkLoop {
    pub fn new() -> Self {
        Self {
            workers: BrokerWorkers::new(),
        }
    }

    pub fn active_count(&self) -> usize {
        self.workers.active_count()
    }

    pub fn stop_and_reap(&mut self, deadline: Instant) -> bool {
        self.workers.stop_and_reap(deadline)
    }

    /// Settings completion includes transport publication and the exit of
    /// workers cancelled by that publication. A previously completed policy
    /// version may still have cancelled workers awaiting bounded I/O exit.
    pub fn acknowledge_version(
        &mut self,
        runtime: &mut BrokerPolicyRuntime<'_>,
        requested: PolicyVersion,
        stop: &Stop,
        deadline: Instant,
    ) -> Result<PolicyAcknowledgment, RuntimeError> {
        let acknowledgment = runtime.acknowledge_version(requested, stop, deadline)?;
        if !self.workers.reap_cancelled(deadline) {
            return Err(RuntimeError::Transport(IpcError::Timeout));
        }
        Ok(acknowledgment)
    }

    /// The caller has already authenticated this OS pipe. Bring older
    /// subscribers up to date before reserving its never-reused key. The
    /// returned receipt belongs to that publication, not to the new pipe.
    pub fn accept(
        &mut self,
        runtime: &mut BrokerPolicyRuntime<'_>,
        pipe: Pipe,
        stop: &Stop,
        deadline: Instant,
    ) -> Result<(u64, Option<PublicationOutcome>), RuntimeError> {
        let published = runtime.publish_current(stop, deadline)?;
        let key = runtime.reserve(pipe)?;
        Ok((key, published))
    }

    /// Product-facing manual conversion dispatch. The runtime and worker
    /// must share the same Host authority; otherwise equal version numbers
    /// from unrelated authorities could authorize the wrong destination.
    /// The worker snapshots its destination immediately before the ordered
    /// send, which rechecks consent, credential identity and cancellation.
    pub fn advance_manual<T>(
        &mut self,
        runtime: &mut BrokerPolicyRuntime<'_>,
        authority: Arc<PolicyAuthority>,
        transport: Arc<T>,
        stop: &Stop,
        deadline: Instant,
    ) -> Result<OwnerStep, RuntimeError>
    where
        T: OrderedConversionTransport + Send + Sync + 'static,
    {
        if !runtime.uses_source(&authority) {
            return Err(RuntimeError::InvalidTarget);
        }
        self.advance(runtime, stop, deadline, move |request| {
            move |ticket: JobTicket| {
                let destination = authority
                    .snapshot()
                    .ok_or(ErrorCode::Unavailable)?
                    .destination;
                manual::convert_ordered(
                    ManualRequest {
                        revision: request.revision,
                        destination: &destination,
                        source: &request.text,
                        deadline: ticket.deadline(),
                    },
                    &authority,
                    &ticket,
                    &*transport,
                )
                .map_err(classify_send_error)
            }
        })
    }

    pub fn advance<F, W>(
        &mut self,
        runtime: &mut BrokerPolicyRuntime<'_>,
        stop: &Stop,
        deadline: Instant,
        make_work: F,
    ) -> Result<OwnerStep, RuntimeError>
    where
        F: FnOnce(Convert) -> W,
        W: FnOnce(JobTicket) -> Result<String, ErrorCode> + Send + 'static,
    {
        if let Some(outcome) = runtime.publish_current(stop, deadline)? {
            return Ok(OwnerStep::Published(outcome));
        }
        if let Some(completion) = self.workers.try_completion() {
            let key = completion.reply.key;
            let request = completion.reply.request;
            let succeeded = completion.outcome.is_ok();
            let now = Instant::now();
            let write_deadline = deadline.min(now + Duration::from_millis(500));
            if write_deadline <= now {
                runtime.disconnect(key);
                return Err(RuntimeError::Transport(IpcError::Timeout));
            }
            return match completion.publish(runtime, stop, write_deadline) {
                Ok(delivered) => Ok(OwnerStep::Completed {
                    key,
                    request,
                    delivered,
                    succeeded,
                }),
                Err(RuntimeError::Source(ChangeError::StalePolicy)) => {
                    let outcome = runtime
                        .publish_current(stop, deadline)?
                        .ok_or(RuntimeError::Incomplete)?;
                    Ok(OwnerStep::Published(outcome))
                }
                Err(error) => Err(error),
            };
        }
        let step = runtime.poll_once(stop, deadline)?;
        if let PollStep::Published(outcome) = step {
            return Ok(OwnerStep::Published(outcome));
        }
        let PollStep::Received {
            key,
            event: Event::Convert(convert),
        } = step
        else {
            return Ok(OwnerStep::Broker(step));
        };
        let request = convert.request;
        let ticket = runtime
            .job_ticket(key, request)
            .ok_or(RuntimeError::Incomplete)?;
        match self.workers.submit(ticket.clone(), make_work(convert)) {
            Ok(()) => Ok(OwnerStep::Submitted { key, request }),
            Err(WorkerPoolError::Duplicate) => {
                runtime.disconnect(key);
                Err(RuntimeError::Incomplete)
            }
            Err(cause) => {
                let code = match cause {
                    WorkerPoolError::Cancelled => ErrorCode::Cancelled,
                    WorkerPoolError::Expired => ErrorCode::Timeout,
                    WorkerPoolError::Full | WorkerPoolError::Spawn => ErrorCode::Unavailable,
                    WorkerPoolError::Duplicate => unreachable!(),
                };
                let now = Instant::now();
                let write_deadline = deadline.min(now + Duration::from_millis(500));
                if write_deadline <= now {
                    runtime.disconnect(key);
                    return Err(RuntimeError::Transport(IpcError::Timeout));
                }
                match runtime.publish_error(ticket.reply_context(now), code, stop, write_deadline) {
                    Ok(delivered) => Ok(OwnerStep::Rejected {
                        key,
                        request,
                        cause,
                        delivered,
                    }),
                    Err(RuntimeError::Source(ChangeError::StalePolicy)) => {
                        let outcome = runtime
                            .publish_current(stop, deadline)?
                            .ok_or(RuntimeError::Incomplete)?;
                        Ok(OwnerStep::Published(outcome))
                    }
                    Err(error) => Err(error),
                }
            }
        }
    }
}

fn classify_send_error(error: SendError) -> ErrorCode {
    match error {
        SendError::PolicyDenied | SendError::Transport(TransportError::Cancelled) => {
            ErrorCode::Cancelled
        }
        SendError::Timeout | SendError::Transport(TransportError::Timeout) => ErrorCode::Timeout,
        SendError::InvalidRequest | SendError::Transport(TransportError::InvalidRequest) => {
            ErrorCode::Rejected
        }
        SendError::InvalidResponse | SendError::Transport(_) => ErrorCode::Unavailable,
    }
}

impl Default for BrokerWorkers {
    fn default() -> Self {
        Self::new()
    }
}

impl BrokerWorkers {
    pub fn new() -> Self {
        let (send, receive) = mpsc::sync_channel(MAX_SESSIONS);
        Self {
            send,
            receive,
            slots: Vec::new(),
        }
    }

    pub fn active_count(&self) -> usize {
        self.slots.len()
    }

    /// The owner submits only a ticket returned by BrokerPolicyRuntime after
    /// accepting Convert. Work never borrows the runtime or its pipe.
    pub fn submit(
        &mut self,
        ticket: JobTicket,
        work: impl FnOnce(JobTicket) -> Result<String, ErrorCode> + Send + 'static,
    ) -> Result<(), WorkerPoolError> {
        if ticket.is_cancelled() {
            return Err(WorkerPoolError::Cancelled);
        }
        if Instant::now() >= ticket.deadline() {
            return Err(WorkerPoolError::Expired);
        }
        let identity = ticket.identity();
        if self.slots.iter().any(|slot| slot.identity == identity) {
            return Err(WorkerPoolError::Duplicate);
        }
        if self.slots.len() == MAX_SESSIONS {
            return Err(WorkerPoolError::Full);
        }
        let send = self.send.clone();
        let worker_ticket = ticket.clone();
        let thread = thread::Builder::new()
            .name("sumibi-broker-job".into())
            .spawn(move || {
                let outcome = catch_unwind(AssertUnwindSafe(|| work(worker_ticket.clone())))
                    .unwrap_or(Err(ErrorCode::Unavailable));
                let reply = worker_ticket.reply_context(Instant::now());
                let _ = send.send(WorkerCompletion { reply, outcome });
            })
            .map_err(|_| WorkerPoolError::Spawn)?;
        self.slots.push(Slot {
            identity,
            ticket,
            thread,
        });
        Ok(())
    }

    /// Nonblocking owner poll. Reap a finished thread before releasing its
    /// slot. The Broker runtime still rechecks policy and request identity
    /// before publishing any returned result.
    pub fn try_completion(&mut self) -> Option<WorkerCompletion> {
        loop {
            let completion = match self.receive.try_recv() {
                Ok(completion) => completion,
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => return None,
            };
            let identity = (
                completion.reply.broker,
                completion.reply.key,
                completion.reply.request,
            );
            let Some(index) = self.slots.iter().position(|slot| slot.identity == identity) else {
                // A cancelled thread may have been joined while its already
                // queued completion was still waiting in the bounded channel.
                continue;
            };
            let slot = self.slots.remove(index);
            let _ = slot.thread.join();
            if !slot.ticket.is_cancelled() {
                return Some(completion);
            }
        }
    }

    /// Reap only tickets the runtime has already cancelled. A live worker's
    /// completion remains queued for the next owner step. At most eight jobs
    /// can send into the eight-slot channel, so an exit cannot block on send.
    pub fn reap_cancelled(&mut self, deadline: Instant) -> bool {
        loop {
            let mut index = 0;
            while index < self.slots.len() {
                if self.slots[index].ticket.is_cancelled() && self.slots[index].thread.is_finished()
                {
                    let slot = self.slots.remove(index);
                    let _ = slot.thread.join();
                } else {
                    index += 1;
                }
            }
            if self.slots.iter().all(|slot| !slot.ticket.is_cancelled()) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(Duration::from_millis(1));
        }
    }

    /// Signal all remaining jobs, then wait only until the owner's finite
    /// shutdown deadline. Workers must observe their Stop event or deadline.
    /// False reports that some jobs have not exited; dropping the pool then
    /// detaches those threads rather than blocking the Broker indefinitely.
    pub fn stop_and_reap(&mut self, deadline: Instant) -> bool {
        for slot in &self.slots {
            slot.ticket.cancel();
        }
        while !self.slots.is_empty() && Instant::now() < deadline {
            let _ = self.try_completion();
            if !self.slots.is_empty() {
                thread::sleep(Duration::from_millis(1));
            }
        }
        self.slots.is_empty()
    }
}

impl Drop for BrokerWorkers {
    fn drop(&mut self) {
        for slot in &self.slots {
            slot.ticket.cancel();
        }
    }
}

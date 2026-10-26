//! Bounded EXE-side worker handoff. Workers own text and a cancellation ticket;
//! only the Broker owner may touch policy fanout or authenticated pipes.
use crate::broker_runtime::{BrokerPolicyRuntime, JobTicket, ReplyContext, RuntimeError};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use sumibi_ipc::Stop;
use sumibi_protocol::message::{ErrorCode, Id};
use sumibi_protocol::policy_fanout::MAX_SESSIONS;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkerPoolError {
    Full,
    Duplicate,
    Cancelled,
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
            let index = self
                .slots
                .iter()
                .position(|slot| slot.identity == identity)?;
            let slot = self.slots.remove(index);
            let _ = slot.thread.join();
            if !slot.ticket.is_cancelled() {
                return Some(completion);
            }
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

//! One owner-thread lifetime for an authenticated shared Broker listener.
//! The caller still supplies each peer pinned by a trusted launch/discovery
//! path; a pipe name or an advertised PID is never sufficient.

use crate::broker_runtime::{BrokerPolicyRuntime, JobTicket, PolicyAcknowledgment, RuntimeError};
use crate::broker_workers::{BrokerWorkLoop, OwnerStep};
use crate::manual::OrderedConversionTransport;
use crate::policy::{PolicyAuthority, PolicyVersion};
use std::sync::Arc;
use std::time::{Duration, Instant};
use sumibi_ipc::{AcceptTask, Error as IpcError, Nonce, PinnedProcess, SharedAnchor, Stop};
use sumibi_protocol::message::{Convert, ErrorCode, Id, Policy};

/// Owns the first pipe instance through idle periods and retires every
/// accepted connection, pending accept and worker before a new boot.
/// Field order drops the anchor last; use `shutdown` or `rotate` to confirm
/// worker and accept threads have exited before releasing the pipe name.
pub struct BrokerBoot<'a> {
    runtime: BrokerPolicyRuntime<'a>,
    owner: BrokerWorkLoop,
    accepts: Vec<AcceptTask>,
    anchor: SharedAnchor,
    name: Nonce,
    max_instances: u32,
    source: &'a PolicyAuthority,
}

/// An elapsed or overlong deadline returns the running boot without retirement.
/// A timed-out retirement returns the retiring boot with its resources still
/// owned for a bounded retry. Its authenticated connections are already closed.
/// A new-boot failure occurs only after the old boot has fully retired.
pub enum RotateError<'a> {
    Expired(Box<BrokerBoot<'a>>),
    InvalidDeadline(Box<BrokerBoot<'a>>),
    Pending(Box<BrokerBoot<'a>>),
    Start(RuntimeError),
}

/// An elapsed or overlong deadline leaves the boot running. A pending retirement has
/// already closed authenticated connections but still owns outstanding work.
pub enum ShutdownError<'a> {
    Expired(Box<BrokerBoot<'a>>),
    InvalidDeadline(Box<BrokerBoot<'a>>),
    Pending(Box<BrokerBoot<'a>>),
}

impl std::fmt::Debug for ShutdownError<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Expired(_) => formatter.write_str("ShutdownError::Expired"),
            Self::InvalidDeadline(_) => formatter.write_str("ShutdownError::InvalidDeadline"),
            Self::Pending(_) => formatter.write_str("ShutdownError::Pending"),
        }
    }
}

impl std::fmt::Debug for RotateError<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Expired(_) => formatter.write_str("RotateError::Expired"),
            Self::InvalidDeadline(_) => formatter.write_str("RotateError::InvalidDeadline"),
            Self::Pending(_) => formatter.write_str("RotateError::Pending"),
            Self::Start(error) => formatter
                .debug_tuple("RotateError::Start")
                .field(error)
                .finish(),
        }
    }
}

impl<'a> BrokerBoot<'a> {
    /// Create a fresh, unadvertised name and boot ID for one Broker lifetime.
    /// The caller advertises `name()` only to a process it has independently
    /// pinned and authenticated.
    pub fn start(
        source: &'a PolicyAuthority,
        max_instances: u32,
        deadline: Instant,
    ) -> Result<Self, RuntimeError> {
        if let Some(error) = deadline_error(deadline) {
            return Err(RuntimeError::Transport(error));
        }
        let name = Nonce::random().map_err(RuntimeError::Transport)?;
        let anchor = SharedAnchor::create(&name, max_instances, deadline)
            .map_err(RuntimeError::Transport)?;
        let runtime = BrokerPolicyRuntime::new(source, name.id())?;
        Ok(Self {
            runtime,
            owner: BrokerWorkLoop::new(),
            accepts: Vec::new(),
            anchor,
            name,
            max_instances,
            source,
        })
    }

    pub fn name(&self) -> &Nonce {
        &self.name
    }

    pub fn boot_id(&self) -> Id {
        self.name.id()
    }

    pub fn policy(&self) -> Policy {
        self.runtime.policy()
    }

    pub fn active_count(&self) -> usize {
        self.runtime.active_count()
    }

    /// Exhaustion stops new accepts; the owner can then retire this boot and
    /// create a fresh random name without interrupting an existing result.
    pub fn remaining_attachment_budget(&self) -> usize {
        self.runtime.remaining_attachment_budget()
    }

    pub fn pending_accept_count(&self) -> usize {
        self.accepts.len()
    }

    pub fn active_worker_count(&self) -> usize {
        self.owner.active_count()
    }

    /// Let the Host decide whether to pin another trusted process before
    /// allocating an OS pipe instance. Capacity can recover when a pending
    /// accept fails or an authenticated connection closes; Exhausted requires
    /// a new Broker boot after existing work has retired.
    pub fn check_accept_capacity(&self) -> Result<(), RuntimeError> {
        self.runtime
            .check_anchor_accept_capacity(self.accepts.len(), self.max_instances)
    }

    /// The owner supplies a trusted process pin before an OS instance exists.
    /// A rejected/expired accept can be replaced from the same anchor.
    pub fn begin_accept(
        &mut self,
        peer: PinnedProcess,
        deadline: Instant,
    ) -> Result<(), RuntimeError> {
        self.check_accept_capacity()?;
        self.accepts.push(
            self.runtime
                .begin_anchor_accept(&self.anchor, peer, deadline)?,
        );
        Ok(())
    }

    /// Advance one bounded accept, policy, worker or authenticated frame step.
    pub fn advance<F, W>(
        &mut self,
        stop: &Stop,
        deadline: Instant,
        make_work: F,
    ) -> Result<OwnerStep, RuntimeError>
    where
        F: FnOnce(Convert) -> W,
        W: FnOnce(JobTicket) -> Result<String, ErrorCode> + Send + 'static,
    {
        self.owner.advance_with_accept(
            &mut self.runtime,
            &mut self.accepts,
            stop,
            deadline,
            make_work,
        )
    }

    /// Product-facing manual dispatch with the same authority as this boot.
    pub fn advance_manual<T>(
        &mut self,
        authority: Arc<PolicyAuthority>,
        transport: Arc<T>,
        stop: &Stop,
        deadline: Instant,
    ) -> Result<OwnerStep, RuntimeError>
    where
        T: OrderedConversionTransport + Send + Sync + 'static,
    {
        self.owner.advance_manual_with_accept(
            &mut self.runtime,
            &mut self.accepts,
            authority,
            transport,
            stop,
            deadline,
        )
    }

    /// Complete a settings receipt only after cancelled workers have exited.
    pub fn acknowledge_version(
        &mut self,
        requested: PolicyVersion,
        stop: &Stop,
        deadline: Instant,
    ) -> Result<PolicyAcknowledgment, RuntimeError> {
        self.owner
            .acknowledge_version(&mut self.runtime, requested, stop, deadline)
    }

    /// Low-level retirement step for callers already holding this boot. Prefer
    /// `shutdown` or `rotate`, which validate the deadline and retain ownership
    /// on failure. False means a worker or OS accept still owns resources.
    #[must_use = "keep this Broker boot alive and retry if retirement is incomplete"]
    pub fn retire_for_restart(&mut self, deadline: Instant) -> bool {
        self.owner
            .retire_for_restart(&mut self.runtime, &mut self.accepts, deadline)
    }

    /// Release this boot only after all authenticated pipes, pending accepts
    /// and workers have retired. On failure the caller keeps ownership and
    /// retries with a deadline within 60 seconds. No pipe name is reused here.
    #[must_use = "retry shutdown with the returned BrokerBoot"]
    pub fn shutdown(mut self, deadline: Instant) -> Result<(), ShutdownError<'a>> {
        match deadline_error(deadline) {
            Some(IpcError::Timeout) => return Err(ShutdownError::Expired(Box::new(self))),
            Some(_) => return Err(ShutdownError::InvalidDeadline(Box::new(self))),
            None => {}
        }
        if !self.retire_for_restart(deadline) || Instant::now() >= deadline {
            return Err(ShutdownError::Pending(Box::new(self)));
        }
        Ok(())
    }

    /// Reap the old boot, release its first pipe instance, then create a new
    /// random name and ID under the same authority. The one deadline covers
    /// both phases. Expired and InvalidDeadline return a running boot. Pending
    /// returns the retiring boot, whose old connections cannot resume. Start
    /// means the old boot is gone.
    #[must_use = "retry a pending rotation with its returned BrokerBoot"]
    pub fn rotate(self, deadline: Instant) -> Result<Self, RotateError<'a>> {
        let source = self.source;
        let max_instances = self.max_instances;
        self.shutdown(deadline).map_err(|error| match error {
            ShutdownError::Expired(boot) => RotateError::Expired(boot),
            ShutdownError::InvalidDeadline(boot) => RotateError::InvalidDeadline(boot),
            ShutdownError::Pending(boot) => RotateError::Pending(boot),
        })?;
        Self::start(source, max_instances, deadline).map_err(RotateError::Start)
    }
}

fn deadline_error(deadline: Instant) -> Option<IpcError> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        Some(IpcError::Timeout)
    } else if remaining > Duration::from_secs(60) {
        Some(IpcError::InvalidArgument)
    } else {
        None
    }
}

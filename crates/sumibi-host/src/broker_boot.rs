//! One owner-thread lifetime for an authenticated shared Broker listener.
//! The caller still supplies each peer pinned by a trusted launch/discovery
//! path; a pipe name or an advertised PID is never sufficient.

use crate::broker_runtime::{BrokerPolicyRuntime, JobTicket, PolicyAcknowledgment, RuntimeError};
use crate::broker_workers::{BrokerWorkLoop, OwnerStep};
use crate::manual::OrderedConversionTransport;
use crate::policy::{PolicyAuthority, PolicyVersion};
use std::sync::Arc;
use std::time::Instant;
use sumibi_ipc::{AcceptTask, Nonce, PinnedProcess, SharedAnchor, Stop};
use sumibi_protocol::message::{Convert, ErrorCode, Id, Policy};

/// Owns the first pipe instance through idle periods and retires every
/// accepted connection, pending accept and worker before a new boot.
/// Field order keeps the anchor alive until the other resources are dropped.
pub struct BrokerBoot<'a> {
    runtime: BrokerPolicyRuntime<'a>,
    owner: BrokerWorkLoop,
    accepts: Vec<AcceptTask>,
    anchor: SharedAnchor,
    name: Nonce,
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

    /// The owner supplies a trusted process pin before an OS instance exists.
    /// A rejected/expired accept can be replaced from the same anchor.
    pub fn begin_accept(
        &mut self,
        peer: PinnedProcess,
        deadline: Instant,
    ) -> Result<(), RuntimeError> {
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

    /// Call before dropping this boot and starting another. False means a
    /// worker or OS accept still owns resources; retain this value and retry.
    #[must_use = "keep this Broker boot alive and retry if retirement is incomplete"]
    pub fn retire_for_restart(&mut self, deadline: Instant) -> bool {
        self.owner
            .retire_for_restart(&mut self.runtime, &mut self.accepts, deadline)
    }
}

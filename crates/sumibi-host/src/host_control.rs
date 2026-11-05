//! Explicit Host lifetime for a responsive consent window. Peer discovery and
//! autostart integration remain separate: the boot is unadvertised.
use crate::broker_boot::{AcceptId, BrokerBoot};
use crate::broker_runtime::RuntimeError;
use crate::consent_task::{ConsentOffer, ConsentTask, ConsentTaskError, ConsentUpdate};
use crate::policy::{ChangeError, PolicyAuthority};
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, SyncSender},
    Arc, Mutex, TryLockError,
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use sumibi_ipc::{Nonce, PinnedProcess, Stop};

const STEP: Duration = Duration::from_millis(25);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Starting,
    Ready,
    Updating,
    Stopping,
    Stopped,
    Failed,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlError {
    Startup,
    Broker,
    Busy,
    Closed,
    ForeignOffer,
    StaleOffer,
    Consent,
    WorkerFailed,
    Unavailable,
    AlreadyRunning,
}

#[derive(Clone)]
pub struct HostStatus {
    pub phase: Phase,
    pub offer: Option<ConsentOffer>,
    pub credential_error: bool,
    pub error: Option<ControlError>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DispatchMode {
    NativeChat,
    #[cfg(any(test, feature = "spike"))]
    DiagnosticChat,
}

enum Command {
    Grant(ConsentOffer),
    Revoke,
}

#[derive(Clone)]
pub struct PrivateAccept {
    pub id: AcceptId,
    pub name: Nonce,
    pub broker: Nonce,
}

enum PeerRequest {
    Shared {
        peer: PinnedProcess,
        reply: SyncSender<Result<AcceptId, ControlError>>,
    },
    Private {
        peer: PinnedProcess,
        reply: SyncSender<Result<PrivateAccept, ControlError>>,
    },
}

/// Commands, status capture and completion polling never wait on native I/O.
/// One consent command is admitted at a time. Closing takes precedence over
/// queued actions and cannot be blocked by a full command queue.
#[must_use = "request close and poll completion before ending the UI"]
pub struct HostControl {
    commands: SyncSender<Command>,
    peer_requests: SyncSender<PeerRequest>,
    closing: Arc<AtomicBool>,
    active: Arc<AtomicBool>,
    status: Arc<Mutex<HostStatus>>,
    worker: Mutex<Option<JoinHandle<Result<(), ControlError>>>>,
}
impl HostControl {
    pub fn start(path: PathBuf) -> Result<Self, ControlError> {
        Self::start_scoped(
            move || {
                let host =
                    crate::startup::PreparedHost::load(&path).map_err(|_| ControlError::Startup)?;
                Ok((host.authority().clone(), host.credential_outcome().is_err()))
            },
            || {
                sumibi_ipc::host_instance::HostInstance::try_acquire()
                    .map_err(|_| ControlError::Unavailable)?
                    .ok_or(ControlError::AlreadyRunning)
            },
            DispatchMode::NativeChat,
        )
    }
    #[cfg(test)]
    pub(crate) fn start_with(
        load: impl FnOnce() -> Result<(Arc<PolicyAuthority>, bool), ControlError> + Send + 'static,
    ) -> Result<Self, ControlError> {
        // Lifecycle tests use an isolated lease. The native fixed-name lease
        // is tested with private random names in sumibi-ipc, so test runs never
        // contend with a user's actual running Host.
        Self::start_scoped(load, || Ok(()), DispatchMode::NativeChat)
    }
    #[cfg(any(test, feature = "spike"))]
    pub fn start_diagnostic(
        load: impl FnOnce() -> Result<(Arc<PolicyAuthority>, bool), ControlError> + Send + 'static,
    ) -> Result<Self, ControlError> {
        Self::start_scoped(load, || Ok(()), DispatchMode::DiagnosticChat)
    }
    fn start_scoped<L: 'static>(
        load: impl FnOnce() -> Result<(Arc<PolicyAuthority>, bool), ControlError> + Send + 'static,
        acquire: impl FnOnce() -> Result<L, ControlError> + Send + 'static,
        mode: DispatchMode,
    ) -> Result<Self, ControlError> {
        let (send, receive) = mpsc::sync_channel(1);
        let (peer_send, peer_receive) = mpsc::sync_channel(8);
        let closing = Arc::new(AtomicBool::new(false));
        let active = Arc::new(AtomicBool::new(false));
        let status = Arc::new(Mutex::new(HostStatus {
            phase: Phase::Starting,
            offer: None,
            credential_error: false,
            error: None,
        }));
        let worker_status = status.clone();
        let worker_closing = closing.clone();
        let worker_active = active.clone();
        let worker = thread::Builder::new()
            .name("sumibi-host-owner".into())
            .spawn(move || {
                let result = match acquire() {
                    Ok(_lease) => run(
                        load,
                        receive,
                        peer_receive,
                        &worker_closing,
                        &worker_active,
                        &worker_status,
                        mode,
                    ),
                    Err(error) => Err(error),
                };
                publish(
                    &worker_status,
                    HostStatus {
                        phase: if result.is_ok() {
                            Phase::Stopped
                        } else {
                            Phase::Failed
                        },
                        offer: None,
                        credential_error: false,
                        error: result.err(),
                    },
                );
                worker_active.store(false, Ordering::Release);
                result
            })
            .map_err(|_| ControlError::WorkerFailed)?;
        Ok(Self {
            commands: send,
            peer_requests: peer_send,
            closing,
            active,
            status,
            worker: Mutex::new(Some(worker)),
        })
    }
    pub fn try_status(&self) -> Result<Option<HostStatus>, ControlError> {
        match self.status.try_lock() {
            Ok(status) => Ok(Some(status.clone())),
            Err(TryLockError::WouldBlock) => Ok(None),
            Err(TryLockError::Poisoned(_)) => Err(ControlError::Unavailable),
        }
    }
    pub fn try_grant(&self, displayed: ConsentOffer) -> Result<(), ControlError> {
        self.submit(Command::Grant(displayed))
    }
    pub fn try_revoke(&self) -> Result<(), ControlError> {
        self.submit(Command::Revoke)
    }
    pub fn try_accept_peer(&self, peer: PinnedProcess) -> Result<AcceptId, ControlError> {
        let (send, receive) = mpsc::sync_channel(1);
        self.submit_peer_request(PeerRequest::Shared { peer, reply: send })?;
        receive.recv().map_err(|_| ControlError::Closed)?
    }
    pub fn try_accept_private_peer(
        &self,
        peer: PinnedProcess,
    ) -> Result<PrivateAccept, ControlError> {
        let (send, receive) = mpsc::sync_channel(1);
        self.submit_peer_request(PeerRequest::Private { peer, reply: send })?;
        receive.recv().map_err(|_| ControlError::Closed)?
    }
    /// Accept one client peer over a bootstrap pipe, create a private accept in this
    /// HostControl, and hand off the private pipe coordinates to the client.
    pub fn serve_bootstrap_handoff(
        &self,
        server: sumibi_ipc::BootstrapServer,
        stop: &Stop,
        deadline: Instant,
    ) -> Result<PinnedProcess, ControlError> {
        let (pipe, client_peer) = server
            .accept(stop, deadline)
            .map_err(|_| ControlError::Broker)?;
        let accept = self
            .try_accept_private_peer(client_peer.try_clone().map_err(|_| ControlError::Broker)?)?;
        sumibi_ipc::handoff::send(
            pipe,
            &client_peer,
            &accept.name,
            &accept.broker,
            stop,
            deadline,
        )
        .map_err(|_| ControlError::Broker)?;
        Ok(client_peer)
    }
    pub fn is_updating(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }
    fn submit_peer_request(&self, request: PeerRequest) -> Result<(), ControlError> {
        let worker_guard = self.worker.lock().unwrap();
        if self.closing.load(Ordering::Acquire)
            || worker_guard
                .as_ref()
                .is_none_or(|worker| worker.is_finished())
        {
            return Err(ControlError::Closed);
        }
        self.peer_requests
            .try_send(request)
            .map_err(|error| match error {
                mpsc::TrySendError::Full(_) => ControlError::Busy,
                mpsc::TrySendError::Disconnected(_) => ControlError::Closed,
            })?;
        if let Some(worker) = worker_guard.as_ref() {
            worker.thread().unpark();
        }
        Ok(())
    }
    fn submit(&self, command: Command) -> Result<(), ControlError> {
        let worker_guard = self.worker.lock().unwrap();
        if self.closing.load(Ordering::Acquire)
            || worker_guard
                .as_ref()
                .is_none_or(|worker| worker.is_finished())
        {
            return Err(ControlError::Closed);
        }
        if self
            .active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(ControlError::Busy);
        }
        if let Err(error) = self.commands.try_send(command) {
            self.active.store(false, Ordering::Release);
            return Err(match error {
                mpsc::TrySendError::Full(_) => ControlError::Busy,
                mpsc::TrySendError::Disconnected(_) => ControlError::Closed,
            });
        }
        if let Some(worker) = worker_guard.as_ref() {
            worker.thread().unpark();
        }
        Ok(())
    }
    pub fn request_close(&self) {
        self.closing.store(true, Ordering::Release);
        if let Some(worker) = self.worker.lock().unwrap().as_ref() {
            worker.thread().unpark();
        }
    }
    pub fn try_finish(&self) -> Option<Result<(), ControlError>> {
        let mut guard = self.worker.lock().unwrap();
        if !guard.as_ref()?.is_finished() {
            return None;
        }
        Some(
            guard
                .take()
                .unwrap()
                .join()
                .unwrap_or(Err(ControlError::WorkerFailed)),
        )
    }
    pub fn join(&self) -> Result<(), ControlError> {
        let worker = self.worker.lock().unwrap().take();
        if let Some(handle) = worker {
            handle.join().unwrap_or(Err(ControlError::WorkerFailed))
        } else {
            Ok(())
        }
    }
}
impl Drop for HostControl {
    fn drop(&mut self) {
        self.request_close();
    }
}

fn publish(status: &Mutex<HostStatus>, update: HostStatus) {
    *status.lock().unwrap_or_else(|poison| poison.into_inner()) = update;
}
fn classify(error: ConsentTaskError) -> ControlError {
    match error {
        ConsentTaskError::Busy => ControlError::Busy,
        ConsentTaskError::Policy(ChangeError::StalePolicy) => ControlError::StaleOffer,
        _ => ControlError::Consent,
    }
}

fn run(
    load: impl FnOnce() -> Result<(Arc<PolicyAuthority>, bool), ControlError>,
    commands: Receiver<Command>,
    peer_requests: Receiver<PeerRequest>,
    closing: &AtomicBool,
    active: &AtomicBool,
    status: &Mutex<HostStatus>,
    mode: DispatchMode,
) -> Result<(), ControlError> {
    let (authority, credential_error) = load()?;
    let stop = Stop::new().map_err(|_| ControlError::Broker)?;
    let mut boot = BrokerBoot::start(&authority, 64, Instant::now() + Duration::from_secs(2))
        .map_err(|_| ControlError::Broker)?;
    let mut pending: Option<ConsentTask> = None;
    let mut update: Option<ConsentUpdate> = None;
    let mut failure = None;
    let mut action_error = None;
    let mut revoking = false;
    let mut revoked = false;
    loop {
        let is_closing = closing.load(Ordering::Acquire);
        if is_closing {
            while let Ok(request) = peer_requests.try_recv() {
                match request {
                    PeerRequest::Shared { reply, .. } => {
                        let _ = reply.send(Err(ControlError::Closed));
                    }
                    PeerRequest::Private { reply, .. } => {
                        let _ = reply.send(Err(ControlError::Closed));
                    }
                }
            }
            // Retire first so even a pending grant cannot admit a new request.
            // Each retry retains the boot and its unfinished native resources.
            let retired = boot.retire_for_restart(Instant::now() + STEP);
            update = None;
            if let Some(task) = pending.as_mut() {
                if let Some(result) = task.try_take() {
                    pending = None;
                    if revoking {
                        match result {
                            Ok(_receipt) => revoked = true,
                            Err(error) => failure = Some(classify(error)),
                        }
                    }
                }
            }
            if pending.is_none() && !revoking {
                match ConsentTask::revoke(authority.clone()) {
                    Ok(task) => {
                        pending = Some(task);
                        revoking = true;
                    }
                    Err(ConsentTaskError::Busy) => {}
                    Err(error) => {
                        failure = Some(classify(error));
                        revoking = true;
                    }
                }
            }
            publish(
                status,
                HostStatus {
                    phase: Phase::Stopping,
                    offer: None,
                    credential_error,
                    error: failure,
                },
            );
            if retired && pending.is_none() && (revoked || failure.is_some()) {
                return failure.map_or(Ok(()), Err);
            }
        } else {
            while let Ok(request) = peer_requests.try_recv() {
                let deadline = Instant::now() + Duration::from_secs(10);
                match request {
                    PeerRequest::Shared { peer, reply } => {
                        let result = boot
                            .begin_tracked_accept(peer, deadline)
                            .map_err(|_| ControlError::Broker);
                        let _ = reply.send(result);
                    }
                    PeerRequest::Private { peer, reply } => {
                        let result = boot
                            .begin_private_accept(peer, deadline)
                            .map(|(id, name)| PrivateAccept {
                                id,
                                name,
                                broker: boot.name().clone(),
                            })
                            .map_err(|_| ControlError::Broker);
                        let _ = reply.send(result);
                    }
                }
            }
            if pending.is_none() && update.is_none() {
                match commands.try_recv() {
                    Ok(command) => {
                        action_error = None;
                        let started = match command {
                            Command::Grant(offer) if !offer.belongs_to(&authority) => {
                                Err(ControlError::ForeignOffer)
                            }
                            Command::Grant(offer) => ConsentTask::grant(offer).map_err(classify),
                            Command::Revoke => {
                                ConsentTask::revoke(authority.clone()).map_err(classify)
                            }
                        };
                        match started {
                            Ok(task) => pending = Some(task),
                            Err(error) => {
                                action_error = Some(error);
                                active.store(false, Ordering::Release);
                            }
                        }
                    }
                    Err(mpsc::TryRecvError::Disconnected) => {
                        closing.store(true, Ordering::Release);
                        continue;
                    }
                    Err(mpsc::TryRecvError::Empty) => {}
                }
            }
            if let Some(task) = pending.as_mut() {
                if let Some(result) = task.try_take() {
                    pending = None;
                    match result {
                        Ok(receipt) => update = Some(receipt),
                        Err(error) => {
                            action_error = Some(classify(error));
                            active.store(false, Ordering::Release);
                        }
                    }
                }
            }
            if let Some(receipt) = update.as_ref() {
                match boot.acknowledge_consent_update(receipt, &stop, Instant::now() + STEP) {
                    Ok(_) => {
                        update = None;
                        active.store(false, Ordering::Release);
                    }
                    Err(RuntimeError::Transport(sumibi_ipc::Error::Timeout)) => {}
                    Err(_) => {
                        failure = Some(ControlError::Broker);
                        closing.store(true, Ordering::Release);
                        continue;
                    }
                }
            }
            let busy = active.load(Ordering::Acquire);
            // Advance broker peer connections and conversion dispatch only when
            // not transitioning policy or waiting behind a send guard.
            if pending.is_none() && update.is_none() && !busy {
                let step_deadline = Instant::now() + Duration::from_millis(5);
                let dispatch_result = match mode {
                    DispatchMode::NativeChat => {
                        boot.advance_native_chat(authority.clone(), &stop, step_deadline)
                    }
                    #[cfg(any(test, feature = "spike"))]
                    DispatchMode::DiagnosticChat => {
                        boot.advance_diagnostic_chat(authority.clone(), &stop, step_deadline)
                    }
                };
                if let Err(error) = dispatch_result {
                    if !matches!(error, RuntimeError::Transport(sumibi_ipc::Error::Timeout)) {
                        // Non-timeout errors in broker dispatch: unadmitted or invalid state
                    }
                }
            }
            let offer = if busy {
                None
            } else {
                match ConsentOffer::try_capture(authority.clone()) {
                    Ok(offer) => offer,
                    Err(_) => {
                        failure = Some(ControlError::Unavailable);
                        closing.store(true, Ordering::Release);
                        continue;
                    }
                }
            };
            publish(
                status,
                HostStatus {
                    phase: if busy { Phase::Updating } else { Phase::Ready },
                    offer,
                    credential_error,
                    error: action_error,
                },
            );
        }
        thread::park_timeout(Duration::from_millis(10));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manual::{Credential, Destination, PolicySource, Scheme};
    use sumibi_protocol::message::Revision;

    fn configured() -> Arc<PolicyAuthority> {
        let authority = Arc::new(PolicyAuthority::new(
            Destination::for_chat_completions(Scheme::Https, "example.test", 443, "/").unwrap(),
        ));
        authority.set_model("fixture-model").unwrap();
        authority
            .replace_credential(Credential::new("synthetic-key".into()))
            .unwrap();
        authority
    }
    fn supported() -> bool {
        match sumibi_ipc::PinnedProcess::current() {
            Ok(_) => true,
            Err(
                sumibi_ipc::Error::UnsupportedAppContainer
                | sumibi_ipc::Error::UnsupportedElevation
                | sumibi_ipc::Error::UnsupportedIntegrity
                | sumibi_ipc::Error::UnsupportedToken(_),
            ) => false,
            Err(error) => panic!("cannot inspect fixture identity: {error:?}"),
        }
    }
    fn until(control: &HostControl, test: impl Fn(&HostStatus) -> bool) -> HostStatus {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if let Some(status) = control.try_status().unwrap() {
                assert_ne!(status.phase, Phase::Failed);
                if test(&status) {
                    return status;
                }
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        }
    }
    fn finish(control: &mut HostControl) -> Result<(), ControlError> {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if let Some(result) = control.try_finish() {
                return result;
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        }
    }
    fn start(authority: Arc<PolicyAuthority>) -> HostControl {
        HostControl::start_with(move || Ok((authority, false))).unwrap()
    }

    #[test]
    fn explicit_actions_publish_before_ready_and_close_revokes_the_authority() {
        let _guard = crate::HOST_PIPE_TEST_LOCK.lock().unwrap();
        if !supported() {
            return;
        }
        let authority = configured();
        let mut control = start(authority.clone());
        let ready = until(&control, |status| status.phase == Phase::Ready);
        assert!(!ready.offer.as_ref().unwrap().allowed());
        control.try_grant(ready.offer.unwrap()).unwrap();
        until(&control, |status| {
            status.offer.as_ref().is_some_and(ConsentOffer::allowed)
        });
        control.try_revoke().unwrap();
        until(&control, |status| {
            status.phase == Phase::Ready
                && status.offer.as_ref().is_some_and(|offer| !offer.allowed())
        });
        control.request_close();
        assert_eq!(finish(&mut control), Ok(()));
        assert!(!authority.snapshot().unwrap().allowed);
        assert!(matches!(control.try_revoke(), Err(ControlError::Closed)));
        assert_eq!(control.try_status().unwrap().unwrap().phase, Phase::Stopped);
    }

    #[test]
    fn foreign_and_stale_offers_do_not_modify_the_wrong_policy() {
        let _guard = crate::HOST_PIPE_TEST_LOCK.lock().unwrap();
        if !supported() {
            return;
        }
        let authority = configured();
        let foreign = configured();
        let mut control = start(authority.clone());
        until(&control, |status| status.phase == Phase::Ready);
        let foreign_offer = ConsentOffer::try_capture(foreign.clone()).unwrap().unwrap();
        control.try_grant(foreign_offer).unwrap();
        until(&control, |status| {
            status.error == Some(ControlError::ForeignOffer)
        });
        assert!(!foreign.snapshot().unwrap().allowed);
        let stale = ConsentOffer::try_capture(authority.clone())
            .unwrap()
            .unwrap();
        authority.set_model("changed-model").unwrap();
        control.try_grant(stale).unwrap();
        until(&control, |status| {
            status.error == Some(ControlError::StaleOffer)
        });
        assert!(!authority.snapshot().unwrap().allowed);
        control.request_close();
        assert_eq!(finish(&mut control), Ok(()));
    }

    #[test]
    fn close_during_a_blocked_grant_waits_then_revokes_without_readmitting_work() {
        let _guard = crate::HOST_PIPE_TEST_LOCK.lock().unwrap();
        if !supported() {
            return;
        }
        let authority = configured();
        let destination = authority.snapshot().unwrap().destination;
        let version = authority.grant_manual_consent(&destination).unwrap();
        let mut control = start(authority.clone());
        let ready = until(&control, |status| status.phase == Phase::Ready);
        let state = authority.snapshot().unwrap();
        let guard = authority
            .authorize_send_start(
                Revision {
                    input: 1,
                    settings: version.settings,
                    authorization: version.authorization,
                },
                &destination,
                state.credential.as_ref().unwrap(),
            )
            .unwrap();
        control.try_grant(ready.offer.unwrap()).unwrap();
        until(&control, |status| status.phase == Phase::Updating);
        assert_eq!(control.try_revoke(), Err(ControlError::Busy));
        control.request_close();
        until(&control, |status| status.phase == Phase::Stopping);
        assert!(control.try_finish().is_none());
        drop(guard);
        assert_eq!(finish(&mut control), Ok(()));
        assert!(!authority.snapshot().unwrap().allowed);
        assert!(authority.snapshot().unwrap().manual_consent.is_none());
    }

    #[test]
    fn close_during_startup_discards_queued_approval_and_load_failure_is_observable() {
        let _guard = crate::HOST_PIPE_TEST_LOCK.lock().unwrap();
        if !supported() {
            return;
        }
        let authority = configured();
        let loaded = authority.clone();
        let (send, receive) = mpsc::channel();
        let mut control = HostControl::start_with(move || {
            receive.recv_timeout(Duration::from_secs(2)).unwrap();
            Ok((loaded, false))
        })
        .unwrap();
        let offer = ConsentOffer::try_capture(authority.clone())
            .unwrap()
            .unwrap();
        control.try_grant(offer).unwrap();
        control.request_close();
        send.send(()).unwrap();
        assert_eq!(finish(&mut control), Ok(()));
        assert!(!authority.snapshot().unwrap().allowed);
        let mut failed = HostControl::start_with(|| Err(ControlError::Startup)).unwrap();
        assert_eq!(finish(&mut failed), Err(ControlError::Startup));
        let status = failed.try_status().unwrap().unwrap();
        assert_eq!(status.phase, Phase::Failed);
        assert!(status.offer.is_none());
    }

    #[test]
    fn instance_lease_precedes_loading_and_outlives_pending_shutdown() {
        use std::sync::atomic::AtomicUsize;
        let _guard = crate::HOST_PIPE_TEST_LOCK.lock().unwrap();
        if !supported() {
            return;
        }
        struct Lease(Arc<AtomicBool>);
        impl Drop for Lease {
            fn drop(&mut self) {
                self.0.store(false, Ordering::Release);
            }
        }
        let held = Arc::new(AtomicBool::new(false));
        let loads = Arc::new(AtomicUsize::new(0));
        let launch = |authority: Arc<PolicyAuthority>, fail: bool| {
            let held = held.clone();
            let loads = loads.clone();
            HostControl::start_scoped(
                move || {
                    loads.fetch_add(1, Ordering::AcqRel);
                    if fail {
                        Err(ControlError::Startup)
                    } else {
                        Ok((authority, false))
                    }
                },
                move || {
                    held.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                        .map_err(|_| ControlError::AlreadyRunning)?;
                    Ok(Lease(held))
                },
                DispatchMode::NativeChat,
            )
            .unwrap()
        };
        let authority = configured();
        let destination = authority.snapshot().unwrap().destination;
        let version = authority.grant_manual_consent(&destination).unwrap();
        let mut first = launch(authority.clone(), false);
        until(&first, |status| status.phase == Phase::Ready);
        assert!(held.load(Ordering::Acquire));
        let state = authority.snapshot().unwrap();
        let guard = authority
            .authorize_send_start(
                Revision {
                    input: 1,
                    settings: version.settings,
                    authorization: version.authorization,
                },
                &destination,
                state.credential.as_ref().unwrap(),
            )
            .unwrap();
        first.try_revoke().unwrap();
        until(&first, |status| status.phase == Phase::Updating);
        first.request_close();
        until(&first, |status| status.phase == Phase::Stopping);
        let mut second = launch(configured(), false);
        assert_eq!(finish(&mut second), Err(ControlError::AlreadyRunning));
        assert_eq!(
            loads.load(Ordering::Acquire),
            1,
            "duplicate startup touched the loader"
        );
        assert!(held.load(Ordering::Acquire));
        drop(guard);
        assert_eq!(finish(&mut first), Ok(()));
        assert!(!held.load(Ordering::Acquire));
        assert!(!authority.snapshot().unwrap().allowed);
        let mut failed = launch(configured(), true);
        assert_eq!(finish(&mut failed), Err(ControlError::Startup));
        assert!(!held.load(Ordering::Acquire));
        let mut restarted = launch(configured(), false);
        let ready = until(&restarted, |status| status.phase == Phase::Ready);
        assert!(!ready.offer.unwrap().allowed(), "restart inherited consent");
        restarted.request_close();
        assert_eq!(finish(&mut restarted), Ok(()));
        assert!(!held.load(Ordering::Acquire));
    }

    fn wait_prepared_outcome(
        worker: &sumibi_ipc::worker::Worker,
        deadline: Instant,
    ) -> Result<sumibi_protocol::message::Policy, sumibi_ipc::Error> {
        loop {
            assert!(Instant::now() < deadline, "preparation timed out");
            match worker.prepared_policy() {
                Ok(Some(policy)) => return Ok(policy),
                Ok(None) => thread::sleep(Duration::from_millis(5)),
                Err(error) => return Err(error),
            }
        }
    }

    #[test]
    fn host_control_accepts_pinned_peer_and_dispatches_when_consented() {
        let _guard = crate::HOST_PIPE_TEST_LOCK.lock().unwrap();
        if !supported() {
            return;
        }
        let authority = configured();
        let mut control =
            HostControl::start_diagnostic(move || Ok((authority.clone(), false))).unwrap();
        let ready = until(&control, |status| status.phase == Phase::Ready);
        let current = sumibi_ipc::PinnedProcess::current().unwrap();

        // 1. Before consent: accept and connect, preparation fails with PolicyDenied
        let accept_unconsented = control
            .try_accept_private_peer(current.try_clone().unwrap())
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut worker_unconsented = sumibi_ipc::worker::Worker::prepare_pinned(
            sumibi_ipc::job::Reservation::acquire().unwrap(),
            accept_unconsented.name,
            accept_unconsented.broker,
            sumibi_ipc::PinnedProcess::current().unwrap(),
            deadline,
        )
        .unwrap();
        assert!(matches!(
            wait_prepared_outcome(&worker_unconsented, deadline),
            Err(sumibi_ipc::Error::PolicyDenied)
        ));
        assert!(worker_unconsented.reserve(1).is_err());
        worker_unconsented.shutdown(Duration::from_secs(1)).unwrap();

        // 2. Grant consent
        control.try_grant(ready.offer.unwrap()).unwrap();
        until(&control, |status| {
            status.offer.as_ref().is_some_and(ConsentOffer::allowed)
        });

        // 3. After consent: accept and connect, preparation succeeds with allowed policy
        let accept_consented = control
            .try_accept_private_peer(current.try_clone().unwrap())
            .unwrap();
        let mut worker_consented = sumibi_ipc::worker::Worker::prepare_pinned(
            sumibi_ipc::job::Reservation::acquire().unwrap(),
            accept_consented.name,
            accept_consented.broker,
            sumibi_ipc::PinnedProcess::current().unwrap(),
            deadline,
        )
        .unwrap();
        let policy = wait_prepared_outcome(&worker_consented, deadline).unwrap();
        assert!(policy.allowed);
        let permit = worker_consented.reserve(1).unwrap();
        drop(permit);

        // 4. Revoke consent
        control.try_revoke().unwrap();
        until(&control, |status| {
            status.phase == Phase::Ready && status.offer.as_ref().is_some_and(|o| !o.allowed())
        });

        // 5. After revoke: reservation on the existing connection fails
        // Wait briefly for the revocation notice to propagate across the pipe
        let revoke_deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if worker_consented.reserve(2).is_err() {
                break;
            }
            assert!(
                Instant::now() < revoke_deadline,
                "revocation notice timed out"
            );
            thread::sleep(Duration::from_millis(5));
        }

        worker_consented.shutdown(Duration::from_secs(1)).unwrap();
        control.request_close();
        assert_eq!(finish(&mut control), Ok(()));
    }

    #[test]
    fn host_control_serves_discovery_and_dispatches_to_discovered_worker() {
        let _guard = crate::HOST_PIPE_TEST_LOCK.lock().unwrap();
        if !supported() {
            return;
        }
        let authority = configured();
        let a = authority.clone();
        let mut control =
            HostControl::start_diagnostic(move || Ok((a, false))).expect("control starts");

        // 1. Grant consent
        let offer = until(&control, |status| {
            status.phase == Phase::Ready && status.offer.is_some()
        })
        .offer
        .unwrap();
        control.try_grant(offer).unwrap();
        until(&control, |status| {
            status.phase == Phase::Ready && status.offer.as_ref().is_some_and(|o| o.allowed())
        });

        // 2. Set up discovery server
        let bootstrap_name = format!(
            r"\\.\pipe\Sumibi.Bootstrap.Test.{}",
            Nonce::random().unwrap().bootstrap()
        );
        let bootstrap_server = sumibi_ipc::BootstrapServer::create(&bootstrap_name, 1).unwrap();
        let stop = Stop::new().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);

        // Run discovery handoff on a separate responder thread using thread::scope
        let s_stop = stop.clone();
        thread::scope(|s| {
            s.spawn(|| {
                control
                    .serve_bootstrap_handoff(bootstrap_server, &s_stop, deadline)
                    .unwrap();
            });

            // 3. Client (TIP) connects via discovery
            let mut worker = sumibi_ipc::worker::Worker::prepare_discovery(
                sumibi_ipc::job::Reservation::acquire().unwrap(),
                bootstrap_name,
                deadline,
            )
            .unwrap();

            let policy = wait_prepared_outcome(&worker, deadline).unwrap();
            assert!(policy.allowed);
            let permit = worker.reserve(1).unwrap();
            drop(permit);

            // 4. Revoke consent
            control.try_revoke().unwrap();
            until(&control, |status| {
                status.phase == Phase::Ready && status.offer.as_ref().is_some_and(|o| !o.allowed())
            });

            // 5. Reservation fails after revoke
            let revoke_deadline = Instant::now() + Duration::from_secs(2);
            loop {
                if worker.reserve(2).is_err() {
                    break;
                }
                assert!(
                    Instant::now() < revoke_deadline,
                    "revocation notice timed out"
                );
                thread::sleep(Duration::from_millis(5));
            }

            worker.shutdown(Duration::from_secs(1)).unwrap();
        });

        control.request_close();
        assert_eq!(finish(&mut control), Ok(()));
    }
}

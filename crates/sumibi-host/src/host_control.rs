//! Explicit Host lifetime for a responsive consent window. Peer discovery and
//! singleton/autostart integration remain separate: the boot is unadvertised.
use crate::broker_boot::BrokerBoot;
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
use sumibi_ipc::Stop;

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
}

#[derive(Clone)]
pub struct HostStatus {
    pub phase: Phase,
    pub offer: Option<ConsentOffer>,
    pub credential_error: bool,
    pub error: Option<ControlError>,
}
enum Command {
    Grant(ConsentOffer),
    Revoke,
}

/// Commands, status capture and completion polling never wait on native I/O.
/// One consent command is admitted at a time. Closing takes precedence over
/// queued actions and cannot be blocked by a full command queue.
#[must_use = "request close and poll completion before ending the UI"]
pub struct HostControl {
    commands: SyncSender<Command>,
    closing: Arc<AtomicBool>,
    active: Arc<AtomicBool>,
    status: Arc<Mutex<HostStatus>>,
    worker: Option<JoinHandle<Result<(), ControlError>>>,
}
impl HostControl {
    pub fn start(path: PathBuf) -> Result<Self, ControlError> {
        Self::start_with(move || {
            let host =
                crate::startup::PreparedHost::load(&path).map_err(|_| ControlError::Startup)?;
            Ok((host.authority().clone(), host.credential_outcome().is_err()))
        })
    }
    fn start_with(
        load: impl FnOnce() -> Result<(Arc<PolicyAuthority>, bool), ControlError> + Send + 'static,
    ) -> Result<Self, ControlError> {
        let (send, receive) = mpsc::sync_channel(1);
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
                let result = run(
                    load,
                    receive,
                    &worker_closing,
                    &worker_active,
                    &worker_status,
                );
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
            closing,
            active,
            status,
            worker: Some(worker),
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
    pub fn is_updating(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }
    fn submit(&self, command: Command) -> Result<(), ControlError> {
        if self.closing.load(Ordering::Acquire)
            || self
                .worker
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
        if let Some(worker) = &self.worker {
            worker.thread().unpark();
        }
        Ok(())
    }
    pub fn request_close(&self) {
        self.closing.store(true, Ordering::Release);
        if let Some(worker) = &self.worker {
            worker.thread().unpark();
        }
    }
    pub fn try_finish(&mut self) -> Option<Result<(), ControlError>> {
        if !self.worker.as_ref()?.is_finished() {
            return None;
        }
        Some(
            self.worker
                .take()
                .unwrap()
                .join()
                .unwrap_or(Err(ControlError::WorkerFailed)),
        )
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
    closing: &AtomicBool,
    active: &AtomicBool,
    status: &Mutex<HostStatus>,
) -> Result<(), ControlError> {
    let (authority, credential_error) = load()?;
    let stop = Stop::new().map_err(|_| ControlError::Broker)?;
    let mut boot = BrokerBoot::start(&authority, 8, Instant::now() + Duration::from_secs(2))
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
            // No peer is advertised or accepted in this entry yet. Do not
            // poll conversion dispatch while waiting behind a send guard;
            // only explicit consent receipts need Broker acknowledgment here.
            let busy = active.load(Ordering::Acquire);
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
                | sumibi_ipc::Error::UnsupportedElevation,
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
}

//! Logon-scoped long-running Host service managing continuous discovery and graceful shutdown.
use crate::host_control::{ControlError, HostControl, HostStatus};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use sumibi_ipc::{current_bootstrap_pipe_name, handoff, BootstrapServer, Stop};

const DISCOVERY_MAX_INSTANCES: u32 = 64;
const DISCOVERY_ACCEPT_TIMEOUT: Duration = Duration::from_secs(60);

/// Long-running Host service integrating HostControl with continuous
/// logon-scoped bootstrap discovery and graceful teardown.
pub struct HostService {
    control: Arc<HostControl>,
    stop: Stop,
    listener_thread: Option<JoinHandle<Result<(), String>>>,
    is_running: Arc<AtomicBool>,
}

impl HostService {
    /// Start the service by loading settings from disk and acquiring the logon host lease.
    pub fn start(settings_path: PathBuf) -> Result<Self, ControlError> {
        let control = Arc::new(HostControl::start(settings_path)?);
        Self::start_with_control(control)
    }

    /// Start the service with an already initialized HostControl instance.
    pub fn start_with_control(control: Arc<HostControl>) -> Result<Self, ControlError> {
        let pipe_name = current_bootstrap_pipe_name().map_err(|_| ControlError::Startup)?;
        Self::start_on_pipe(control, pipe_name)
    }

    #[cfg(feature = "spike")]
    pub fn start_fixture_with_control(control: Arc<HostControl>) -> Result<Self, ControlError> {
        let pipe_name =
            sumibi_ipc::fixture_bootstrap_pipe_name().map_err(|_| ControlError::Startup)?;
        Self::start_on_pipe(control, pipe_name)
    }

    fn start_on_pipe(control: Arc<HostControl>, pipe_name: String) -> Result<Self, ControlError> {
        let stop = Stop::new().map_err(|_| ControlError::WorkerFailed)?;
        let is_running = Arc::new(AtomicBool::new(true));
        let initial_server = BootstrapServer::create(&pipe_name, DISCOVERY_MAX_INSTANCES)
            .map_err(|_| ControlError::Startup)?;

        let thread_control = control.clone();
        let thread_stop = stop.clone();
        let thread_running = is_running.clone();

        let listener_thread = std::thread::Builder::new()
            .name("sumibi-discovery-listener".into())
            .spawn(move || -> Result<(), String> {
                let mut current_server = initial_server;

                while !thread_stop.is_cancelled().unwrap_or(true)
                    && thread_running.load(Ordering::Acquire)
                {
                    // Prepare the next instance ahead of accepting so the pipe name is always available
                    let next_server = match current_server.additional() {
                        Ok(server) => server,
                        Err(error) => {
                            // If additional fails because max instances are active or stopping, break or retry
                            if thread_stop.is_cancelled().unwrap_or(true) {
                                break;
                            }
                            return Err(format!(
                                "failed to create additional bootstrap instance: {error:?}"
                            ));
                        }
                    };

                    let server = std::mem::replace(&mut current_server, next_server);
                    let deadline = Instant::now() + DISCOVERY_ACCEPT_TIMEOUT;

                    match server.accept(&thread_stop, deadline) {
                        Ok((pipe, client_peer)) => {
                            println!(
                                "[Host] Accepted connection from client PID {}",
                                client_peer
                                    .observed_image_path()
                                    .map(|p| p.display().to_string())
                                    .unwrap_or_else(|_| "unknown".into())
                            );
                            // Hand off private pipe coordinates through HostControl
                            match thread_control.try_accept_private_peer(
                                client_peer.try_clone().map_err(|e| format!("{e:?}"))?,
                            ) {
                                Ok(accept) => {
                                    println!("[Host] Handing off private pipe to client...");
                                    let handoff_deadline = Instant::now() + Duration::from_secs(5);
                                    let send_res = handoff::send(
                                        pipe,
                                        &client_peer,
                                        &accept.name,
                                        &accept.broker,
                                        &thread_stop,
                                        handoff_deadline,
                                    );
                                    println!("[Host] Handoff send result: {send_res:?}");
                                }
                                Err(ControlError::Closed) => break,
                                Err(ControlError::Busy) => {
                                    println!("[Host] Peer queue busy");
                                }
                                Err(e) => {
                                    println!("[Host] try_accept_private_peer failed: {e:?}");
                                }
                            }
                        }
                        Err(sumibi_ipc::Error::Timeout) => {}
                        Err(sumibi_ipc::Error::Cancelled) => break,
                        Err(err) => {
                            println!("[Host] server.accept failed: {err:?}");
                        }
                    }
                }
                Ok(())
            })
            .map_err(|_| ControlError::WorkerFailed)?;

        Ok(Self {
            control,
            stop,
            listener_thread: Some(listener_thread),
            is_running,
        })
    }

    pub fn control(&self) -> &Arc<HostControl> {
        &self.control
    }

    pub fn try_status(&self) -> Result<Option<HostStatus>, ControlError> {
        self.control.try_status()
    }

    pub fn try_finish(&self) -> Option<Result<(), ControlError>> {
        self.control.try_finish()
    }

    pub fn stop(&mut self) -> Result<(), ControlError> {
        self.is_running.store(false, Ordering::Release);
        let _ = self.stop.cancel();
        self.control.request_close();

        if let Some(thread) = self.listener_thread.take() {
            let _ = thread.join();
        }
        let _ = self.control.join();
        Ok(())
    }
}

impl Drop for HostService {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host_control::Phase;
    use crate::manual::{Credential, Destination, Scheme};
    use crate::policy::PolicyAuthority;
    use sumibi_ipc::{job::Reservation, worker::Worker};

    fn configured_authority() -> Arc<PolicyAuthority> {
        let authority = Arc::new(PolicyAuthority::new(
            Destination::for_chat_completions(Scheme::Https, "example.test", 443, "/").unwrap(),
        ));
        authority.set_model("test-model").unwrap();
        authority
            .replace_credential(Credential::new("synthetic-test-key".into()))
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

    #[test]
    fn host_service_starts_accepts_discovery_and_shuts_down() {
        let _lock = crate::HOST_PIPE_TEST_LOCK.lock().unwrap();
        if !supported() {
            return;
        }
        let authority = configured_authority();
        let control =
            Arc::new(HostControl::start_diagnostic(move || Ok((authority, false))).unwrap());

        let mut service = HostService::start_with_control(control).expect("service start");

        // 1. Grant consent via service control
        let deadline = Instant::now() + Duration::from_secs(5);
        let offer = loop {
            if let Ok(Some(status)) = service.try_status() {
                if status.phase == Phase::Ready {
                    if let Some(offer) = status.offer {
                        break offer;
                    }
                }
            }
            if Instant::now() >= deadline {
                panic!("timed out waiting for service offer");
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        assert!(offer.can_grant());
        service.control().try_grant(offer).expect("grant consent");

        // Wait until consented status is reflected
        let consent_deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(Some(status)) = service.try_status() {
                if status.phase == Phase::Ready
                    && status.offer.as_ref().is_some_and(|o| o.allowed())
                {
                    break;
                }
            }
            if Instant::now() >= consent_deadline {
                panic!("timed out waiting for granted status");
            }
            std::thread::sleep(Duration::from_millis(5));
        }

        // 2. Connect worker via discovery over current logon bootstrap pipe
        let pipe_name = current_bootstrap_pipe_name().expect("current bootstrap pipe name");
        let mut worker = Worker::prepare_discovery(
            Reservation::acquire().expect("reservation"),
            pipe_name,
            Instant::now() + Duration::from_secs(5),
        )
        .expect("prepare_discovery");

        // 3. Worker receives Ready policy with allowed == true
        let ready_deadline = Instant::now() + Duration::from_secs(5);
        let policy = loop {
            if let Some(policy) = worker.prepared_policy().expect("prepared_policy") {
                break policy;
            }
            if Instant::now() >= ready_deadline {
                panic!("timed out waiting for worker policy");
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        assert!(policy.allowed);

        // 4. Worker can reserve while consented
        let permit = worker.reserve(1).expect("reserve permit");
        drop(permit);

        // 5. Revoke consent
        service.control().try_revoke().expect("revoke consent");

        // 6. Reservation fails after revoke
        let revoke_deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if worker.reserve(2).is_err() {
                break;
            }
            if Instant::now() >= revoke_deadline {
                panic!("revocation notice timed out");
            }
            std::thread::sleep(Duration::from_millis(5));
        }

        // 7. Stop the service gracefully
        worker
            .shutdown(Duration::from_secs(1))
            .expect("shutdown worker");
        service.stop().expect("service stop");
        assert!(worker.is_closed());
    }

    #[test]
    fn host_service_starts_with_invalid_settings_path_reports_failed() {
        let _lock = crate::HOST_PIPE_TEST_LOCK.lock().unwrap();
        if !supported() {
            return;
        }
        let invalid_path = PathBuf::from("Z:\\nonexistent\\sumibi-settings.json");
        let mut service = HostService::start(invalid_path).expect("service should start control");

        let deadline = Instant::now() + Duration::from_secs(5);
        let status = loop {
            if let Ok(Some(status)) = service.try_status() {
                if status.phase == Phase::Failed {
                    break status;
                }
            }
            if Instant::now() >= deadline {
                panic!("timed out waiting for failed status");
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(status.phase, Phase::Failed);
        assert_eq!(status.error, Some(ControlError::Startup));

        service.stop().expect("service stop");
    }
}

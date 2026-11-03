//! Disposable Host discovery fixture for the TSF preserved key test.
//! Uses logon-scoped bootstrap pipe discovery (ADR 0085).
use std::os::windows::io::AsRawHandle;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use sumibi_host::broker_boot::BrokerBoot;
use sumibi_host::broker_runtime::{JobTicket, PollStep};
use sumibi_host::broker_workers::OwnerStep;
use sumibi_host::manual::{Credential, Destination, Scheme};
use sumibi_host::policy::PolicyAuthority;
use sumibi_ipc::{handoff, job::Reservation, worker::Worker, BootstrapServer, Nonce, Stop};
use windows::Win32::Foundation::{HANDLE, WAIT_OBJECT_0};
use windows::Win32::System::Threading::WaitForSingleObject;

type Failure = Box<dyn std::error::Error>;

pub(crate) struct DiscoveryFixture {
    stop: Stop,
    _source: Arc<PolicyAuthority>,
    _submitted: Arc<AtomicBool>,
    thread: Option<JoinHandle<std::result::Result<(), String>>>,
}

impl DiscoveryFixture {
    pub(crate) fn start() -> std::result::Result<(Self, Worker), Failure> {
        let destination = Destination::new(Scheme::Https, "example.org", 443, "/")
            .ok_or("invalid fixture destination")?;
        let source = Arc::new(PolicyAuthority::new(destination.clone()));
        source
            .replace_credential(Credential::new("fixture".into()))
            .map_err(|_| "fixture credential")?;
        source
            .grant_manual_consent(&destination)
            .map_err(|_| "fixture consent")?;

        let bootstrap_name = format!(
            r"\\.\pipe\Sumibi.Bootstrap.Test.{}",
            Nonce::random().map_err(|_| "nonce")?.bootstrap()
        );
        let bootstrap_server =
            BootstrapServer::create(&bootstrap_name, 1).map_err(|_| "bootstrap server")?;

        let stop = Stop::new().map_err(|_| "stop")?;
        let thread_stop = stop.clone();
        let thread_source = source.clone();
        let submitted = Arc::new(AtomicBool::new(false));
        let thread_submitted = submitted.clone();

        let thread = std::thread::spawn(move || -> std::result::Result<(), String> {
            let deadline = Instant::now() + Duration::from_secs(30);
            let mut boot = BrokerBoot::start(&thread_source, 2, deadline)
                .map_err(|error| format!("{error:?}"))?;

            // 1. Accept client over the bootstrap pipe
            let (pipe, client_peer) = bootstrap_server
                .accept(&thread_stop, deadline)
                .map_err(|error| format!("{error:?}"))?;

            // 2. Register one-shot private accept
            let (_, name) = boot
                .begin_private_accept(
                    client_peer.try_clone().map_err(|e| format!("{e:?}"))?,
                    deadline,
                )
                .map_err(|error| format!("{error:?}"))?;

            // 3. Send handoff offer
            handoff::send(
                pipe,
                &client_peer,
                &name,
                boot.name(),
                &thread_stop,
                deadline,
            )
            .map_err(|error| format!("{error:?}"))?;

            // 4. Advance loop and convert
            while !thread_stop
                .is_cancelled()
                .map_err(|error| format!("{error:?}"))?
                && Instant::now() < deadline
            {
                match boot.advance(&thread_stop, deadline, |_| {
                    move |_: JobTicket| Ok("日本語".into())
                }) {
                    Ok(OwnerStep::Broker(PollStep::Idle)) => {
                        std::thread::sleep(Duration::from_millis(1))
                    }
                    Ok(OwnerStep::Submitted { .. }) => {
                        thread_submitted.store(true, Ordering::Release);
                    }
                    Ok(_) => {}
                    Err(_) if thread_stop.is_cancelled().unwrap_or(true) => break,
                    Err(error) => return Err(format!("{error:?}")),
                }
            }
            boot.shutdown(Instant::now() + Duration::from_secs(2))
                .map_err(|error| format!("{error:?}"))?;
            Ok(())
        });

        let fixture = Self {
            stop,
            _source: source,
            _submitted: submitted,
            thread: Some(thread),
        };

        // Client worker discovers via bootstrap pipe name
        let worker = Worker::prepare_discovery(
            Reservation::acquire().map_err(|_| "reservation")?,
            bootstrap_name,
            Instant::now() + Duration::from_secs(5),
        )
        .map_err(|_| "prepare_discovery")?;

        let ready = worker
            .preparation_handle()
            .ok_or("missing preparation event")?;
        if unsafe { WaitForSingleObject(HANDLE(ready.as_raw_handle()), 5000) } != WAIT_OBJECT_0 {
            return Err("discovery fixture preparation timed out".into());
        }
        let retry_deadline = Instant::now() + Duration::from_secs(1);
        loop {
            match worker.prepared_policy().map_err(|_| "prepared_policy")? {
                Some(policy) if policy.allowed => break,
                Some(_) => return Err("discovery fixture preparation denied".into()),
                None if Instant::now() < retry_deadline => std::thread::yield_now(),
                None => return Err("discovery fixture policy unavailable".into()),
            }
        }
        Ok((fixture, worker))
    }

    pub(crate) fn finish(mut self) -> std::result::Result<(), Failure> {
        self.stop.cancel().map_err(|_| "cancel")?;
        self.thread
            .take()
            .ok_or("fixture already closed")?
            .join()
            .map_err(|_| "fixture panicked")?
            .map_err(|error| error.into())
    }
}

impl Drop for DiscoveryFixture {
    fn drop(&mut self) {
        let _ = self.stop.cancel();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sumibi_ipc::bootstrap::Mode;
    use sumibi_ipc::job::Request;
    use sumibi_protocol::message::Revision;

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
    fn discovery_fixture_serves_discovery_and_converts() {
        if !supported() {
            return;
        }
        let (fixture, mut worker) = DiscoveryFixture::start().expect("fixture start");
        let policy = worker
            .prepared_policy()
            .expect("policy query")
            .expect("policy available");
        assert!(policy.allowed);

        let permit = worker.reserve(1).expect("reserve permit");
        worker
            .submit(
                permit,
                Request {
                    id: 1,
                    revision: Revision {
                        input: 1,
                        settings: policy.settings,
                        authorization: policy.authorization,
                    },
                    text: "nihongo".into(),
                    deadline: Instant::now() + Duration::from_secs(5),
                    mode: Mode::Pinned,
                },
            )
            .expect("submit conversion");

        let deadline = Instant::now() + Duration::from_secs(5);
        let completion = loop {
            if let Some(completion) = worker.try_result().expect("try_result") {
                break completion;
            }
            if Instant::now() >= deadline {
                panic!("timed out waiting for conversion completion");
            }
            std::thread::sleep(Duration::from_millis(5));
        };

        assert_eq!(completion.id, 1);
        assert_eq!(completion.outcome().expect("outcome"), "日本語");

        worker.shutdown(Duration::from_secs(2)).expect("shutdown");
        fixture.finish().expect("fixture finish");
    }
}

//! Disposable shared Host for the explicitly opted-in TSF diagnostic.
//! Both endpoints belong to this process; no installed Host is discovered.
use std::os::windows::io::AsRawHandle;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use sumibi_host::broker_boot::BrokerBoot;
use sumibi_host::broker_runtime::{JobTicket, PollStep};
use sumibi_host::broker_workers::OwnerStep;
use sumibi_host::manual::{Credential, Destination, Scheme};
use sumibi_host::policy::PolicyAuthority;
use sumibi_ipc::{job::Reservation, worker::Worker, Nonce, PinnedProcess, Stop};
use windows::Win32::Foundation::{HANDLE, WAIT_OBJECT_0};
use windows::Win32::System::Threading::WaitForSingleObject;

type Failure = Box<dyn std::error::Error>;

pub(crate) struct Fixture {
    stop: Stop,
    source: Arc<PolicyAuthority>,
    submitted: Arc<AtomicBool>,
    thread: Option<JoinHandle<std::result::Result<(), String>>>,
}

impl Fixture {
    pub(crate) fn start(
        block_conversion: bool,
        delay_conversion: bool,
    ) -> std::result::Result<(Self, Worker), Failure> {
        let destination = Destination::new(Scheme::Https, "example.org", 443, "/")
            .ok_or("invalid fixture destination")?;
        let source = Arc::new(PolicyAuthority::new(destination.clone()));
        source
            .replace_credential(Credential::new("fixture".into()))
            .map_err(|_| "fixture credential")?;
        source
            .grant_manual_consent(&destination)
            .map_err(|_| "fixture consent")?;
        let stop = Stop::new()?;
        let thread_stop = stop.clone();
        let thread_source = source.clone();
        let submitted = Arc::new(AtomicBool::new(false));
        let thread_submitted = submitted.clone();
        let (sender, receiver) = mpsc::sync_channel::<(Nonce, Nonce)>(1);
        let thread = std::thread::spawn(move || -> std::result::Result<(), String> {
            let deadline = Instant::now() + Duration::from_secs(30);
            let mut boot = BrokerBoot::start(&thread_source, 2, deadline)
                .map_err(|error| format!("{error:?}"))?;
            let (_, name) = boot
                .begin_private_accept(
                    PinnedProcess::current().map_err(|error| format!("{error:?}"))?,
                    deadline,
                )
                .map_err(|error| format!("{error:?}"))?;
            sender
                .send((name, boot.name().clone()))
                .map_err(|_| "fixture owner gone".to_string())?;
            while !thread_stop
                .is_cancelled()
                .map_err(|error| format!("{error:?}"))?
                && Instant::now() < deadline
            {
                match boot.advance(&thread_stop, deadline, |_| {
                    move |ticket: JobTicket| {
                        if block_conversion {
                            let _ = ticket.stop().wait_until(ticket.deadline());
                            Err(sumibi_protocol::message::ErrorCode::Cancelled)
                        } else if delay_conversion {
                            let until = ticket
                                .deadline()
                                .min(Instant::now() + Duration::from_millis(250));
                            if ticket.stop().wait_until(until).unwrap_or(true)
                                || ticket.is_cancelled()
                            {
                                Err(sumibi_protocol::message::ErrorCode::Cancelled)
                            } else {
                                Ok("日本語".into())
                            }
                        } else {
                            Ok("日本語".into())
                        }
                    }
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
            source,
            submitted,
            thread: Some(thread),
        };
        let (name, broker) = receiver.recv_timeout(Duration::from_secs(5))?;
        let worker = Worker::prepare_pinned(
            Reservation::acquire()?,
            name,
            broker,
            PinnedProcess::current()?,
            Instant::now() + Duration::from_secs(5),
        )?;
        let ready = worker
            .preparation_handle()
            .ok_or("missing preparation event")?;
        // SAFETY: the worker owns this handle until it is moved into the bridge.
        if unsafe { WaitForSingleObject(HANDLE(ready.as_raw_handle()), 5000) } != WAIT_OBJECT_0 {
            return Err("shared fixture preparation timed out".into());
        }
        let retry_deadline = Instant::now() + Duration::from_secs(1);
        loop {
            match worker.prepared_policy()? {
                Some(policy) if policy.allowed => break,
                Some(_) => return Err("shared fixture preparation denied".into()),
                None if Instant::now() < retry_deadline => std::thread::yield_now(),
                None => return Err("shared fixture policy unavailable".into()),
            }
        }
        Ok((fixture, worker))
    }
    pub(crate) fn revoke(&self) -> std::result::Result<(), Failure> {
        self.source
            .revoke_manual_consent()
            .map_err(|_| "fixture revoke")?;
        Ok(())
    }
    pub(crate) fn submitted(&self) -> bool {
        self.submitted.load(Ordering::Acquire)
    }
    pub(crate) fn finish(mut self) -> std::result::Result<(), Failure> {
        self.stop.cancel()?;
        self.thread
            .take()
            .ok_or("fixture already closed")?
            .join()
            .map_err(|_| "fixture panicked")?
            .map_err(|error| error.into())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.stop.cancel();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

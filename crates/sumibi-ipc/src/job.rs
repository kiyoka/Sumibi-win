//! Bounded worker facade. Only owned text/metadata crosses the thread boundary;
//! the caller retains and retires every TSF target on its own thread.
use crate::bootstrap::{Launched, Mode};
use crate::support::{own, raw};
use crate::{Error, Nonce, Pipe, Result, Stop};
use std::os::windows::io::{AsHandle, BorrowedHandle, OwnedHandle};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use sumibi_protocol::message::{Revision, MAX_TEXT_UNITS};
use sumibi_protocol::session::{ClientSession, Event, Outcome};
use windows::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::System::Threading::{CreateEventW, SetEvent, WaitForSingleObject};

/// Includes reservations, running workers, retained jobs and delivered results.
pub const MAX_JOBS: usize = 4;
static JOBS: AtomicUsize = AtomicUsize::new(0);
#[cfg(test)]
pub(crate) static ADMISSION_TEST: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Reserve before capturing a target or consuming a conversion key.
/// The private field prevents callers from constructing an uncounted slot.
pub struct Reservation(());
impl Reservation {
    pub fn acquire() -> Result<Self> {
        JOBS.try_update(Ordering::AcqRel, Ordering::Acquire, |count| {
            (count < MAX_JOBS).then_some(count + 1)
        })
        .map_err(|_| Error::Capacity)?;
        Ok(Self(()))
    }
    pub fn start(self, executable: PathBuf, request: Request) -> Result<BrokerJob> {
        request.validate()?;
        if !executable.is_absolute() {
            return Err(Error::InvalidArgument);
        }
        let stop = Stop::new()?;
        // SAFETY: private completion event auto-resets; done remains signalled.
        let wake = Arc::new(unsafe { own(CreateEventW(None, false, false, None)?) });
        let done = Arc::new(unsafe { own(CreateEventW(None, true, false, None)?) });
        let (send, receive) = mpsc::sync_channel(1);
        // Both the facade and worker own the same admission. Abandoning the
        // facade cannot free a running worker's slot; finishing the worker
        // cannot free an undrained result's slot either.
        let reservation = Arc::new(self);
        let worker_reservation = reservation.clone();
        let worker_stop = stop.clone();
        let worker_wake = wake.clone();
        let worker_done = done.clone();
        let worker = std::thread::Builder::new()
            .name("sumibi-broker".into())
            .spawn(move || {
                let reservation = worker_reservation;
                let id = request.id;
                let revision = request.revision;
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run(executable, request, &worker_stop)
                }))
                .unwrap_or(Err(Error::WorkerFailed));
                // run has already released/reaped the child and all pipe buffers.
                let _ = send.try_send(Completion {
                    id,
                    revision,
                    outcome,
                    _reservation: reservation.clone(),
                });
                unsafe {
                    let _ = SetEvent(raw(&worker_wake));
                    let _ = SetEvent(raw(&worker_done));
                }
            })
            .map_err(|_| Error::WorkerFailed)?;
        Ok(BrokerJob {
            stop,
            wake,
            done,
            receive,
            worker: Some(worker),
            _reservation: reservation,
        })
    }
}
impl Drop for Reservation {
    fn drop(&mut self) {
        JOBS.fetch_sub(1, Ordering::AcqRel);
    }
}

pub struct Request {
    pub id: u64,
    pub revision: Revision,
    pub text: String,
    pub deadline: Instant,
    pub mode: Mode,
}
impl Request {
    pub(crate) fn validate(&self) -> Result<()> {
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        if self.id == 0
            || self.revision.input == 0
            || self.revision.settings == 0
            || self.revision.authorization == 0
            || self.text.is_empty()
            || self.text.len() > MAX_TEXT_UNITS * 4
            || self.text.encode_utf16().take(MAX_TEXT_UNITS + 1).count() > MAX_TEXT_UNITS
            || remaining.is_zero()
            || remaining > Duration::from_secs(60)
        {
            return Err(Error::InvalidArgument);
        }
        Ok(())
    }
}
pub struct Completion {
    pub id: u64,
    pub revision: Revision,
    outcome: Result<String>,
    // Moving the completion out of BrokerJob does not release its admission.
    // Keep the original allocation with its lease; adapters only borrow it.
    // No COM or callback enters this lease.
    _reservation: Arc<Reservation>,
}
impl Completion {
    pub fn outcome(&self) -> Result<&str> {
        self.outcome.as_deref().map_err(|error| *error)
    }
}
pub struct BrokerJob {
    stop: Stop,
    wake: Arc<OwnedHandle>,
    done: Arc<OwnedHandle>,
    receive: mpsc::Receiver<Completion>,
    worker: Option<JoinHandle<()>>,
    // Last field: release after the buffered completion and OS handles.
    _reservation: Arc<Reservation>,
}
impl BrokerJob {
    pub fn completion_handle(&self) -> BorrowedHandle<'_> {
        self.wake.as_handle()
    }
    /// Consumes the single result. Drop the job and completion after handling
    /// it to release capacity; shutdown alone only joins the worker.
    pub fn try_result(&self) -> Option<Completion> {
        self.receive.try_recv().ok()
    }
    pub fn cancel(&self) -> Result<()> {
        self.stop.cancel()
    }
    /// Call after leaving the window/TSF callback. Drop itself never waits.
    pub fn shutdown(&mut self, timeout: Duration) -> Result<()> {
        self.cancel()?;
        if timeout > Duration::from_secs(3) {
            return Err(Error::InvalidArgument);
        }
        let wait = unsafe { WaitForSingleObject(raw(&self.done), timeout.as_millis() as u32) };
        if wait != WAIT_OBJECT_0 {
            return Err(if wait == WAIT_TIMEOUT {
                Error::Timeout
            } else {
                Error::WorkerFailed
            });
        }
        if let Some(worker) = self.worker.take() {
            worker.join().map_err(|_| Error::WorkerFailed)?;
        }
        Ok(())
    }
}
impl Drop for BrokerJob {
    fn drop(&mut self) {
        let _ = self.stop.cancel();
    }
}

fn run(executable: PathBuf, request: Request, stop: &Stop) -> Result<String> {
    let (_child, mut pipe, boot) =
        Launched::connect(&executable, request.mode, stop, request.deadline)?;
    let mut client = ClientSession::new(Nonce::random()?.id(), boot.id());
    pipe.write_all(
        client.hello().map_err(|_| Error::InvalidMessage)?,
        stop,
        request.deadline,
    )?;
    let Event::Ready(policy) = read_event(&mut pipe, &mut client, stop, request.deadline)? else {
        return Err(Error::InvalidMessage);
    };
    // Consent/configuration is checked BEFORE constructing/transmitting Convert.
    if !policy.allowed
        || policy.settings != request.revision.settings
        || policy.authorization != request.revision.authorization
    {
        return Err(Error::PolicyDenied);
    }
    let now = Instant::now();
    let milliseconds = request
        .deadline
        .saturating_duration_since(now)
        .as_millis()
        .min(60_000) as u32;
    if milliseconds == 0 {
        return Err(Error::Timeout);
    }
    let bytes = client
        .begin(request.revision.input, &request.text, milliseconds, now)
        .map_err(|_| Error::InvalidMessage)?;
    drop(request.text);
    pipe.write_all(bytes, stop, request.deadline)?;
    match read_event(&mut pipe, &mut client, stop, request.deadline)? {
        Event::Reply {
            revision,
            outcome: Outcome::Result(text),
            ..
        } if revision == request.revision => Ok(text),
        Event::Expired(_) => Err(Error::Timeout),
        _ => Err(Error::InvalidMessage),
    }
}
pub(crate) fn read_event(
    pipe: &mut Pipe,
    client: &mut ClientSession,
    stop: &Stop,
    deadline: Instant,
) -> Result<Event> {
    loop {
        let read = pipe.start_read(stop, deadline)?.wait()?;
        let received = client
            .receive(&read.bytes, Instant::now())
            .map_err(|_| Error::InvalidMessage)?;
        if received.consumed != read.bytes.len() {
            return Err(Error::InvalidMessage);
        }
        if let Some(event) = received.event {
            return Ok(event);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Barrier;
    #[test]
    fn reservations_are_bounded_and_released_without_starting_workers() {
        let _serial = ADMISSION_TEST.lock().unwrap();
        let slots = (0..MAX_JOBS)
            .map(|_| Reservation::acquire().unwrap())
            .collect::<Vec<_>>();
        assert!(matches!(Reservation::acquire(), Err(Error::Capacity)));
        drop(slots);
        assert!(Reservation::acquire().is_ok());
    }
    #[test]
    fn simultaneous_threads_share_one_admission_limit() {
        let _serial = ADMISSION_TEST.lock().unwrap();
        let ready = Arc::new(Barrier::new(17));
        let release = Arc::new(Barrier::new(17));
        let admitted = Arc::new(AtomicUsize::new(0));
        std::thread::scope(|scope| {
            for _ in 0..16 {
                let (ready, release, admitted) = (ready.clone(), release.clone(), admitted.clone());
                scope.spawn(move || {
                    let slot = Reservation::acquire();
                    if slot.is_ok() {
                        admitted.fetch_add(1, Ordering::AcqRel);
                    }
                    ready.wait();
                    release.wait();
                    drop(slot);
                });
            }
            ready.wait();
            let count = admitted.load(Ordering::Acquire);
            let at_capacity = matches!(Reservation::acquire(), Err(Error::Capacity));
            release.wait();
            assert_eq!(count, MAX_JOBS);
            assert!(at_capacity);
        });
        let slots = (0..MAX_JOBS)
            .map(|_| Reservation::acquire().unwrap())
            .collect::<Vec<_>>();
        drop(slots);
    }
    #[test]
    fn abandoned_owner_keeps_capacity_until_the_worker_releases_it() {
        let _serial = ADMISSION_TEST.lock().unwrap();
        let owner = Arc::new(Reservation::acquire().unwrap());
        let worker = owner.clone();
        let release = Arc::new(Barrier::new(2));
        let worker_release = release.clone();
        let worker = std::thread::spawn(move || {
            worker_release.wait();
            drop(worker);
        });
        drop(owner);
        let slots = (1..MAX_JOBS)
            .map(|_| Reservation::acquire().unwrap())
            .collect::<Vec<_>>();
        let at_capacity = matches!(Reservation::acquire(), Err(Error::Capacity));
        release.wait();
        worker.join().unwrap();
        assert!(at_capacity);
        let last = Reservation::acquire().unwrap();
        assert!(matches!(Reservation::acquire(), Err(Error::Capacity)));
        drop((slots, last));
    }
    #[test]
    fn invalid_requests_are_rejected_before_launch() {
        let mut request = Request {
            id: 1,
            revision: Revision {
                input: 1,
                settings: 1,
                authorization: 1,
            },
            text: "fixed".into(),
            deadline: Instant::now() + Duration::from_secs(2),
            mode: Mode::Disabled,
        };
        assert!(request.validate().is_ok());
        request.text = "x".repeat(MAX_TEXT_UNITS + 1);
        assert!(request.validate().is_err());
        request.text = "fixed".into();
        request.revision.authorization = 0;
        assert!(request.validate().is_err());
    }
}

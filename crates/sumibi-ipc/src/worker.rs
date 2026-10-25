//! Nonblocking owner facade for an EXE-owned reusable broker worker.
//! Only owned text and scalar IDs cross threads. No COM, DLL references or
//! callbacks are accepted. Explicit shutdown waits outside owner callbacks.
use crate::bootstrap::Mode;
use crate::channel::{Channel, Reply, MAX_REQUESTS};
use crate::job::{Request, Reservation};
use crate::support::{own, raw};
use crate::{Error, PinnedProcess, Result, Stop};
use std::os::windows::io::{AsHandle, AsRawHandle, BorrowedHandle, OwnedHandle};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use sumibi_protocol::message::Revision;
use windows::Win32::Foundation::{HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::System::Threading::{
    CreateEventW, GetProcessId, SetEvent, WaitForMultipleObjects, WaitForSingleObject, INFINITE,
};

#[derive(Default)]
struct Requests {
    active: AtomicU64,
    last: AtomicU64,
}
impl Requests {
    fn reserve(&self, id: u64) -> Result<()> {
        if id == 0 || id <= self.last.load(Ordering::Acquire) {
            return Err(Error::InvalidArgument);
        }
        self.active
            .compare_exchange(0, id, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| Error::Capacity)?;
        self.last.store(id, Ordering::Release);
        Ok(())
    }
    fn release(&self, id: u64) {
        let _ = self
            .active
            .compare_exchange(id, 0, Ordering::AcqRel, Ordering::Acquire);
    }
}
struct State {
    stop: Stop,
    requests: Requests,
    closed: AtomicBool,
    cancelled: AtomicBool,
}
/// One exclusive request, from pre-capture reservation through result delivery.
/// The private fields prevent forgery or transfer to a different worker.
pub struct Permit {
    state: Arc<State>,
    id: u64,
    _reservation: Arc<Reservation>,
}
impl Drop for Permit {
    fn drop(&mut self) {
        self.state.requests.release(self.id);
    }
}
struct Command {
    request: Request,
    permit: Permit,
}
pub struct Completion {
    pub id: u64,
    pub revision: Revision,
    outcome: Result<Reply>,
    peer: u32,
    deadline: Instant,
    // Release result before allowing the next pre-capture reservation.
    _permit: Permit,
}
impl Completion {
    pub fn outcome(&self) -> Result<&str> {
        if self._permit.state.cancelled.load(Ordering::Acquire) {
            return Err(Error::Cancelled);
        }
        if self.outcome.is_ok() && Instant::now() >= self.deadline {
            return Err(Error::Timeout);
        }
        self.outcome
            .as_ref()
            .map_err(|error| *error)
            .and_then(|reply| reply.text())
    }
    /// Metadata only, for verifying process reuse; not an authentication API.
    pub fn exchange(&self) -> Option<(u32, u64)> {
        if self._permit.state.cancelled.load(Ordering::Acquire) || Instant::now() >= self.deadline {
            return None;
        }
        self.outcome
            .as_ref()
            .ok()
            .map(|reply| (self.peer, reply.request))
    }
}
pub struct Worker {
    state: Arc<State>,
    mode: Mode,
    command: Arc<OwnedHandle>,
    wake: Arc<OwnedHandle>,
    done: Arc<OwnedHandle>,
    send: mpsc::SyncSender<Command>,
    receive: mpsc::Receiver<Completion>,
    worker: Option<JoinHandle<()>>,
    _reservation: Arc<Reservation>,
}
struct Finished {
    state: Arc<State>,
    wake: Arc<OwnedHandle>,
    done: Arc<OwnedHandle>,
}
impl Drop for Finished {
    fn drop(&mut self) {
        self.state.closed.store(true, Ordering::Release);
        unsafe {
            let _ = SetEvent(raw(&self.wake));
            let _ = SetEvent(raw(&self.done));
        }
    }
}
impl Worker {
    /// Reserve the global slot before capture. Spawns no broker until submit;
    /// this call creates bounded events/thread but performs no I/O or waits.
    pub fn new(reservation: Reservation, executable: PathBuf, mode: Mode) -> Result<Self> {
        if !executable.is_absolute() {
            return Err(Error::InvalidArgument);
        }
        let reservation = Arc::new(reservation);
        let state = Arc::new(State {
            stop: Stop::new()?,
            requests: Requests::default(),
            closed: AtomicBool::new(false),
            cancelled: AtomicBool::new(false),
        });
        let command = Arc::new(unsafe { own(CreateEventW(None, false, false, None)?) });
        let wake = Arc::new(unsafe { own(CreateEventW(None, false, false, None)?) });
        let done = Arc::new(unsafe { own(CreateEventW(None, true, false, None)?) });
        let (send, commands) = mpsc::sync_channel(1);
        let (results, receive) = mpsc::sync_channel(1);
        let (worker_state, worker_command, worker_wake, worker_done, worker_slot) = (
            state.clone(),
            command.clone(),
            wake.clone(),
            done.clone(),
            reservation.clone(),
        );
        let worker = std::thread::Builder::new()
            .name("sumibi-channel".into())
            .spawn(move || {
                let _reservation = worker_slot;
                let _finished = Finished {
                    state: worker_state.clone(),
                    wake: worker_wake.clone(),
                    done: worker_done,
                };
                run(
                    executable,
                    mode,
                    &worker_state,
                    &worker_command,
                    &worker_wake,
                    &commands,
                    &results,
                    &_reservation,
                );
            })
            .map_err(|_| Error::WorkerFailed)?;
        Ok(Self {
            state,
            mode,
            command,
            wake,
            done,
            send,
            receive,
            worker: Some(worker),
            _reservation: reservation,
        })
    }
    pub fn is_closed(&self) -> bool {
        self.state.closed.load(Ordering::Acquire)
    }
    pub fn reserve(&self, id: u64) -> Result<Permit> {
        if id == 0 {
            return Err(Error::InvalidArgument);
        }
        if self.is_closed() {
            return Err(Error::Closed);
        }
        self.state.requests.reserve(id)?;
        let permit = Permit {
            state: self.state.clone(),
            id,
            _reservation: self._reservation.clone(),
        };
        if self.is_closed() {
            return Err(Error::Closed);
        }
        Ok(permit)
    }
    pub fn submit(&self, permit: Permit, request: Request) -> Result<()> {
        if !Arc::ptr_eq(&permit.state, &self.state)
            || permit.id != request.id
            || request.mode != self.mode
        {
            return Err(Error::InvalidArgument);
        }
        request.validate()?;
        if self.is_closed() {
            return Err(Error::Closed);
        }
        self.send
            .try_send(Command { request, permit })
            .map_err(|_| Error::WorkerFailed)?;
        if let Err(error) = unsafe { SetEvent(raw(&self.command)) } {
            self.state.closed.store(true, Ordering::Release);
            let _ = self.state.stop.cancel();
            return Err(error.into());
        }
        Ok(())
    }
    pub fn completion_handle(&self) -> BorrowedHandle<'_> {
        self.wake.as_handle()
    }
    /// Duplicate this exact thread handle to observe full thread exit after
    /// dropping the facade. Waiting belongs outside callbacks, like shutdown.
    pub fn finished_handle(&self) -> Result<BorrowedHandle<'_>> {
        self.worker
            .as_ref()
            .map(|worker| worker.as_handle())
            .ok_or(Error::Closed)
    }
    pub fn try_result(&self) -> Result<Option<Completion>> {
        match self.receive.try_recv() {
            Ok(completion) => Ok(Some(completion)),
            Err(mpsc::TryRecvError::Empty) => Ok(None),
            Err(mpsc::TryRecvError::Disconnected) => Err(Error::WorkerFailed),
        }
    }
    /// A late cancellation cannot terminate another request on this worker.
    pub fn cancel(&self, id: u64) -> Result<()> {
        if id != 0 && self.state.requests.active.load(Ordering::Acquire) == id {
            self.state.cancelled.store(true, Ordering::Release);
            self.state.closed.store(true, Ordering::Release);
            self.state.stop.cancel()?;
        }
        Ok(())
    }
    /// Reap the worker outside callbacks. This stops in-flight work but does
    /// not revoke an already received completion; call cancel(id) for that.
    pub fn shutdown(&mut self, timeout: Duration) -> Result<()> {
        if timeout > Duration::from_secs(3) {
            return Err(Error::InvalidArgument);
        }
        // Stop pending I/O and reap the thread. A result already received by
        // the owner remains valid until its deadline unless cancel(id) was
        // explicitly requested; teardown is not a late owner cancellation.
        self.state.closed.store(true, Ordering::Release);
        self.state.stop.cancel()?;
        match unsafe { WaitForSingleObject(raw(&self.done), timeout.as_millis() as u32) } {
            WAIT_OBJECT_0 => {}
            WAIT_TIMEOUT => return Err(Error::Timeout),
            _ => return Err(Error::WorkerFailed),
        }
        if let Some(worker) = self.worker.take() {
            worker.join().map_err(|_| Error::WorkerFailed)?;
        }
        Ok(())
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.state.closed.store(true, Ordering::Release);
        let _ = self.state.stop.cancel();
    }
}

#[allow(clippy::too_many_arguments)]
fn run(
    executable: PathBuf,
    mode: Mode,
    state: &Arc<State>,
    command: &OwnedHandle,
    wake: &OwnedHandle,
    commands: &mpsc::Receiver<Command>,
    results: &mpsc::SyncSender<Completion>,
    reservation: &Arc<Reservation>,
) {
    let mut channel: Option<Channel> = None;
    let mut peer: Option<PinnedProcess> = None;
    loop {
        let mut handles = vec![HANDLE(state.stop.as_handle().as_raw_handle()), raw(command)];
        if let Some(peer) = &peer {
            handles.push(HANDLE(peer.as_handle().as_raw_handle()));
        }
        let wait = unsafe { WaitForMultipleObjects(&handles, false, INFINITE) };
        if wait.0 != WAIT_OBJECT_0.0 + 1 {
            break;
        }
        let Ok(Command { request, permit }) = commands.try_recv() else {
            break;
        };
        let id = request.id;
        let revision = request.revision;
        let mut outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if state.closed.load(Ordering::Acquire) {
                return Err(Error::Cancelled);
            }
            if channel.is_none() {
                let opened = Channel::connect_reserved(
                    &executable,
                    mode,
                    &state.stop,
                    request.deadline,
                    reservation.clone(),
                )?;
                peer = Some(opened.peer_process()?);
                channel = Some(opened);
            }
            channel.as_mut().ok_or(Error::Closed)?.convert(
                revision,
                &request.text,
                request.deadline,
            )
        }))
        .unwrap_or(Err(Error::WorkerFailed));
        drop(request.text);
        // Channel cleanup also signals Stop on policy/I/O errors. Preserve
        // that original error; only an explicit owner cancellation overrides it.
        if state.cancelled.load(Ordering::Acquire) {
            outcome = Err(Error::Cancelled);
        } else if outcome.is_ok() {
            if state.stop.is_cancelled().unwrap_or(true) {
                outcome = Err(Error::Cancelled);
            } else if Instant::now() >= request.deadline {
                outcome = Err(Error::Timeout);
            }
        }
        let retire = outcome
            .as_ref()
            .map_or(true, |reply| reply.request as usize >= MAX_REQUESTS);
        if retire {
            state.closed.store(true, Ordering::Release);
        }
        let pid = peer.as_ref().map_or(0, |peer| unsafe {
            GetProcessId(HANDLE(peer.as_handle().as_raw_handle()))
        });
        if results
            .try_send(Completion {
                id,
                revision,
                outcome,
                peer: pid,
                deadline: request.deadline,
                _permit: permit,
            })
            .is_err()
        {
            break;
        }
        unsafe {
            let _ = SetEvent(raw(wake));
        }
        if retire {
            break;
        }
    }
    state.closed.store(true, Ordering::Release);
    // Stop can win over a just-enqueued command. Still deliver one terminal
    // completion and wake the owner before child/pipe cleanup can block.
    if let Ok(Command { request, permit }) = commands.try_recv() {
        if results
            .try_send(Completion {
                id: request.id,
                revision: request.revision,
                outcome: Err(Error::Cancelled),
                peer: 0,
                deadline: request.deadline,
                _permit: permit,
            })
            .is_ok()
        {
            unsafe {
                let _ = SetEvent(raw(wake));
            }
        }
    }
    // Channel, pipe and child cleanup happens here, before Finished signals done.
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn request_ids_never_repeat_or_wrap_after_release() {
        let requests = Requests::default();
        assert_eq!(requests.reserve(0), Err(Error::InvalidArgument));
        assert_eq!(requests.reserve(1), Ok(()));
        requests.release(1);
        assert_eq!(requests.reserve(1), Err(Error::InvalidArgument));
        assert_eq!(requests.reserve(u64::MAX), Ok(()));
        requests.release(u64::MAX);
        assert_eq!(requests.reserve(2), Err(Error::InvalidArgument));
    }
    #[test]
    fn busy_reservation_and_late_release_do_not_consume_or_unlock_the_next_id() {
        let requests = Requests::default();
        assert_eq!(requests.reserve(10), Ok(()));
        assert_eq!(requests.reserve(11), Err(Error::Capacity));
        requests.release(11);
        assert_eq!(requests.reserve(11), Err(Error::Capacity));
        requests.release(10);
        assert_eq!(requests.reserve(11), Ok(()));
        requests.release(10);
        assert_eq!(requests.reserve(12), Err(Error::Capacity));
        requests.release(11);
        assert_eq!(requests.reserve(12), Ok(()));
    }

    #[test]
    fn stopped_worker_wakes_owner_for_queued_terminal_completion() {
        let _serial = crate::job::ADMISSION_TEST.lock().unwrap();
        let state = Arc::new(State {
            stop: Stop::new().unwrap(),
            requests: Requests::default(),
            closed: AtomicBool::new(false),
            cancelled: AtomicBool::new(true),
        });
        state.requests.reserve(1).unwrap();
        let command = unsafe { own(CreateEventW(None, false, false, None).unwrap()) };
        let wake = unsafe { own(CreateEventW(None, false, false, None).unwrap()) };
        let reservation = Arc::new(Reservation::acquire().unwrap());
        let (send, commands) = mpsc::sync_channel(1);
        let (results, receive) = mpsc::sync_channel(1);
        send.send(Command {
            request: Request {
                id: 1,
                revision: Revision {
                    input: 1,
                    settings: 1,
                    authorization: 1,
                },
                text: "synthetic".into(),
                deadline: Instant::now() + Duration::from_secs(1),
                mode: Mode::Disabled,
            },
            permit: Permit {
                state: state.clone(),
                id: 1,
                _reservation: reservation.clone(),
            },
        })
        .unwrap();
        state.stop.cancel().unwrap();
        run(
            std::env::current_exe().unwrap(),
            Mode::Disabled,
            &state,
            &command,
            &wake,
            &commands,
            &results,
            &reservation,
        );
        assert_eq!(
            unsafe { WaitForSingleObject(raw(&wake), 0) },
            WAIT_OBJECT_0,
            "the owner must be notified when its queued request is retired"
        );
        let completion = receive.try_recv().unwrap();
        assert_eq!(completion.id, 1);
        assert!(matches!(completion.outcome(), Err(Error::Cancelled)));
    }
}

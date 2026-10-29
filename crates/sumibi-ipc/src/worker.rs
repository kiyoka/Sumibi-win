//! Nonblocking owner facade for an EXE-owned reusable broker worker.
//! Only owned text and scalar IDs cross threads. No COM, DLL references or
//! callbacks are accepted. Explicit shutdown waits outside owner callbacks.
use crate::bootstrap::Mode;
use crate::channel::{Channel, Reply, MAX_REQUESTS};
use crate::job::{Request, Reservation};
use crate::support::{own, raw};
use crate::{Error, Nonce, PinnedProcess, Result, Stop};
use std::os::windows::io::{AsHandle, AsRawHandle, BorrowedHandle, OwnedHandle};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use sumibi_protocol::message::{Policy, Revision};
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
    preparation: Option<Arc<Preparation>>,
}
struct Preparation {
    event: OwnedHandle,
    result: Mutex<Option<Result<Policy>>>,
}
impl Preparation {
    fn finish(&self, result: Result<Policy>) {
        let mut slot = self
            .result
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if slot.is_none() {
            *slot = Some(result);
            // SAFETY: event is owned by this shared preparation state.
            unsafe {
                let _ = SetEvent(raw(&self.event));
            }
        }
    }

    fn invalidate(&self, error: Error) {
        let mut slot = self
            .result
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        *slot = Some(Err(error));
        // SAFETY: this event is owned by the preparation state.
        unsafe {
            let _ = SetEvent(raw(&self.event));
        }
    }
}
enum Target {
    Launched {
        executable: PathBuf,
        mode: Mode,
    },
    Pinned {
        name: Nonce,
        boot: Nonce,
        peer: PinnedProcess,
    },
}
struct Finished {
    state: Arc<State>,
    wake: Arc<OwnedHandle>,
    done: Arc<OwnedHandle>,
    preparation: Option<Arc<Preparation>>,
}
impl Drop for Finished {
    fn drop(&mut self) {
        self.state.closed.store(true, Ordering::Release);
        if let Some(preparation) = &self.preparation {
            preparation.finish(Err(if self.state.stop.is_cancelled().unwrap_or(true) {
                Error::Cancelled
            } else {
                Error::WorkerFailed
            }));
        }
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
        if !executable.is_absolute() || mode == Mode::Pinned {
            return Err(Error::InvalidArgument);
        }
        Self::spawn(
            reservation,
            Target::Launched { executable, mode },
            mode,
            None,
        )
    }

    /// Create a nonblocking worker for a separately pinned shared Host.
    /// Discovery and the authenticated transfer of name, boot and peer are
    /// the caller's responsibility. The first submit opens the pipe on the
    /// worker thread; cancellation closes only this worker's connection.
    pub fn new_pinned(
        reservation: Reservation,
        name: Nonce,
        boot: Nonce,
        peer: PinnedProcess,
    ) -> Result<Self> {
        Self::spawn(
            reservation,
            Target::Pinned { name, boot, peer },
            Mode::Pinned,
            None,
        )
    }

    /// Start authentication and Ready reception on the worker before input is
    /// captured. Observe `preparation_handle` and `prepared_policy` on the
    /// owner thread. The snapshot is rechecked when text is later sent.
    pub fn prepare_pinned(
        reservation: Reservation,
        name: Nonce,
        boot: Nonce,
        peer: PinnedProcess,
        deadline: Instant,
    ) -> Result<Self> {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(Error::Timeout);
        }
        if remaining > Duration::from_secs(60) {
            return Err(Error::InvalidArgument);
        }
        Self::spawn(
            reservation,
            Target::Pinned { name, boot, peer },
            Mode::Pinned,
            Some(deadline),
        )
    }

    fn spawn(
        reservation: Reservation,
        target: Target,
        mode: Mode,
        prepare_deadline: Option<Instant>,
    ) -> Result<Self> {
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
        let preparation = if prepare_deadline.is_some() {
            Some(Arc::new(Preparation {
                event: unsafe { own(CreateEventW(None, true, false, None)?) },
                result: Mutex::new(None),
            }))
        } else {
            None
        };
        let (send, commands) = mpsc::sync_channel(1);
        let (results, receive) = mpsc::sync_channel(1);
        let (worker_state, worker_command, worker_wake, worker_done, worker_slot) = (
            state.clone(),
            command.clone(),
            wake.clone(),
            done.clone(),
            reservation.clone(),
        );
        let worker_preparation = preparation.clone();
        let worker = std::thread::Builder::new()
            .name("sumibi-channel".into())
            .spawn(move || {
                let _reservation = worker_slot;
                let _finished = Finished {
                    state: worker_state.clone(),
                    wake: worker_wake.clone(),
                    done: worker_done,
                    preparation: worker_preparation.clone(),
                };
                run(
                    target,
                    &worker_state,
                    &worker_command,
                    &worker_wake,
                    &commands,
                    &results,
                    &_reservation,
                    prepare_deadline.zip(worker_preparation.as_deref()),
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
            preparation,
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
        if let Some(preparation) = &self.preparation {
            match *preparation.result.lock().map_err(|_| Error::WorkerFailed)? {
                None => return Err(Error::Capacity),
                Some(Err(error)) => return Err(error),
                Some(Ok(_)) => {}
            }
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
    /// Manual-reset event signalled once preparation finishes, including errors.
    pub fn preparation_handle(&self) -> Option<BorrowedHandle<'_>> {
        self.preparation
            .as_ref()
            .map(|state| state.event.as_handle())
    }

    /// None means preparation is still in progress; the returned Ready policy
    /// is a snapshot and does not authorize a later request by itself. A
    /// later idle policy notice retires the worker and wakes completion_handle.
    pub fn prepared_policy(&self) -> Result<Option<Policy>> {
        let preparation = self.preparation.as_ref().ok_or(Error::InvalidArgument)?;
        match *preparation.result.lock().map_err(|_| Error::WorkerFailed)? {
            None => Ok(None),
            Some(Ok(_)) if self.is_closed() => Err(Error::Closed),
            Some(result) => result.map(Some),
        }
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
    target: Target,
    state: &Arc<State>,
    command: &OwnedHandle,
    wake: &OwnedHandle,
    commands: &mpsc::Receiver<Command>,
    results: &mpsc::SyncSender<Completion>,
    reservation: &Arc<Reservation>,
    preparation: Option<(Instant, &Preparation)>,
) {
    let mut target = Some(target);
    let mut channel: Option<Channel> = None;
    let mut peer: Option<PinnedProcess> = None;
    if let Some((deadline, preparation)) = preparation {
        let opened = match target.take() {
            Some(Target::Pinned { name, boot, peer }) => Channel::connect_pinned_reserved(
                &name,
                boot,
                peer,
                &state.stop,
                deadline,
                reservation.clone(),
            ),
            _ => Err(Error::InvalidArgument),
        };
        match opened.and_then(|mut opened| {
            opened.check_idle_policy(deadline)?;
            let policy = opened.policy().ok_or(Error::Closed)?;
            let pinned = opened.peer_process()?;
            if state.stop.is_cancelled()? {
                return Err(Error::Cancelled);
            }
            Ok((opened, pinned, policy))
        }) {
            Ok((opened, pinned, policy)) => {
                peer = Some(pinned);
                channel = Some(opened);
                preparation.finish(Ok(policy));
            }
            Err(error) => {
                preparation.finish(Err(error));
                return;
            }
        }
    }
    loop {
        let mut handles = vec![HANDLE(state.stop.as_handle().as_raw_handle()), raw(command)];
        let waiting_peer = peer.as_ref().or(match target.as_ref() {
            Some(Target::Pinned { peer, .. }) => Some(peer),
            _ => None,
        });
        if let Some(peer) = waiting_peer {
            handles.push(HANDLE(peer.as_handle().as_raw_handle()));
        }
        let timeout = if preparation.is_some() { 50 } else { INFINITE };
        let wait = unsafe { WaitForMultipleObjects(&handles, false, timeout) };
        if wait == WAIT_TIMEOUT && preparation.is_some() {
            let result = channel.as_mut().ok_or(Error::Closed).and_then(|channel| {
                channel.check_idle_policy(Instant::now() + Duration::from_millis(500))
            });
            if let Err(error) = result {
                if let Some((_, preparation)) = preparation {
                    preparation.invalidate(error);
                }
                break;
            }
            continue;
        }
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
                let opened = match target.take().ok_or(Error::Closed)? {
                    Target::Launched { executable, mode } => Channel::connect_reserved(
                        &executable,
                        mode,
                        &state.stop,
                        request.deadline,
                        reservation.clone(),
                    )?,
                    Target::Pinned { name, boot, peer } => Channel::connect_pinned_reserved(
                        &name,
                        boot,
                        peer,
                        &state.stop,
                        request.deadline,
                        reservation.clone(),
                    )?,
                };
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
        // A launched child's cleanup may signal Stop on policy/I/O errors.
        // Preserve that error; only explicit owner cancellation overrides it.
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
            Target::Launched {
                executable: std::env::current_exe().unwrap(),
                mode: Mode::Disabled,
            },
            &state,
            &command,
            &wake,
            &commands,
            &results,
            &reservation,
            None,
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

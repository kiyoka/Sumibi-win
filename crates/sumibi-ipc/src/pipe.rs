use crate::identity::PinnedProcess;
use crate::support::{last_error, own, raw, Error, Local, Nonce, Result};
use std::os::windows::io::{AsHandle, AsRawHandle, BorrowedHandle, OwnedHandle};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use windows::core::{HRESULT, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Security::Authorization::*;
use windows::Win32::Security::*;
use windows::Win32::Storage::FileSystem::*;
use windows::Win32::System::Pipes::*;
use windows::Win32::System::Threading::*;
use windows::Win32::System::IO::*;

pub const MAX_ENDPOINTS: usize = 8;
const CHUNK: usize = 4096;
const MAX_IO: usize = sumibi_protocol::MAX_FRAME_BYTES + 4;
static ENDPOINTS: AtomicUsize = AtomicUsize::new(0);

struct Permit;
impl Permit {
    fn acquire() -> Result<Self> {
        ENDPOINTS
            .try_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < MAX_ENDPOINTS).then_some(count + 1)
            })
            .map(|_| Self)
            .map_err(|_| Error::Capacity)
    }
}
impl Drop for Permit {
    fn drop(&mut self) {
        ENDPOINTS.fetch_sub(1, Ordering::AcqRel);
    }
}

#[derive(Clone)]
pub struct Stop(Arc<OwnedHandle>);
impl Stop {
    pub fn new() -> Result<Self> {
        // SAFETY: fresh, unnamed manual-reset event owned by the Arc.
        Ok(Self(Arc::new(unsafe {
            own(CreateEventW(None, true, false, None)?)
        })))
    }
    pub fn from_bootstrap(handle: BorrowedHandle<'_>) -> Result<Self> {
        let mut duplicate = HANDLE::default();
        // SAFETY: caller supplies a trusted, live launch event handle. Duplicate
        // only event modification/synchronize rights, without inheritance.
        unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                HANDLE(handle.as_raw_handle()),
                GetCurrentProcess(),
                &mut duplicate,
                EVENT_MODIFY_STATE.0 | SYNCHRONIZE.0,
                false,
                DUPLICATE_HANDLE_OPTIONS(0),
            )?;
            Ok(Self(Arc::new(own(duplicate))))
        }
    }
    pub fn as_handle(&self) -> BorrowedHandle<'_> {
        self.0.as_handle()
    }
    fn raw(&self) -> HANDLE {
        raw(&self.0)
    }
    pub fn cancel(&self) -> Result<()> {
        // SAFETY: the shared event stays alive until every worker releases it.
        unsafe {
            SetEvent(self.raw())?;
        }
        Ok(())
    }
    pub fn is_cancelled(&self) -> Result<bool> {
        match unsafe { WaitForSingleObject(self.raw(), 0) } {
            WAIT_OBJECT_0 => Ok(true),
            WAIT_TIMEOUT => Ok(false),
            _ => Err(last_error()),
        }
    }

    /// Worker-only bounded wait for a cancellation signal. `false` means the
    /// deadline elapsed; a worker must still apply its own request deadline.
    pub fn wait_until(&self, deadline: Instant) -> Result<bool> {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return self.is_cancelled();
        }
        if remaining > Duration::from_secs(60) {
            return Err(Error::InvalidArgument);
        }
        match unsafe {
            WaitForSingleObject(self.raw(), remaining.as_millis().clamp(1, 60_000) as u32)
        } {
            WAIT_OBJECT_0 => Ok(true),
            WAIT_TIMEOUT => Ok(false),
            _ => Err(last_error()),
        }
    }
}

/// One local first-instance server. No application I/O is exposed until accept
/// verifies the exact pinned client and both desktop token identities.
pub struct Server {
    handle: OwnedHandle,
    permit: Permit,
    name: Nonce,
    max_instances: u32,
}
impl Server {
    pub fn create(name: &Nonce) -> Result<Self> {
        Self::create_instance(name, true, 1)
    }
    /// Allow exactly two instances for the disposable multi-client probe.
    /// Normal one-client services keep their original one-instance boundary.
    pub(crate) fn create_pair(name: &Nonce) -> Result<Self> {
        Self::create_shared(name, 2)
    }

    /// Reserve a local pipe name for up to eight independently authenticated
    /// client processes. The caller must obtain and pin each expected process
    /// through a trusted launch or discovery path before accepting it.
    pub fn create_shared(name: &Nonce, max_instances: u32) -> Result<Self> {
        if !(2..=MAX_ENDPOINTS as u32).contains(&max_instances) {
            return Err(Error::InvalidArgument);
        }
        Self::create_instance(name, true, max_instances)
    }

    /// Create another instance while this pipe name is still owned. Every
    /// instance receives the same restricted ACL and consumes an endpoint
    /// permit before opening a kernel handle.
    pub fn additional(&self) -> Result<Self> {
        if self.max_instances == 1 {
            return Err(Error::InvalidArgument);
        }
        Self::create_instance(&self.name, false, self.max_instances)
    }
    fn create_instance(name: &Nonce, first: bool, max_instances: u32) -> Result<Self> {
        let permit = Permit::acquire()?;
        let local = PinnedProcess::current()?;
        let descriptor: Vec<u16> = format!("D:P(A;;GA;;;{})", local.identity().logon_text)
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let mut security = PSECURITY_DESCRIPTOR::default();
        // SAFETY: descriptor is a kernel-derived SID only; no caller SDDL. API
        // allocations outlive creation and are freed once afterward.
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(descriptor.as_ptr()),
                SDDL_REVISION_1,
                &mut security,
                None,
            )?;
            let _allocation = Local(security.0);
            let attributes = SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: security.0,
                bInheritHandle: false.into(),
            };
            let pipe_name = name.pipe_name();
            let mut access = PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED;
            if first {
                access |= FILE_FLAG_FIRST_PIPE_INSTANCE;
            }
            let handle = CreateNamedPipeW(
                PCWSTR(pipe_name.as_ptr()),
                access,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                max_instances,
                CHUNK as u32,
                CHUNK as u32,
                0,
                Some(&attributes),
            );
            if handle.is_invalid() {
                return Err(last_error());
            }
            Ok(Self {
                handle: own(handle),
                permit,
                name: name.clone(),
                max_instances,
            })
        }
    }
    pub fn accept(self, peer: PinnedProcess, stop: &Stop, deadline: Instant) -> Result<Pipe> {
        let mut pipe = Pipe {
            resource: Arc::new(Resource {
                handle: self.handle,
                peer,
                _permit: self.permit,
                group: (self.max_instances > 1).then_some((self.name, self.max_instances)),
            }),
            closed: false,
            authenticated: false,
            busy: false,
        };
        Operation::start(&mut pipe, Kind::Connect, Vec::new(), stop, deadline)?.wait()?;
        pipe.resource
            .peer
            .verify(raw(&pipe.resource.handle), true)?;
        remaining(deadline)?;
        pipe.authenticated = true;
        Ok(pipe)
    }
}

/// One bounded OS accept running outside the Broker owner thread. The result
/// contains a pipe only after the pinned peer and desktop token checks pass.
/// An owner may poll other connections while this task waits. Dropping the
/// task signals cancellation; the worker retains its kernel buffers until the
/// overlapped operation drains, within the supplied finite deadline.
pub struct AcceptTask {
    stop: Stop,
    deadline: Instant,
    receive: Receiver<Result<Pipe>>,
    worker: Option<JoinHandle<()>>,
    finished: bool,
}

impl AcceptTask {
    pub fn start(server: Server, peer: PinnedProcess, deadline: Instant) -> Result<Self> {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(Error::Timeout);
        }
        if remaining > Duration::from_secs(60) {
            return Err(Error::InvalidArgument);
        }
        let stop = Stop::new()?;
        let worker_stop = stop.clone();
        let (send, receive) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("sumibi-pipe-accept".into())
            .spawn(move || {
                let accepted = server.accept(peer, &worker_stop, deadline);
                let _ = send.send(accepted);
            })
            .map_err(|_| Error::WorkerFailed)?;
        Ok(Self {
            stop,
            deadline,
            receive,
            worker: Some(worker),
            finished: false,
        })
    }

    pub fn cancel(&self) -> Result<()> {
        self.stop.cancel()
    }

    /// Cancel and confirm that the OS accept worker has released its pipe
    /// instance before the caller reuses this name for another Broker boot.
    /// False means the worker still owns the instance at the caller's finite
    /// deadline; dropping the task still signals cancellation.
    pub fn cancel_and_reap(&mut self, deadline: Instant) -> Result<bool> {
        self.cancel()?;
        if self.finished {
            return Ok(true);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining > Duration::from_secs(60) {
            return Err(Error::InvalidArgument);
        }
        loop {
            if self.poll().is_some() {
                return Ok(true);
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            thread::sleep(Duration::from_millis(1));
        }
    }

    /// Return immediately. A result is delivered once; a failed accept does
    /// not expose a partially authenticated endpoint to the owner.
    pub fn poll(&mut self) -> Option<Result<Pipe>> {
        if self.finished {
            return None;
        }
        let result = match self.receive.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => Err(Error::WorkerFailed),
        };
        self.finished = true;
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        Some(
            match self.stop.is_cancelled() {
                Ok(false) => result,
                Ok(true) => Err(Error::Cancelled),
                Err(error) => Err(error),
            }
            .and_then(|pipe| {
                remaining(self.deadline)?;
                Ok(pipe)
            }),
        )
    }
}

impl Drop for AcceptTask {
    fn drop(&mut self) {
        let _ = self.stop.cancel();
    }
}

/// Keep the first shared pipe instance occupied by this Broker process, so
/// the name remains owned even when no real client is connected or pending.
/// The self-client handle carries no application data and does not consume an
/// I/O permit; the server side still consumes one of the eight endpoint slots.
/// At most `max_instances - 1` real clients can be served at once.
pub struct SharedAnchor {
    server: Pipe,
    _self_client: OwnedHandle,
}

impl SharedAnchor {
    pub fn create(name: &Nonce, max_instances: u32, deadline: Instant) -> Result<Self> {
        let server = Server::create_shared(name, max_instances)?;
        let server_peer = PinnedProcess::current()?;
        let client_peer = PinnedProcess::current()?;
        let mut accepting = AcceptTask::start(server, server_peer, deadline)?;
        let pipe_name = name.pipe_name();
        // SAFETY: the first instance is already owned by this process and no
        // name has been advertised yet. This handle is held only as a sentinel;
        // it never reads or writes user data. The usual peer check still runs.
        let self_client = unsafe {
            match CreateFileW(
                PCWSTR(pipe_name.as_ptr()),
                GENERIC_READ.0 | GENERIC_WRITE.0,
                FILE_SHARE_MODE(0),
                None,
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
                None,
            ) {
                Ok(handle) => Ok(own(handle)),
                Err(error) => Err(Error::from(error)),
            }
        };
        let self_client = match self_client {
            Ok(handle) => handle,
            Err(error) => {
                let _ = accepting.cancel_and_reap(deadline);
                return Err(error);
            }
        };
        if let Err(error) = client_peer.verify(raw(&self_client), false) {
            let _ = accepting.cancel_and_reap(deadline);
            return Err(error);
        }
        let server = loop {
            if let Some(result) = accepting.poll() {
                break result?;
            }
            if Instant::now() >= deadline {
                let _ = accepting.cancel_and_reap(deadline);
                return Err(Error::Timeout);
            }
            thread::sleep(Duration::from_millis(1));
        };
        Ok(Self {
            server,
            _self_client: self_client,
        })
    }

    /// The expected process must come from a trusted launch or discovery
    /// path. A caller may replace a rejected or timed-out accept without
    /// releasing the first instance or changing the pipe name.
    pub fn begin_accept(&self, peer: PinnedProcess, deadline: Instant) -> Result<AcceptTask> {
        AcceptTask::start(self.server.replacement()?, peer, deadline)
    }
}

struct Resource {
    handle: OwnedHandle,
    peer: PinnedProcess,
    _permit: Permit,
    group: Option<(Nonce, u32)>,
}
pub struct Pipe {
    resource: Arc<Resource>,
    closed: bool,
    authenticated: bool,
    busy: bool,
}
impl Pipe {
    /// Replenish a shared pipe group while an authenticated server instance
    /// still owns the name. A departed client need not remain alive for the
    /// owner to create the next listener. The kernel and process-wide permit
    /// enforce its capacity.
    pub fn replacement(&self) -> Result<Server> {
        if !self.authenticated {
            return Err(Error::Closed);
        }
        let (name, max_instances) = self.resource.group.as_ref().ok_or(Error::InvalidArgument)?;
        Server::create_instance(name, false, *max_instances)
    }
    /// Opening does not wait/retry. The launcher must ensure that this exact
    /// expected server is ready; missing/busy pipes are terminal to this attempt.
    pub fn connect(name: &Nonce, peer: PinnedProcess) -> Result<Self> {
        let permit = Permit::acquire()?;
        peer.alive()?;
        let name = name.pipe_name();
        // SAFETY: name is always a local generated/validated suffix. Prevent
        // impersonation by an unexpected server while checking its identity.
        let handle = unsafe {
            own(CreateFileW(
                PCWSTR(name.as_ptr()),
                GENERIC_READ.0 | GENERIC_WRITE.0,
                FILE_SHARE_MODE(0),
                None,
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
                None,
            )?)
        };
        peer.verify(raw(&handle), false)?;
        Ok(Self {
            resource: Arc::new(Resource {
                handle,
                peer,
                _permit: permit,
                group: None,
            }),
            closed: false,
            authenticated: true,
            busy: false,
        })
    }
    pub fn start_read(&mut self, stop: &Stop, deadline: Instant) -> Result<Operation<'_>> {
        self.check()?;
        Operation::start(self, Kind::Read, vec![0; CHUNK], stop, deadline)
    }
    pub fn start_write(
        &mut self,
        bytes: Vec<u8>,
        stop: &Stop,
        deadline: Instant,
    ) -> Result<Operation<'_>> {
        self.check()?;
        if bytes.is_empty() || bytes.len() > MAX_IO {
            return Err(Error::InvalidArgument);
        }
        Operation::start(self, Kind::Write, bytes, stop, deadline)
    }
    pub fn write_all(&mut self, bytes: Vec<u8>, stop: &Stop, deadline: Instant) -> Result<()> {
        let mut write = Some(self.start_write(bytes, stop, deadline)?);
        while let Some(operation) = write.take() {
            write = operation.advance_write()?;
        }
        Ok(())
    }
    /// Check only for data already queued on this authenticated, asynchronous
    /// pipe. The caller still needs server-side policy enforcement for a change
    /// racing with a subsequent write.
    pub(crate) fn has_data(&self) -> Result<bool> {
        self.check()?;
        let mut available = 0;
        unsafe {
            PeekNamedPipe(
                raw(&self.resource.handle),
                None,
                0,
                None,
                Some(&mut available),
                None,
            )
            .map_err(classify)?;
        }
        Ok(available != 0)
    }
    fn check(&self) -> Result<()> {
        if self.closed || self.busy || !self.authenticated {
            return Err(Error::Closed);
        }
        self.resource.peer.alive()
    }
}

#[derive(Clone, Copy)]
enum Kind {
    Connect,
    Read,
    Write,
}
struct State {
    overlapped: OVERLAPPED,
    event: OwnedHandle,
    resource: Arc<Resource>,
    bytes: Vec<u8>,
    kind: Kind,
    pending: bool,
    transferred: usize,
}

/// Exclusive borrow prevents concurrent I/O or queues on one endpoint. The
/// OVERLAPPED and bytes stay in stable heap allocations until completion.
/// Drop also cancels and drains; it never releases a kernel-owned buffer early.
pub struct Operation<'a> {
    owner: &'a mut Pipe,
    state: Option<Box<State>>,
    stop: Stop,
    deadline: Instant,
}
/// Contains private frame bytes; deliberately no Debug implementation.
pub struct Completion {
    pub bytes: Vec<u8>,
    pub transferred: usize,
}

/// Wait for one of a bounded set of in-flight pipe operations. This never
/// consumes an operation: the caller must run `wait` on the returned index
/// before reusing that endpoint, and may keep the other operations pending.
/// A stop, peer exit or deadline also selects its affected operation so its
/// own `wait` performs the usual cancellation and drain.
pub fn wait_any(operations: &[&Operation<'_>]) -> Result<usize> {
    if operations.is_empty() || operations.len() > MAX_ENDPOINTS {
        return Err(Error::InvalidArgument);
    }
    let mut handles = Vec::with_capacity(operations.len() * 3);
    let mut earliest = (0, operations[0].deadline);
    for (index, operation) in operations.iter().enumerate() {
        let state = operation.state.as_ref().ok_or(Error::Closed)?;
        if !state.pending {
            return Ok(index);
        }
        if operation.deadline < earliest.1 {
            earliest = (index, operation.deadline);
        }
        handles.extend([
            operation.stop.raw(),
            raw(&state.event),
            state.resource.peer.raw(),
        ]);
    }
    let remaining = earliest.1.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Ok(earliest.0);
    }
    let outcome = unsafe {
        WaitForMultipleObjects(
            &handles,
            false,
            remaining.as_millis().clamp(1, 60_000) as u32,
        )
    };
    if outcome == WAIT_TIMEOUT {
        return Ok(earliest.0);
    }
    let offset = outcome.0.wrapping_sub(WAIT_OBJECT_0.0) as usize;
    if offset < handles.len() {
        Ok(offset / 3)
    } else {
        Err(last_error())
    }
}

/// One bounded read per authenticated endpoint. The caller must collect all
/// results before reusing any pipe; dropping a live batch retires its pending
/// endpoints through `Operation::drop`.
pub struct ReadRequest<'a> {
    pub pipe: &'a mut Pipe,
    pub stop: &'a Stop,
    pub deadline: Instant,
}

#[must_use = "collect each endpoint result or dropping the batch retires pending reads"]
pub struct ReadBatch<'a> {
    reads: Vec<Option<Operation<'a>>>,
    start_errors: Vec<Option<Error>>,
    remaining: usize,
    cursor: usize,
}

impl<'a> ReadBatch<'a> {
    pub fn new(requests: Vec<ReadRequest<'a>>) -> Result<Self> {
        if requests.is_empty() || requests.len() > MAX_ENDPOINTS {
            return Err(Error::InvalidArgument);
        }
        let remaining = requests.len();
        let mut reads = Vec::with_capacity(remaining);
        let mut start_errors = Vec::with_capacity(remaining);
        for request in requests {
            match request.pipe.start_read(request.stop, request.deadline) {
                Ok(read) => {
                    reads.push(Some(read));
                    start_errors.push(None);
                }
                Err(error) => {
                    reads.push(None);
                    start_errors.push(Some(error));
                }
            }
        }
        Ok(Self {
            reads,
            start_errors,
            remaining,
            cursor: 0,
        })
    }

    /// Return input-order coordinates and exactly one completed read. A failed
    /// endpoint does not consume another's bytes. On an outer selector error,
    /// drop the batch and retire every still-pending endpoint.
    pub fn wait_next(&mut self) -> Result<Option<(usize, Result<Vec<u8>>)>> {
        if self.remaining == 0 {
            return Ok(None);
        }
        if let Some((index, error)) = self
            .start_errors
            .iter_mut()
            .enumerate()
            .find_map(|(index, error)| error.take().map(|error| (index, error)))
        {
            self.remaining -= 1;
            return Ok(Some((index, Err(error))));
        }
        let index = {
            let live: Vec<_> = (0..self.reads.len())
                .map(|offset| (self.cursor + offset) % self.reads.len())
                .filter_map(|index| self.reads[index].as_ref().map(|read| (index, read)))
                .collect();
            let selected = wait_any(&live.iter().map(|(_, read)| *read).collect::<Vec<_>>())?;
            live[selected].0
        };
        self.cursor = (index + 1) % self.reads.len();
        self.remaining -= 1;
        let read = self.reads[index].take().expect("selected live read");
        Ok(Some((
            index,
            read.wait().map(|completion| completion.bytes),
        )))
    }
}

/// One bounded write per authenticated endpoint. The batch owns the live
/// operations until the caller receives each completion or drops the batch.
pub struct WriteRequest<'a> {
    pub pipe: &'a mut Pipe,
    pub bytes: Vec<u8>,
    pub stop: &'a Stop,
    pub deadline: Instant,
}

#[must_use = "collect each endpoint result or dropping the batch cancels pending writes"]
pub struct WriteBatch<'a> {
    writes: Vec<Option<Operation<'a>>>,
    start_errors: Vec<Option<Error>>,
    remaining: usize,
    cursor: usize,
}

impl<'a> WriteBatch<'a> {
    /// Start all writes before waiting. Individual start failures are returned
    /// by `wait_next`, so one bad endpoint does not discard another live write.
    pub fn new(requests: Vec<WriteRequest<'a>>) -> Result<Self> {
        if requests.is_empty() || requests.len() > MAX_ENDPOINTS {
            return Err(Error::InvalidArgument);
        }
        let remaining = requests.len();
        let mut writes = Vec::with_capacity(remaining);
        let mut start_errors = Vec::with_capacity(remaining);
        for request in requests {
            match request
                .pipe
                .start_write(request.bytes, request.stop, request.deadline)
            {
                Ok(write) => {
                    writes.push(Some(write));
                    start_errors.push(None);
                }
                Err(error) => {
                    writes.push(None);
                    start_errors.push(Some(error));
                }
            }
        }
        Ok(Self {
            writes,
            start_errors,
            remaining,
            cursor: 0,
        })
    }

    /// Return the next completed endpoint in input order coordinates. A
    /// partial transfer is restarted and goes to the back of the ready scan.
    /// An inner error belongs only to that endpoint; the caller closes it.
    /// An outer selector error requires dropping the batch and closing every
    /// still-pending endpoint before reporting a publication complete.
    pub fn wait_next(&mut self) -> Result<Option<(usize, Result<()>)>> {
        if self.remaining == 0 {
            return Ok(None);
        }
        if let Some((index, error)) = self
            .start_errors
            .iter_mut()
            .enumerate()
            .find_map(|(index, error)| error.take().map(|error| (index, error)))
        {
            self.remaining -= 1;
            return Ok(Some((index, Err(error))));
        }
        loop {
            let index = {
                let live: Vec<_> = (0..self.writes.len())
                    .map(|offset| (self.cursor + offset) % self.writes.len())
                    .filter_map(|index| self.writes[index].as_ref().map(|write| (index, write)))
                    .collect();
                let selected = wait_any(&live.iter().map(|(_, write)| *write).collect::<Vec<_>>())?;
                live[selected].0
            };
            self.cursor = (index + 1) % self.writes.len();
            let write = self.writes[index].take().expect("selected live write");
            match write.advance_write() {
                Ok(Some(remaining)) => self.writes[index] = Some(remaining),
                Ok(None) => {
                    self.remaining -= 1;
                    return Ok(Some((index, Ok(()))));
                }
                Err(error) => {
                    self.remaining -= 1;
                    return Ok(Some((index, Err(error))));
                }
            }
        }
    }
}

impl Operation<'_> {
    fn start<'a>(
        owner: &'a mut Pipe,
        kind: Kind,
        bytes: Vec<u8>,
        stop: &Stop,
        deadline: Instant,
    ) -> Result<Operation<'a>> {
        if owner.busy {
            return Err(Error::Closed);
        }
        if stop.is_cancelled()? {
            owner.closed = true;
            return Err(Error::Cancelled);
        }
        if let Err(error) = remaining(deadline) {
            if error == Error::Timeout {
                owner.closed = true;
            }
            return Err(error);
        }
        owner.resource.peer.alive()?;
        let event = unsafe { own(CreateEventW(None, true, false, None)?) };
        let overlapped = OVERLAPPED {
            hEvent: raw(&event),
            ..Default::default()
        };
        let state = Box::new(State {
            overlapped,
            event,
            resource: owner.resource.clone(),
            bytes,
            kind,
            pending: false,
            transferred: 0,
        });
        // Keep a runtime guard as well as the borrow: forgetting an Operation
        // must not permit a second kernel request on this endpoint.
        owner.busy = true;
        let mut operation = Operation {
            owner,
            state: Some(state),
            stop: stop.clone(),
            deadline,
        };
        let state = operation.state.as_mut().expect("operation state");
        let mut count = 0;
        // Preserve kernel-owned memory even if a wrapper unwinds before return.
        state.pending = true;
        // SAFETY: heap addresses remain stable even if Operation is moved. The
        // count slot is used only for synchronous completion, as in Win32's
        // overlapped pipe example; asynchronous completion uses the event/result.
        let started = unsafe {
            match kind {
                Kind::Connect => {
                    ConnectNamedPipe(raw(&state.resource.handle), Some(&mut state.overlapped))
                }
                Kind::Read => ReadFile(
                    raw(&state.resource.handle),
                    Some(&mut state.bytes),
                    Some(&mut count),
                    Some(&mut state.overlapped),
                ),
                Kind::Write => WriteFile(
                    raw(&state.resource.handle),
                    Some(&state.bytes),
                    Some(&mut count),
                    Some(&mut state.overlapped),
                ),
            }
        };
        match started {
            Ok(()) => {
                state.pending = false;
                state.transferred = count as usize;
            }
            Err(error) if error.code() == HRESULT::from_win32(ERROR_IO_PENDING.0) => {
                state.pending = true;
            }
            Err(error)
                if matches!(kind, Kind::Connect)
                    && error.code() == HRESULT::from_win32(ERROR_PIPE_CONNECTED.0) =>
            {
                state.pending = false;
            }
            Err(error) => {
                state.pending = false;
                return Err(classify(error));
            }
        }
        Ok(operation)
    }
    pub fn is_pending(&self) -> bool {
        self.state.as_ref().is_some_and(|state| state.pending)
    }
    pub fn wait(mut self) -> Result<Completion> {
        let result = self.finish();
        if let Err(error) = result {
            if !self.abort() {
                return Err(Error::Retired);
            }
            return Err(error);
        }
        let mut state = self.state.take().expect("completed state");
        let transferred = state.transferred;
        if !matches!(state.kind, Kind::Connect)
            && (transferred == 0 || transferred > state.bytes.len())
        {
            self.owner.closed = true;
            return Err(Error::Disconnected);
        }
        if matches!(state.kind, Kind::Read) {
            state.bytes.truncate(transferred);
        }
        self.owner.busy = false;
        Ok(Completion {
            bytes: std::mem::take(&mut state.bytes),
            transferred,
        })
    }

    /// Complete one write chunk and, if the kernel accepted only a prefix,
    /// start its remainder without releasing this endpoint's exclusive borrow.
    /// The caller can then wait across other endpoints before advancing again.
    pub fn advance_write(mut self) -> Result<Option<Self>> {
        if !self
            .state
            .as_ref()
            .is_some_and(|state| matches!(state.kind, Kind::Write))
        {
            return Err(Error::InvalidArgument);
        }
        if let Err(error) = self.finish() {
            if !self.abort() {
                return Err(Error::Retired);
            }
            return Err(error);
        }
        let state = self.state.as_mut().expect("completed write state");
        let transferred = state.transferred;
        if transferred == 0 || transferred > state.bytes.len() {
            self.owner.closed = true;
            return Err(Error::Disconnected);
        }
        if transferred == state.bytes.len() {
            self.state.take();
            self.owner.busy = false;
            return Ok(None);
        }
        state.bytes.drain(..transferred);
        let next = (|| {
            remaining(self.deadline)?;
            if self.stop.is_cancelled()? {
                return Err(Error::Cancelled);
            }
            self.owner.resource.peer.alive()
        })();
        if let Err(error) = next {
            self.owner.closed = true;
            return Err(error);
        }
        let state = self.state.as_mut().expect("partial write state");
        if let Err(error) = unsafe { ResetEvent(raw(&state.event)) } {
            self.owner.closed = true;
            return Err(classify(error));
        }
        state.overlapped = OVERLAPPED {
            hEvent: raw(&state.event),
            ..Default::default()
        };
        state.transferred = 0;
        state.pending = true;
        let mut count = 0;
        let started = unsafe {
            WriteFile(
                raw(&state.resource.handle),
                Some(&state.bytes),
                Some(&mut count),
                Some(&mut state.overlapped),
            )
        };
        match started {
            Ok(()) => {
                state.pending = false;
                state.transferred = count as usize;
            }
            Err(error) if error.code() == HRESULT::from_win32(ERROR_IO_PENDING.0) => {}
            Err(error) => {
                state.pending = false;
                return Err(classify(error));
            }
        }
        Ok(Some(self))
    }
    fn finish(&mut self) -> Result<()> {
        let remaining = remaining(self.deadline)?;
        if self.stop.is_cancelled()? {
            return Err(Error::Cancelled);
        }
        let state = self.state.as_mut().expect("live operation");
        if !state.pending {
            return Ok(());
        }
        let handles = [
            self.stop.raw(),
            raw(&state.event),
            state.resource.peer.raw(),
        ];
        // SAFETY: every handle stays alive until wait returns; stop wins ties.
        match unsafe {
            WaitForMultipleObjects(
                &handles,
                false,
                remaining.as_millis().clamp(1, 60_000) as u32,
            )
        }
        .0
        {
            value if value == WAIT_OBJECT_0.0 => Err(Error::Cancelled),
            value if value == WAIT_OBJECT_0.0 + 1 => {
                let mut count = 0;
                let completed = unsafe {
                    GetOverlappedResult(
                        raw(&state.resource.handle),
                        &state.overlapped,
                        &mut count,
                        false,
                    )
                };
                if completed
                    .as_ref()
                    .is_err_and(|error| error.code() == HRESULT::from_win32(ERROR_IO_INCOMPLETE.0))
                {
                    return Err(Error::Closed);
                }
                state.pending = false;
                completed.map_err(classify)?;
                state.transferred = count as usize;
                Ok(())
            }
            value if value == WAIT_OBJECT_0.0 + 2 => Err(Error::Disconnected),
            value if value == WAIT_TIMEOUT.0 => Err(Error::Timeout),
            _ => Err(last_error()),
        }
    }
    fn abort(&mut self) -> bool {
        self.owner.closed = true;
        let Some(state) = self.state.as_mut() else {
            return true;
        };
        if !state.pending {
            return true;
        }
        // SAFETY: request cancellation of this exact stable operation, then
        // confirm completion before freeing any buffer, event or pipe handle.
        unsafe {
            let _ = CancelIoEx(raw(&state.resource.handle), Some(&state.overlapped));
            if WaitForSingleObject(raw(&state.event), 1000) == WAIT_OBJECT_0 {
                let mut count = 0;
                let completion = GetOverlappedResult(
                    raw(&state.resource.handle),
                    &state.overlapped,
                    &mut count,
                    false,
                );
                if !completion
                    .as_ref()
                    .is_err_and(|error| error.code() == HRESULT::from_win32(ERROR_IO_INCOMPLETE.0))
                {
                    state.pending = false;
                    return true;
                }
            }
        }
        // A kernel request not observed complete must keep all its memory alive.
        // Keep the endpoint permit too: at most MAX_ENDPOINTS retirements can
        // exist, and this closed Pipe cannot start another operation.
        Box::leak(self.state.take().expect("retired state"));
        false
    }
}
impl Drop for Operation<'_> {
    fn drop(&mut self) {
        if self.state.is_some() {
            self.abort();
        }
    }
}

fn remaining(deadline: Instant) -> Result<Duration> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        Err(Error::Timeout)
    } else if remaining > Duration::from_secs(60) {
        Err(Error::InvalidArgument)
    } else {
        Ok(remaining)
    }
}

fn classify(error: windows::core::Error) -> Error {
    if [ERROR_BROKEN_PIPE, ERROR_PIPE_NOT_CONNECTED, ERROR_NO_DATA]
        .into_iter()
        .any(|code| error.code() == HRESULT::from_win32(code.0))
    {
        Error::Disconnected
    } else if error.code() == HRESULT::from_win32(ERROR_OPERATION_ABORTED.0) {
        Error::Cancelled
    } else {
        Error::from(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::os::windows::io::AsHandle;
    use std::process::{Child, Command, Stdio};
    use std::sync::Mutex;

    static PIPE_TEST_LOCK: Mutex<()> = Mutex::new(());

    fn native_pipe_tests_supported() -> bool {
        match PinnedProcess::current() {
            Ok(_) => true,
            Err(Error::UnsupportedElevation | Error::UnsupportedAppContainer) => {
                eprintln!("native pipe test requires an ordinary desktop process");
                false
            }
            Err(error) => panic!("cannot inspect current process for native pipe test: {error:?}"),
        }
    }

    fn accept_test_child(name: Option<&Nonce>) -> Child {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", "pipe::tests::accept_task_child_fixture"])
            .env("SUMIBI_ACCEPT_TEST_CHILD", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if let Some(name) = name {
            command.env("SUMIBI_ACCEPT_TEST_NAME", name.bootstrap());
        }
        command.spawn().unwrap()
    }

    #[test]
    fn accept_task_child_fixture() {
        if std::env::var_os("SUMIBI_ACCEPT_TEST_CHILD").is_none() {
            return;
        }
        let _pipe = if let Ok(value) = std::env::var("SUMIBI_ACCEPT_TEST_NAME") {
            let name = Nonce::from_bootstrap(&value).unwrap();
            let pipe_name = name.pipe_name();
            let handle = unsafe {
                CreateFileW(
                    PCWSTR(pipe_name.as_ptr()),
                    GENERIC_READ.0 | GENERIC_WRITE.0,
                    FILE_SHARE_MODE(0),
                    None,
                    OPEN_EXISTING,
                    FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
                    None,
                )
            }
            .unwrap();
            Some(unsafe { own(handle) })
        } else {
            None
        };
        let mut signal = [0];
        let _ = std::io::stdin().read(&mut signal).unwrap();
    }

    fn connected_pair(stop: &Stop, deadline: Instant) -> (Pipe, Pipe, Pipe, Pipe) {
        let name = Nonce::random().unwrap();
        let first = Server::create_pair(&name).unwrap();
        let second = first.additional().unwrap();
        std::thread::scope(|scope| {
            let first_stop = stop.clone();
            let second_stop = stop.clone();
            let first_worker = scope.spawn(move || {
                first.accept(PinnedProcess::current().unwrap(), &first_stop, deadline)
            });
            let second_worker = scope.spawn(move || {
                second.accept(PinnedProcess::current().unwrap(), &second_stop, deadline)
            });
            let client_one = Pipe::connect(&name, PinnedProcess::current().unwrap()).unwrap();
            let client_two = Pipe::connect(&name, PinnedProcess::current().unwrap()).unwrap();
            (
                first_worker.join().unwrap().unwrap(),
                second_worker.join().unwrap().unwrap(),
                client_one,
                client_two,
            )
        })
    }

    fn connected_one(stop: &Stop, deadline: Instant) -> (Pipe, Pipe) {
        let name = Nonce::random().unwrap();
        let server = Server::create(&name).unwrap();
        std::thread::scope(|scope| {
            let server_stop = stop.clone();
            let accepted = scope.spawn(move || {
                server.accept(PinnedProcess::current().unwrap(), &server_stop, deadline)
            });
            let client = Pipe::connect(&name, PinnedProcess::current().unwrap()).unwrap();
            (accepted.join().unwrap().unwrap(), client)
        })
    }

    #[test]
    fn accept_task_returns_only_an_authenticated_pipe_without_blocking_owner_poll() {
        if !native_pipe_tests_supported() {
            return;
        }
        let _guard = PIPE_TEST_LOCK.lock().unwrap();
        let name = Nonce::random().unwrap();
        let server = Server::create(&name).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut accepting =
            AcceptTask::start(server, PinnedProcess::current().unwrap(), deadline).unwrap();
        assert!(accepting.poll().is_none());
        let stop = Stop::new().unwrap();
        let mut client = Pipe::connect(&name, PinnedProcess::current().unwrap()).unwrap();
        let mut accepted = loop {
            if let Some(result) = accepting.poll() {
                break result.unwrap();
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        };
        assert!(accepting.poll().is_none());
        client.write_all(vec![29], &stop, deadline).unwrap();
        assert_eq!(
            accepted
                .start_read(&stop, deadline)
                .unwrap()
                .wait()
                .unwrap()
                .bytes,
            [29]
        );
    }

    #[test]
    fn accept_task_cancel_retires_pending_server_without_exposing_pipe() {
        if !native_pipe_tests_supported() {
            return;
        }
        let _guard = PIPE_TEST_LOCK.lock().unwrap();
        let name = Nonce::random().unwrap();
        let server = Server::create(&name).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut accepting =
            AcceptTask::start(server, PinnedProcess::current().unwrap(), deadline).unwrap();
        accepting.cancel().unwrap();
        loop {
            if let Some(result) = accepting.poll() {
                assert!(matches!(result, Err(Error::Cancelled)));
                break;
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        }
        assert!(accepting.poll().is_none());
    }

    #[test]
    fn shared_pending_accepts_reap_before_reusing_the_pipe_name() {
        if !native_pipe_tests_supported() {
            return;
        }
        let _guard = PIPE_TEST_LOCK.lock().unwrap();
        let name = Nonce::random().unwrap();
        let first = Server::create_shared(&name, 2).unwrap();
        let second = first.additional().unwrap();
        let accept_deadline = Instant::now() + Duration::from_secs(5);
        let mut first =
            AcceptTask::start(first, PinnedProcess::current().unwrap(), accept_deadline).unwrap();
        let mut second =
            AcceptTask::start(second, PinnedProcess::current().unwrap(), accept_deadline).unwrap();
        let shutdown_deadline = Instant::now() + Duration::from_millis(500);
        assert!(first.cancel_and_reap(shutdown_deadline).unwrap());
        assert!(second.cancel_and_reap(shutdown_deadline).unwrap());
        assert!(second.cancel_and_reap(shutdown_deadline).unwrap());
        assert!(Server::create_shared(&name, 2).is_ok());
    }

    #[test]
    fn accept_task_cancel_discards_an_already_connected_peer() {
        if !native_pipe_tests_supported() {
            return;
        }
        let _guard = PIPE_TEST_LOCK.lock().unwrap();
        let name = Nonce::random().unwrap();
        let server = Server::create(&name).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut accepting =
            AcceptTask::start(server, PinnedProcess::current().unwrap(), deadline).unwrap();
        let _client = Pipe::connect(&name, PinnedProcess::current().unwrap()).unwrap();
        accepting.cancel().unwrap();
        loop {
            if let Some(result) = accepting.poll() {
                assert!(matches!(result, Err(Error::Cancelled)));
                break;
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn accept_task_timeout_releases_the_first_instance() {
        if !native_pipe_tests_supported() {
            return;
        }
        let _guard = PIPE_TEST_LOCK.lock().unwrap();
        let name = Nonce::random().unwrap();
        let server = Server::create(&name).unwrap();
        let deadline = Instant::now() + Duration::from_millis(100);
        let mut accepting =
            AcceptTask::start(server, PinnedProcess::current().unwrap(), deadline).unwrap();
        loop {
            if let Some(result) = accepting.poll() {
                assert!(matches!(result, Err(Error::Timeout)));
                break;
            }
            assert!(Instant::now() < deadline + Duration::from_secs(1));
            thread::sleep(Duration::from_millis(1));
        }
        assert!(Server::create(&name).is_ok());
    }

    #[test]
    fn accept_task_discards_a_valid_connection_collected_after_its_deadline() {
        if !native_pipe_tests_supported() {
            return;
        }
        let _guard = PIPE_TEST_LOCK.lock().unwrap();
        let name = Nonce::random().unwrap();
        let server = Server::create(&name).unwrap();
        let deadline = Instant::now() + Duration::from_millis(100);
        let mut accepting =
            AcceptTask::start(server, PinnedProcess::current().unwrap(), deadline).unwrap();
        let client = Pipe::connect(&name, PinnedProcess::current().unwrap()).unwrap();
        thread::sleep(Duration::from_millis(150));
        loop {
            if let Some(result) = accepting.poll() {
                assert!(matches!(result, Err(Error::Timeout)));
                break;
            }
            assert!(Instant::now() < deadline + Duration::from_secs(1));
            thread::sleep(Duration::from_millis(1));
        }
        assert!(accepting.poll().is_none());
        drop(client);
        assert!(Server::create(&name).is_ok());
    }

    #[test]
    fn dropping_a_pending_accept_releases_the_first_instance() {
        if !native_pipe_tests_supported() {
            return;
        }
        let _guard = PIPE_TEST_LOCK.lock().unwrap();
        let name = Nonce::random().unwrap();
        let server = Server::create(&name).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        let accepting =
            AcceptTask::start(server, PinnedProcess::current().unwrap(), deadline).unwrap();
        drop(accepting);
        loop {
            if Server::create(&name).is_ok() {
                break;
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn replacement_accept_preserves_the_other_authenticated_connection() {
        if !native_pipe_tests_supported() {
            return;
        }
        let _guard = PIPE_TEST_LOCK.lock().unwrap();
        let name = Nonce::random().unwrap();
        let first = Server::create_pair(&name).unwrap();
        let second = first.additional().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let peer = || PinnedProcess::current().unwrap();
        let mut accepts = vec![
            AcceptTask::start(first, peer(), deadline).unwrap(),
            AcceptTask::start(second, peer(), deadline).unwrap(),
        ];
        let first_client = Pipe::connect(&name, peer()).unwrap();
        let first_server = loop {
            if let Some((index, result)) = accepts
                .iter_mut()
                .enumerate()
                .find_map(|(index, task)| task.poll().map(|result| (index, result)))
            {
                accepts.remove(index);
                break result.unwrap();
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        };
        assert_eq!(accepts.len(), 1);
        assert!(accepts[0].poll().is_none());
        let mut second_client = Pipe::connect(&name, peer()).unwrap();
        let mut second_server = loop {
            if let Some(result) = accepts[0].poll() {
                break result.unwrap();
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        };
        drop(accepts);
        drop(first_client);
        drop(first_server);

        let replacement = second_server.replacement().unwrap();
        let mut accepting = AcceptTask::start(replacement, peer(), deadline).unwrap();
        assert!(accepting.poll().is_none());
        let mut replacement_client = Pipe::connect(&name, peer()).unwrap();
        let mut replacement_server = loop {
            if let Some(result) = accepting.poll() {
                break result.unwrap();
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        };
        let stop = Stop::new().unwrap();
        second_client.write_all(vec![1], &stop, deadline).unwrap();
        replacement_client
            .write_all(vec![2], &stop, deadline)
            .unwrap();
        assert_eq!(
            second_server
                .start_read(&stop, deadline)
                .unwrap()
                .wait()
                .unwrap()
                .bytes,
            [1]
        );
        assert_eq!(
            replacement_server
                .start_read(&stop, deadline)
                .unwrap()
                .wait()
                .unwrap()
                .bytes,
            [2]
        );
    }

    #[test]
    fn shared_group_accepts_three_and_replenishes_one_instance() {
        if !native_pipe_tests_supported() {
            return;
        }
        let _guard = PIPE_TEST_LOCK.lock().unwrap();
        let name = Nonce::random().unwrap();
        assert!(matches!(
            Server::create_shared(&name, 1),
            Err(Error::InvalidArgument)
        ));
        assert!(matches!(
            Server::create_shared(&name, MAX_ENDPOINTS as u32 + 1),
            Err(Error::InvalidArgument)
        ));
        let first = Server::create_shared(&name, 3).unwrap();
        let second = first.additional().unwrap();
        let third = first.additional().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let peer = || PinnedProcess::current().unwrap();
        let mut accepts = vec![
            AcceptTask::start(first, peer(), deadline).unwrap(),
            AcceptTask::start(second, peer(), deadline).unwrap(),
            AcceptTask::start(third, peer(), deadline).unwrap(),
        ];
        let clients: Vec<_> = (0..3)
            .map(|_| Pipe::connect(&name, peer()).unwrap())
            .collect();
        let mut servers = Vec::new();
        while !accepts.is_empty() {
            if let Some((index, result)) = accepts
                .iter_mut()
                .enumerate()
                .find_map(|(index, task)| task.poll().map(|result| (index, result)))
            {
                accepts.remove(index);
                servers.push(result.unwrap());
            } else {
                assert!(Instant::now() < deadline);
                thread::sleep(Duration::from_millis(1));
            }
        }
        assert_eq!(servers.len(), 3);
        drop(servers.remove(0));
        drop(clients);
        let replacement = servers[0].replacement().unwrap();
        let mut accepting = AcceptTask::start(replacement, peer(), deadline).unwrap();
        let mut client = Pipe::connect(&name, peer()).unwrap();
        let mut server = loop {
            if let Some(result) = accepting.poll() {
                break result.unwrap();
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        };
        let stop = Stop::new().unwrap();
        client.write_all(vec![42], &stop, deadline).unwrap();
        assert_eq!(
            server
                .start_read(&stop, deadline)
                .unwrap()
                .wait()
                .unwrap()
                .bytes,
            [42]
        );
    }

    #[test]
    fn shared_anchor_keeps_the_first_instance_across_zero_client_intervals() {
        if !native_pipe_tests_supported() {
            return;
        }
        let _guard = PIPE_TEST_LOCK.lock().unwrap();
        let name = Nonce::random().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let anchor = SharedAnchor::create(&name, 3, deadline).unwrap();
        assert!(Server::create_shared(&name, 3).is_err());
        let stop = Stop::new().unwrap();

        for byte in [11, 22, 33] {
            let mut accepting = anchor
                .begin_accept(PinnedProcess::current().unwrap(), deadline)
                .unwrap();
            let mut client = Pipe::connect(&name, PinnedProcess::current().unwrap()).unwrap();
            let mut server = loop {
                if let Some(result) = accepting.poll() {
                    break result.unwrap();
                }
                assert!(Instant::now() < deadline);
                thread::sleep(Duration::from_millis(1));
            };
            client.write_all(vec![byte], &stop, deadline).unwrap();
            assert_eq!(
                server
                    .start_read(&stop, deadline)
                    .unwrap()
                    .wait()
                    .unwrap()
                    .bytes,
                [byte]
            );
            drop(server);
            drop(client);
        }
        drop(anchor);
        assert!(Server::create_shared(&name, 3).is_ok());
    }

    #[test]
    fn accept_task_authenticates_a_distinct_pinned_process() {
        if !native_pipe_tests_supported() {
            return;
        }
        let _guard = PIPE_TEST_LOCK.lock().unwrap();
        let name = Nonce::random().unwrap();
        let server = Server::create(&name).unwrap();
        let mut child = accept_test_child(Some(&name));
        let peer = PinnedProcess::from_launch_handle(child.as_handle()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut accepting = AcceptTask::start(server, peer, deadline).unwrap();
        let accepted = loop {
            if let Some(result) = accepting.poll() {
                break result.unwrap();
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        };
        assert!(accepted.authenticated);
        drop(child.stdin.take());
        assert!(child.wait().unwrap().success());
    }

    #[test]
    fn shared_group_rearms_after_its_only_client_process_exits() {
        if !native_pipe_tests_supported() {
            return;
        }
        let _guard = PIPE_TEST_LOCK.lock().unwrap();
        let name = Nonce::random().unwrap();
        let first = Server::create_shared(&name, 2).unwrap();
        let mut child = accept_test_child(Some(&name));
        let peer = PinnedProcess::from_launch_handle(child.as_handle()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut accepting = AcceptTask::start(first, peer, deadline).unwrap();
        let old_server = loop {
            if let Some(result) = accepting.poll() {
                break result.unwrap();
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        };
        drop(child.stdin.take());
        assert!(child.wait().unwrap().success());
        let replacement = old_server.replacement().unwrap();
        let mut accepting =
            AcceptTask::start(replacement, PinnedProcess::current().unwrap(), deadline).unwrap();
        let mut client = Pipe::connect(&name, PinnedProcess::current().unwrap()).unwrap();
        let mut server = loop {
            if let Some(result) = accepting.poll() {
                break result.unwrap();
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        };
        let stop = Stop::new().unwrap();
        client.write_all(vec![7], &stop, deadline).unwrap();
        assert_eq!(
            server
                .start_read(&stop, deadline)
                .unwrap()
                .wait()
                .unwrap()
                .bytes,
            [7]
        );
    }

    #[test]
    fn wrong_peer_does_not_retire_another_shared_connection() {
        if !native_pipe_tests_supported() {
            return;
        }
        let _guard = PIPE_TEST_LOCK.lock().unwrap();
        let name = Nonce::random().unwrap();
        let first = Server::create_shared(&name, 2).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut accepting =
            AcceptTask::start(first, PinnedProcess::current().unwrap(), deadline).unwrap();
        let mut first_client = Pipe::connect(&name, PinnedProcess::current().unwrap()).unwrap();
        let mut first_server = loop {
            if let Some(result) = accepting.poll() {
                break result.unwrap();
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        };

        let mut child = accept_test_child(None);
        let wrong_peer = PinnedProcess::from_launch_handle(child.as_handle()).unwrap();
        let second = first_server.replacement().unwrap();
        let mut accepting = AcceptTask::start(second, wrong_peer, deadline).unwrap();
        let wrong_client = Pipe::connect(&name, PinnedProcess::current().unwrap()).unwrap();
        loop {
            if let Some(result) = accepting.poll() {
                assert!(matches!(result, Err(Error::WrongPeer)));
                break;
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        }
        drop(wrong_client);
        drop(child.stdin.take());
        assert!(child.wait().unwrap().success());

        let replacement = first_server.replacement().unwrap();
        let mut accepting =
            AcceptTask::start(replacement, PinnedProcess::current().unwrap(), deadline).unwrap();
        let mut second_client = Pipe::connect(&name, PinnedProcess::current().unwrap()).unwrap();
        let mut second_server = loop {
            if let Some(result) = accepting.poll() {
                break result.unwrap();
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        };
        let stop = Stop::new().unwrap();
        first_client.write_all(vec![1], &stop, deadline).unwrap();
        second_client.write_all(vec![2], &stop, deadline).unwrap();
        assert_eq!(
            first_server
                .start_read(&stop, deadline)
                .unwrap()
                .wait()
                .unwrap()
                .bytes,
            [1]
        );
        assert_eq!(
            second_server
                .start_read(&stop, deadline)
                .unwrap()
                .wait()
                .unwrap()
                .bytes,
            [2]
        );
    }

    #[test]
    fn accept_task_rejects_a_same_user_client_other_than_the_pinned_process() {
        if !native_pipe_tests_supported() {
            return;
        }
        let _guard = PIPE_TEST_LOCK.lock().unwrap();
        let name = Nonce::random().unwrap();
        let server = Server::create(&name).unwrap();
        let mut child = accept_test_child(None);
        let peer = PinnedProcess::from_launch_handle(child.as_handle()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut accepting = AcceptTask::start(server, peer, deadline).unwrap();
        let _wrong_client = Pipe::connect(&name, PinnedProcess::current().unwrap()).unwrap();
        loop {
            if let Some(result) = accepting.poll() {
                assert!(matches!(result, Err(Error::WrongPeer)));
                break;
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        }
        drop(child.stdin.take());
        assert!(child.wait().unwrap().success());
    }

    #[test]
    fn endpoint_budget_is_reserved_before_creation_and_returned_on_drop() {
        let _guard = PIPE_TEST_LOCK.lock().unwrap();
        let permits: Vec<_> = (0..MAX_ENDPOINTS)
            .map(|_| Permit::acquire().unwrap())
            .collect();
        assert!(matches!(Permit::acquire(), Err(Error::Capacity)));
        drop(permits);
        let _permit = Permit::acquire().unwrap();
    }

    #[test]
    fn read_batch_delivers_each_pending_pipe_without_consuming_the_other() {
        if !native_pipe_tests_supported() {
            return;
        }
        let _guard = PIPE_TEST_LOCK.lock().unwrap();
        let stop = Stop::new().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let (mut first, mut second, mut client_one, mut client_two) =
            connected_pair(&stop, deadline);
        let mut batch = ReadBatch::new(vec![
            ReadRequest {
                pipe: &mut first,
                stop: &stop,
                deadline,
            },
            ReadRequest {
                pipe: &mut second,
                stop: &stop,
                deadline,
            },
        ])
        .unwrap();
        client_one.write_all(vec![17], &stop, deadline).unwrap();
        let (first_index, first_result) = batch.wait_next().unwrap().unwrap();
        assert_eq!(first_result.unwrap(), [17]);
        client_two.write_all(vec![19], &stop, deadline).unwrap();
        let (second_index, second_result) = batch.wait_next().unwrap().unwrap();
        assert_ne!(first_index, second_index);
        assert_eq!(second_result.unwrap(), [19]);
        assert!(batch.wait_next().unwrap().is_none());
    }

    #[test]
    fn read_batch_start_failure_leaves_other_pipe_live() {
        if !native_pipe_tests_supported() {
            return;
        }
        let _guard = PIPE_TEST_LOCK.lock().unwrap();
        let setup_stop = Stop::new().unwrap();
        let failed_stop = Stop::new().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let (mut first, mut second, mut client_one, mut client_two) =
            connected_pair(&setup_stop, deadline);
        failed_stop.cancel().unwrap();
        let mut batch = ReadBatch::new(vec![
            ReadRequest {
                pipe: &mut first,
                stop: &failed_stop,
                deadline,
            },
            ReadRequest {
                pipe: &mut second,
                stop: &setup_stop,
                deadline,
            },
        ])
        .unwrap();
        assert!(matches!(
            batch.wait_next().unwrap(),
            Some((0, Err(Error::Cancelled)))
        ));
        let one = client_one.write_all(vec![23], &setup_stop, deadline);
        let two = client_two.write_all(vec![23], &setup_stop, deadline);
        assert!(one.is_ok() || two.is_ok());
        let (index, result) = batch.wait_next().unwrap().unwrap();
        assert_eq!(index, 1);
        assert_eq!(result.unwrap(), [23]);
    }

    #[test]
    fn wait_any_selects_the_pipe_that_completes_after_both_reads_are_pending() {
        if !native_pipe_tests_supported() {
            return;
        }
        let _guard = PIPE_TEST_LOCK.lock().unwrap();
        let stop = Stop::new().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let (mut server_one, mut server_two, mut client_one, mut client_two) =
            connected_pair(&stop, deadline);
        let first_read = server_one.start_read(&stop, deadline).unwrap();
        let second_read = server_two.start_read(&stop, deadline).unwrap();
        assert!(first_read.is_pending() && second_read.is_pending());
        client_one.write_all(vec![7], &stop, deadline).unwrap();
        match wait_any(&[&first_read, &second_read]).unwrap() {
            0 => {
                assert_eq!(first_read.wait().unwrap().bytes, [7]);
                client_two.write_all(vec![8], &stop, deadline).unwrap();
                assert_eq!(wait_any(&[&second_read]).unwrap(), 0);
                assert_eq!(second_read.wait().unwrap().bytes, [8]);
            }
            1 => {
                assert_eq!(second_read.wait().unwrap().bytes, [7]);
                client_two.write_all(vec![8], &stop, deadline).unwrap();
                assert_eq!(wait_any(&[&first_read]).unwrap(), 0);
                assert_eq!(first_read.wait().unwrap().bytes, [8]);
            }
            _ => panic!("only two pending pipe reads"),
        }
        let first_read = server_one.start_read(&stop, deadline).unwrap();
        let second_read = server_two.start_read(&stop, deadline).unwrap();
        assert!(first_read.is_pending() && second_read.is_pending());
        stop.cancel().unwrap();
        assert_eq!(wait_any(&[&first_read, &second_read]).unwrap(), 0);
        assert!(matches!(first_read.wait(), Err(Error::Cancelled)));
        assert!(matches!(second_read.wait(), Err(Error::Cancelled)));
    }

    #[test]
    fn wait_any_expires_one_read_without_consuming_the_other() {
        if !native_pipe_tests_supported() {
            return;
        }
        let _guard = PIPE_TEST_LOCK.lock().unwrap();
        let stop = Stop::new().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let (mut server_one, mut server_two, mut client_one, mut client_two) =
            connected_pair(&stop, deadline);
        let first_deadline = Instant::now() + Duration::from_millis(75);
        let first_read = server_one.start_read(&stop, first_deadline).unwrap();
        let second_read = server_two.start_read(&stop, deadline).unwrap();
        assert!(first_read.is_pending() && second_read.is_pending());
        assert_eq!(wait_any(&[&first_read, &second_read]).unwrap(), 0);
        assert!(matches!(first_read.wait(), Err(Error::Timeout)));
        // Either client may have been assigned to the still-live server pipe.
        // A timed-out read on the other instance must not consume this byte.
        let one = client_one.write_all(vec![9], &stop, deadline);
        let two = client_two.write_all(vec![9], &stop, deadline);
        assert!(one.is_ok() || two.is_ok());
        assert_eq!(wait_any(&[&second_read]).unwrap(), 0);
        assert_eq!(second_read.wait().unwrap().bytes, [9]);
    }

    #[test]
    fn wait_any_isolates_closed_peer_from_other_pending_read() {
        if !native_pipe_tests_supported() {
            return;
        }
        let _guard = PIPE_TEST_LOCK.lock().unwrap();
        let stop = Stop::new().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let (mut server_one, mut server_two, client_one, mut client_two) =
            connected_pair(&stop, deadline);
        let first_read = server_one.start_read(&stop, deadline).unwrap();
        let second_read = server_two.start_read(&stop, deadline).unwrap();
        assert!(first_read.is_pending() && second_read.is_pending());
        drop(client_one);
        let closed = wait_any(&[&first_read, &second_read]).unwrap();
        client_two.write_all(vec![11], &stop, deadline).unwrap();
        if closed == 0 {
            assert!(matches!(first_read.wait(), Err(Error::Disconnected)));
            assert_eq!(wait_any(&[&second_read]).unwrap(), 0);
            assert_eq!(second_read.wait().unwrap().bytes, [11]);
        } else {
            assert_eq!(closed, 1);
            assert!(matches!(second_read.wait(), Err(Error::Disconnected)));
            assert_eq!(wait_any(&[&first_read]).unwrap(), 0);
            assert_eq!(first_read.wait().unwrap().bytes, [11]);
        }
    }

    #[test]
    fn cancelling_one_pipe_does_not_cancel_another_pending_read() {
        if !native_pipe_tests_supported() {
            return;
        }
        let _guard = PIPE_TEST_LOCK.lock().unwrap();
        let setup_stop = Stop::new().unwrap();
        let first_stop = Stop::new().unwrap();
        let second_stop = Stop::new().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let (mut server_one, mut server_two, mut client_one, mut client_two) =
            connected_pair(&setup_stop, deadline);
        let first_read = server_one.start_read(&first_stop, deadline).unwrap();
        let second_read = server_two.start_read(&second_stop, deadline).unwrap();
        assert!(first_read.is_pending() && second_read.is_pending());
        first_stop.cancel().unwrap();
        assert_eq!(wait_any(&[&first_read, &second_read]).unwrap(), 0);
        assert!(matches!(first_read.wait(), Err(Error::Cancelled)));
        drop(server_one);
        // Pipe assignment is decided by the OS, so only one client remains
        // connected to the second server instance.
        let one = client_one.write_all(vec![13], &setup_stop, deadline);
        let two = client_two.write_all(vec![13], &setup_stop, deadline);
        assert!(one.is_ok() || two.is_ok());
        assert_eq!(wait_any(&[&second_read]).unwrap(), 0);
        assert_eq!(second_read.wait().unwrap().bytes, [13]);
    }

    #[test]
    fn advancing_a_large_write_preserves_exact_bytes() {
        if !native_pipe_tests_supported() {
            return;
        }
        let _guard = PIPE_TEST_LOCK.lock().unwrap();
        let stop = Stop::new().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let (mut server, mut client) = connected_one(&stop, deadline);
        let expected: Vec<u8> = (0..64 * 1024).map(|index| index as u8).collect();
        std::thread::scope(|scope| {
            let receiver = scope.spawn(|| {
                let mut received = Vec::new();
                while received.len() < expected.len() {
                    received.extend(
                        client
                            .start_read(&stop, deadline)
                            .unwrap()
                            .wait()
                            .unwrap()
                            .bytes,
                    );
                }
                received
            });
            let mut write = Some(
                server
                    .start_write(expected.clone(), &stop, deadline)
                    .unwrap(),
            );
            while let Some(operation) = write.take() {
                write = operation.advance_write().unwrap();
            }
            assert_eq!(receiver.join().unwrap(), expected);
        });
    }

    #[test]
    fn partial_write_restarts_only_the_unwritten_suffix() {
        if !native_pipe_tests_supported() {
            return;
        }
        let _guard = PIPE_TEST_LOCK.lock().unwrap();
        let stop = Stop::new().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let (mut server, mut client) = connected_one(&stop, deadline);
        let mut write = server
            .start_write(vec![1, 2, 3, 4], &stop, deadline)
            .unwrap();
        write.finish().unwrap();
        let state = write.state.as_mut().unwrap();
        assert_eq!(state.transferred, 4);
        // A four-byte write completes atomically on this fixture. Model a
        // two-byte kernel completion to exercise the otherwise rare restart.
        // The first four bytes are already on the wire in this test.
        state.transferred = 2;
        let mut write = write.advance_write().unwrap();
        assert!(write.is_some());
        while let Some(operation) = write.take() {
            write = operation.advance_write().unwrap();
        }
        let mut received = Vec::new();
        while received.len() < 6 {
            received.extend(
                client
                    .start_read(&stop, deadline)
                    .unwrap()
                    .wait()
                    .unwrap()
                    .bytes,
            );
        }
        assert_eq!(received, [1, 2, 3, 4, 3, 4]);
    }

    #[test]
    fn cancellation_between_write_chunks_does_not_start_a_suffix() {
        if !native_pipe_tests_supported() {
            return;
        }
        let _guard = PIPE_TEST_LOCK.lock().unwrap();
        let stop = Stop::new().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let (mut server, mut client) = connected_one(&stop, deadline);
        let mut write = server
            .start_write(vec![1, 2, 3, 4], &stop, deadline)
            .unwrap();
        write.finish().unwrap();
        let state = write.state.as_mut().unwrap();
        assert_eq!(state.transferred, 4);
        state.transferred = 2;
        stop.cancel().unwrap();
        assert!(matches!(write.advance_write(), Err(Error::Cancelled)));
        assert!(server.closed);
        // Read with an independent Stop to inspect what was already sent.
        let read_stop = Stop::new().unwrap();
        assert_eq!(
            client
                .start_read(&read_stop, deadline)
                .unwrap()
                .wait()
                .unwrap()
                .bytes,
            [1, 2, 3, 4]
        );
    }

    #[test]
    fn slow_write_does_not_hold_a_second_pipe_notification() {
        if !native_pipe_tests_supported() {
            return;
        }
        let _guard = PIPE_TEST_LOCK.lock().unwrap();
        let setup_stop = Stop::new().unwrap();
        let slow_stop = Stop::new().unwrap();
        let fast_stop = Stop::new().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let (mut slow_server, _slow_client) = connected_one(&setup_stop, deadline);
        let (mut fast_server, mut fast_client) = connected_one(&setup_stop, deadline);
        let mut batch = WriteBatch::new(vec![
            WriteRequest {
                pipe: &mut slow_server,
                bytes: vec![7; MAX_IO],
                stop: &slow_stop,
                deadline,
            },
            WriteRequest {
                pipe: &mut fast_server,
                bytes: vec![8],
                stop: &fast_stop,
                deadline,
            },
        ])
        .unwrap();
        assert!(matches!(batch.wait_next(), Ok(Some((1, Ok(()))))));
        assert_eq!(
            fast_client
                .start_read(&fast_stop, deadline)
                .unwrap()
                .wait()
                .unwrap()
                .bytes,
            [8]
        );
        slow_stop.cancel().unwrap();
        assert!(matches!(
            batch.wait_next(),
            Ok(Some((0, Err(Error::Cancelled))))
        ));
        assert!(matches!(batch.wait_next(), Ok(None)));
        drop(batch);
        fast_server
            .write_all(vec![9], &fast_stop, deadline)
            .unwrap();
        assert_eq!(
            fast_client
                .start_read(&fast_stop, deadline)
                .unwrap()
                .wait()
                .unwrap()
                .bytes,
            [9]
        );
    }

    #[test]
    fn failed_write_start_does_not_discard_another_pipe_notification() {
        if !native_pipe_tests_supported() {
            return;
        }
        let _guard = PIPE_TEST_LOCK.lock().unwrap();
        let setup_stop = Stop::new().unwrap();
        let failed_stop = Stop::new().unwrap();
        let live_stop = Stop::new().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let (mut failed_server, _failed_client) = connected_one(&setup_stop, deadline);
        let (mut live_server, mut live_client) = connected_one(&setup_stop, deadline);
        failed_stop.cancel().unwrap();
        let mut batch = WriteBatch::new(vec![
            WriteRequest {
                pipe: &mut failed_server,
                bytes: vec![1],
                stop: &failed_stop,
                deadline,
            },
            WriteRequest {
                pipe: &mut live_server,
                bytes: vec![2],
                stop: &live_stop,
                deadline,
            },
        ])
        .unwrap();
        assert!(matches!(
            batch.wait_next(),
            Ok(Some((0, Err(Error::Cancelled))))
        ));
        assert!(matches!(batch.wait_next(), Ok(Some((1, Ok(()))))));
        assert!(matches!(batch.wait_next(), Ok(None)));
        assert_eq!(
            live_client
                .start_read(&live_stop, deadline)
                .unwrap()
                .wait()
                .unwrap()
                .bytes,
            [2]
        );
    }

    #[test]
    fn bootstrap_values_cannot_be_paths_or_zero_ids() {
        for value in [
            "",
            "0",
            "00000000000000000000000000000000",
            "\\\\remote\\pipe\\name",
            "0000000000000000000000000000000A",
        ] {
            assert!(Nonce::from_bootstrap(value).is_err());
        }
        assert!(Nonce::from_bootstrap("00000000000000000000000000000001").is_ok());
    }
}

use crate::identity::PinnedProcess;
use crate::support::{last_error, own, raw, Error, Local, Nonce, Result};
use std::os::windows::io::{AsHandle, AsRawHandle, BorrowedHandle, OwnedHandle};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
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
}

/// One local first-instance server. No application I/O is exposed until accept
/// verifies the exact pinned client and both desktop token identities.
pub struct Server {
    handle: OwnedHandle,
    permit: Permit,
}
impl Server {
    pub fn create(name: &Nonce) -> Result<Self> {
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
            let name = name.pipe_name();
            let handle = CreateNamedPipeW(
                PCWSTR(name.as_ptr()),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE | FILE_FLAG_OVERLAPPED,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
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
            })
        }
    }
    pub fn accept(self, peer: PinnedProcess, stop: &Stop, deadline: Instant) -> Result<Pipe> {
        let mut pipe = Pipe {
            resource: Arc::new(Resource {
                handle: self.handle,
                peer,
                _permit: self.permit,
            }),
            closed: false,
            authenticated: false,
            busy: false,
        };
        Operation::start(&mut pipe, Kind::Connect, Vec::new(), stop, deadline)?.wait()?;
        pipe.resource
            .peer
            .verify(raw(&pipe.resource.handle), true)?;
        pipe.authenticated = true;
        Ok(pipe)
    }
}

struct Resource {
    handle: OwnedHandle,
    peer: PinnedProcess,
    _permit: Permit,
}
pub struct Pipe {
    resource: Arc<Resource>,
    closed: bool,
    authenticated: bool,
    busy: bool,
}
impl Pipe {
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
    pub fn write_all(&mut self, mut bytes: Vec<u8>, stop: &Stop, deadline: Instant) -> Result<()> {
        loop {
            let completion = self.start_write(bytes, stop, deadline)?.wait()?;
            if completion.transferred == completion.bytes.len() {
                return Ok(());
            }
            bytes = completion.bytes;
            bytes.drain(..completion.transferred);
        }
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
    #[test]
    fn endpoint_budget_is_reserved_before_creation_and_returned_on_drop() {
        let permits: Vec<_> = (0..MAX_ENDPOINTS)
            .map(|_| Permit::acquire().unwrap())
            .collect();
        assert!(matches!(Permit::acquire(), Err(Error::Capacity)));
        drop(permits);
        let _permit = Permit::acquire().unwrap();
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

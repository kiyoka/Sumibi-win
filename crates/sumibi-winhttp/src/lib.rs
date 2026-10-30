//! Native asynchronous HTTPS request lifetime and bounded JSON exchange.
#![cfg(windows)]
#![deny(unsafe_op_in_unsafe_fn)]
use std::cell::UnsafeCell;
use std::ffi::c_void;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Condvar, Mutex,
};
use std::time::{Duration, Instant};
use windows::core::{w, PCWSTR};
use windows::Win32::Networking::WinHttp::*;

const CAPACITY: usize = 8;
const READ_BYTES: usize = 8192;
const MAX_RESPONSE_BYTES: usize = 64 * 1024;
static ACTIVE: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidDestination,
    Capacity,
    Native,
    Cleanup,
    Timeout,
    InvalidDeadline,
    InvalidPayload,
    InvalidState,
    Cancelled,
    InvalidResponse,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Completion {
    Sent,
    Headers,
    Read(usize),
}

struct ReadBuffer(UnsafeCell<[u8; READ_BYTES]>);
// SAFETY: the unique request owner starts at most one native read at a time.
// Rust reads only after READ_COMPLETE is published through the transfer mutex,
// and before the next native call. Callbacks never access the buffer contents.
// The final native callback retains its allocation even after cancellation.
unsafe impl Sync for ReadBuffer {}

struct Payload {
    headers: Vec<u16>,
    body: Vec<u8>,
}
#[derive(Default)]
struct Transfer {
    payload: Option<Arc<Payload>>,
    completion: Option<Result<Completion, Error>>,
}

struct Permit;
impl Permit {
    fn acquire() -> Result<Self, Error> {
        ACTIVE
            .try_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < CAPACITY).then_some(count + 1)
            })
            .map(|_| Self)
            .map_err(|_| Error::Capacity)
    }
}
impl Drop for Permit {
    fn drop(&mut self) {
        ACTIVE.fetch_sub(1, Ordering::AcqRel);
    }
}

struct Handle(*mut c_void, Option<Arc<Permit>>);
// SAFETY: these unique owners never expose their handles. Native operations
// are serialized by the request owner; parents are retained without further
// operations until the request's final callback releases its context.
unsafe impl Send for Handle {}
unsafe impl Sync for Handle {}
impl Handle {
    fn new(value: *mut c_void, permit: &Arc<Permit>) -> Result<Self, Error> {
        if value.is_null() {
            Err(Error::Native)
        } else {
            Ok(Self(value, Some(permit.clone())))
        }
    }
    fn close(mut self) -> Result<(), Error> {
        let value = std::mem::replace(&mut self.0, std::ptr::null_mut());
        // SAFETY: ownership ensures exactly one close; there is no native call
        // in progress on the caller thread when cancellation is requested.
        if unsafe { WinHttpCloseHandle(value) }.is_err() {
            // A failed native close must not make a leaked handle invisible
            // to the process budget, including construction-failure paths.
            std::mem::forget(self.1.take());
            return Err(Error::Cleanup);
        }
        Ok(())
    }
}
impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                if WinHttpCloseHandle(self.0).is_err() {
                    std::mem::forget(self.1.take());
                }
            }
        }
    }
}

struct Context {
    closed: Mutex<bool>,
    changed: Condvar,
    transfer: Mutex<Transfer>,
    transfer_changed: Condvar,
    read: ReadBuffer,
    // Connection must close before session; both outlive the last callback.
    _connection: Handle,
    _session: Handle,
    _permit: Arc<Permit>,
}
unsafe extern "system" fn callback(
    _: *mut c_void,
    context: usize,
    status: u32,
    information: *mut c_void,
    length: u32,
) {
    if context == 0 {
        return;
    }
    if status != WINHTTP_CALLBACK_STATUS_HANDLE_CLOSING {
        // SAFETY: the native Arc reference stays alive until HANDLE_CLOSING,
        // which Windows guarantees follows all callbacks on this request.
        let state = unsafe { &*(context as *const Context) };
        let completion = match status {
            WINHTTP_CALLBACK_STATUS_SENDREQUEST_COMPLETE => {
                let mut transfer = state
                    .transfer
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner());
                let valid = transfer.payload.as_ref().is_some_and(|payload| {
                    payload.body.len() == length as usize
                        && payload.body.as_ptr() == information.cast()
                });
                if transfer.completion.is_none() {
                    transfer.completion = Some(if valid {
                        Ok(Completion::Sent)
                    } else {
                        Err(Error::Native)
                    });
                }
                drop(transfer);
                state.transfer_changed.notify_all();
                return;
            }
            WINHTTP_CALLBACK_STATUS_HEADERS_AVAILABLE => Some(Ok(Completion::Headers)),
            WINHTTP_CALLBACK_STATUS_READ_COMPLETE => {
                if information == state.read.0.get().cast() && length as usize <= READ_BYTES {
                    Some(Ok(Completion::Read(length as usize)))
                } else {
                    Some(Err(Error::Native))
                }
            }
            WINHTTP_CALLBACK_STATUS_REQUEST_ERROR => {
                let error = if !information.is_null()
                    && length as usize == std::mem::size_of::<WINHTTP_ASYNC_RESULT>()
                {
                    // SAFETY: this status supplies WINHTTP_ASYNC_RESULT for
                    // this callback only; copy the fixed error classification.
                    match unsafe {
                        std::ptr::read_unaligned(information.cast::<WINHTTP_ASYNC_RESULT>())
                    }
                    .dwError
                    {
                        ERROR_WINHTTP_TIMEOUT => Error::Timeout,
                        ERROR_WINHTTP_OPERATION_CANCELLED => Error::Cancelled,
                        _ => Error::Native,
                    }
                } else {
                    Error::Native
                };
                Some(Err(error))
            }
            _ => None,
        };
        if let Some(completion) = completion {
            let mut transfer = state
                .transfer
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            if transfer.completion.is_none() {
                transfer.completion = Some(completion);
            }
            drop(transfer);
            state.transfer_changed.notify_all();
        }
        return;
    }
    // SAFETY: registration transfers one Arc reference to WinHTTP. Only its
    // last HANDLE_CLOSING callback reclaims that reference, exactly once.
    let state = unsafe { Arc::from_raw(context as *const Context) };
    *state
        .closed
        .lock()
        .unwrap_or_else(|poison| poison.into_inner()) = true;
    state.changed.notify_all();
}

/// HTTPS POST prepared with TLS 1.2+ defaults, revocation checking and disabled
/// redirects, cookies and automatic authentication. No network request starts.
/// Direct connections only; product proxy policy remains to be implemented.
#[must_use = "close the request and retain its cleanup budget until the last callback"]
pub struct PreparedRequest {
    request: Option<Handle>,
    state: Arc<Context>,
    started: bool,
    deadline: Option<Instant>,
}
impl PreparedRequest {
    pub fn new(host: &str, port: u16, path: &str) -> Result<Self, Error> {
        Self::build(host, port, path, WINHTTP_FLAG_SECURE)
    }
    fn build(
        host: &str,
        port: u16,
        path: &str,
        flags: WINHTTP_OPEN_REQUEST_FLAGS,
    ) -> Result<Self, Error> {
        validate(host, port, path)?;
        let permit = Arc::new(Permit::acquire()?);
        // SECURE_DEFAULTS includes ASYNC and prevents downgrades below TLS1.2.
        let session = Handle::new(
            unsafe {
                WinHttpOpen(
                    w!("Sumibi/0.1"),
                    WINHTTP_ACCESS_TYPE_NO_PROXY,
                    None,
                    None,
                    if flags == WINHTTP_FLAG_SECURE {
                        WINHTTP_FLAG_SECURE_DEFAULTS
                    } else {
                        WINHTTP_FLAG_ASYNC
                    },
                )
            },
            &permit,
        )?;
        let host: Vec<u16> = host.encode_utf16().chain(Some(0)).collect();
        let connection = Handle::new(
            unsafe { WinHttpConnect(session.0, PCWSTR(host.as_ptr()), port, 0) },
            &permit,
        )?;
        let path: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
        let request = Handle::new(
            unsafe {
                WinHttpOpenRequest(
                    connection.0,
                    w!("POST"),
                    PCWSTR(path.as_ptr()),
                    None,
                    None,
                    std::ptr::null(),
                    flags,
                )
            },
            &permit,
        )?;
        let disabled =
            WINHTTP_DISABLE_COOKIES | WINHTTP_DISABLE_REDIRECTS | WINHTTP_DISABLE_AUTHENTICATION;
        option(
            &request,
            WINHTTP_OPTION_DISABLE_FEATURE,
            &disabled.to_ne_bytes(),
        )?;
        option(
            &request,
            WINHTTP_OPTION_MAX_RESPONSE_HEADER_SIZE,
            &(16 * 1024u32).to_ne_bytes(),
        )?;
        if flags == WINHTTP_FLAG_SECURE {
            option(
                &request,
                WINHTTP_OPTION_ENABLE_FEATURE,
                &WINHTTP_ENABLE_SSL_REVOCATION.to_ne_bytes(),
            )?;
        }
        let state = Arc::new(Context {
            closed: Mutex::new(false),
            changed: Condvar::new(),
            transfer: Mutex::new(Transfer::default()),
            transfer_changed: Condvar::new(),
            read: ReadBuffer(UnsafeCell::new([0; READ_BYTES])),
            _connection: connection,
            _session: session,
            _permit: permit,
        });
        let raw = Arc::into_raw(state.clone());
        if option(
            &request,
            WINHTTP_OPTION_CONTEXT_VALUE,
            &(raw as usize).to_ne_bytes(),
        )
        .is_err()
        {
            let _ = request.close();
            unsafe {
                drop(Arc::from_raw(raw));
            } // no callback has been installed
            return Err(Error::Native);
        }
        let previous = unsafe {
            WinHttpSetStatusCallback(
                request.0,
                Some(callback),
                WINHTTP_CALLBACK_STATUS_HANDLE_CREATED
                    | WINHTTP_CALLBACK_STATUS_HANDLE_CLOSING
                    | WINHTTP_CALLBACK_STATUS_SENDREQUEST_COMPLETE
                    | WINHTTP_CALLBACK_STATUS_HEADERS_AVAILABLE
                    | WINHTTP_CALLBACK_STATUS_READ_COMPLETE
                    | WINHTTP_CALLBACK_STATUS_REQUEST_ERROR,
                0,
            )
        };
        if previous.is_some_and(|function| function as usize == usize::MAX) {
            let _ = request.close();
            unsafe {
                drop(Arc::from_raw(raw));
            } // installation failed, no callback can use raw
            return Err(Error::Native);
        }
        Ok(Self {
            request: Some(request),
            state,
            started: false,
            deadline: None,
        })
    }

    /// Low-level single send. A Host adapter must hold its ordered consent
    /// guard through this entire call, not just WinHttpSendRequest's return.
    /// Payload and header storage remain in the context through final close.
    /// No response is received, automatic retry or redirect is attempted.
    pub fn send(
        &mut self,
        credential: &str,
        body: Vec<u8>,
        deadline: Instant,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), Error> {
        if self.started || self.request.is_none() {
            return Err(Error::InvalidState);
        }
        if credential.is_empty()
            || credential.len() > 2560
            || !credential.bytes().all(|byte| byte.is_ascii_graphic())
            || body.is_empty()
            || body.len() > 64 * 1024
        {
            return Err(Error::InvalidPayload);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(Error::Timeout);
        }
        if remaining > Duration::from_secs(60) {
            return Err(Error::InvalidDeadline);
        }
        if cancelled() {
            return Err(Error::Cancelled);
        }
        let payload = Arc::new(Payload {
            headers: format!(
                "Content-Type: application/json\r\nAuthorization: Bearer {credential}\r\n"
            )
            .encode_utf16()
            .collect(),
            body,
        });
        self.state
            .transfer
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .payload = Some(payload.clone());
        self.started = true;
        self.deadline = Some(deadline);
        // SAFETY: native I/O writes no payload bytes. Both vectors are retained
        // unchanged in context even if cancellation closes the native handle.
        let sent = unsafe {
            WinHttpSendRequest(
                self.request.as_ref().unwrap().0,
                Some(&payload.headers),
                Some(payload.body.as_ptr().cast()),
                payload.body.len() as u32,
                payload.body.len() as u32,
                Arc::as_ptr(&self.state) as usize,
            )
        };
        let result = sent.map_err(|_| Error::Native).and_then(|()| {
            match self.wait_completion(deadline, cancelled)? {
                Completion::Sent => Ok(()),
                _ => Err(Error::Native),
            }
        });
        if result.is_err() {
            if let Some(request) = self.request.take() {
                let _ = request.close();
            }
        }
        result
    }

    fn wait_completion(
        &self,
        deadline: Instant,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Completion, Error> {
        loop {
            if cancelled() {
                return Err(Error::Cancelled);
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(Error::Timeout);
            }
            let transfer = self
                .state
                .transfer
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            if let Some(completion) = transfer.completion {
                return completion;
            }
            drop(
                self.state
                    .transfer_changed
                    .wait_timeout(transfer, remaining.min(Duration::from_millis(50)))
                    .unwrap_or_else(|poison| poison.into_inner()),
            );
        }
    }

    /// Receive one HTTP 200 JSON response, then close the request on return.
    /// Uses the original send deadline, with no retry, redirect or decompression.
    pub fn receive(self, cancelled: &dyn Fn() -> bool) -> Result<Vec<u8>, Error> {
        let deadline = self.deadline.ok_or(Error::InvalidState)?;
        let handle = self.request.as_ref().ok_or(Error::InvalidState)?.0;
        if self.wait_completion(deadline, cancelled)? != Completion::Sent {
            return Err(Error::InvalidState);
        }
        self.reset_completion();
        // SAFETY: send has completed and no native call is in progress. The
        // context remains alive, including if this call completes immediately.
        unsafe { WinHttpReceiveResponse(handle, std::ptr::null_mut()) }
            .map_err(|_| Error::Native)?;
        if self.wait_completion(deadline, cancelled)? != Completion::Headers {
            return Err(Error::Native);
        }
        let expected_length = validate_response_headers(handle)?;
        let mut response = Vec::new();
        loop {
            if cancelled() {
                return Err(Error::Cancelled);
            }
            if Instant::now() >= deadline {
                return Err(Error::Timeout);
            }
            self.reset_completion();
            // SAFETY: only this owner can start a read. The fixed buffer lives
            // in the callback context; no Rust reference exists during I/O.
            // Async bytes-read MUST be null, even for immediate completion.
            unsafe {
                WinHttpReadData(
                    handle,
                    self.state.read.0.get().cast(),
                    READ_BYTES as u32,
                    std::ptr::null_mut(),
                )
            }
            .map_err(|_| Error::Native)?;
            let Completion::Read(count) = self.wait_completion(deadline, cancelled)? else {
                return Err(Error::Native);
            };
            if count == 0 {
                return if response.is_empty()
                    || expected_length.is_some_and(|expected| expected != response.len())
                {
                    Err(Error::InvalidResponse)
                } else {
                    Ok(response)
                };
            }
            if response.len() + count > MAX_RESPONSE_BYTES {
                return Err(Error::InvalidResponse);
            }
            // SAFETY: READ_COMPLETE and the native call have both finished.
            // The mutex publication orders native writes before this read;
            // no subsequent read starts until the copy has completed.
            response.extend_from_slice(unsafe { &(&*self.state.read.0.get())[..count] });
        }
    }

    fn reset_completion(&self) {
        self.state
            .transfer
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .completion = None;
    }

    /// Wait at most 60 seconds for native handle destruction. Timeout does
    /// not free callback state or return its capacity slot prematurely.
    pub fn close(mut self, deadline: Instant) -> Result<(), Error> {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() || remaining > Duration::from_secs(60) {
            return Err(Error::InvalidDeadline);
        }
        if let Some(request) = self.request.take() {
            request.close()?;
        }
        let mut closed = self
            .state
            .closed
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        while !*closed {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(Error::Timeout);
            }
            closed = self
                .state
                .changed
                .wait_timeout(closed, remaining)
                .unwrap_or_else(|poison| poison.into_inner())
                .0;
        }
        Ok(())
    }
}
impl Drop for PreparedRequest {
    fn drop(&mut self) {
        // If closing fails, the native context deliberately retains parents
        // and its slot. Repeated failures cannot exceed the eight-slot budget.
        if let Some(request) = self.request.take() {
            let _ = request.close();
        }
    }
}
fn option(handle: &Handle, name: u32, bytes: &[u8]) -> Result<(), Error> {
    unsafe { WinHttpSetOption(Some(handle.0), name, Some(bytes)) }.map_err(|_| Error::Native)
}

fn validate_response_headers(handle: *mut c_void) -> Result<Option<usize>, Error> {
    let mut status = 0u32;
    let mut length = 4u32;
    // SAFETY: header queries are synchronous and have fixed live output buffers.
    unsafe {
        WinHttpQueryHeaders(
            handle,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            None,
            Some((&mut status as *mut u32).cast()),
            &mut length,
            std::ptr::null_mut(),
        )
    }
    .map_err(|_| Error::InvalidResponse)?;
    if status != 200 || length != 4 {
        return Err(Error::InvalidResponse);
    }
    let content_type =
        single_header(handle, WINHTTP_QUERY_CONTENT_TYPE)?.ok_or(Error::InvalidResponse)?;
    let mut parts = content_type.split(';');
    if !parts
        .next()
        .unwrap_or_default()
        .trim()
        .eq_ignore_ascii_case("application/json")
    {
        return Err(Error::InvalidResponse);
    }
    if let Some(charset) = parts.next() {
        if !charset.trim().eq_ignore_ascii_case("charset=utf-8") || parts.next().is_some() {
            return Err(Error::InvalidResponse);
        }
    }
    if single_header(handle, WINHTTP_QUERY_CONTENT_ENCODING)?
        .is_some_and(|value| !value.eq_ignore_ascii_case("identity"))
    {
        return Err(Error::InvalidResponse);
    }
    let mut expected_length = None;
    if let Some(length) = single_header(handle, WINHTTP_QUERY_CONTENT_LENGTH)? {
        if length.is_empty()
            || !length.bytes().all(|byte| byte.is_ascii_digit())
            || length
                .parse::<usize>()
                .ok()
                .is_none_or(|size| size == 0 || size > MAX_RESPONSE_BYTES)
        {
            return Err(Error::InvalidResponse);
        }
        expected_length = length.parse().ok();
    }
    if let Some(encoding) = single_header(handle, WINHTTP_QUERY_TRANSFER_ENCODING)? {
        if expected_length.is_some() || !encoding.eq_ignore_ascii_case("chunked") {
            return Err(Error::InvalidResponse);
        }
    }
    Ok(expected_length)
}

fn single_header(handle: *mut c_void, name: u32) -> Result<Option<String>, Error> {
    let mut index = 0;
    let mut value = None;
    for _ in 0..2 {
        let mut buffer = [0u16; 128];
        let mut bytes = std::mem::size_of_val(&buffer) as u32;
        // SAFETY: this synchronous query receives a fixed, sized UTF-16 buffer.
        let result = unsafe {
            WinHttpQueryHeaders(
                handle,
                name,
                None,
                Some(buffer.as_mut_ptr().cast()),
                &mut bytes,
                &mut index,
            )
        };
        match result {
            Err(error)
                if error.code()
                    == windows::core::HRESULT::from_win32(ERROR_WINHTTP_HEADER_NOT_FOUND) =>
            {
                return Ok(value)
            }
            Err(_) => return Err(Error::InvalidResponse),
            Ok(())
                if value.is_some()
                    || bytes as usize >= std::mem::size_of_val(&buffer)
                    || !bytes.is_multiple_of(2) =>
            {
                return Err(Error::InvalidResponse)
            }
            Ok(()) => {
                value = Some(
                    String::from_utf16(&buffer[..bytes as usize / 2])
                        .map_err(|_| Error::InvalidResponse)?,
                )
            }
        }
    }
    Err(Error::InvalidResponse)
}
fn validate(host: &str, port: u16, path: &str) -> Result<(), Error> {
    let ip = host.parse::<std::net::IpAddr>().is_ok();
    let dns = !host.is_empty()
        && host.len() <= 253
        && host.split('.').all(|label| {
            (1..=63).contains(&label.len())
                && label
                    .as_bytes()
                    .first()
                    .is_some_and(u8::is_ascii_alphanumeric)
                && label
                    .as_bytes()
                    .last()
                    .is_some_and(u8::is_ascii_alphanumeric)
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        });
    if port == 0
        || (!ip && !dns)
        || path.len() > 2048
        || !path.starts_with('/')
        || path.contains("//")
        || !path.split('/').skip(1).all(|segment| {
            segment != "."
                && segment != ".."
                && segment.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~')
                })
        })
    {
        return Err(Error::InvalidDestination);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    static NATIVE_TEST_LOCK: Mutex<()> = Mutex::new(());
    #[test]
    fn rejects_ambiguous_destinations_before_native_allocation() {
        for (host, port, path) in [
            ("https://example.test", 443, "/"),
            ("example.test", 0, "/"),
            ("example.test", 443, "/../x"),
            ("example.test", 443, "/v1?key=x"),
        ] {
            assert!(matches!(
                PreparedRequest::new(host, port, path),
                Err(Error::InvalidDestination)
            ));
        }
    }
    #[test]
    fn native_https_lifetime_is_bounded_and_cleanup_releases_capacity() {
        let _guard = NATIVE_TEST_LOCK.lock().unwrap();
        let mut held = Vec::new();
        for _ in 0..CAPACITY {
            held.push(PreparedRequest::new("example.test", 443, "/v1/chat/completions").unwrap());
        }
        assert!(matches!(
            PreparedRequest::new("example.test", 443, "/"),
            Err(Error::Capacity)
        ));
        for request in held {
            request
                .close(Instant::now() + Duration::from_secs(2))
                .unwrap();
        }
        PreparedRequest::new("example.test", 443, "/")
            .unwrap()
            .close(Instant::now() + Duration::from_secs(2))
            .unwrap();
        let request = PreparedRequest::new("example.test", 443, "/").unwrap();
        let lifetime = Arc::downgrade(&request.state);
        drop(request);
        let deadline = Instant::now() + Duration::from_secs(2);
        while lifetime.upgrade().is_some() {
            assert!(
                Instant::now() < deadline,
                "last callback did not release request context"
            );
            std::thread::yield_now();
        }
        println!("native asynchronous HTTPS preparation and final callback cleanup completed; no request sent");
    }

    #[test]
    fn native_send_completes_and_tls_handshake_cancellation_releases_context() {
        use std::io::Read;
        use std::net::TcpListener;
        use std::sync::atomic::AtomicBool;
        let _guard = NATIVE_TEST_LOCK.lock().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            listener.set_nonblocking(true).unwrap();
            let limit = Instant::now() + Duration::from_secs(3);
            let mut stream = loop {
                if let Ok((stream, _)) = listener.accept() {
                    break stream;
                }
                assert!(Instant::now() < limit);
                std::thread::sleep(Duration::from_millis(1));
            };
            // Windows can inherit the listener's nonblocking mode. Restore
            // blocking reads so the timeout covers fragmented native writes.
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = Vec::new();
            let mut chunk = [0u8; 1024];
            let end = loop {
                let count = stream.read(&mut chunk).unwrap();
                assert!(count > 0);
                request.extend_from_slice(&chunk[..count]);
                if let Some(end) = request.windows(4).position(|part| part == b"\r\n\r\n") {
                    break end + 4;
                }
            };
            let headers = std::str::from_utf8(&request[..end]).unwrap();
            let length: usize = headers
                .lines()
                .find_map(|line| {
                    let (key, value) = line.split_once(':')?;
                    key.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse().unwrap())
                })
                .unwrap();
            while request.len() < end + length {
                let count = stream.read(&mut chunk).unwrap();
                assert!(count > 0);
                request.extend_from_slice(&chunk[..count]);
            }
            request
        });
        // Plain HTTP is used only by this private test constructor on a
        // numeric loopback address; the public constructor always uses TLS.
        let mut request =
            PreparedRequest::build("127.0.0.1", port, "/convert", WINHTTP_OPEN_REQUEST_FLAGS(0))
                .unwrap();
        let result = request.send(
            "synthetic-key",
            br#"{"text":"fixture"}"#.to_vec(),
            Instant::now() + Duration::from_secs(2),
            &|| false,
        );
        let captured = server.join().unwrap();
        assert_eq!(result, Ok(()));
        let captured = std::str::from_utf8(&captured).unwrap();
        assert!(captured.starts_with("POST /convert HTTP/1.1\r\n"));
        assert!(captured.contains("Authorization: Bearer synthetic-key\r\n"));
        assert!(captured.ends_with(r#"{"text":"fixture"}"#));
        assert_eq!(
            request.send(
                "synthetic-key",
                vec![1],
                Instant::now() + Duration::from_secs(1),
                &|| false
            ),
            Err(Error::InvalidState)
        );
        request
            .close(Instant::now() + Duration::from_secs(2))
            .unwrap();

        for should_cancel in [true, false] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            let cancelled = Arc::new(AtomicBool::new(false));
            let signal = cancelled.clone();
            let (release, wait_release) = std::sync::mpsc::channel();
            let server = std::thread::spawn(move || {
                listener.set_nonblocking(true).unwrap();
                let limit = Instant::now() + Duration::from_secs(3);
                let _stream = loop {
                    if let Ok((stream, _)) = listener.accept() {
                        break stream;
                    }
                    assert!(Instant::now() < limit);
                    std::thread::sleep(Duration::from_millis(1));
                };
                signal.store(should_cancel, Ordering::Release);
                let _ = wait_release.recv_timeout(Duration::from_secs(3));
            });
            let mut request = PreparedRequest::new("127.0.0.1", port, "/convert").unwrap();
            let lifetime = Arc::downgrade(&request.state);
            let result = request.send(
                "synthetic-key",
                vec![1; 4096],
                Instant::now()
                    + if should_cancel {
                        Duration::from_secs(2)
                    } else {
                        Duration::from_millis(150)
                    },
                &|| cancelled.load(Ordering::Acquire),
            );
            let cleanup = request.close(Instant::now() + Duration::from_secs(2));
            let _ = release.send(());
            server.join().unwrap();
            assert_eq!(
                result,
                Err(if should_cancel {
                    Error::Cancelled
                } else {
                    Error::Timeout
                })
            );
            assert_eq!(cleanup, Ok(()));
            let limit = Instant::now() + Duration::from_secs(2);
            while lifetime.upgrade().is_some() {
                assert!(Instant::now() < limit);
                std::thread::yield_now();
            }
        }
        println!("native send completion and pending TLS cancellation checked on loopback");
    }

    // All payloads and credentials in these fixtures are synthetic. The only
    // non-TLS constructor is private and used exclusively on numeric loopback.
    fn receive_fixture(response: Vec<u8>, stall: bool, cancel: bool) -> Result<Vec<u8>, Error> {
        use std::io::{Read, Write};
        use std::sync::atomic::AtomicBool;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let (release, wait_release) = std::sync::mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let signal = cancelled.clone();
        let server = std::thread::spawn(move || {
            let limit = Instant::now() + Duration::from_secs(3);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < limit);
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    Err(error) => panic!("fixture accept failed: {error}"),
                }
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut input = Vec::new();
            let mut chunk = [0; 1024];
            loop {
                let count = stream.read(&mut chunk).unwrap();
                assert!(count > 0 && input.len() + count < 8192);
                input.extend_from_slice(&chunk[..count]);
                if input
                    .windows(4)
                    .position(|part| part == b"\r\n\r\n")
                    .is_some_and(|end| input.len() > end + 4)
                {
                    break;
                }
            }
            for part in response.chunks(511) {
                if stream.write_all(part).is_err() {
                    break;
                }
            }
            if stall {
                // Give the client time to enter the pending native receive.
                // No socket close releases it before cancellation/deadline.
                std::thread::sleep(Duration::from_millis(75));
                signal.store(cancel, Ordering::Release);
                let _ = wait_release.recv_timeout(Duration::from_secs(3));
            }
        });
        let mut request =
            PreparedRequest::build("127.0.0.1", port, "/convert", WINHTTP_OPEN_REQUEST_FLAGS(0))
                .unwrap();
        let lifetime = Arc::downgrade(&request.state);
        let sent = request.send(
            "synthetic-key",
            vec![b'x'],
            Instant::now()
                + if stall && !cancel {
                    Duration::from_millis(300)
                } else {
                    Duration::from_secs(2)
                },
            &|| cancelled.load(Ordering::Acquire),
        );
        let result = match sent {
            Ok(()) => request.receive(&|| cancelled.load(Ordering::Acquire)),
            Err(error) => {
                drop(request);
                Err(error)
            }
        };
        let _ = release.send(());
        server.join().unwrap();
        let limit = Instant::now() + Duration::from_secs(2);
        while lifetime.upgrade().is_some() {
            assert!(
                Instant::now() < limit,
                "receive did not release native context"
            );
            std::thread::yield_now();
        }
        result
    }

    #[test]
    fn native_receive_accepts_fragmented_and_chunked_json_with_exact_limit() {
        let _guard = NATIVE_TEST_LOCK.lock().unwrap();
        let body = vec![b'a'; MAX_RESPONSE_BYTES];
        let mut response = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).into_bytes();
        response.extend_from_slice(&body);
        assert_eq!(receive_fixture(response, false, false), Ok(body));
        assert_eq!(receive_fixture(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n3\r\n{\"x\r\n4\r\n\":1}\r\n0\r\n\r\n".to_vec(), false, false), Ok(br#"{"x":1}"#.to_vec()));
    }

    #[test]
    fn native_receive_rejects_status_format_duplicates_truncation_and_overflow() {
        let _guard = NATIVE_TEST_LOCK.lock().unwrap();
        for (index, response) in [
            "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/redirect\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}",
            "HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}",
            "HTTP/1.1 429 Limited\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}",
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 2\r\n\r\n{}",
            "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}",
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}",
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Encoding: gzip\r\nContent-Length: 2\r\n\r\n{}",
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 65537\r\n\r\n{}",
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 0\r\n\r\n",
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 20\r\n\r\n{}",
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nContent-Length: 2\r\n\r\n{}",
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n{}\r\n0\r\n\r\n",
            "HTTP/1.1 200 OK\r\nContent-Type: application/json; charset=shift_jis\r\nContent-Length: 2\r\n\r\n{}",
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n3\r\n{}",
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n{}\r\n",
        ].into_iter().enumerate() {
            assert!(receive_fixture(response.as_bytes().to_vec(), false, false).is_err(), "invalid response case {index} was accepted");
        }
        let mut response =
            b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n"
                .to_vec();
        response.extend(vec![b'a'; MAX_RESPONSE_BYTES + 1]);
        assert_eq!(
            receive_fixture(response, false, false),
            Err(Error::InvalidResponse)
        );
        let response = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nX-Fill: {}\r\nContent-Length: 2\r\n\r\n{{}}", "x".repeat(17 * 1024));
        assert!(receive_fixture(response.into_bytes(), false, false).is_err());
    }

    #[test]
    fn pending_headers_or_body_observe_cancellation_and_original_deadline() {
        let _guard = NATIVE_TEST_LOCK.lock().unwrap();
        for response in [
            Vec::new(),
            b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 20000\r\n\r\nx"
                .to_vec(),
        ] {
            assert_eq!(
                receive_fixture(response.clone(), true, true),
                Err(Error::Cancelled)
            );
            assert_eq!(receive_fixture(response, true, false), Err(Error::Timeout));
        }
    }
}

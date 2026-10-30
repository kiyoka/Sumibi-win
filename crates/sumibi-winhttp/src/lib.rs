//! Native asynchronous HTTPS request lifetime. No send operation is exposed.
#![cfg(windows)]
#![deny(unsafe_op_in_unsafe_fn)]
use std::ffi::c_void;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Condvar, Mutex,
};
use std::time::{Duration, Instant};
use windows::core::{w, PCWSTR};
use windows::Win32::Networking::WinHttp::*;

const CAPACITY: usize = 8;
static ACTIVE: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidDestination,
    Capacity,
    Native,
    Cleanup,
    Timeout,
    InvalidDeadline,
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
    // Connection must close before session; both outlive the last callback.
    _connection: Handle,
    _session: Handle,
    _permit: Arc<Permit>,
}
unsafe extern "system" fn callback(
    _: *mut c_void,
    context: usize,
    status: u32,
    _: *mut c_void,
    _: u32,
) {
    if context == 0 || status != WINHTTP_CALLBACK_STATUS_HANDLE_CLOSING {
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
}
impl PreparedRequest {
    pub fn new(host: &str, port: u16, path: &str) -> Result<Self, Error> {
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
                    WINHTTP_FLAG_SECURE_DEFAULTS,
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
                    WINHTTP_FLAG_SECURE,
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
            WINHTTP_OPTION_ENABLE_FEATURE,
            &WINHTTP_ENABLE_SSL_REVOCATION.to_ne_bytes(),
        )?;
        let state = Arc::new(Context {
            closed: Mutex::new(false),
            changed: Condvar::new(),
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
                WINHTTP_CALLBACK_STATUS_HANDLE_CREATED | WINHTTP_CALLBACK_STATUS_HANDLE_CLOSING,
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
        })
    }

    /// Wait at most 60 seconds for native handle destruction. Timeout does
    /// not free callback state or return its capacity slot prematurely.
    pub fn close(mut self, deadline: Instant) -> Result<(), Error> {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() || remaining > Duration::from_secs(60) {
            return Err(Error::InvalidDeadline);
        }
        self.request.take().unwrap().close()?;
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
}

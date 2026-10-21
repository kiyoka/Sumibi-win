use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{GetLastError, LocalFree, HANDLE, HLOCAL};
use windows::Win32::Security::Cryptography::{BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Os(u32),
    WrongPeer,
    UnsupportedHost,
    UnsupportedToken(u32),
    UnsupportedIntegrity,
    UnsupportedElevation,
    UnsupportedAppContainer,
    UnsupportedImpersonation,
    Timeout,
    Cancelled,
    Disconnected,
    Capacity,
    InvalidArgument,
    Closed,
    Retired,
}
pub type Result<T> = std::result::Result<T, Error>;
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "desktop IPC: {self:?}")
    }
}
impl std::error::Error for Error {}
impl From<windows::core::Error> for Error {
    fn from(error: windows::core::Error) -> Self {
        Self::Os(error.code().0 as u32)
    }
}
pub(crate) fn last_error() -> Error {
    // SAFETY: immediately read the calling thread's fixed OS classification.
    Error::Os(unsafe { GetLastError() }.0)
}
pub(crate) fn raw(handle: &OwnedHandle) -> HANDLE {
    HANDLE(handle.as_raw_handle())
}

/// Caller transfers a valid, non-pseudo, uniquely owned Win32 handle.
pub(crate) unsafe fn own(handle: HANDLE) -> OwnedHandle {
    // SAFETY: enforced by the caller, including successful handle creation.
    unsafe { OwnedHandle::from_raw_handle(handle.0) }
}

pub(crate) struct Local(pub(crate) *mut std::ffi::c_void);
impl Drop for Local {
    fn drop(&mut self) {
        // SAFETY: each wrapper owns one allocation returned by a LocalAlloc API.
        unsafe {
            let _ = LocalFree(Some(HLOCAL(self.0)));
        }
    }
}

/// Public correlation value, not a secret or authentication credential.
/// Production launch binding still requires pinned kernel process handles.
#[derive(Clone)]
pub struct Nonce(u128);
impl Nonce {
    pub fn random() -> Result<Self> {
        let mut bytes = [0u8; 16];
        // SAFETY: live output buffer; system RNG needs no provider handle.
        unsafe {
            BCryptGenRandom(None, &mut bytes, BCRYPT_USE_SYSTEM_PREFERRED_RNG).ok()?;
        }
        let value = u128::from_le_bytes(bytes);
        if value == 0 {
            return Err(Error::InvalidArgument);
        }
        Ok(Self(value))
    }
    pub fn from_bootstrap(text: &str) -> Result<Self> {
        if text.len() != 32
            || !text
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(Error::InvalidArgument);
        }
        let value = u128::from_str_radix(text, 16).map_err(|_| Error::InvalidArgument)?;
        if value == 0 {
            return Err(Error::InvalidArgument);
        }
        Ok(Self(value))
    }
    /// For the explicit bootstrap channel only. Do not log pipe names or IDs.
    pub fn bootstrap(&self) -> String {
        format!("{:032x}", self.0)
    }
    pub fn id(&self) -> sumibi_protocol::message::Id {
        sumibi_protocol::message::Id::new(self.0).expect("nonzero nonce")
    }
    pub(crate) fn pipe_name(&self) -> Vec<u16> {
        format!("\\\\.\\pipe\\Sumibi-peer-{:032x}", self.0)
            .encode_utf16()
            .chain(Some(0))
            .collect()
    }
}

pub(crate) fn sid_text(sid: windows::Win32::Security::PSID) -> Result<String> {
    let mut text = PWSTR::null();
    // SAFETY: SID came from a validated kernel token buffer; free API output.
    unsafe {
        windows::Win32::Security::Authorization::ConvertSidToStringSidW(sid, &mut text)?;
        let _allocation = Local(text.0.cast());
        PCWSTR(text.0)
            .to_string()
            .map_err(|_| Error::UnsupportedHost)
    }
}

use crate::support::{last_error, own, raw, sid_text, Error, Result};
use std::os::windows::ffi::OsStringExt;
use std::os::windows::io::{AsHandle, BorrowedHandle, OwnedHandle};
use std::path::PathBuf;
use windows::core::PWSTR;
use windows::Win32::Foundation::*;
use windows::Win32::Security::*;
use windows::Win32::System::Pipes::{GetNamedPipeClientProcessId, GetNamedPipeServerProcessId};
use windows::Win32::System::Threading::*;

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Identity {
    user: Vec<u8>,
    logon: Vec<u8>,
    pub(crate) logon_text: String,
    session: u32,
    integrity: u32,
    app_container: bool,
    elevated: bool,
}
impl Identity {
    fn supported(&self) -> bool {
        !self.app_container && !self.elevated && self.integrity == 0x2000
    }
    fn same_desktop(&self, other: &Self) -> bool {
        self.supported() && other.supported() && self == other
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Stamp {
    pid: u32,
    created: u64,
}

/// Pins one kernel process object, its creation time and initial token identity.
/// Construct from a trusted launch handle, never an arbitrary advertised PID.
pub struct PinnedProcess {
    handle: OwnedHandle,
    stamp: Stamp,
    identity: Identity,
}
impl PinnedProcess {
    pub fn current() -> Result<Self> {
        primary_thread()?;
        // SAFETY: pseudo handle is borrowed only for DuplicateHandle.
        unsafe { Self::duplicate(GetCurrentProcess()) }
    }
    pub fn from_launch_handle(handle: BorrowedHandle<'_>) -> Result<Self> {
        use std::os::windows::io::AsRawHandle;
        // SAFETY: caller borrows a live process launch handle for this call.
        unsafe { Self::duplicate(HANDLE(handle.as_raw_handle())) }
    }
    unsafe fn duplicate(source: HANDLE) -> Result<Self> {
        let mut duplicated = HANDLE::default();
        // SAFETY: duplicate into this process with only query/synchronize rights.
        unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                source,
                GetCurrentProcess(),
                &mut duplicated,
                (PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE).0,
                false,
                DUPLICATE_HANDLE_OPTIONS(0),
            )?;
            Self::owned(own(duplicated))
        }
    }
    fn owned(handle: OwnedHandle) -> Result<Self> {
        let stamp = stamp(raw(&handle))?;
        let identity = identity(raw(&handle))?;
        if identity.app_container {
            return Err(Error::UnsupportedAppContainer);
        }
        if identity.elevated {
            return Err(Error::UnsupportedElevation);
        }
        if identity.integrity != 0x2000 {
            return Err(Error::UnsupportedIntegrity);
        }
        let peer = Self {
            handle,
            stamp,
            identity,
        };
        peer.alive()?;
        Ok(peer)
    }
    /// A narrow, non-inheritable duplicate for use by a restricted launch list.
    /// The launcher chooses inheritance only for the actual CreateProcess call.
    pub fn as_handle(&self) -> BorrowedHandle<'_> {
        self.handle.as_handle()
    }
    /// Duplicate the same already pinned process, retaining its original
    /// identity and creation-time checks. Never construct from a bare PID.
    pub fn try_clone(&self) -> Result<Self> {
        self.alive()?;
        let duplicate = Self::from_launch_handle(self.as_handle())?;
        self.require_same(&duplicate)?;
        Ok(duplicate)
    }
    /// Observe the Win32 image path of this already pinned live process.
    /// This synchronous metadata query belongs off the TSF owner thread. The
    /// path is not proof of signer, protected placement, or file identity:
    /// the filesystem entry may have changed since the process was launched.
    pub fn observed_image_path(&self) -> Result<PathBuf> {
        self.require_same(self)?;
        let mut buffer = vec![0u16; 32768];
        let mut length = buffer.len() as u32;
        // SAFETY: the pinned handle has query rights and the output buffer
        // remains valid for the bounded query.
        unsafe {
            QueryFullProcessImageNameW(
                self.raw(),
                PROCESS_NAME_WIN32,
                PWSTR(buffer.as_mut_ptr()),
                &mut length,
            )?;
        }
        let path = image_path_from_buffer(&buffer, length as usize)?;
        self.require_same(self)?;
        Ok(path)
    }
    pub(crate) fn require_same(&self, other: &Self) -> Result<()> {
        primary_thread()?;
        self.alive()?;
        other.alive()?;
        if self.stamp != other.stamp
            || self.identity != other.identity
            || identity(self.raw())? != self.identity
            || !self
                .identity
                .same_desktop(&identity(unsafe { GetCurrentProcess() })?)
        {
            return Err(Error::WrongPeer);
        }
        self.alive()?;
        other.alive()
    }
    pub(crate) fn raw(&self) -> HANDLE {
        raw(&self.handle)
    }
    pub(crate) fn identity(&self) -> &Identity {
        &self.identity
    }
    pub(crate) fn alive(&self) -> Result<()> {
        // SAFETY: synchronize access is held on the exact pinned process object.
        match unsafe { WaitForSingleObject(self.raw(), 0) } {
            WAIT_TIMEOUT => Ok(()),
            WAIT_OBJECT_0 => Err(Error::Disconnected),
            _ => Err(last_error()),
        }
    }
    pub(crate) fn verify(&self, pipe: HANDLE, server_side: bool) -> Result<()> {
        primary_thread()?;
        self.alive()?;
        let mut pid = 0;
        // SAFETY: caller owns the connected local pipe; PID is kernel supplied.
        unsafe {
            if server_side {
                GetNamedPipeClientProcessId(pipe, &mut pid)?;
            } else {
                GetNamedPipeServerProcessId(pipe, &mut pid)?;
            }
        }
        if pid == 0 || pid != self.stamp.pid {
            return Err(Error::WrongPeer);
        }
        // The pinned object must still be alive while opening the reported PID.
        let observed = unsafe {
            own(OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                false,
                pid,
            )?)
        };
        same_stamp(self.stamp, stamp(raw(&observed))?)?;
        if identity(raw(&observed))? != self.identity
            || !self
                .identity
                .same_desktop(&identity(unsafe { GetCurrentProcess() })?)
        {
            return Err(Error::WrongPeer);
        }
        self.alive()?;
        Ok(())
    }
}

fn image_path_from_buffer(buffer: &[u16], length: usize) -> Result<PathBuf> {
    if length == 0 || length >= buffer.len() || buffer[..length].contains(&0) {
        return Err(Error::InvalidMessage);
    }
    let path = PathBuf::from(std::ffi::OsString::from_wide(&buffer[..length]));
    if !path.is_absolute() {
        return Err(Error::InvalidMessage);
    }
    Ok(path)
}

fn primary_thread() -> Result<()> {
    let mut token = HANDLE::default();
    // SAFETY: inspect the effective thread without changing its token. Reject
    // even identity-only impersonation instead of relying on the process token.
    match unsafe { OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, true, &mut token) } {
        Ok(()) => {
            let _token = unsafe { own(token) };
            Err(Error::UnsupportedImpersonation)
        }
        Err(error) if error.code() == windows::core::HRESULT::from_win32(ERROR_NO_TOKEN.0) => {
            Ok(())
        }
        Err(error) => Err(Error::from(error)),
    }
}

fn same_stamp(expected: Stamp, observed: Stamp) -> Result<()> {
    if expected.pid == 0 || expected.created == 0 || expected != observed {
        Err(Error::WrongPeer)
    } else {
        Ok(())
    }
}

fn stamp(process: HANDLE) -> Result<Stamp> {
    let mut created = FILETIME::default();
    let mut exited = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    // SAFETY: valid process query handle and fully initialized output slots.
    unsafe {
        let pid = GetProcessId(process);
        if pid == 0 {
            return Err(last_error());
        }
        GetProcessTimes(process, &mut created, &mut exited, &mut kernel, &mut user)?;
        Ok(Stamp {
            pid,
            created: (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime),
        })
    }
}

fn token_info(token: HANDLE, kind: TOKEN_INFORMATION_CLASS) -> Result<Vec<usize>> {
    let mut required = 0;
    // SAFETY: size query then an aligned, bounded kernel-token output buffer.
    unsafe {
        let error = GetTokenInformation(token, kind, None, 0, &mut required)
            .err()
            .ok_or(Error::InvalidArgument)?;
        if ![ERROR_INSUFFICIENT_BUFFER, ERROR_BAD_LENGTH]
            .into_iter()
            .any(|code| error.code() == windows::core::HRESULT::from_win32(code.0))
            || required == 0
            || required > 65536
        {
            return Err(Error::UnsupportedToken(kind.0 as u32));
        }
        let mut data = vec![0usize; (required as usize).div_ceil(std::mem::size_of::<usize>())];
        let mut used = 0;
        GetTokenInformation(
            token,
            kind,
            Some(data.as_mut_ptr().cast()),
            required,
            &mut used,
        )?;
        if used == 0 || used > required {
            return Err(Error::UnsupportedToken(kind.0 as u32));
        }
        Ok(data)
    }
}

fn sid_bytes(sid: PSID) -> Result<Vec<u8>> {
    // SAFETY: SID is kernel supplied and remains inside the live token buffer.
    unsafe {
        if !IsValidSid(sid).as_bool() {
            return Err(Error::UnsupportedHost);
        }
        let length = GetLengthSid(sid) as usize;
        if length == 0 || length > 256 {
            return Err(Error::UnsupportedHost);
        }
        Ok(std::slice::from_raw_parts(sid.0.cast::<u8>(), length).to_vec())
    }
}

pub(crate) fn identity(process: HANDLE) -> Result<Identity> {
    let mut token = HANDLE::default();
    // SAFETY: read only a process token; all pointer-bearing buffers remain live.
    unsafe {
        OpenProcessToken(process, TOKEN_QUERY, &mut token)?;
        let token = own(token);
        let user = token_info(raw(&token), TokenUser)?;
        let user = sid_bytes((*user.as_ptr().cast::<TOKEN_USER>()).User.Sid)?;
        let groups = token_info(raw(&token), TokenLogonSid)?;
        let groups_ref = &*groups.as_ptr().cast::<TOKEN_GROUPS>();
        // SE_GROUP_LOGON_ID from winnt.h; both high attribute bits are required.
        if groups_ref.GroupCount != 1 || groups_ref.Groups[0].Attributes & 0xc0000000 != 0xc0000000
        {
            return Err(Error::UnsupportedToken(TokenLogonSid.0 as u32));
        }
        let logon = sid_bytes(groups_ref.Groups[0].Sid)?;
        let logon_text = sid_text(groups_ref.Groups[0].Sid)?;
        let integrity = token_info(raw(&token), TokenIntegrityLevel)?;
        let sid = (*integrity.as_ptr().cast::<TOKEN_MANDATORY_LABEL>())
            .Label
            .Sid;
        sid_bytes(sid)?;
        let count = *GetSidSubAuthorityCount(sid);
        if count == 0 {
            return Err(Error::UnsupportedHost);
        }
        let integrity = *GetSidSubAuthority(sid, u32::from(count - 1));
        let session = token_info(raw(&token), TokenSessionId)?;
        let app_container = token_info(raw(&token), TokenIsAppContainer)?;
        let elevated = token_info(raw(&token), TokenElevation)?;
        Ok(Identity {
            user,
            logon,
            logon_text,
            session: *(session.as_ptr().cast::<u32>()),
            integrity,
            app_container: *(app_container.as_ptr().cast::<u32>()) != 0,
            elevated: (*elevated.as_ptr().cast::<TOKEN_ELEVATION>()).TokenIsElevated != 0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::io::AsHandle;
    use std::process::{Command, Stdio};
    fn normal() -> Identity {
        Identity {
            user: vec![1],
            logon: vec![2],
            logon_text: "synthetic".into(),
            session: 1,
            integrity: 0x2000,
            app_container: false,
            elevated: false,
        }
    }
    #[test]
    fn every_token_boundary_is_fail_closed() {
        let expected = normal();
        assert!(expected.same_desktop(&normal()));
        for field in 0..6 {
            let mut other = normal();
            match field {
                0 => other.user[0] += 1,
                1 => other.logon[0] += 1,
                2 => other.session += 1,
                3 => other.integrity = 0x3000,
                4 => other.app_container = true,
                _ => other.elevated = true,
            }
            assert!(!expected.same_desktop(&other));
        }
    }
    #[test]
    fn pid_reuse_requires_creation_time_equality() {
        let expected = Stamp {
            pid: 42,
            created: 100,
        };
        assert_eq!(same_stamp(expected, expected), Ok(()));
        for observed in [
            Stamp {
                pid: 42,
                created: 101,
            },
            Stamp {
                pid: 43,
                created: 100,
            },
            Stamp {
                pid: 0,
                created: 100,
            },
        ] {
            assert_eq!(same_stamp(expected, observed), Err(Error::WrongPeer));
        }
    }
    #[test]
    fn image_path_rejects_invalid_length_embedded_nul_and_relative_names() {
        let path: Vec<u16> = r"C:\日本語\host.exe"
            .encode_utf16()
            .chain(Some(0))
            .collect();
        assert!(image_path_from_buffer(&path, path.len() - 1).is_ok());
        assert_eq!(image_path_from_buffer(&path, 0), Err(Error::InvalidMessage));
        assert_eq!(
            image_path_from_buffer(&path, path.len()),
            Err(Error::InvalidMessage)
        );
        let nul = [b'C' as u16, 0, b'a' as u16, 0];
        assert_eq!(image_path_from_buffer(&nul, 3), Err(Error::InvalidMessage));
        let relative: Vec<u16> = "host.exe".encode_utf16().chain(Some(0)).collect();
        assert_eq!(
            image_path_from_buffer(&relative, relative.len() - 1),
            Err(Error::InvalidMessage)
        );
    }

    #[test]
    fn pinned_image_observes_current_process_without_authorizing_it() {
        let process = match PinnedProcess::current() {
            Ok(process) => process,
            Err(
                Error::UnsupportedAppContainer
                | Error::UnsupportedElevation
                | Error::UnsupportedIntegrity,
            ) => {
                eprintln!("image observation skipped: unsupported caller");
                return;
            }
            Err(error) => panic!("failed to pin current process: {error:?}"),
        };
        let observed = process.observed_image_path().unwrap();
        let current = std::env::current_exe().unwrap();
        assert_eq!(observed, current);
        println!("pinned current process image observed");
    }

    #[test]
    fn pinned_image_uses_child_handle_and_rejects_exit() {
        if matches!(
            PinnedProcess::current(),
            Err(Error::UnsupportedAppContainer
                | Error::UnsupportedElevation
                | Error::UnsupportedIntegrity)
        ) {
            eprintln!("child image observation skipped: unsupported caller");
            return;
        }
        let command_path = PathBuf::from(r"C:\Windows\System32\cmd.exe");
        let mut child = Command::new(&command_path)
            .args(["/d", "/q", "/c", "pause"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let pinned = PinnedProcess::from_launch_handle(child.as_handle()).unwrap();
        let result = pinned.observed_image_path();
        drop(child.stdin.take());
        let status = child.wait().unwrap();
        assert!(status.success());
        assert_eq!(result.unwrap(), command_path);
        assert_eq!(pinned.observed_image_path(), Err(Error::Disconnected));
        println!("pinned child image observed and rejected after exit");
    }
}

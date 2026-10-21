//! Disposable desktop pipe probe. Not a production broker or AppContainer bridge.
use std::ffi::c_void;
use windows::core::{Error, Result, HRESULT, PCWSTR, PWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Security::Authorization::*;
use windows::Win32::Security::*;
use windows::Win32::Storage::FileSystem::*;
use windows::Win32::System::Com::CoCreateGuid;
use windows::Win32::System::Pipes::*;
use windows::Win32::System::Threading::*;

struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: owns a valid, non-pseudo handle exactly once.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}
struct Local(*mut c_void);
impl Drop for Local {
    fn drop(&mut self) {
        // SAFETY: owns an allocation returned by a LocalAlloc-based Win32 API.
        unsafe {
            let _ = LocalFree(Some(HLOCAL(self.0)));
        }
    }
}

fn same_peer(actual: u32, expected: u32) -> Result<()> {
    if actual == 0 || expected == 0 || actual != expected {
        Err(E_ACCESSDENIED.into())
    } else {
        Ok(())
    }
}

pub(super) fn run() -> Result<()> {
    // SAFETY: disposable handles and fixed synthetic text only. Nonblocking
    // pipe operations never wait for an unknown peer or arbitrary input.
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)?;
        let token = Handle(token);
        // usize storage gives TOKEN_GROUPS/SID pointers their native alignment.
        let mut groups = [0usize; 512];
        let mut count = 0;
        GetTokenInformation(
            token.0,
            TokenLogonSid,
            Some(groups.as_mut_ptr().cast()),
            std::mem::size_of_val(&groups) as u32,
            &mut count,
        )?;
        if count < std::mem::size_of::<TOKEN_GROUPS>() as u32
            || count as usize > std::mem::size_of_val(&groups)
        {
            return Err(E_FAIL.into());
        }
        let group = &*groups.as_ptr().cast::<TOKEN_GROUPS>();
        if group.GroupCount != 1 {
            return Err(E_ACCESSDENIED.into());
        }
        let mut sid = PWSTR::null();
        ConvertSidToStringSidW(group.Groups[0].Sid, &mut sid)?;
        let _sid_allocation = Local(sid.0.cast());
        let descriptor: Vec<u16> = format!("D:P(A;;GA;;;{})", sid.to_string()?)
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let mut security = PSECURITY_DESCRIPTOR::default();
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            PCWSTR(descriptor.as_ptr()),
            SDDL_REVISION_1,
            &mut security,
            None,
        )?;
        let _security_allocation = Local(security.0);
        let attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: security.0,
            bInheritHandle: false.into(),
        };
        let name: Vec<u16> = format!("\\\\.\\pipe\\Sumibi-probe-{:?}", CoCreateGuid()?)
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let mode = PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_NOWAIT | PIPE_REJECT_REMOTE_CLIENTS;
        let server = CreateNamedPipeW(
            PCWSTR(name.as_ptr()),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
            mode,
            1,
            4096,
            4096,
            0,
            Some(&attributes),
        );
        if server.is_invalid() {
            return Err(Error::from_thread());
        }
        let server = Handle(server);
        // Refuse a second server, even if it knows the random pipe name.
        let duplicate = CreateNamedPipeW(
            PCWSTR(name.as_ptr()),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
            mode,
            1,
            4096,
            4096,
            0,
            Some(&attributes),
        );
        if !duplicate.is_invalid() {
            let _duplicate = Handle(duplicate);
            return Err(E_FAIL.into());
        }
        if let Err(error) = ConnectNamedPipe(server.0, None) {
            if error.code() != HRESULT::from_win32(ERROR_PIPE_LISTENING.0) {
                return Err(error);
            }
        }
        let client = Handle(CreateFileW(
            PCWSTR(name.as_ptr()),
            GENERIC_READ.0 | GENERIC_WRITE.0,
            FILE_SHARE_MODE(0),
            None,
            OPEN_EXISTING,
            FILE_FLAGS_AND_ATTRIBUTES(0),
            None,
        )?);
        if let Err(error) = ConnectNamedPipe(server.0, None) {
            if error.code() != HRESULT::from_win32(ERROR_PIPE_CONNECTED.0) {
                return Err(error);
            }
        }
        let mut client_pid = 0;
        let mut server_pid = 0;
        GetNamedPipeClientProcessId(server.0, &mut client_pid)?;
        GetNamedPipeServerProcessId(client.0, &mut server_pid)?;
        same_peer(client_pid, GetCurrentProcessId())?;
        same_peer(server_pid, GetCurrentProcessId())?;
        assert!(same_peer(client_pid, 0).is_err());
        assert!(same_peer(server_pid, server_pid.wrapping_add(1)).is_err());
        let encoded = sumibi_protocol::encode("synthetic pipe response")
            .map_err(|_| Error::from_hresult(E_FAIL))?;
        let mut written = 0;
        WriteFile(client.0, Some(&encoded), Some(&mut written), None)?;
        if written as usize != encoded.len() {
            return Err(E_FAIL.into());
        }
        let mut decoder = sumibi_protocol::Decoder::default();
        let mut buffer = [0; 7];
        let mut received = 0;
        loop {
            let mut read = 0;
            ReadFile(server.0, Some(&mut buffer), Some(&mut read), None)?;
            if read == 0 || received + read as usize > encoded.len() {
                return Err(E_FAIL.into());
            }
            received += read as usize;
            let part = decoder
                .decode(&buffer[..read as usize])
                .map_err(|_| Error::from_hresult(E_FAIL))?;
            if let Some(body) = part.body {
                if body != "synthetic pipe response" || received != encoded.len() {
                    return Err(E_FAIL.into());
                }
                break;
            }
        }
        decoder.finish().map_err(|_| Error::from_hresult(E_FAIL))?;
        DisconnectNamedPipe(server.0)?;
    }
    println!("Desktop pipe: explicit logon-SID DACL / remote rejection flag / first instance / kernel peer IDs / fragmented frame: OK");
    println!("Scope: same-process probe only; production peer authentication and AppContainer are not validated");
    Ok(())
}

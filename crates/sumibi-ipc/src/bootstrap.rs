//! Restricted broker launch. This is not per-user broker discovery or
//! executable signature verification. Callers must supply a trusted full path.
use crate::support::{own, raw, Error, Nonce, Result};
use crate::{PinnedProcess, Pipe, Stop};
use std::ffi::OsString;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsHandle, AsRawHandle, BorrowedHandle, OwnedHandle};
use std::path::Path;
use std::time::Instant;
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::System::Threading::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Disabled,
    DiagnosticFixed { delay_ms: u32 },
    DiagnosticRevoke,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Service {
    Once,
    Channel,
}
impl Service {
    fn argument(self) -> &'static str {
        match self {
            Self::Once => "serve",
            Self::Channel => "serve-channel",
        }
    }
}
impl Mode {
    fn argument(self) -> Result<String> {
        match self {
            Self::Disabled => Ok("disabled".into()),
            Self::DiagnosticFixed { delay_ms } if delay_ms <= 2000 => {
                Ok(format!("fixed:{delay_ms}"))
            }
            Self::DiagnosticRevoke => Ok("revoke".into()),
            _ => Err(Error::InvalidArgument),
        }
    }
}
struct Attributes {
    list: LPPROC_THREAD_ATTRIBUTE_LIST,
    _storage: Vec<usize>,
}
impl Attributes {
    fn new(handles: &[HANDLE]) -> Result<Self> {
        let mut bytes = 0;
        // SAFETY: aligned, bounded startup attribute storage and explicit list.
        unsafe {
            let error = InitializeProcThreadAttributeList(None, 1, None, &mut bytes)
                .err()
                .ok_or(Error::InvalidArgument)?;
            if error.code() != windows::core::HRESULT::from_win32(ERROR_INSUFFICIENT_BUFFER.0)
                || bytes == 0
                || bytes > 65536
            {
                return Err(Error::InvalidArgument);
            }
            let mut storage = vec![0usize; bytes.div_ceil(std::mem::size_of::<usize>())];
            let list = LPPROC_THREAD_ATTRIBUTE_LIST(storage.as_mut_ptr().cast());
            InitializeProcThreadAttributeList(Some(list), 1, None, &mut bytes)?;
            let result = Self {
                list,
                _storage: storage,
            };
            UpdateProcThreadAttribute(
                list,
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                Some(handles.as_ptr().cast()),
                std::mem::size_of_val(handles),
                None,
                None,
            )?;
            Ok(result)
        }
    }
}
impl Drop for Attributes {
    fn drop(&mut self) {
        unsafe {
            DeleteProcThreadAttributeList(self.list);
        }
    }
}
fn inheritable(source: BorrowedHandle<'_>) -> Result<OwnedHandle> {
    let mut handle = HANDLE::default();
    // SAFETY: only the explicit bootstrap handles are inherited by the child.
    unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            HANDLE(source.as_raw_handle()),
            GetCurrentProcess(),
            &mut handle,
            0,
            true,
            DUPLICATE_SAME_ACCESS,
        )?;
        Ok(own(handle))
    }
}

pub(crate) struct Launched {
    process: OwnedHandle,
    stop: Stop,
}
impl Launched {
    pub(crate) fn connect(
        path: &Path,
        mode: Mode,
        stop: &Stop,
        deadline: Instant,
    ) -> Result<(Self, Pipe, Nonce)> {
        Self::connect_service(path, mode, stop, deadline, Service::Once)
    }
    pub(crate) fn connect_channel(
        path: &Path,
        mode: Mode,
        stop: &Stop,
        deadline: Instant,
    ) -> Result<(Self, Pipe, Nonce)> {
        Self::connect_service(path, mode, stop, deadline, Service::Channel)
    }
    pub(crate) fn peer(&self) -> Result<PinnedProcess> {
        PinnedProcess::from_launch_handle(self.process.as_handle())
    }
    fn connect_service(
        path: &Path,
        mode: Mode,
        stop: &Stop,
        deadline: Instant,
        service: Service,
    ) -> Result<(Self, Pipe, Nonce)> {
        let path_text = path.to_str().ok_or(Error::InvalidArgument)?;
        if !path.is_absolute() || path_text.contains(['\0', '"']) {
            return Err(Error::InvalidArgument);
        }
        if stop.is_cancelled()? {
            return Err(Error::Cancelled);
        }
        if Instant::now() >= deadline {
            return Err(Error::Timeout);
        }
        let parent = PinnedProcess::current()?;
        let parent_handle = inheritable(parent.as_handle())?;
        let stop_handle = inheritable(stop.as_handle())?;
        let ready = unsafe { own(CreateEventW(None, true, false, None)?) };
        let ready_handle = inheritable(ready.as_handle())?;
        let handles = [raw(&parent_handle), raw(&stop_handle), raw(&ready_handle)];
        let attributes = Attributes::new(&handles)?;
        let name = Nonce::random()?;
        let boot = Nonce::random()?;
        let application: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut command: Vec<u16> = format!(
            "\"{}\" {} {} {} {} {} {} {}",
            path_text,
            service.argument(),
            parent_handle.as_raw_handle() as usize,
            stop_handle.as_raw_handle() as usize,
            ready_handle.as_raw_handle() as usize,
            name.bootstrap(),
            boot.bootstrap(),
            mode.argument()?
        )
        .encode_utf16()
        .chain(Some(0))
        .collect();
        let startup = STARTUPINFOEXW {
            StartupInfo: STARTUPINFOW {
                cb: std::mem::size_of::<STARTUPINFOEXW>() as u32,
                ..Default::default()
            },
            lpAttributeList: attributes.list,
        };
        let mut process = PROCESS_INFORMATION::default();
        // SAFETY: exact executable, no shell, hidden console, three-handle list.
        unsafe {
            CreateProcessW(
                PCWSTR(application.as_ptr()),
                Some(PWSTR(command.as_mut_ptr())),
                None,
                None,
                true,
                EXTENDED_STARTUPINFO_PRESENT | CREATE_NO_WINDOW,
                None,
                None,
                &startup.StartupInfo,
                &mut process,
            )?;
        }
        let _thread = unsafe { own(process.hThread) };
        let launched = Self {
            process: unsafe { own(process.hProcess) },
            stop: stop.clone(),
        };
        let peer = PinnedProcess::from_launch_handle(launched.process.as_handle())?;
        let remaining = deadline.saturating_duration_since(Instant::now());
        let timeout = remaining.as_millis().min(3000) as u32;
        let wait = unsafe {
            WaitForMultipleObjects(
                &[
                    HANDLE(stop.as_handle().as_raw_handle()),
                    raw(&ready),
                    raw(&launched.process),
                ],
                false,
                timeout,
            )
        };
        if wait.0 == WAIT_OBJECT_0.0 {
            return Err(Error::Cancelled);
        }
        if wait.0 != WAIT_OBJECT_0.0 + 1 {
            return Err(if wait == WAIT_TIMEOUT {
                Error::Timeout
            } else {
                Error::Disconnected
            });
        }
        let pipe = Pipe::connect(&name, peer)?;
        Ok((launched, pipe, boot))
    }
}
impl Drop for Launched {
    fn drop(&mut self) {
        let _ = self.stop.cancel();
        // SAFETY: only our own launched child. Stop covers its I/O and backend;
        // error paths force termination if it does not exit within the budget.
        unsafe {
            if WaitForSingleObject(raw(&self.process), 1000) != WAIT_OBJECT_0 {
                let _ = TerminateProcess(raw(&self.process), 1);
                let _ = WaitForSingleObject(raw(&self.process), 1000);
            }
        }
    }
}

pub struct Bootstrap {
    pub(crate) parent: PinnedProcess,
    pub(crate) stop: Stop,
    ready: OwnedHandle,
    pub(crate) name: Nonce,
    pub(crate) boot: Nonce,
    pub(crate) mode: Mode,
    pub(crate) service: Service,
}
impl Bootstrap {
    pub fn parse(args: &[OsString]) -> Result<Self> {
        if args.len() != 7
            || (args[0] != "serve" && args[0] != "serve-channel")
            || args[1] == args[2]
            || args[1] == args[3]
            || args[2] == args[3]
        {
            return Err(Error::InvalidArgument);
        }
        let parent = receive_handle(&args[1])?;
        let stop = receive_handle(&args[2])?;
        let ready = receive_handle(&args[3])?;
        let name = Nonce::from_bootstrap(args[4].to_str().ok_or(Error::InvalidArgument)?)?;
        let boot = Nonce::from_bootstrap(args[5].to_str().ok_or(Error::InvalidArgument)?)?;
        let mode = match args[6].to_str().ok_or(Error::InvalidArgument)? {
            "disabled" => Mode::Disabled,
            "revoke" => Mode::DiagnosticRevoke,
            value => {
                let delay_ms = value
                    .strip_prefix("fixed:")
                    .and_then(|value| value.parse::<u32>().ok())
                    .ok_or(Error::InvalidArgument)?;
                if delay_ms > 2000 {
                    return Err(Error::InvalidArgument);
                }
                Mode::DiagnosticFixed { delay_ms }
            }
        };
        Ok(Self {
            parent: PinnedProcess::from_launch_handle(parent.as_handle())?,
            stop: Stop::from_bootstrap(stop.as_handle())?,
            ready,
            name,
            boot,
            mode,
            service: if args[0] == "serve-channel" {
                Service::Channel
            } else {
                Service::Once
            },
        })
    }
    pub(crate) fn ready(&self) -> Result<()> {
        unsafe {
            SetEvent(raw(&self.ready))?;
        }
        Ok(())
    }
}
fn receive_handle(value: &OsString) -> Result<OwnedHandle> {
    let number: usize = value
        .to_str()
        .and_then(|value| value.parse().ok())
        .ok_or(Error::InvalidArgument)?;
    let handle = HANDLE(number as *mut _);
    if handle.is_invalid() {
        return Err(Error::InvalidArgument);
    }
    let mut flags = 0;
    // SAFETY: take ownership only after verifying a child-local inherited handle.
    unsafe {
        GetHandleInformation(handle, &mut flags)?;
        if flags & HANDLE_FLAG_INHERIT.0 == 0 {
            return Err(Error::InvalidArgument);
        }
        SetHandleInformation(handle, HANDLE_FLAG_INHERIT.0, HANDLE_FLAGS(0))?;
        Ok(own(handle))
    }
}

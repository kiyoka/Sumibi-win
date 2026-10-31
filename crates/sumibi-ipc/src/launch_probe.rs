//! Suspended signed-file diagnostics, not an authenticated Host launcher.
//! The observed path's current file ID is not proof of the mapped image section.
//! Creation, observation and cleanup belong on a worker, never a TSF callback.
use crate::authenticode::{path_identity, InspectionError};
use crate::support::{own, raw, Error};
use crate::PinnedProcess;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsHandle, OwnedHandle};
use std::path::{Component, Path, Prefix};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::System::JobObjects::{
    CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
    JOBOBJECT_BASIC_LIMIT_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows::Win32::System::Threading::{
    CreateProcessW, DeleteProcThreadAttributeList, InitializeProcThreadAttributeList,
    TerminateProcess, UpdateProcThreadAttribute, WaitForSingleObject, CREATE_NO_WINDOW,
    CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, EXTENDED_STARTUPINFO_PRESENT,
    LPPROC_THREAD_ATTRIBUTE_LIST, PROCESS_INFORMATION, PROC_THREAD_ATTRIBUTE_JOB_LIST,
    STARTUPINFOEXW, STARTUPINFOW,
};

const CAPACITY: usize = 4;
static ACTIVE: AtomicUsize = AtomicUsize::new(0);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProbeError {
    InvalidPath,
    UnsupportedArtifact,
    File(InspectionError),
    Peer(Error),
    FileMismatch,
    Capacity,
    CreateFailed,
    JobUnavailable,
    CleanupFailed,
    Timeout,
    InvalidDeadline,
    Image(crate::host_image::ImageError),
}
struct Permit;
impl Permit {
    fn acquire() -> Result<Self, ProbeError> {
        ACTIVE
            .try_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < CAPACITY).then_some(count + 1)
            })
            .map(|_| Self)
            .map_err(|_| ProbeError::Capacity)
    }
}
impl Drop for Permit {
    fn drop(&mut self) {
        ACTIVE.fetch_sub(1, Ordering::AcqRel);
    }
}

struct State<L> {
    process: OwnedHandle,
    _thread: OwnedHandle,
    peer: Option<PinnedProcess>,
    expected: (u64, [u8; 16]),
    _job: Option<OwnedHandle>,
    _lease: L,
    _permit: Permit,
}

/// Owns the exact created child and retained file/placement. No process/thread
/// handles, PID, resume method or pipe authentication token are exposed.
/// Explicit close is required to report cleanup success. On cleanup failure,
/// the entire state is retained and charged to the four-probe process budget.
#[must_use = "close and confirm child termination before releasing the retained file"]
pub struct SuspendedImageProbe<L> {
    state: Option<Box<State<L>>>,
}
impl<L> SuspendedImageProbe<L> {
    /// Observe the live pinned child's Win32 image path and compare the current
    /// filesystem entry with the retained verified file. This is metadata only:
    /// unprotected ancestors or dependencies still cannot authorize execution.
    pub fn verify_observed_path(&self) -> Result<(), ProbeError> {
        let state = self.state.as_ref().ok_or(ProbeError::CleanupFailed)?;
        let peer = state.peer.as_ref().ok_or(ProbeError::CleanupFailed)?;
        let path = peer.observed_image_path().map_err(ProbeError::Peer)?;
        if path_identity(&path).map_err(ProbeError::File)? != state.expected {
            return Err(ProbeError::FileMismatch);
        }
        peer.require_same(peer).map_err(ProbeError::Peer)?;
        Ok(())
    }
    /// Terminate this created child and wait up to five seconds. A timeout or
    /// cleanup failure retains the handles, lease and capacity until this
    /// diagnostic process exits, rather than pretending cleanup completed.
    pub fn close(mut self, deadline: Instant) -> Result<(), ProbeError> {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() || remaining > Duration::from_secs(5) {
            return Err(ProbeError::InvalidDeadline);
        }
        self.reap(deadline)
    }
    fn reap(&mut self, deadline: Instant) -> Result<(), ProbeError> {
        let Some(state) = self.state.take() else {
            return Ok(());
        };
        let result = stop_child(&state.process, deadline);
        if result.is_err() {
            std::mem::forget(state);
        }
        result
    }
}
impl<L> Drop for SuspendedImageProbe<L> {
    fn drop(&mut self) {
        let _ = self.reap(Instant::now() + Duration::from_millis(100));
    }
}

pub(crate) fn start<L: 'static>(
    lease: L,
    path: &Path,
    expected: (u64, [u8; 16]),
) -> Result<SuspendedImageProbe<L>, ProbeError> {
    let LaunchStrings {
        application,
        mut command,
        directory,
    } = launch_strings(path)?;
    // A file at the requested name must still be the retained verified file.
    if path_identity(path).map_err(ProbeError::File)? != expected {
        return Err(ProbeError::FileMismatch);
    }
    let _caller = PinnedProcess::current().map_err(ProbeError::Peer)?;
    let permit = Permit::acquire()?;
    let job = create_job()?;
    let jobs = [raw(&job)];
    let attributes = JobAttributes::new(&jobs)?;
    let startup = STARTUPINFOEXW {
        StartupInfo: STARTUPINFOW {
            cb: std::mem::size_of::<STARTUPINFOEXW>() as u32,
            ..Default::default()
        },
        lpAttributeList: attributes.list,
    };
    let mut info = PROCESS_INFORMATION::default();
    // SAFETY: exact non-null application name, writable quoted command line,
    // valid parent directory and output buffers. No handle inheritance, shell,
    // elevation fallback or entrypoint resume is requested.
    unsafe {
        CreateProcessW(
            PCWSTR(application.as_ptr()),
            Some(PWSTR(command.as_mut_ptr())),
            None,
            None,
            false,
            CREATE_SUSPENDED
                | CREATE_NO_WINDOW
                | CREATE_UNICODE_ENVIRONMENT
                | EXTENDED_STARTUPINFO_PRESENT,
            None,
            PCWSTR(directory.as_ptr()),
            &startup.StartupInfo,
            &mut info,
        )
    }
    .map_err(|_| ProbeError::CreateFailed)?;
    drop(attributes);
    // SAFETY: a successful CreateProcess supplies two unique owned handles.
    let mut probe = SuspendedImageProbe {
        state: Some(Box::new(State {
            process: unsafe { own(info.hProcess) },
            _thread: unsafe { own(info.hThread) },
            peer: None,
            expected,
            _job: Some(job),
            _lease: lease,
            _permit: permit,
        })),
    };
    let state = probe.state.as_mut().unwrap();
    state.peer = Some(
        PinnedProcess::from_launch_handle(state.process.as_handle()).map_err(ProbeError::Peer)?,
    );
    probe.verify_observed_path()?;
    Ok(probe)
}

fn create_job() -> Result<OwnedHandle, ProbeError> {
    // SAFETY: an unnamed, non-inheritable job. The child is assigned atomically
    // by its creation attributes, never by a post-creation assignment race.
    let job = unsafe { own(CreateJobObjectW(None, None).map_err(|_| ProbeError::JobUnavailable)?) };
    let limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION {
        BasicLimitInformation: JOBOBJECT_BASIC_LIMIT_INFORMATION {
            LimitFlags: JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            ..Default::default()
        },
        ..Default::default()
    };
    unsafe {
        SetInformationJobObject(
            raw(&job),
            JobObjectExtendedLimitInformation,
            (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            std::mem::size_of_val(&limits) as u32,
        )
    }
    .map_err(|_| ProbeError::JobUnavailable)?;
    Ok(job)
}

struct JobAttributes<'a> {
    list: LPPROC_THREAD_ATTRIBUTE_LIST,
    _storage: Vec<usize>,
    _jobs: &'a [HANDLE],
}
impl<'a> JobAttributes<'a> {
    fn new(jobs: &'a [HANDLE]) -> Result<Self, ProbeError> {
        let mut bytes = 0;
        // SAFETY: query and bound allocation size, then retain aligned storage
        // and the caller's job list until DeleteProcThreadAttributeList.
        unsafe {
            let error = InitializeProcThreadAttributeList(None, 1, None, &mut bytes)
                .err()
                .ok_or(ProbeError::JobUnavailable)?;
            if error.code() != windows::core::HRESULT::from_win32(ERROR_INSUFFICIENT_BUFFER.0)
                || bytes == 0
                || bytes > 65536
            {
                return Err(ProbeError::JobUnavailable);
            }
            let mut storage = vec![0usize; bytes.div_ceil(std::mem::size_of::<usize>())];
            let list = LPPROC_THREAD_ATTRIBUTE_LIST(storage.as_mut_ptr().cast());
            InitializeProcThreadAttributeList(Some(list), 1, None, &mut bytes)
                .map_err(|_| ProbeError::JobUnavailable)?;
            let result = Self {
                list,
                _storage: storage,
                _jobs: jobs,
            };
            UpdateProcThreadAttribute(
                list,
                0,
                PROC_THREAD_ATTRIBUTE_JOB_LIST as usize,
                Some(jobs.as_ptr().cast()),
                std::mem::size_of_val(jobs),
                None,
                None,
            )
            .map_err(|_| ProbeError::JobUnavailable)?;
            Ok(result)
        }
    }
}
impl Drop for JobAttributes<'_> {
    fn drop(&mut self) {
        unsafe {
            DeleteProcThreadAttributeList(self.list);
        }
    }
}

fn stop_child(process: &OwnedHandle, deadline: Instant) -> Result<(), ProbeError> {
    // SAFETY: the handle belongs only to the process created by this probe.
    // TerminateProcess is asynchronous, so its return alone is not completion.
    match unsafe { WaitForSingleObject(raw(process), 0) } {
        WAIT_OBJECT_0 => return Ok(()),
        WAIT_TIMEOUT => {}
        _ => return Err(ProbeError::CleanupFailed),
    }
    if unsafe { TerminateProcess(raw(process), 0) }.is_err() {
        return if unsafe { WaitForSingleObject(raw(process), 0) } == WAIT_OBJECT_0 {
            Ok(())
        } else {
            Err(ProbeError::CleanupFailed)
        };
    }
    let remaining = deadline.saturating_duration_since(Instant::now());
    let millis = remaining.as_millis().min(5000) as u32;
    match unsafe { WaitForSingleObject(raw(process), millis) } {
        WAIT_OBJECT_0 => Ok(()),
        WAIT_TIMEOUT => Err(ProbeError::Timeout),
        _ => Err(ProbeError::CleanupFailed),
    }
}

struct LaunchStrings {
    application: Vec<u16>,
    command: Vec<u16>,
    directory: Vec<u16>,
}
fn launch_strings(path: &Path) -> Result<LaunchStrings, ProbeError> {
    if !path.is_absolute()
        || !matches!(path.components().next(), Some(Component::Prefix(prefix)) if matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)))
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
        || !path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
    {
        return Err(ProbeError::InvalidPath);
    }
    let mut application: Vec<u16> = path.as_os_str().encode_wide().collect();
    if application.len() > 32763
        || application
            .iter()
            .any(|unit| *unit == 0 || *unit == b'"' as u16)
    {
        return Err(ProbeError::InvalidPath);
    }
    let mut command = vec![b'"' as u16];
    command.extend_from_slice(&application);
    command.extend([b'"' as u16, 0]);
    application.push(0);
    let directory = path
        .parent()
        .ok_or(ProbeError::InvalidPath)?
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    Ok(LaunchStrings {
        application,
        command,
        directory,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authenticode::{inspect_signer, retain_expected_signer, SignerFingerprint};
    use std::fs::OpenOptions;
    use std::path::PathBuf;
    use std::sync::Mutex;
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    struct Fixture {
        directory: PathBuf,
        path: PathBuf,
        other: PathBuf,
        signer: SignerFingerprint,
    }
    impl Fixture {
        fn new() -> Option<Self> {
            let required = std::env::var_os("SUMIBI_REQUIRE_SIGNED_LAUNCH_PROBE")
                .is_some_and(|value| value == "1");
            match PinnedProcess::current() {
                Ok(_) => {}
                Err(Error::UnsupportedAppContainer | Error::UnsupportedElevation) => {
                    assert!(
                        !required,
                        "suspended probe requires an ordinary desktop process"
                    );
                    return None;
                }
                Err(error) => panic!("cannot inspect fixture process: {error:?}"),
            }
            let source =
                Path::new(r"C:\Program Files (x86)\Microsoft Visual Studio\Installer\vswhere.exe");
            if !source.is_file() {
                assert!(!required, "signed vswhere fixture is required");
                println!("signed suspended probe fixture unavailable");
                return None;
            }
            // A runtime-derived signer is only a test expectation. A product
            // launcher must obtain its pin from protected installation policy.
            let signer = inspect_signer(source).unwrap();
            let directory = std::env::temp_dir().join(format!(
                "sumibi-suspended-{}-{}",
                std::process::id(),
                crate::Nonce::random().unwrap().bootstrap()
            ));
            std::fs::create_dir(&directory).unwrap();
            let path = directory.join("入力 検査.exe");
            let other = directory.join("other.exe");
            std::fs::copy(source, &path).unwrap();
            std::fs::copy(source, &other).unwrap();
            Some(Self {
                directory,
                path,
                other,
                signer,
            })
        }
        fn probe(&self) -> SuspendedImageProbe<crate::authenticode::VerifiedFile> {
            retain_expected_signer(&self.path, self.signer)
                .unwrap()
                .probe_suspended(&self.path)
                .unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
            let _ = std::fs::remove_file(&self.other);
            let _ = std::fs::remove_dir(&self.directory);
        }
    }

    #[test]
    fn exact_application_and_mutable_quoted_command_reject_ambiguous_inputs() {
        let path = Path::new(r"C:\Program Files\日本語\sumibi-host.exe");
        let LaunchStrings {
            application,
            command,
            directory,
        } = launch_strings(path).unwrap();
        let decode = |units: &[u16]| String::from_utf16(&units[..units.len() - 1]).unwrap();
        assert_eq!(decode(&application), path.to_str().unwrap());
        assert_eq!(decode(&command), format!("\"{}\"", path.display()));
        assert_eq!(decode(&directory), r"C:\Program Files\日本語");
        for path in [
            "relative.exe",
            r"\\server\share\host.exe",
            r"C:\x\..\host.exe",
            r"C:\x\host.cmd",
            "C:\\x\\host.exe\0",
            "C:\\x\\host\".exe",
        ] {
            assert!(matches!(
                launch_strings(Path::new(path)),
                Err(ProbeError::InvalidPath)
            ));
        }
    }

    #[test]
    fn suspended_signed_image_keeps_file_until_exact_child_is_reaped() {
        let _guard = TEST_LOCK.lock().unwrap();
        let Some(fixture) = Fixture::new() else {
            return;
        };
        let probe = fixture.probe();
        assert_eq!(probe.verify_observed_path(), Ok(()));
        assert!(OpenOptions::new().write(true).open(&fixture.path).is_err());
        assert!(std::fs::remove_file(&fixture.path).is_err());
        let child = probe
            .state
            .as_ref()
            .unwrap()
            .peer
            .as_ref()
            .unwrap()
            .try_clone()
            .unwrap();
        probe
            .close(Instant::now() + Duration::from_secs(2))
            .unwrap();
        assert!(matches!(
            child.observed_image_path(),
            Err(Error::Disconnected)
        ));
        assert!(OpenOptions::new().write(true).open(&fixture.path).is_ok());
        let dropped = fixture.probe();
        let child = dropped
            .state
            .as_ref()
            .unwrap()
            .peer
            .as_ref()
            .unwrap()
            .try_clone()
            .unwrap();
        drop(dropped);
        assert!(matches!(
            child.observed_image_path(),
            Err(Error::Disconnected)
        ));
        println!("signed suspended child image observed and reaped; no resume performed");
    }

    #[test]
    fn separate_copy_and_full_budget_cannot_start_an_additional_probe() {
        let _guard = TEST_LOCK.lock().unwrap();
        let Some(fixture) = Fixture::new() else {
            return;
        };
        let verified = retain_expected_signer(&fixture.path, fixture.signer).unwrap();
        assert!(matches!(
            verified.probe_suspended(&fixture.other),
            Err(ProbeError::FileMismatch)
        ));
        let mut held = Vec::new();
        for _ in 0..CAPACITY {
            held.push(fixture.probe());
        }
        let verified = retain_expected_signer(&fixture.path, fixture.signer).unwrap();
        assert!(matches!(
            verified.probe_suspended(&fixture.path),
            Err(ProbeError::Capacity)
        ));
        for probe in held {
            probe
                .close(Instant::now() + Duration::from_secs(2))
                .unwrap();
        }
        assert_eq!(ACTIVE.load(Ordering::Acquire), 0);
        fixture
            .probe()
            .close(Instant::now() + Duration::from_secs(2))
            .unwrap();
    }

    #[test]
    fn job_close_terminates_the_suspended_child_before_lease_release() {
        let _guard = TEST_LOCK.lock().unwrap();
        let Some(fixture) = Fixture::new() else {
            return;
        };
        let mut probe = fixture.probe();
        let state = probe.state.as_mut().unwrap();
        drop(state._job.take());
        assert_eq!(
            unsafe { WaitForSingleObject(raw(&state.process), 2000) },
            WAIT_OBJECT_0
        );
        assert!(matches!(
            probe.verify_observed_path(),
            Err(ProbeError::Peer(Error::Disconnected))
        ));
        assert!(OpenOptions::new().write(true).open(&fixture.path).is_err());
        probe
            .close(Instant::now() + Duration::from_secs(2))
            .unwrap();
        assert!(OpenOptions::new().write(true).open(&fixture.path).is_ok());
    }
}

//! Logon-scoped Host exclusion only. A held or existing mutex never proves the
//! identity of a running Host, and is never a discovery or handoff channel.
use crate::support::{last_error, own, raw, Error, Local, Result};
use crate::PinnedProcess;
use std::marker::PhantomData;
use std::os::windows::io::OwnedHandle;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{WAIT_ABANDONED, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::Security::{
    Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1},
    PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES,
};
use windows::Win32::System::Threading::{
    CreateMutexExW, ReleaseMutex, WaitForSingleObject, MUTEX_MODIFY_STATE,
    SYNCHRONIZATION_SYNCHRONIZE,
};

static ACTIVE: AtomicBool = AtomicBool::new(false);
struct ProcessPermit;
impl ProcessPermit {
    fn acquire() -> Option<Self> {
        ACTIVE
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| Self)
    }
}
impl Drop for ProcessPermit {
    fn drop(&mut self) {
        ACTIVE.store(false, Ordering::Release);
    }
}

/// Keep on the owning thread until all Host operations and Broker resources
/// have stopped. !Send/!Sync prevents releasing native ownership on another
/// thread. An abandoned predecessor conveys no policy, consent or peer trust.
/// ```compile_fail
/// fn require_send<T: Send>() {}
/// require_send::<sumibi_ipc::host_instance::HostInstance>();
/// ```
#[must_use = "retain the Host instance lease through complete shutdown"]
pub struct HostInstance {
    handle: OwnedHandle,
    permit: Option<ProcessPermit>,
    abandoned: bool,
    _thread: PhantomData<Rc<()>>,
}
impl HostInstance {
    /// Nonblocking admission for an ordinary desktop process. None means busy,
    /// including a second controller on the same process/owning thread.
    pub fn try_acquire() -> Result<Option<Self>> {
        let local = PinnedProcess::current()?;
        let sid = &local.identity().logon_text;
        Self::try_named(&format!(r"Local\Sumibi.Host.v1.{sid}"), sid)
    }
    fn try_named(name: &str, sid: &str) -> Result<Option<Self>> {
        // Windows mutexes are recursive for one thread; a process permit is
        // needed to reject that second call, rather than granting two leases.
        let Some(permit) = ProcessPermit::acquire() else {
            return Ok(None);
        };
        let descriptor: Vec<u16> = format!("D:P(A;;0x00100001;;;{sid})")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        let mut security = PSECURITY_DESCRIPTOR::default();
        // SAFETY: SID comes from the current validated primary token. The
        // allocation is retained through creation, and the handle is not inherited.
        let handle = unsafe {
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
            own(CreateMutexExW(
                Some(&attributes),
                PCWSTR(name.as_ptr()),
                0,
                (SYNCHRONIZATION_SYNCHRONIZE | MUTEX_MODIFY_STATE).0,
            )?)
        };
        // SAFETY: a live mutex, never waited on recursively through this API.
        // Zero wait means that another Host cannot stall startup indefinitely.
        let waited = unsafe { WaitForSingleObject(raw(&handle), 0) };
        match waited {
            WAIT_OBJECT_0 | WAIT_ABANDONED => Ok(Some(Self {
                handle,
                permit: Some(permit),
                abandoned: waited == WAIT_ABANDONED,
                _thread: PhantomData,
            })),
            WAIT_TIMEOUT => Ok(None),
            WAIT_FAILED => Err(last_error()),
            _ => Err(Error::InvalidMessage),
        }
    }
    pub fn recovered_abandoned(&self) -> bool {
        self.abandoned
    }
}
impl Drop for HostInstance {
    fn drop(&mut self) {
        // SAFETY: thread affinity and unique ownership retain this mutex until
        // shutdown. On an unexpected failure keep the process permit charged;
        // another start in this process must not claim a successful release.
        if unsafe { ReleaseMutex(raw(&self.handle)) }.is_err() {
            std::mem::forget(self.permit.take());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Nonce;
    use std::io::{BufRead, Read, Write};
    use std::process::{Child, Command, Stdio};
    use std::sync::{mpsc, Mutex};
    use std::thread::JoinHandle;
    use std::time::{Duration, Instant};
    use windows::Win32::System::Threading::{CreateEventW, OpenMutexW};
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn local() -> Option<PinnedProcess> {
        match PinnedProcess::current() {
            Ok(local) => Some(local),
            Err(Error::UnsupportedAppContainer | Error::UnsupportedElevation) => None,
            Err(error) => panic!("cannot inspect fixture token: {error:?}"),
        }
    }
    fn name() -> String {
        format!(
            r"Local\Sumibi.Host.fixture.{}",
            Nonce::random().unwrap().bootstrap()
        )
    }
    struct Fixture {
        child: Child,
        reader: Option<JoinHandle<()>>,
    }
    impl Fixture {
        fn start(name: &str, mode: &str) -> Self {
            let mut child = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "host_instance::tests::child_fixture",
                    "--nocapture",
                    "--test-threads=1",
                ])
                .env("SUMIBI_INSTANCE_FIXTURE_NAME", name)
                .env("SUMIBI_INSTANCE_FIXTURE_MODE", mode)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            let stdout = child.stdout.take().unwrap();
            let (send, receive) = mpsc::sync_channel(1);
            let reader = std::thread::spawn(move || {
                let mut send = Some(send);
                for line in std::io::BufReader::new(stdout).lines() {
                    let Ok(line) = line else {
                        break;
                    };
                    if line.contains("INSTANCE_READY") {
                        if let Some(send) = send.take() {
                            let _ = send.send(());
                        }
                    }
                }
            });
            let fixture = Self {
                child,
                reader: Some(reader),
            };
            receive
                .recv_timeout(Duration::from_secs(3))
                .expect("fixture did not become ready");
            fixture
        }
        fn finish(mut self) {
            self.child.stdin.take().unwrap().write_all(&[1]).unwrap();
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                if let Some(exit) = self.child.try_wait().unwrap() {
                    assert!(exit.success());
                    break;
                }
                assert!(Instant::now() < deadline, "fixture did not exit");
                std::thread::sleep(Duration::from_millis(1));
            }
            self.reader.take().unwrap().join().unwrap();
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            if self.child.try_wait().ok().flatten().is_none() {
                let _ = self.child.kill();
            }
            let _ = self.child.wait();
            if let Some(reader) = self.reader.take() {
                let _ = reader.join();
            }
        }
    }

    #[test]
    fn child_fixture() {
        let Ok(name) = std::env::var("SUMIBI_INSTANCE_FIXTURE_NAME") else {
            return;
        };
        assert!(name.starts_with(r"Local\Sumibi.Host.fixture."));
        let local = PinnedProcess::current().unwrap();
        let mode = std::env::var("SUMIBI_INSTANCE_FIXTURE_MODE").unwrap();
        let lease = HostInstance::try_named(&name, &local.identity().logon_text).unwrap();
        if mode == "probe-busy" {
            assert!(lease.is_none());
        } else {
            assert!(lease.is_some());
        }
        println!("INSTANCE_READY");
        std::io::stdout().flush().unwrap();
        let mut release = [0];
        std::io::stdin().read_exact(&mut release).unwrap();
        if mode == "abandon" {
            std::mem::forget(lease);
            std::process::exit(0); // Intentional crash-style exit without Drop.
        }
        drop(lease);
    }

    #[test]
    fn excludes_a_second_process_and_recursive_owner_then_releases() {
        let _guard = TEST_LOCK.lock().unwrap();
        let Some(local) = local() else {
            return;
        };
        let name = name();
        let sid = &local.identity().logon_text;
        let lease = HostInstance::try_named(&name, sid).unwrap().unwrap();
        assert!(!lease.recovered_abandoned());
        assert!(HostInstance::try_named(&name, sid).unwrap().is_none());
        Fixture::start(&name, "probe-busy").finish();
        drop(lease);
        Fixture::start(&name, "hold").finish();
        assert!(HostInstance::try_named(&name, sid).unwrap().is_some());
    }

    #[test]
    fn abandoned_ownership_is_recovered_without_inheriting_state() {
        let _guard = TEST_LOCK.lock().unwrap();
        let Some(local) = local() else {
            return;
        };
        let name = name();
        let fixture = Fixture::start(&name, "abandon");
        let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        // Keep the kernel object alive across the child process's exit, so
        // this tests WAIT_ABANDONED rather than creation of a fresh object.
        let keep = unsafe {
            own(OpenMutexW(
                SYNCHRONIZATION_SYNCHRONIZE | MUTEX_MODIFY_STATE,
                false,
                PCWSTR(wide.as_ptr()),
            )
            .unwrap())
        };
        assert!(HostInstance::try_named(&name, &local.identity().logon_text)
            .unwrap()
            .is_none());
        fixture.finish();
        let lease = HostInstance::try_named(&name, &local.identity().logon_text)
            .unwrap()
            .unwrap();
        assert!(lease.recovered_abandoned());
        drop(lease);
        drop(keep);
        let fresh = HostInstance::try_named(&name, &local.identity().logon_text)
            .unwrap()
            .unwrap();
        assert!(!fresh.recovered_abandoned());
    }

    #[test]
    fn object_type_collision_is_rejected_without_losing_process_capacity() {
        let _guard = TEST_LOCK.lock().unwrap();
        let Some(local) = local() else {
            return;
        };
        let name = name();
        let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        let event = unsafe { own(CreateEventW(None, true, false, PCWSTR(wide.as_ptr())).unwrap()) };
        assert!(HostInstance::try_named(&name, &local.identity().logon_text).is_err());
        drop(event);
        assert!(HostInstance::try_named(&name, &local.identity().logon_text)
            .unwrap()
            .is_some());
    }
}

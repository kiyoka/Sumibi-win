//! Disposable broker-channel diagnostics; synthetic text, no COM/TSF or HTTP.
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use sumibi_ipc::bootstrap::Mode;
use sumibi_ipc::channel::{Channel, MAX_REQUESTS};
use sumibi_ipc::job::{Reservation, MAX_JOBS};
use sumibi_ipc::{Error, PinnedProcess, Result, Stop};
use sumibi_protocol::message::Revision;
use windows::Win32::Foundation::{HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetProcessHandleCount, GetProcessId, WaitForSingleObject,
};

fn deadline() -> Instant {
    Instant::now() + Duration::from_secs(5)
}
fn revision(input: u64) -> Revision {
    Revision {
        input,
        settings: 1,
        authorization: 1,
    }
}
fn executable() -> Result<PathBuf> {
    Ok(std::env::current_exe()
        .map_err(|_| Error::InvalidArgument)?
        .parent()
        .ok_or(Error::InvalidArgument)?
        .join("sumibi-host.exe"))
}
fn connect(path: &Path, delay_ms: u32, stop: &Stop) -> Result<Channel> {
    Channel::connect(path, Mode::DiagnosticFixed { delay_ms }, stop, deadline())
}
fn exited(peer: &PinnedProcess) {
    assert_eq!(
        unsafe { WaitForSingleObject(HANDLE(peer.as_handle().as_raw_handle()), 3000) },
        WAIT_OBJECT_0
    );
}
fn handle_count() -> Result<u32> {
    let mut count = 0;
    unsafe {
        GetProcessHandleCount(GetCurrentProcess(), &mut count)?;
    }
    Ok(count)
}

pub(super) fn run(default_host: bool) -> Result<()> {
    // All blocking launch, I/O and destruction execute on a dedicated worker.
    std::thread::spawn(move || exercise(default_host))
        .join()
        .map_err(|_| Error::WorkerFailed)?
}
fn exercise(default_host: bool) -> Result<()> {
    let path = executable()?;
    let denied = Channel::connect(&path, Mode::Disabled, &Stop::new()?, deadline());
    assert!(matches!(denied, Err(Error::PolicyDenied)));
    println!("Broker channel / disabled Ready rejects before source: OK");
    if default_host {
        assert!(matches!(
            connect(&path, 0, &Stop::new()?),
            Err(Error::Disconnected | Error::PolicyDenied)
        ));
        assert!(matches!(
            Channel::connect(&path, Mode::DiagnosticRevoke, &Stop::new()?, deadline()),
            Err(Error::Disconnected | Error::PolicyDenied)
        ));
        assert!(matches!(
            Channel::connect(&path, Mode::DiagnosticRevokeIdle, &Stop::new()?, deadline()),
            Err(Error::Disconnected | Error::PolicyDenied)
        ));
        assert!(matches!(
            Channel::connect(
                &path,
                Mode::DiagnosticRevokeCoalesced,
                &Stop::new()?,
                deadline()
            ),
            Err(Error::Disconnected | Error::PolicyDenied)
        ));
        assert!(matches!(
            Channel::connect(
                &path,
                Mode::DiagnosticRevokeAfterResult,
                &Stop::new()?,
                deadline()
            ),
            Err(Error::Disconnected | Error::PolicyDenied)
        ));
        assert!(matches!(
            Channel::connect(
                &path,
                Mode::DiagnosticRegrantAfterResult,
                &Stop::new()?,
                deadline()
            ),
            Err(Error::Disconnected | Error::PolicyDenied)
        ));
        assert!(matches!(
            Channel::connect(
                &path,
                Mode::DiagnosticRevokeAtAccept,
                &Stop::new()?,
                deadline()
            ),
            Err(Error::Disconnected | Error::PolicyDenied)
        ));
        assert!(matches!(
            Channel::connect(
                &path,
                Mode::DiagnosticRegrantAtAccept,
                &Stop::new()?,
                deadline()
            ),
            Err(Error::Disconnected | Error::PolicyDenied)
        ));
        println!("Broker channel / default host rejects diagnostic converter: OK");
        return Ok(());
    }
    // Warm up process creation, pipe and thread runtime before handle auditing.
    {
        let mut channel = connect(&path, 0, &Stop::new()?)?;
        drop(channel.convert(revision(1), "warmup", deadline())?);
    }
    std::thread::spawn(|| {})
        .join()
        .map_err(|_| Error::WorkerFailed)?;
    let baseline = handle_count()?;
    {
        let mut channel = connect(&path, 0, &Stop::new()?)?;
        let peer = channel.peer_process()?;
        let pid = unsafe { GetProcessId(HANDLE(peer.as_handle().as_raw_handle())) };
        assert_ne!(pid, 0);
        for input in 1..=3 {
            let reply = channel.convert(revision(input), "synthetic 😀", deadline())?;
            assert_eq!(reply.request, input);
            assert_eq!(reply.revision.input, input);
            assert_eq!(
                (reply.revision.settings, reply.revision.authorization),
                (1, 1)
            );
            assert_eq!(reply.text(), Ok("日本語"));
            let current = channel.peer_process()?;
            assert_eq!(
                unsafe { GetProcessId(HANDLE(current.as_handle().as_raw_handle())) },
                pid
            );
            assert_eq!(
                unsafe { WaitForSingleObject(HANDLE(peer.as_handle().as_raw_handle()), 0) },
                WAIT_TIMEOUT
            );
        }
        channel.close();
        assert!(channel.is_closed());
        assert!(matches!(
            channel.convert(revision(4), "unused", deadline()),
            Err(Error::Closed)
        ));
        exited(&peer);
    }
    println!("Broker channel / sequential requests keep exact launched peer: OK");
    {
        let mut channel = connect(&path, 0, &Stop::new()?)?;
        let deadline = Instant::now() + Duration::from_secs(5);
        let reply = channel.convert(revision(1), "synthetic", deadline)?;
        assert_eq!(reply.text(), Ok("日本語"));
        std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
        assert_eq!(reply.text(), Err(Error::Timeout));
        drop(reply);
        channel.close();
    }
    println!("Broker channel / completed reply expires before late direct read: OK");
    {
        let mut channel = connect(&path, 0, &Stop::new()?)?;
        let reply = channel.convert(revision(1), "fixed", deadline())?;
        assert!(matches!(
            channel.convert(revision(2), "unused", deadline()),
            Err(Error::Capacity)
        ));
        assert!(!channel.is_closed());
        drop(reply);
        let reply = channel.convert(revision(2), "fixed", deadline())?;
        assert_eq!(reply.request, 2);
    }
    println!("Broker channel / retained reply blocks next request without consuming ID: OK");
    {
        let mut channel = connect(&path, 0, &Stop::new()?)?;
        for (rev, text, until) in [
            (revision(0), "fixed".to_owned(), deadline()),
            (revision(1), String::new(), deadline()),
            (revision(1), "x".repeat(1025), deadline()),
            (
                revision(1),
                "fixed".to_owned(),
                Instant::now() + Duration::from_secs(61),
            ),
        ] {
            assert!(matches!(
                channel.convert(rev, &text, until),
                Err(Error::InvalidArgument)
            ));
            assert!(!channel.is_closed());
        }
        let reply = channel.convert(revision(1), &"😀".repeat(512), deadline())?;
        assert_eq!(reply.request, 1);
    }
    println!("Broker channel / preflight rejection and maximum UTF-16 payload: OK");
    for setting in [true, false] {
        let mut channel = connect(&path, 0, &Stop::new()?)?;
        let peer = channel.peer_process()?;
        drop(channel.convert(revision(1), "fixed", deadline())?);
        let mut next = revision(2);
        if setting {
            next.settings = 2;
        } else {
            next.authorization = 2;
        }
        assert!(matches!(
            channel.convert(next, "unused", deadline()),
            Err(Error::PolicyDenied)
        ));
        assert!(channel.is_closed());
        exited(&peer);
        println!("Broker channel / policy mismatch retires before source / settings={setting}: OK");
    }
    {
        let stop = Stop::new()?;
        let mut channel = connect(&path, 0, &stop)?;
        let peer = channel.peer_process()?;
        stop.cancel()?;
        assert!(matches!(
            channel.convert(revision(1), "unused", deadline()),
            Err(Error::Cancelled)
        ));
        assert!(channel.is_closed());
        exited(&peer);
    }
    println!("Broker channel / cancellation before request retires channel: OK");
    {
        let stop = Stop::new()?;
        let mut channel = connect(&path, 1000, &stop)?;
        let peer = channel.peer_process()?;
        let cancel = stop.clone();
        let cancelling = std::thread::spawn(move || -> Result<()> {
            // Fixed, bounded diagnostic delay, interrupted if already stopped.
            unsafe {
                WaitForSingleObject(HANDLE(cancel.as_handle().as_raw_handle()), 100);
            }
            cancel.cancel()
        });
        let result = channel.convert(revision(1), "fixed", deadline());
        cancelling.join().map_err(|_| Error::WorkerFailed)??;
        assert!(matches!(result, Err(Error::Cancelled)));
        assert!(channel.is_closed());
        assert!(matches!(
            channel.convert(revision(2), "unused", deadline()),
            Err(Error::Closed)
        ));
        exited(&peer);
    }
    println!("Broker channel / cancellation interrupts pending read and prevents replay: OK");
    {
        let mut channel =
            Channel::connect(&path, Mode::DiagnosticRevoke, &Stop::new()?, deadline())?;
        let peer = channel.peer_process()?;
        let revoked = channel.convert(revision(1), "synthetic", deadline());
        assert!(
            matches!(revoked, Err(Error::PolicyDenied)),
            "{:?}",
            revoked.err()
        );
        assert!(channel.is_closed());
        assert!(matches!(
            channel.convert(revision(2), "unused", deadline()),
            Err(Error::Closed)
        ));
        exited(&peer);
    }
    println!("Broker channel / in-flight revocation retires without a result or replay: OK");
    for mode in [
        Mode::DiagnosticRevokeAtAccept,
        Mode::DiagnosticRegrantAtAccept,
    ] {
        let mut channel = Channel::connect(&path, mode, &Stop::new()?, deadline())?;
        let peer = channel.peer_process()?;
        let denied = channel.convert(revision(1), "synthetic", deadline());
        assert!(
            matches!(denied, Err(Error::PolicyDenied)),
            "{:?}",
            denied.err()
        );
        assert!(channel.is_closed());
        assert!(matches!(
            channel.convert(revision(2), "unused", deadline()),
            Err(Error::Closed)
        ));
        exited(&peer);
        let label = if mode == Mode::DiagnosticRegrantAtAccept {
            "re-consent"
        } else {
            "revocation"
        };
        println!("Broker channel / {label} at acceptance rejects already-arrived old request: OK");
    }
    {
        let mut channel =
            Channel::connect(&path, Mode::DiagnosticRevokeIdle, &Stop::new()?, deadline())?;
        let peer = channel.peer_process()?;
        std::thread::sleep(Duration::from_millis(500));
        let revoked = channel.convert(revision(1), "synthetic", deadline());
        assert!(
            matches!(revoked, Err(Error::PolicyDenied)),
            "{:?}",
            revoked.err()
        );
        assert!(channel.is_closed());
        exited(&peer);
    }
    println!("Broker channel / queued idle revocation rejects before source: OK");
    {
        let mut channel = Channel::connect(
            &path,
            Mode::DiagnosticRevokeCoalesced,
            &Stop::new()?,
            deadline(),
        )?;
        let peer = channel.peer_process()?;
        let revoked = channel.convert(revision(1), "synthetic", deadline());
        assert!(
            matches!(revoked, Err(Error::PolicyDenied)),
            "{:?}",
            revoked.err()
        );
        assert!(channel.is_closed());
        exited(&peer);
    }
    println!("Broker channel / coalesced Ready and revocation keep the unread frame: OK");
    {
        let mut channel = Channel::connect(
            &path,
            Mode::DiagnosticRevokeAfterResult,
            &Stop::new()?,
            deadline(),
        )?;
        let peer = channel.peer_process()?;
        let revoked = channel.convert(revision(1), "synthetic", deadline());
        assert!(
            matches!(revoked, Err(Error::PolicyDenied)),
            "{:?}",
            revoked.err()
        );
        assert!(channel.is_closed());
        exited(&peer);
    }
    println!("Broker channel / result followed by queued revocation is not delivered: OK");
    {
        let mut channel = Channel::connect(
            &path,
            Mode::DiagnosticRegrantAfterResult,
            &Stop::new()?,
            deadline(),
        )?;
        let peer = channel.peer_process()?;
        let stale = channel.convert(revision(1), "synthetic", deadline());
        assert!(
            matches!(stale, Err(Error::PolicyDenied)),
            "{:?}",
            stale.err()
        );
        assert!(channel.is_closed());
        assert!(matches!(
            channel.convert(revision(2), "unused", deadline()),
            Err(Error::Closed)
        ));
        exited(&peer);
    }
    println!("Broker channel / coalesced re-consent after result still discards old result: OK");
    {
        let mut channel = connect(&path, 1000, &Stop::new()?)?;
        let peer = channel.peer_process()?;
        let result = channel.convert(
            revision(1),
            "fixed",
            Instant::now() + Duration::from_millis(50),
        );
        // Either side can reach its deadline first; both errors are terminal.
        assert!(matches!(result, Err(Error::Timeout | Error::Disconnected)));
        assert!(channel.is_closed());
        exited(&peer);
    }
    println!("Broker channel / pending deadline retires channel: OK");
    {
        let mut channel = connect(&path, 0, &Stop::new()?)?;
        let peer = channel.peer_process()?;
        assert!(matches!(
            channel.convert(revision(1), "unused", Instant::now()),
            Err(Error::Timeout)
        ));
        assert!(channel.is_closed());
        exited(&peer);
    }
    println!("Broker channel / expired request is never sent: OK");
    {
        let mut channel = connect(&path, 0, &Stop::new()?)?;
        let peer = channel.peer_process()?;
        for input in 1..=MAX_REQUESTS {
            let reply = channel.convert(revision(input as u64), "fixed", deadline())?;
            assert_eq!(reply.request, input as u64);
        }
        assert!(matches!(
            channel.convert(revision(65), "unused", deadline()),
            Err(Error::Retired)
        ));
        assert!(channel.is_closed());
        exited(&peer);
    }
    println!("Broker channel / request lifetime limit and child cleanup: OK");
    {
        let mut channels = (0..MAX_JOBS)
            .map(|_| connect(&path, 0, &Stop::new()?))
            .collect::<Result<Vec<_>>>()?;
        assert!(matches!(
            connect(&path, 0, &Stop::new()?),
            Err(Error::Capacity)
        ));
        assert!(matches!(Reservation::acquire(), Err(Error::Capacity)));
        let mut channel = channels.pop().ok_or(Error::WorkerFailed)?;
        let reply = channel.convert(revision(1), "fixed", deadline())?;
        drop(channel);
        assert!(matches!(Reservation::acquire(), Err(Error::Capacity)));
        drop(reply);
        let slot = Reservation::acquire()?;
        assert!(matches!(Reservation::acquire(), Err(Error::Capacity)));
        drop(slot);
        drop(channels);
    }
    println!("Broker channel / shares admission with jobs and replies after close: OK");
    {
        assert!(matches!(
            Channel::connect(
                Path::new("relative.exe"),
                Mode::Disabled,
                &Stop::new()?,
                deadline()
            ),
            Err(Error::InvalidArgument)
        ));
        assert!(matches!(
            Channel::connect(&path, Mode::Disabled, &Stop::new()?, Instant::now()),
            Err(Error::Timeout)
        ));
        assert!(matches!(
            Channel::connect(
                &path,
                Mode::Disabled,
                &Stop::new()?,
                Instant::now() + Duration::from_secs(61)
            ),
            Err(Error::InvalidArgument)
        ));
        let slots = (0..MAX_JOBS)
            .map(|_| Reservation::acquire())
            .collect::<Result<Vec<_>>>()?;
        drop(slots);
    }
    println!("Broker channel / rejected launch returns admission: OK");
    assert_eq!(
        handle_count()?,
        baseline,
        "broker-channel parent handle leak"
    );
    println!("Broker channel / parent handle cleanup: OK");
    Ok(())
}

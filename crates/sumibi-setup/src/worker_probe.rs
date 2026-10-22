//! Real reusable worker tests, with synthetic text and no COM or user input.
use std::os::windows::io::AsRawHandle;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use sumibi_ipc::bootstrap::Mode;
use sumibi_ipc::job::{Request, Reservation, MAX_JOBS};
use sumibi_ipc::worker::{Completion, Worker};
use sumibi_ipc::{Error, Result};
use sumibi_protocol::message::Revision;
use windows::Win32::Foundation::{HANDLE, WAIT_OBJECT_0};
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetProcessHandleCount, WaitForSingleObject,
};

fn path() -> Result<PathBuf> {
    Ok(std::env::current_exe()
        .map_err(|_| Error::InvalidArgument)?
        .parent()
        .ok_or(Error::InvalidArgument)?
        .join("sumibi-host.exe"))
}
fn mode() -> Mode {
    Mode::DiagnosticFixed { delay_ms: 100 }
}
fn worker(mode: Mode) -> Result<Worker> {
    Worker::new(Reservation::acquire()?, path()?, mode)
}
fn request(id: u64, mode: Mode) -> Request {
    Request {
        id,
        revision: Revision {
            input: id,
            settings: 1,
            authorization: 1,
        },
        text: "synthetic 😀".into(),
        deadline: Instant::now() + Duration::from_secs(5),
        mode,
    }
}
fn wait(worker: &Worker) -> Result<Completion> {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        if let Some(completion) = worker.try_result()? {
            return Ok(completion);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(Error::Timeout);
        }
        if unsafe {
            WaitForSingleObject(
                HANDLE(worker.completion_handle().as_raw_handle()),
                remaining.as_millis() as u32,
            )
        } != WAIT_OBJECT_0
        {
            return Err(Error::Timeout);
        }
    }
}
fn handles() -> Result<u32> {
    let mut count = 0;
    unsafe {
        GetProcessHandleCount(GetCurrentProcess(), &mut count)?;
    }
    Ok(count)
}
pub(super) fn run() -> Result<()> {
    // Warm up thread/process runtime before measuring owned handle cleanup.
    {
        let mut worker = worker(mode())?;
        worker.submit(worker.reserve(1)?, request(1, mode()))?;
        drop(wait(&worker)?);
        worker.shutdown(Duration::from_secs(3))?;
    }
    let baseline = handles()?;
    {
        let mut worker = worker(mode())?;
        let reserved = worker.reserve(10)?;
        assert!(matches!(worker.reserve(11), Err(Error::Capacity)));
        drop(reserved);
        assert!(matches!(worker.reserve(10), Err(Error::InvalidArgument)));
        worker.submit(worker.reserve(11)?, request(11, mode()))?;
        let first = wait(&worker)?;
        assert_eq!(first.id, 11);
        assert_eq!(first.revision.input, 11);
        assert_eq!(first.outcome(), Ok("日本語"));
        let (pid, wire) = first.exchange().ok_or(Error::InvalidMessage)?;
        assert_eq!(wire, 1);
        assert!(matches!(worker.reserve(12), Err(Error::Capacity)));
        drop(first);
        worker.submit(worker.reserve(12)?, request(12, mode()))?;
        worker.cancel(11)?;
        let second = wait(&worker)?;
        assert_eq!(second.id, 12);
        assert_eq!(second.outcome(), Ok("日本語"));
        assert_eq!(second.exchange(), Some((pid, 2)));
        drop(second);
        worker.shutdown(Duration::from_secs(3))?;
    }
    println!("Reusable worker / reserve, retained result, same peer and stale cancel: OK");
    {
        let mut first = worker(mode())?;
        let mut second = worker(mode())?;
        assert!(matches!(
            second.submit(first.reserve(1)?, request(1, mode())),
            Err(Error::InvalidArgument)
        ));
        let mut wrong = request(2, mode());
        wrong.id = 3;
        assert!(matches!(
            first.submit(first.reserve(2)?, wrong),
            Err(Error::InvalidArgument)
        ));
        assert!(matches!(
            first.submit(first.reserve(3)?, request(3, Mode::Disabled)),
            Err(Error::InvalidArgument)
        ));
        let mut wrong = request(4, mode());
        wrong.text.clear();
        assert!(matches!(
            first.submit(first.reserve(4)?, wrong),
            Err(Error::InvalidArgument)
        ));
        let mut wrong = request(5, mode());
        wrong.deadline = Instant::now();
        assert!(matches!(
            first.submit(first.reserve(5)?, wrong),
            Err(Error::InvalidArgument)
        ));
        first.submit(first.reserve(6)?, request(6, mode()))?;
        let reply = wait(&first)?;
        assert_eq!(reply.outcome(), Ok("日本語"));
        assert_eq!(reply.exchange().map(|(_, wire)| wire), Some(1));
        drop(reply);
        first.shutdown(Duration::from_secs(3))?;
        second.shutdown(Duration::from_secs(3))?;
    }
    println!("Reusable worker / foreign permit and preflight rejection before dispatch: OK");
    {
        let mut worker = worker(mode())?;
        let permit = worker.reserve(1)?;
        worker.cancel(1)?;
        assert!(matches!(
            worker.submit(permit, request(1, mode())),
            Err(Error::Closed)
        ));
        assert!(matches!(worker.reserve(2), Err(Error::Closed)));
        worker.shutdown(Duration::from_secs(3))?;
    }
    println!("Reusable worker / cancellation of reservation prevents dispatch: OK");
    {
        let mut worker = worker(Mode::DiagnosticFixed { delay_ms: 1000 })?;
        worker.submit(
            worker.reserve(1)?,
            request(1, Mode::DiagnosticFixed { delay_ms: 1000 }),
        )?;
        worker.cancel(1)?;
        let reply = wait(&worker)?;
        assert_eq!(reply.outcome(), Err(Error::Cancelled));
        assert!(worker.is_closed());
        assert!(matches!(worker.reserve(2), Err(Error::Closed)));
        drop(reply);
        worker.shutdown(Duration::from_secs(3))?;
    }
    println!("Reusable worker / queued or pending cancellation always completes once: OK");
    {
        let mut worker = worker(Mode::Disabled)?;
        worker.submit(worker.reserve(1)?, request(1, Mode::Disabled))?;
        let reply = wait(&worker)?;
        assert_eq!(reply.outcome(), Err(Error::PolicyDenied));
        assert!(worker.is_closed());
        drop(reply);
        worker.shutdown(Duration::from_secs(3))?;
    }
    println!("Reusable worker / Ready policy denial retires worker: OK");
    {
        let mut worker = worker(mode())?;
        worker.submit(worker.reserve(1)?, request(1, mode()))?;
        drop(wait(&worker)?);
        let mut next = request(2, mode());
        next.deadline = Instant::now() + Duration::from_millis(30);
        worker.submit(worker.reserve(2)?, next)?;
        let reply = wait(&worker)?;
        assert!(matches!(
            reply.outcome(),
            Err(Error::Timeout | Error::Disconnected)
        ));
        assert!(worker.is_closed());
        drop(reply);
        worker.shutdown(Duration::from_secs(3))?;
    }
    println!("Reusable worker / pending deadline retires without retry: OK");
    {
        let mut worker = worker(mode())?;
        worker.submit(worker.reserve(1)?, request(1, mode()))?;
        let reply = wait(&worker)?;
        assert_eq!(reply.outcome(), Ok("日本語"));
        worker.cancel(1)?;
        assert_eq!(reply.outcome(), Err(Error::Cancelled));
        assert!(reply.exchange().is_none());
        drop(reply);
        worker.shutdown(Duration::from_secs(3))?;
    }
    println!("Reusable worker / cancellation after completion blocks delivery: OK");
    {
        let mode = Mode::DiagnosticFixed { delay_ms: 0 };
        let mut worker = worker(mode)?;
        for wire in 1..=sumibi_ipc::channel::MAX_REQUESTS {
            let id = wire as u64 + 100;
            worker.submit(worker.reserve(id)?, request(id, mode))?;
            let reply = wait(&worker)?;
            assert_eq!(reply.outcome(), Ok("日本語"));
            assert_eq!(
                reply.exchange().map(|(_, sequence)| sequence),
                Some(wire as u64)
            );
        }
        assert!(worker.is_closed());
        assert!(matches!(worker.reserve(1000), Err(Error::Closed)));
        worker.shutdown(Duration::from_secs(3))?;
    }
    println!("Reusable worker / final valid reply then retirement before next capture: OK");
    {
        let mut worker = worker(mode())?;
        worker.submit(worker.reserve(1)?, request(1, mode()))?;
        let reply = wait(&worker)?;
        worker.shutdown(Duration::from_secs(3))?;
        drop(worker);
        let slots = (1..MAX_JOBS)
            .map(|_| Reservation::acquire())
            .collect::<Result<Vec<_>>>()?;
        assert!(matches!(Reservation::acquire(), Err(Error::Capacity)));
        drop(reply);
        let last = Reservation::acquire()?;
        assert!(matches!(Reservation::acquire(), Err(Error::Capacity)));
        drop((last, slots));
    }
    println!("Reusable worker / result keeps global capacity after facade and thread exit: OK");
    {
        let worker = worker(Mode::DiagnosticFixed { delay_ms: 1000 })?;
        let finished = worker
            .finished_handle()?
            .try_clone_to_owned()
            .map_err(|_| Error::WorkerFailed)?;
        worker.submit(
            worker.reserve(1)?,
            request(1, Mode::DiagnosticFixed { delay_ms: 1000 }),
        )?;
        drop(worker);
        assert_eq!(
            unsafe { WaitForSingleObject(HANDLE(finished.as_raw_handle()), 8000) },
            WAIT_OBJECT_0
        );
    }
    println!("Reusable worker / owner drop stops queued work and signals child cleanup: OK");
    assert_eq!(handles()?, baseline, "reusable worker parent handle leak");
    println!("Reusable worker / parent handle cleanup: OK");
    Ok(())
}

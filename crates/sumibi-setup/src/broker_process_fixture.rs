//! Separate-process Host for the opt-in TSF diagnostic, not product discovery.
use std::io::{BufRead, BufReader, Read, Write};
use std::os::windows::io::{AsHandle, AsRawHandle, FromRawHandle, OwnedHandle};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use sumibi_host::broker_boot::BrokerBoot;
use sumibi_host::broker_runtime::{JobTicket, PollStep};
use sumibi_host::broker_workers::OwnerStep;
use sumibi_host::manual::{Credential, Destination, Scheme};
use sumibi_host::policy::PolicyAuthority;
use sumibi_ipc::{job::Reservation, worker::Worker, Nonce, PinnedProcess, Pipe, Server, Stop};
use windows::Win32::Foundation::{
    DuplicateHandle, GetHandleInformation, DUPLICATE_SAME_ACCESS, HANDLE, WAIT_OBJECT_0,
};
use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessId, WaitForSingleObject};

type Failure = Box<dyn std::error::Error>;

pub(crate) struct Fixture(Child);

impl Fixture {
    pub(crate) fn terminate(mut self) -> Result<(), Failure> {
        self.0.kill()?;
        self.0.wait()?;
        Ok(())
    }
    pub(crate) fn start() -> Result<(Self, Worker), Failure> {
        Self::start_inner(false)
    }
    pub(crate) fn start_pending() -> Result<(Self, Worker), Failure> {
        Self::start_inner(true)
    }
    pub(crate) fn release(&mut self) -> Result<(), Failure> {
        self.0
            .stdin
            .as_mut()
            .ok_or("missing child stdin")?
            .write_all(&[1])?;
        Ok(())
    }
    fn start_inner(delayed: bool) -> Result<(Self, Worker), Failure> {
        let child = Command::new(std::env::current_exe()?)
            .arg(if delayed {
                "broker-process-fixture-delayed"
            } else {
                "broker-process-fixture"
            })
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        let mut fixture = Self(child);
        let host = PinnedProcess::from_launch_handle(fixture.0.as_handle())?;
        let parent = PinnedProcess::current()?;
        let bootstrap_name = Nonce::random()?;
        let bootstrap_server = Server::create(&bootstrap_name)?;
        let bootstrap_stop = Stop::new()?;
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut remote = HANDLE::default();
        // SAFETY: the fresh child receives only a duplicate of this process's
        // pinned handle. Its child-local value travels over the private stdin.
        unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                HANDLE(parent.as_handle().as_raw_handle()),
                HANDLE(fixture.0.as_raw_handle()),
                &mut remote,
                0,
                false,
                DUPLICATE_SAME_ACCESS,
            )?;
        }
        writeln!(
            fixture.0.stdin.as_mut().ok_or("missing child stdin")?,
            "{} {}",
            remote.0 as usize,
            bootstrap_name.bootstrap()
        )?;
        let pipe = bootstrap_server.accept(host.try_clone()?, &bootstrap_stop, deadline)?;
        let worker = Worker::prepare_handoff(Reservation::acquire()?, pipe, host, deadline)?;
        if delayed {
            return Ok((fixture, worker));
        }
        let ready = worker
            .preparation_handle()
            .ok_or("missing preparation event")?;
        // SAFETY: the worker owns the event for this bounded wait.
        if unsafe { WaitForSingleObject(HANDLE(ready.as_raw_handle()), 5000) } != WAIT_OBJECT_0 {
            return Err("process fixture preparation timed out".into());
        }
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            match worker.prepared_policy()? {
                Some(policy) if policy.allowed => break,
                Some(_) => return Err("process fixture denied".into()),
                None if Instant::now() < deadline => std::thread::yield_now(),
                None => return Err("process fixture policy unavailable".into()),
            }
        }
        Ok((fixture, worker))
    }

    pub(crate) fn finish(mut self) -> Result<(), Failure> {
        drop(self.0.stdin.take());
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = self.0.try_wait()? {
                return if status.success() {
                    Ok(())
                } else {
                    Err("process fixture failed".into())
                };
            }
            if Instant::now() >= deadline {
                return Err("process fixture shutdown timed out".into());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

pub(crate) fn child(delayed: bool) -> Result<(), Failure> {
    let mut input = BufReader::new(std::io::stdin());
    let mut line = String::new();
    input.read_line(&mut line)?;
    let mut fields = line.split_whitespace();
    let handle: usize = fields.next().ok_or("missing parent handle")?.parse()?;
    let bootstrap_name = Nonce::from_bootstrap(fields.next().ok_or("missing bootstrap name")?)?;
    if fields.next().is_some() {
        return Err("invalid fixture bootstrap".into());
    }
    let handle = HANDLE(handle as *mut _);
    if handle.is_invalid() {
        return Err("missing parent handle".into());
    }
    let mut flags = 0;
    // SAFETY: validate the child-local handle before taking ownership. This
    // diagnostic process has no other thread that can close it at this point.
    unsafe {
        GetHandleInformation(handle, &mut flags)?;
        if GetProcessId(handle) == 0 {
            return Err("parent handle is not a process".into());
        }
    }
    // SAFETY: the validated handle belongs to this child and is not a pseudo handle.
    let parent = unsafe { OwnedHandle::from_raw_handle(handle.0) };
    let peer = PinnedProcess::from_launch_handle(parent.as_handle())?;
    let destination =
        Destination::new(Scheme::Https, "example.org", 443, "/").ok_or("destination")?;
    let source = PolicyAuthority::new(destination.clone());
    source
        .replace_credential(Credential::new("fixture".into()))
        .map_err(|_| "fixture credential")?;
    source
        .grant_manual_consent(&destination)
        .map_err(|_| "fixture consent")?;
    let pipe = Pipe::connect(&bootstrap_name, peer.try_clone()?)?;
    if delayed {
        let mut signal = [0];
        input.read_exact(&mut signal)?;
        if signal != [1] {
            return Err("invalid fixture release".into());
        }
    }
    let stop = Stop::new()?;
    let reader_stop = stop.clone();
    std::thread::spawn(move || {
        let mut byte = [0];
        let _ = input.read(&mut byte);
        let _ = reader_stop.cancel();
    });
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut boot = BrokerBoot::start(&source, 2, deadline).map_err(|error| format!("{error:?}"))?;
    boot.handoff_private_over_pipe(peer, pipe, &stop, deadline)
        .map_err(|_| "fixture handoff failed")?;
    while !stop.is_cancelled()? && Instant::now() < deadline {
        match boot.advance(&stop, deadline, |_| |_: JobTicket| Ok("日本語".into())) {
            Ok(OwnerStep::Broker(PollStep::Idle)) => std::thread::sleep(Duration::from_millis(1)),
            Ok(_) => {}
            Err(_) if stop.is_cancelled()? => break,
            Err(error) => return Err(format!("{error:?}").into()),
        }
    }
    if !stop.is_cancelled()? {
        return Err("process fixture lifetime exceeded".into());
    }
    boot.shutdown(Instant::now() + Duration::from_secs(2))
        .map_err(|error| format!("{error:?}"))?;
    Ok(())
}

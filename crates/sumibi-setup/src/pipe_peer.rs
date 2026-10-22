//! Two-process desktop diagnostics. No user documents, HTTP, COM or credentials.
use std::ffi::OsString;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsHandle, AsRawHandle, BorrowedHandle, FromRawHandle, OwnedHandle};
use std::time::{Duration, Instant};
use sumibi_core::{ManualConversion, RequestId, Revision as CoreRevision, Target};
use sumibi_ipc::{Error, Nonce, PinnedProcess, Pipe, Result, Server, Stop, MAX_ENDPOINTS};
use sumibi_protocol::message::{self, ConversionResult, Envelope, Message, Policy, Ready};
use sumibi_protocol::session::{ClientSession, Event, Outcome};
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::System::Threading::*;

fn raw(handle: &OwnedHandle) -> HANDLE {
    HANDLE(handle.as_raw_handle())
}
fn deadline() -> Instant {
    Instant::now() + Duration::from_secs(3)
}
fn invalid() -> Error {
    Error::InvalidArgument
}
fn handle_count() -> Result<u32> {
    let mut count = 0;
    unsafe {
        GetProcessHandleCount(GetCurrentProcess(), &mut count)?;
    }
    Ok(count)
}

struct Attributes {
    list: LPPROC_THREAD_ATTRIBUTE_LIST,
    _storage: Vec<usize>,
}
impl Attributes {
    fn new(handles: &[HANDLE]) -> Result<Self> {
        let mut bytes = 0;
        // SAFETY: documented size query then aligned storage. The handle array
        // and the list both outlive CreateProcess; no other handles are listed.
        unsafe {
            let error = InitializeProcThreadAttributeList(None, 1, None, &mut bytes)
                .err()
                .ok_or_else(invalid)?;
            if error.code() != windows::core::HRESULT::from_win32(ERROR_INSUFFICIENT_BUFFER.0)
                || bytes == 0
                || bytes > 65536
            {
                return Err(invalid());
            }
            let mut storage = vec![0usize; bytes.div_ceil(std::mem::size_of::<usize>())];
            let list = LPPROC_THREAD_ATTRIBUTE_LIST(storage.as_mut_ptr().cast());
            InitializeProcThreadAttributeList(Some(list), 1, None, &mut bytes)?;
            let attributes = Self {
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
            Ok(attributes)
        }
    }
}
impl Drop for Attributes {
    fn drop(&mut self) {
        // SAFETY: successfully initialized list, with backing storage still live.
        unsafe {
            DeleteProcThreadAttributeList(self.list);
        }
    }
}

fn inherited(handle: BorrowedHandle<'_>) -> Result<OwnedHandle> {
    let mut duplicate = HANDLE::default();
    // SAFETY: duplicate this process's explicit bootstrap handle only. The
    // restricted launch attribute list prevents unrelated inheritable handles.
    unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            HANDLE(handle.as_raw_handle()),
            GetCurrentProcess(),
            &mut duplicate,
            0,
            true,
            DUPLICATE_SAME_ACCESS,
        )?;
        Ok(OwnedHandle::from_raw_handle(duplicate.0))
    }
}

struct Child {
    handle: OwnedHandle,
}
impl Child {
    fn spawn(name: &Nonce, broker: &Nonce, stop: &Stop, case: &str) -> Result<Self> {
        let current = PinnedProcess::current()?;
        let parent = inherited(current.as_handle())?;
        let stop = inherited(stop.as_handle())?;
        let handles = [raw(&parent), raw(&stop)];
        let attributes = Attributes::new(&handles)?;
        let executable = std::env::current_exe().map_err(|_| invalid())?;
        let application: Vec<u16> = executable
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect();
        // Only the executable path needs quoting. Every following value is a
        // validated decimal handle, lowercase hex nonce, or fixed fixture case.
        let mut command: Vec<u16> = format!(
            "\"{}\" ipc-peer-child {} {} {} {} {}",
            executable.display(),
            parent.as_raw_handle() as usize,
            stop.as_raw_handle() as usize,
            name.bootstrap(),
            broker.bootstrap(),
            case
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
        // SAFETY: launch this exact executable without a shell, window or broad
        // inheritance. Parent/stop handles stay alive until creation returns.
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
            let _thread = OwnedHandle::from_raw_handle(process.hThread.0);
            Ok(Self {
                handle: OwnedHandle::from_raw_handle(process.hProcess.0),
            })
        }
    }
    fn pin(&self) -> Result<PinnedProcess> {
        PinnedProcess::from_launch_handle(self.handle.as_handle())
    }
    fn wait(&self) -> Result<()> {
        // SAFETY: only the owned diagnostic child is observed, with a deadline.
        unsafe {
            if WaitForSingleObject(raw(&self.handle), 3000) != WAIT_OBJECT_0 {
                return Err(Error::Timeout);
            }
            let mut code = 1;
            GetExitCodeProcess(raw(&self.handle), &mut code)?;
            if code != 0 {
                return Err(Error::Os(code));
            }
        }
        Ok(())
    }
}
impl Drop for Child {
    fn drop(&mut self) {
        // SAFETY: this handle is solely the child returned by our CreateProcess.
        // Error/panic paths cannot strand a fixture process or wait indefinitely.
        unsafe {
            if WaitForSingleObject(raw(&self.handle), 0) == WAIT_TIMEOUT {
                let _ = TerminateProcess(raw(&self.handle), 1);
                let _ = WaitForSingleObject(raw(&self.handle), 1000);
            }
        }
    }
}

fn read_message(pipe: &mut Pipe, stop: &Stop) -> Result<Envelope> {
    let mut decoder = sumibi_protocol::Decoder::default();
    let deadline = deadline();
    loop {
        let bytes = pipe.start_read(stop, deadline)?.wait()?.bytes;
        let decoded = decoder.decode(&bytes).map_err(|_| invalid())?;
        if decoded.consumed != bytes.len() {
            return Err(invalid());
        }
        if let Some(body) = decoded.body {
            return message::decode(body).map_err(|_| invalid());
        }
    }
}
fn write_message(pipe: &mut Pipe, envelope: &Envelope, stop: &Stop) -> Result<()> {
    pipe.write_all(
        message::encode(envelope).map_err(|_| invalid())?,
        stop,
        deadline(),
    )
}
fn read_event(pipe: &mut Pipe, client: &mut ClientSession, stop: &Stop) -> Result<Event> {
    let deadline = deadline();
    loop {
        let bytes = pipe.start_read(stop, deadline)?.wait()?.bytes;
        let received = client.receive(&bytes, Instant::now()).map_err(|error| {
            if error == sumibi_protocol::session::SessionError::WrongBroker {
                Error::WrongPeer
            } else {
                invalid()
            }
        })?;
        if received.consumed != bytes.len() {
            return Err(invalid());
        }
        if let Some(event) = received.event {
            return Ok(event);
        }
    }
}

pub(super) fn run() -> Result<()> {
    Nonce::random()?;
    drop(Stop::new()?);
    drop(PinnedProcess::current()?);
    // The first authenticated round trip initializes Windows process/IPC state.
    // Measure after warmup and repeat the normal path as part of the leak check.
    let mut baseline_handles = 0;
    for (index, case) in [
        "round-trip",
        "round-trip",
        "wrong-server",
        "wrong-client",
        "wrong-boot",
        "stopped-write",
        "expired-write",
        "cancel-read",
        "timeout-read",
        "drop-read",
        "peer-exit",
        "cancel-write",
    ]
    .into_iter()
    .enumerate()
    {
        if index == 1 {
            baseline_handles = handle_count()?;
        }
        let name = Nonce::random()?;
        let broker = Nonce::random()?;
        let lifecycle = Stop::new()?;
        let stop = Stop::new()?;
        let server = Server::create(&name)?;
        assert!(Server::create(&name).is_err(), "first-instance enforcement");
        let child = Child::spawn(&name, &broker, &lifecycle, case)?;
        let peer = if case == "wrong-client" {
            PinnedProcess::current()?
        } else {
            child.pin()?
        };
        let accepted = server.accept(peer, &stop, deadline());
        if case == "wrong-client" {
            assert!(matches!(accepted, Err(Error::WrongPeer)));
            child.wait()?;
            println!("Two-process pipe / {case}: rejected before application read: OK");
            continue;
        }
        if case == "wrong-server" {
            match accepted {
                Ok(mut pipe) => assert!(matches!(
                    read_message(&mut pipe, &stop),
                    Err(Error::Disconnected)
                )),
                Err(Error::Disconnected) => {}
                Err(error) => return Err(error),
            }
            lifecycle.cancel()?;
            child.wait()?;
            println!("Two-process pipe / {case}: rejected before Hello: OK");
            continue;
        }
        let mut pipe = accepted?;
        let mut hello = read_message(&mut pipe, &stop)?;
        let Message::Hello(value) = &hello.message else {
            return Err(invalid());
        };
        if value.broker != broker.id() || hello.generation != 1 {
            return Err(invalid());
        }
        hello.message = Message::Ready(Ready {
            broker: if case == "wrong-boot" {
                Nonce::random()?.id()
            } else {
                broker.id()
            },
            policy: Policy {
                settings: 1,
                authorization: 1,
                allowed: true,
            },
        });
        write_message(&mut pipe, &hello, &stop)?;
        if case == "wrong-boot" {
            assert!(matches!(
                read_message(&mut pipe, &stop),
                Err(Error::Disconnected)
            ));
            child.wait()?;
            println!("Two-process pipe / {case}: rejected before Convert: OK");
            continue;
        }
        let mut request = read_message(&mut pipe, &stop)?;
        if request.session != hello.session || request.generation != hello.generation {
            return Err(invalid());
        }
        let Message::Convert(value) = &request.message else {
            return Err(invalid());
        };
        if value.request != 1
            || value.revision.input != 1
            || value.revision.settings != 1
            || value.revision.authorization != 1
        {
            return Err(invalid());
        }
        if case == "round-trip" {
            request.message = Message::Result(ConversionResult {
                request: value.request,
                revision: value.revision,
                text: "日本語".into(),
            });
            write_message(&mut pipe, &request, &stop)?;
            let ack = read_message(&mut pipe, &stop)?;
            if ack.session != hello.session
                || ack.generation != 1
                || !matches!(ack.message, Message::Cancel(message::Cancel { request: 1 }))
            {
                return Err(invalid());
            }
        } else if matches!(case, "stopped-write" | "expired-write") {
            if case == "stopped-write" {
                stop.cancel()?;
                assert!(matches!(
                    pipe.start_write(vec![1], &stop, deadline()),
                    Err(Error::Cancelled)
                ));
            } else {
                assert!(matches!(
                    pipe.start_write(vec![1], &stop, Instant::now()),
                    Err(Error::Timeout)
                ));
            }
        } else {
            let operation_deadline = if case == "timeout-read" {
                Instant::now() + Duration::from_millis(25)
            } else {
                deadline()
            };
            let operation = if case == "cancel-write" {
                pipe.start_write(
                    vec![0; sumibi_protocol::MAX_FRAME_BYTES],
                    &stop,
                    operation_deadline,
                )?
            } else {
                pipe.start_read(&stop, operation_deadline)?
            };
            assert!(
                operation.is_pending(),
                "fixture must exercise pending kernel I/O"
            );
            match case {
                "cancel-read" | "cancel-write" => {
                    stop.cancel()?;
                    assert!(matches!(operation.wait(), Err(Error::Cancelled)));
                }
                "timeout-read" => {
                    assert!(matches!(operation.wait(), Err(Error::Timeout)));
                }
                "drop-read" => {
                    drop(operation);
                }
                "peer-exit" => {
                    lifecycle.cancel()?;
                    assert!(matches!(operation.wait(), Err(Error::Disconnected)));
                }
                _ => return Err(invalid()),
            }
            assert!(matches!(
                pipe.start_read(&stop, deadline()),
                Err(Error::Closed)
            ));
        }
        lifecycle.cancel()?;
        child.wait()?;
        if index == 0 {
            println!("Two-process pipe / warmup round-trip: OK");
        } else {
            println!("Two-process pipe / {case}: OK");
        }
    }
    // Cancel a pending accept without any client; no application handle exists.
    let server = Server::create(&Nonce::random()?)?;
    let stop = Stop::new()?;
    let peer = PinnedProcess::current()?;
    std::thread::scope(|scope| -> Result<()> {
        let signal = stop.clone();
        let worker = scope.spawn(move || {
            std::thread::sleep(Duration::from_millis(25));
            signal.cancel()
        });
        assert!(matches!(
            server.accept(peer, &stop, deadline()),
            Err(Error::Cancelled)
        ));
        worker.join().map_err(|_| invalid())??;
        Ok(())
    })?;
    println!("Two-process pipe / cancel-accept: OK");
    let servers: Vec<_> = (0..MAX_ENDPOINTS)
        .map(|_| Server::create(&Nonce::random()?))
        .collect::<Result<_>>()?;
    assert!(matches!(
        Server::create(&Nonce::random()?),
        Err(Error::Capacity)
    ));
    drop(servers);
    Server::create(&Nonce::random()?)?;
    println!("Two-process pipe / endpoint capacity and release: OK");
    drop(stop);
    assert_eq!(
        handle_count()?,
        baseline_handles,
        "diagnostic handles must return to baseline"
    );
    println!("Two-process pipe / parent handle cleanup: OK");
    println!("Scope: launched desktop peers and synthetic Core result only; production TSF/Broker is not connected");
    Ok(())
}

fn bootstrap_handle(text: &OsString) -> Result<OwnedHandle> {
    let value: usize = text
        .to_str()
        .ok_or_else(invalid)?
        .parse()
        .map_err(|_| invalid())?;
    let handle = HANDLE(value as *mut _);
    if handle.is_invalid() {
        return Err(invalid());
    }
    let mut flags = 0;
    // SAFETY: validate a child-local inherited handle before taking ownership;
    // clear inheritance immediately. Parent uses an explicit two-handle list.
    unsafe {
        GetHandleInformation(handle, &mut flags)?;
        if flags & HANDLE_FLAG_INHERIT.0 == 0 {
            return Err(invalid());
        }
        SetHandleInformation(handle, HANDLE_FLAG_INHERIT.0, HANDLE_FLAGS(0))?;
        Ok(OwnedHandle::from_raw_handle(handle.0))
    }
}

pub(super) fn child(args: &[OsString]) -> Result<()> {
    if args.len() != 5 || args[0] == args[1] {
        return Err(invalid());
    }
    let parent = bootstrap_handle(&args[0])?;
    let lifecycle_handle = bootstrap_handle(&args[1])?;
    let lifecycle = Stop::from_bootstrap(lifecycle_handle.as_handle())?;
    let name = Nonce::from_bootstrap(args[2].to_str().ok_or_else(invalid)?)?;
    let broker = Nonce::from_bootstrap(args[3].to_str().ok_or_else(invalid)?)?;
    let case = args[4].to_str().ok_or_else(invalid)?;
    if ![
        "round-trip",
        "wrong-server",
        "wrong-client",
        "wrong-boot",
        "stopped-write",
        "expired-write",
        "cancel-read",
        "timeout-read",
        "drop-read",
        "peer-exit",
        "cancel-write",
    ]
    .contains(&case)
    {
        return Err(invalid());
    }
    let expected = if case == "wrong-server" {
        PinnedProcess::current()?
    } else {
        PinnedProcess::from_launch_handle(parent.as_handle())?
    };
    let connected = Pipe::connect(&name, expected);
    if case == "wrong-server" {
        assert!(matches!(connected, Err(Error::WrongPeer)));
        return wait_lifecycle(&lifecycle);
    }
    let mut pipe = connected?;
    let stop = Stop::new()?;
    let mut client = ClientSession::new(Nonce::random()?.id(), broker.id());
    let sent = pipe.write_all(client.hello().map_err(|_| invalid())?, &stop, deadline());
    if case == "wrong-client" {
        if matches!(sent, Err(Error::Disconnected)) {
            return Ok(());
        }
        sent?;
        assert!(matches!(
            read_event(&mut pipe, &mut client, &stop),
            Err(Error::Disconnected)
        ));
        return Ok(());
    }
    sent?;
    if case == "wrong-boot" {
        assert!(matches!(
            read_event(&mut pipe, &mut client, &stop),
            Err(Error::WrongPeer)
        ));
        return Ok(());
    }
    if !matches!(read_event(&mut pipe, &mut client, &stop)?, Event::Ready(_)) {
        return Err(invalid());
    }
    let target = Target {
        context: 1,
        range: 1,
        selection: sumibi_core::Selection::Acp(0, 7),
        text: "nihongo".encode_utf16().collect(),
        revision: CoreRevision {
            input: 1,
            settings: 1,
            authorization: 1,
        },
        allowed: true,
    };
    let mut core = ManualConversion::default();
    core.begin(RequestId(1), target.clone())
        .map_err(|_| invalid())?;
    let request = client
        .begin(1, "nihongo", 2000, Instant::now())
        .map_err(|_| invalid())?;
    let mut decoder = sumibi_protocol::Decoder::default();
    let decoded = decoder.decode(&request).map_err(|_| invalid())?;
    let mut acknowledgement =
        message::decode(decoded.body.ok_or_else(invalid)?).map_err(|_| invalid())?;
    pipe.write_all(request, &stop, deadline())?;
    if case == "round-trip" {
        let Event::Reply {
            request,
            outcome: Outcome::Result(text),
            ..
        } = read_event(&mut pipe, &mut client, &stop)?
        else {
            return Err(invalid());
        };
        let result = core
            .finish(RequestId(request.into()), &target, &text)
            .map_err(|_| invalid())?;
        if result != "日本語".encode_utf16().collect::<Vec<_>>() || core.is_pending() {
            return Err(invalid());
        }
        // Build an idempotent Cancel as a diagnostic completion acknowledgement
        // on the exact session/generation that produced the received result.
        acknowledgement.message = Message::Cancel(message::Cancel { request });
        write_message(&mut pipe, &acknowledgement, &stop)?;
    }
    core.cancel();
    client.disconnect();
    // Keep the expected process alive until the parent has observed completion
    // or has deliberately cancelled/expired a pending read/write.
    wait_lifecycle(&lifecycle)?;
    if matches!(case, "stopped-write" | "expired-write") {
        // Parent must not have started a write at all after cancellation/expiry.
        let operation = pipe.start_read(&stop, Instant::now() + Duration::from_millis(25))?;
        assert!(
            operation.is_pending(),
            "no byte may be sent after stop/deadline"
        );
        assert!(matches!(operation.wait(), Err(Error::Timeout)));
    }
    Ok(())
}

fn wait_lifecycle(stop: &Stop) -> Result<()> {
    // SAFETY: wait only on the owned bootstrap event, with a finite deadline.
    unsafe {
        if WaitForSingleObject(HANDLE(stop.as_handle().as_raw_handle()), 5000) != WAIT_OBJECT_0 {
            return Err(Error::Timeout);
        }
    }
    if stop.is_cancelled()? {
        Ok(())
    } else {
        Err(invalid())
    }
}

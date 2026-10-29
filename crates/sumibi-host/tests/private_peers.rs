#![cfg(windows)]

use std::io::{BufRead, Read, Write};
use std::os::windows::io::{AsHandle, AsRawHandle, FromRawHandle, OwnedHandle};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use sumibi_host::broker_boot::BrokerBoot;
use sumibi_host::broker_runtime::{JobTicket, PollStep};
use sumibi_host::broker_workers::OwnerStep;
use sumibi_host::manual::{Credential, Destination, Scheme};
use sumibi_host::policy::PolicyAuthority;
use sumibi_ipc::{
    bootstrap::Mode,
    channel::Channel,
    job::{Request, Reservation},
    worker::Worker,
    Nonce, PinnedProcess, Pipe, Stop,
};
use sumibi_protocol::message::Revision;
use windows::Win32::Foundation::{DuplicateHandle, DUPLICATE_SAME_ACCESS, HANDLE};
use windows::Win32::System::Threading::GetCurrentProcess;

static PIPE_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct FixtureChild(Child);
impl Drop for FixtureChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn host_lifetime_fixture() {
    if std::env::var_os("SUMIBI_PRIVATE_HOST_FIXTURE").is_some() {
        let mut byte = [0];
        let _ = std::io::stdin().read(&mut byte).unwrap();
    }
}

#[test]
fn idle_pinned_worker_retires_when_its_host_exits_before_first_request() {
    let _guard = PIPE_TEST_LOCK.lock().unwrap();
    if matches!(
        PinnedProcess::current(),
        Err(sumibi_ipc::Error::UnsupportedElevation | sumibi_ipc::Error::UnsupportedAppContainer)
    ) {
        return;
    }
    let mut child = FixtureChild(
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "host_lifetime_fixture"])
            .env("SUMIBI_PRIVATE_HOST_FIXTURE", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let peer = PinnedProcess::from_launch_handle(child.0.as_handle()).unwrap();
    let mut worker = Worker::new_pinned(
        Reservation::acquire().unwrap(),
        Nonce::random().unwrap(),
        Nonce::random().unwrap(),
        peer,
    )
    .unwrap();
    drop(child.0.stdin.take());
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    while !worker.is_closed() {
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert!(matches!(worker.reserve(1), Err(sumibi_ipc::Error::Closed)));
    worker.shutdown(Duration::from_secs(1)).unwrap();
}

#[test]
fn client_fixture() {
    if std::env::var_os("SUMIBI_PRIVATE_PEER_FIXTURE").is_none() {
        return;
    }
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line).unwrap();
    let mut fields = line.split_whitespace();
    let handle: usize = fields.next().unwrap().parse().unwrap();
    let name = Nonce::from_bootstrap(fields.next().unwrap()).unwrap();
    let broker = Nonce::from_bootstrap(fields.next().unwrap()).unwrap();
    assert!(fields.next().is_none());
    // SAFETY: the parent duplicated this process handle into this child and
    // transferred the child-local value over its private stdin pipe.
    let host_handle = unsafe { OwnedHandle::from_raw_handle(handle as *mut _) };
    let expected_host = if std::env::var_os("SUMIBI_PRIVATE_WRONG_HOST").is_some() {
        PinnedProcess::current().unwrap()
    } else {
        PinnedProcess::from_launch_handle(host_handle.as_handle()).unwrap()
    };
    let stop = Stop::new().unwrap();
    let deadline = Instant::now() + Duration::from_secs(8);
    if std::env::var_os("SUMIBI_PRIVATE_WORKER").is_some() {
        let mut signal = [0];
        std::io::stdin().read_exact(&mut signal).unwrap();
        let mut revision = Revision {
            input: 1,
            settings: std::env::var("SUMIBI_TEST_SETTINGS")
                .unwrap()
                .parse()
                .unwrap(),
            authorization: std::env::var("SUMIBI_TEST_AUTHORIZATION")
                .unwrap()
                .parse()
                .unwrap(),
        };
        let stale = std::env::var_os("SUMIBI_PRIVATE_STALE").is_some();
        if stale {
            revision.authorization += 1;
        }
        let mut worker =
            Worker::new_pinned(Reservation::acquire().unwrap(), name, broker, expected_host)
                .unwrap();
        let permit = worker.reserve(1).unwrap();
        worker
            .submit(
                permit,
                Request {
                    id: 1,
                    revision,
                    text: "nihongo".into(),
                    deadline,
                    mode: Mode::Pinned,
                },
            )
            .unwrap();
        let cancel = std::env::var_os("SUMIBI_PRIVATE_CANCEL").is_some();
        if cancel {
            std::io::stdin().read_exact(&mut signal).unwrap();
            worker.cancel(1).unwrap();
        }
        loop {
            if let Some(completion) = worker.try_result().unwrap() {
                if cancel {
                    assert_eq!(completion.outcome(), Err(sumibi_ipc::Error::Cancelled));
                } else if stale {
                    assert_eq!(completion.outcome(), Err(sumibi_ipc::Error::PolicyDenied));
                } else {
                    assert_eq!(completion.outcome().unwrap(), "日本語");
                }
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        worker.shutdown(Duration::from_secs(1)).unwrap();
        return;
    }
    let connected = Channel::connect_pinned(&name, broker, expected_host, &stop, deadline);
    if std::env::var_os("SUMIBI_PRIVATE_WRONG_HOST").is_some() {
        assert!(matches!(connected, Err(sumibi_ipc::Error::WrongPeer)));
        return;
    }
    let mut channel = connected.unwrap();
    let policy = channel.policy().unwrap();
    assert!(policy.allowed);
    if std::env::var_os("SUMIBI_PRIVATE_WAIT_FOR_CONVERT").is_some() {
        let mut signal = [0];
        std::io::stdin().read_exact(&mut signal).unwrap();
    }
    let reply = channel
        .convert(
            Revision {
                input: 1,
                settings: policy.settings,
                authorization: policy.authorization,
            },
            "nihongo",
            deadline,
        )
        .unwrap();
    assert_eq!(reply.text().unwrap(), "日本語");
    drop(reply);
    channel.close();
}

#[test]
fn two_child_processes_authenticate_the_same_host_on_private_pipes() {
    let _guard = PIPE_TEST_LOCK.lock().unwrap();
    if matches!(
        PinnedProcess::current(),
        Err(sumibi_ipc::Error::UnsupportedElevation | sumibi_ipc::Error::UnsupportedAppContainer)
    ) {
        return;
    }
    let destination = Destination::new(Scheme::Https, "example.org", 443, "/").unwrap();
    let source = PolicyAuthority::new(destination.clone());
    source
        .replace_credential(Credential::new("fixture".into()))
        .unwrap();
    source.grant_manual_consent(&destination).unwrap();
    let stop = Stop::new().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut boot = BrokerBoot::start(&source, 3, deadline).unwrap();
    let host = PinnedProcess::current().unwrap();
    let mut children = Vec::new();
    let mut ids = Vec::new();
    for index in 0..2 {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", "client_fixture"])
            .env("SUMIBI_PRIVATE_PEER_FIXTURE", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if index == 1 {
            let policy = source.wire_policy().unwrap();
            command
                .env("SUMIBI_PRIVATE_WORKER", "1")
                .env("SUMIBI_TEST_SETTINGS", policy.settings.to_string())
                .env(
                    "SUMIBI_TEST_AUTHORIZATION",
                    policy.authorization.to_string(),
                );
        }
        let mut child = FixtureChild(command.spawn().unwrap());
        let peer = PinnedProcess::from_launch_handle(child.0.as_handle()).unwrap();
        let (id, name) = boot.begin_private_accept(peer, deadline).unwrap();
        let mut remote = HANDLE::default();
        // SAFETY: duplicate only the pinned current-process query/synchronize
        // handle into the freshly launched child; the returned value belongs
        // to the child, which takes ownership after reading its private stdin.
        unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                HANDLE(host.as_handle().as_raw_handle()),
                HANDLE(child.0.as_raw_handle()),
                &mut remote,
                0,
                false,
                DUPLICATE_SAME_ACCESS,
            )
            .unwrap();
        }
        writeln!(
            child.0.stdin.as_mut().unwrap(),
            "{} {} {}",
            remote.0 as usize,
            name.bootstrap(),
            boot.name().bootstrap()
        )
        .unwrap();
        ids.push(id);
        children.push(child);
    }
    let mut accepted = Vec::new();
    let mut completed = Vec::new();
    while completed.len() < 2 {
        assert!(Instant::now() < deadline);
        let result = boot
            .advance_tracked(&stop, deadline, |_| |_: JobTicket| Ok("日本語".into()))
            .unwrap();
        match result.step {
            OwnerStep::Accepted { key, .. } => {
                let id = result.accept.unwrap();
                assert!(ids.contains(&id) && !accepted.iter().any(|(_, old)| *old == id));
                accepted.push((key, id));
            }
            OwnerStep::Broker(PollStep::Activated(_)) | OwnerStep::Submitted { .. } => {}
            OwnerStep::Completed {
                key,
                request: 1,
                delivered: true,
                succeeded: true,
            } => {
                assert!(
                    accepted.iter().any(|(known, _)| *known == key) && !completed.contains(&key)
                );
                completed.push(key);
                if accepted
                    .iter()
                    .any(|(known, id)| *known == key && *id == ids[0])
                {
                    loop {
                        if let Some(status) = children[0].0.try_wait().unwrap() {
                            assert!(status.success());
                            break;
                        }
                        assert!(Instant::now() < deadline);
                        std::thread::yield_now();
                    }
                    children[1]
                        .0
                        .stdin
                        .as_mut()
                        .unwrap()
                        .write_all(&[1])
                        .unwrap();
                }
            }
            OwnerStep::Broker(PollStep::Closed { key, .. }) if completed.contains(&key) => {}
            OwnerStep::Broker(PollStep::Idle) => std::thread::yield_now(),
            _ => panic!("private peer exchange failed"),
        }
    }
    for child in &mut children {
        loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
    }
    assert!(boot
        .shutdown(Instant::now() + Duration::from_secs(1))
        .is_ok());
}

#[test]
fn private_client_rejects_a_different_host_before_sending_hello() {
    let _guard = PIPE_TEST_LOCK.lock().unwrap();
    if matches!(
        PinnedProcess::current(),
        Err(sumibi_ipc::Error::UnsupportedElevation | sumibi_ipc::Error::UnsupportedAppContainer)
    ) {
        return;
    }
    let destination = Destination::new(Scheme::Https, "example.org", 443, "/").unwrap();
    let source = PolicyAuthority::new(destination);
    let stop = Stop::new().unwrap();
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut boot = BrokerBoot::start(&source, 2, deadline).unwrap();
    let host = PinnedProcess::current().unwrap();
    let mut child = FixtureChild(
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "client_fixture"])
            .env("SUMIBI_PRIVATE_PEER_FIXTURE", "1")
            .env("SUMIBI_PRIVATE_WRONG_HOST", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let peer = PinnedProcess::from_launch_handle(child.0.as_handle()).unwrap();
    let (_, name) = boot.begin_private_accept(peer, deadline).unwrap();
    let mut remote = HANDLE::default();
    // SAFETY: as in the successful fixture, duplicate only the pinned Host
    // handle into this child and transfer the value on its private stdin.
    unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            HANDLE(host.as_handle().as_raw_handle()),
            HANDLE(child.0.as_raw_handle()),
            &mut remote,
            0,
            false,
            DUPLICATE_SAME_ACCESS,
        )
        .unwrap();
    }
    writeln!(
        child.0.stdin.take().unwrap(),
        "{} {} {}",
        remote.0 as usize,
        name.bootstrap(),
        boot.name().bootstrap()
    )
    .unwrap();
    let mut accepted = false;
    loop {
        assert!(Instant::now() < deadline);
        let step = boot
            .advance_tracked(&stop, deadline, |_| |_: JobTicket| Ok("unused".into()))
            .unwrap();
        match step.step {
            OwnerStep::Accepted { .. } => accepted = true,
            OwnerStep::AcceptFailed(sumibi_ipc::Error::Disconnected) => break,
            OwnerStep::Broker(PollStep::Closed { .. }) if accepted => break,
            OwnerStep::Broker(PollStep::Idle) => std::thread::yield_now(),
            _ => panic!("untrusted host led to a protocol message"),
        }
    }
    loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert!(boot
        .shutdown(Instant::now() + Duration::from_secs(1))
        .is_ok());
}

#[test]
fn private_listener_rejects_another_process_then_accepts_the_pinned_child() {
    let _guard = PIPE_TEST_LOCK.lock().unwrap();
    if matches!(
        PinnedProcess::current(),
        Err(sumibi_ipc::Error::UnsupportedElevation | sumibi_ipc::Error::UnsupportedAppContainer)
    ) {
        return;
    }
    let destination = Destination::new(Scheme::Https, "example.org", 443, "/").unwrap();
    let source = PolicyAuthority::new(destination.clone());
    source
        .replace_credential(Credential::new("fixture".into()))
        .unwrap();
    source.grant_manual_consent(&destination).unwrap();
    let stop = Stop::new().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut boot = BrokerBoot::start(&source, 2, deadline).unwrap();
    let host = PinnedProcess::current().unwrap();
    let mut child = FixtureChild(
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "client_fixture"])
            .env("SUMIBI_PRIVATE_PEER_FIXTURE", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let peer = PinnedProcess::from_launch_handle(child.0.as_handle()).unwrap();
    let (wrong_id, wrong_name) = boot.begin_private_accept(peer, deadline).unwrap();
    let mut wrong_client = Pipe::connect(&wrong_name, PinnedProcess::current().unwrap()).unwrap();
    loop {
        assert!(Instant::now() < deadline);
        let result = boot
            .advance_tracked(&stop, deadline, |_| |_: JobTicket| Ok("unused".into()))
            .unwrap();
        match result.step {
            OwnerStep::AcceptFailed(sumibi_ipc::Error::WrongPeer) => {
                assert!(result.accept == Some(wrong_id));
                break;
            }
            OwnerStep::Broker(PollStep::Idle) => std::thread::yield_now(),
            _ => panic!("unexpected acceptance of wrong process"),
        }
    }
    assert!(matches!(
        wrong_client
            .start_read(&stop, deadline)
            .and_then(|operation| operation.wait()),
        Err(sumibi_ipc::Error::Disconnected)
    ));
    drop(wrong_client);
    assert_eq!(boot.active_count(), 0);
    let peer = PinnedProcess::from_launch_handle(child.0.as_handle()).unwrap();
    let (id, name) = boot.begin_private_accept(peer, deadline).unwrap();
    assert!(id != wrong_id);
    assert_ne!(name.bootstrap(), wrong_name.bootstrap());
    let mut remote = HANDLE::default();
    // SAFETY: duplicate this pinned Host handle only into the fresh child.
    unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            HANDLE(host.as_handle().as_raw_handle()),
            HANDLE(child.0.as_raw_handle()),
            &mut remote,
            0,
            false,
            DUPLICATE_SAME_ACCESS,
        )
        .unwrap();
    }
    writeln!(
        child.0.stdin.take().unwrap(),
        "{} {} {}",
        remote.0 as usize,
        name.bootstrap(),
        boot.name().bootstrap()
    )
    .unwrap();
    let mut key = None;
    loop {
        assert!(Instant::now() < deadline);
        let result = boot
            .advance_tracked(&stop, deadline, |_| |_: JobTicket| Ok("日本語".into()))
            .unwrap();
        match result.step {
            OwnerStep::Accepted { key: value, .. } => {
                assert!(result.accept == Some(id));
                key = Some(value);
            }
            OwnerStep::Broker(PollStep::Activated(value)) if Some(value) == key => {}
            OwnerStep::Submitted {
                key: value,
                request: 1,
            } if Some(value) == key => {}
            OwnerStep::Completed {
                key: value,
                request: 1,
                delivered: true,
                succeeded: true,
            } if Some(value) == key => break,
            OwnerStep::Broker(PollStep::Idle) => std::thread::yield_now(),
            _ => panic!("pinned child could not use the replacement pipe"),
        }
    }
    loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert!(boot
        .shutdown(Instant::now() + Duration::from_secs(1))
        .is_ok());
}

#[test]
fn cancelling_a_pinned_worker_closes_its_connection_and_reaps_the_job() {
    let _guard = PIPE_TEST_LOCK.lock().unwrap();
    if matches!(
        PinnedProcess::current(),
        Err(sumibi_ipc::Error::UnsupportedElevation | sumibi_ipc::Error::UnsupportedAppContainer)
    ) {
        return;
    }
    let destination = Destination::new(Scheme::Https, "example.org", 443, "/").unwrap();
    let source = PolicyAuthority::new(destination.clone());
    source
        .replace_credential(Credential::new("fixture".into()))
        .unwrap();
    source.grant_manual_consent(&destination).unwrap();
    let policy = source.wire_policy().unwrap();
    let stop = Stop::new().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut boot = BrokerBoot::start(&source, 2, deadline).unwrap();
    let host = PinnedProcess::current().unwrap();
    let mut child = FixtureChild(
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "client_fixture"])
            .env("SUMIBI_PRIVATE_PEER_FIXTURE", "1")
            .env("SUMIBI_PRIVATE_WORKER", "1")
            .env("SUMIBI_PRIVATE_CANCEL", "1")
            .env("SUMIBI_TEST_SETTINGS", policy.settings.to_string())
            .env(
                "SUMIBI_TEST_AUTHORIZATION",
                policy.authorization.to_string(),
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let peer = PinnedProcess::from_launch_handle(child.0.as_handle()).unwrap();
    let (_, name) = boot.begin_private_accept(peer, deadline).unwrap();
    let mut remote = HANDLE::default();
    // SAFETY: duplicate the pinned Host handle only into our launched child.
    unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            HANDLE(host.as_handle().as_raw_handle()),
            HANDLE(child.0.as_raw_handle()),
            &mut remote,
            0,
            false,
            DUPLICATE_SAME_ACCESS,
        )
        .unwrap();
    }
    let input = child.0.stdin.as_mut().unwrap();
    writeln!(
        input,
        "{} {} {}",
        remote.0 as usize,
        name.bootstrap(),
        boot.name().bootstrap()
    )
    .unwrap();
    input.write_all(&[1]).unwrap();
    let mut submitted = false;
    let mut closed = false;
    loop {
        assert!(Instant::now() < deadline);
        let result = boot
            .advance_tracked(&stop, deadline, |_| {
                |ticket: JobTicket| {
                    let _ = ticket.stop().wait_until(ticket.deadline());
                    Err(sumibi_protocol::message::ErrorCode::Cancelled)
                }
            })
            .unwrap();
        match result.step {
            OwnerStep::Accepted { .. } | OwnerStep::Broker(PollStep::Activated(_)) => {}
            OwnerStep::Submitted { request: 1, .. } => {
                assert!(!submitted);
                submitted = true;
                child.0.stdin.as_mut().unwrap().write_all(&[1]).unwrap();
            }
            OwnerStep::Broker(PollStep::Closed { .. }) if submitted => closed = true,
            OwnerStep::Broker(PollStep::Idle) => std::thread::yield_now(),
            _ => panic!("unexpected step during worker cancellation"),
        }
        if closed && boot.active_worker_count() == 0 {
            break;
        }
    }
    loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert!(boot
        .shutdown(Instant::now() + Duration::from_secs(1))
        .is_ok());
}

#[test]
fn pinned_worker_refuses_a_stale_policy_without_submitting_text() {
    let _guard = PIPE_TEST_LOCK.lock().unwrap();
    if matches!(
        PinnedProcess::current(),
        Err(sumibi_ipc::Error::UnsupportedElevation | sumibi_ipc::Error::UnsupportedAppContainer)
    ) {
        return;
    }
    let destination = Destination::new(Scheme::Https, "example.org", 443, "/").unwrap();
    let source = PolicyAuthority::new(destination.clone());
    source
        .replace_credential(Credential::new("fixture".into()))
        .unwrap();
    source.grant_manual_consent(&destination).unwrap();
    let policy = source.wire_policy().unwrap();
    let stop = Stop::new().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut boot = BrokerBoot::start(&source, 2, deadline).unwrap();
    let host = PinnedProcess::current().unwrap();
    let mut child = FixtureChild(
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "client_fixture"])
            .env("SUMIBI_PRIVATE_PEER_FIXTURE", "1")
            .env("SUMIBI_PRIVATE_WORKER", "1")
            .env("SUMIBI_PRIVATE_STALE", "1")
            .env("SUMIBI_TEST_SETTINGS", policy.settings.to_string())
            .env(
                "SUMIBI_TEST_AUTHORIZATION",
                policy.authorization.to_string(),
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let peer = PinnedProcess::from_launch_handle(child.0.as_handle()).unwrap();
    let (_, name) = boot.begin_private_accept(peer, deadline).unwrap();
    let mut remote = HANDLE::default();
    // SAFETY: duplicate the pinned Host handle only into our launched child.
    unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            HANDLE(host.as_handle().as_raw_handle()),
            HANDLE(child.0.as_raw_handle()),
            &mut remote,
            0,
            false,
            DUPLICATE_SAME_ACCESS,
        )
        .unwrap();
    }
    let input = child.0.stdin.as_mut().unwrap();
    writeln!(
        input,
        "{} {} {}",
        remote.0 as usize,
        name.bootstrap(),
        boot.name().bootstrap()
    )
    .unwrap();
    input.write_all(&[1]).unwrap();
    let mut activated = false;
    loop {
        assert!(Instant::now() < deadline);
        let result = boot
            .advance_tracked(&stop, deadline, |_| |_: JobTicket| Ok("unexpected".into()))
            .unwrap();
        match result.step {
            OwnerStep::Accepted { .. } => {}
            OwnerStep::Broker(PollStep::Activated(_)) => activated = true,
            OwnerStep::Broker(PollStep::Closed { .. }) if activated => break,
            OwnerStep::Broker(PollStep::Idle) => std::thread::yield_now(),
            _ => panic!("stale worker submitted a request"),
        }
    }
    loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert_eq!(boot.active_worker_count(), 0);
    assert!(boot
        .shutdown(Instant::now() + Duration::from_secs(1))
        .is_ok());
}

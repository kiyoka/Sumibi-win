//! Separate real broker process + protocol + Core + real owner-thread TSF.
//! Synthetic, caller-owned ACP stores only; no user document or HTTP access.
use super::*;
use std::os::windows::io::AsRawHandle;
use std::time::{Duration, Instant};
use sumibi_core::RequestId;
use sumibi_ipc::bootstrap::Mode;
use sumibi_ipc::job::{BrokerJob, Completion, Request, Reservation};
use sumibi_tip::edit::manual::ManualEdit;
use windows::Win32::Foundation::{HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::UI::WindowsAndMessaging::{
    MsgWaitForMultipleObjectsEx, MWMO_INPUTAVAILABLE, QS_ALLINPUT,
};

type Failure = Box<dyn std::error::Error>;
pub(crate) fn exercise_broker(
    manager: &ITfThreadMgr,
    client: u32,
) -> std::result::Result<(), Failure> {
    let messages: ITfMessagePump = manager.cast()?;
    let executable = std::env::current_exe()?
        .parent()
        .ok_or("missing host directory")?
        .join("sumibi-host.exe");
    if !executable.is_file() {
        return Err("build sumibi-host with --features spike before broker-test".into());
    }
    // SAFETY: real TSF calls stay in this STA and refer only to these fixtures.
    unsafe {
        for (index, case) in [
            "apply",
            "cancel",
            "cancel-after-result",
            "close",
            "new-request",
            "text",
            "text-restored",
            "selection",
            "selection-restored",
            "focus-restored",
            "revoke",
            "revoke-regranted",
            "settings",
            "readonly",
            "write-denied",
            "read-denied",
            "read-restored",
            "timeout",
            "wrong-request",
            "wrong-revision",
            "disabled",
            "ready-policy",
        ]
        .into_iter()
        .enumerate()
        {
            let id = index as u64 + 1;
            let window = crate::window_host::Window::new(manager)?;
            let store = ComObject::new(TextStore::new("prefix nihongo suffix"));
            let mut selected = store.get().selection.get();
            selected.acpStart = 7;
            selected.acpEnd = 14;
            store.get().selection.set(selected);
            store.get().window.set(window.hwnd());
            let document = Document(manager.CreateDocumentMgr()?);
            let mut context = None;
            let mut cookie = 0;
            document.0.CreateContext(
                client,
                0,
                &store.to_interface::<IUnknown>(),
                &mut context,
                &mut cookie,
            )?;
            let context = context.ok_or_else(|| windows::core::Error::from_hresult(E_FAIL))?;
            document.0.Push(&context)?;
            scope_fixture::initialise_context(&context, client)?;
            let mut association =
                crate::window_host::Association::new(manager, &window, &document.0)?;
            manager.SetFocus(&document.0)?;
            pump_tsf_messages(&messages)?;
            let other = if case == "focus-restored" {
                Some(scope_fixture::other_document(manager, client)?)
            } else {
                None
            };
            manager.SetFocus(&document.0)?;
            pump_tsf_messages(&messages)?;
            let policy = Rc::new(Cell::new(EditPolicy {
                revision: Revision {
                    input: 1,
                    settings: 1,
                    authorization: 1,
                },
                allowed: true,
            }));
            let reservation = Reservation::acquire()?;
            let deadline = Instant::now()
                + if case == "timeout" {
                    Duration::from_millis(150)
                } else {
                    Duration::from_secs(5)
                };
            let mut ticket = Some(ManualEdit::capture(
                manager,
                &context,
                client,
                policy.clone(),
                RequestId(id.into()),
                id.into(),
                deadline,
            )?);
            let mut job = reservation.start(
                executable.clone(),
                Request {
                    id,
                    revision: sumibi_protocol::message::Revision {
                        input: 1,
                        settings: if case == "ready-policy" { 2 } else { 1 },
                        authorization: 1,
                    },
                    text: ticket.as_ref().ok_or("missing target")?.source()?,
                    deadline,
                    mode: if case == "disabled" {
                        Mode::Disabled
                    } else {
                        Mode::DiagnosticFixed { delay_ms: 200 }
                    },
                },
            )?;
            match case {
                "cancel" | "close" => {
                    drop(ticket.take());
                    job.cancel()?;
                    if case == "close" {
                        windows::Win32::UI::WindowsAndMessaging::DestroyWindow(window.hwnd())?;
                    }
                }
                "new-request" => {
                    drop(ticket.take());
                    ticket = Some(ManualEdit::capture(
                        manager,
                        &context,
                        client,
                        policy.clone(),
                        RequestId(u128::from(id) + 1000),
                        u128::from(id) + 1000,
                        deadline,
                    )?);
                }
                "text" | "text-restored" => {
                    store
                        .get()
                        .external_text("prefix NIHONGO suffix".encode_utf16().collect())?;
                    pump_tsf_messages(&messages)?;
                    if case == "text-restored" {
                        store
                            .get()
                            .external_text("prefix nihongo suffix".encode_utf16().collect())?;
                        pump_tsf_messages(&messages)?;
                    }
                }
                "selection" | "selection-restored" => {
                    let original = store.get().selection.get();
                    let mut changed = original;
                    changed.acpEnd = 10;
                    store.get().selection.set(changed);
                    let sink = store
                        .get()
                        .sink
                        .borrow()
                        .clone()
                        .ok_or_else(|| windows::core::Error::from_hresult(E_FAIL))?;
                    sink.OnSelectionChange()?;
                    pump_tsf_messages(&messages)?;
                    if case == "selection-restored" {
                        store.get().selection.set(original);
                        sink.OnSelectionChange()?;
                        pump_tsf_messages(&messages)?;
                    }
                }
                "focus-restored" => {
                    let next = other.as_ref().ok_or("missing second input field")?;
                    // The cooperating owner explicitly reports the boundary.
                    // Retain the ticket to prove restored focus cannot revive it.
                    ticket.as_mut().ok_or("missing request")?.cancel()?;
                    job.cancel()?;
                    manager.SetFocus(&next.0)?;
                    assert!(manager.GetFocus()? == next.0);
                    pump_tsf_messages(&messages)?;
                    manager.SetFocus(&document.0)?;
                    pump_tsf_messages(&messages)?;
                    let reads = sumibi_tip::spike::SumibiSpikeBodyReadCount();
                    assert!(ticket
                        .as_ref()
                        .ok_or("missing original request")?
                        .source()
                        .is_err());
                    assert_eq!(sumibi_tip::spike::SumibiSpikeBodyReadCount(), reads);
                }
                "revoke" | "revoke-regranted" => {
                    let mut changed = policy.get();
                    changed.allowed = false;
                    changed.revision.authorization += 1;
                    policy.set(changed);
                    if case == "revoke-regranted" {
                        changed.allowed = true;
                        changed.revision.authorization += 1;
                        policy.set(changed);
                    }
                }
                "settings" => {
                    let mut changed = policy.get();
                    changed.revision.settings += 1;
                    policy.set(changed);
                }
                "readonly" => store.get().readonly.set(true),
                "write-denied" => store.get().deny_lock.set(true),
                "read-denied" | "read-restored" => {
                    // TSF may retain text in its own cache. The host must report
                    // loss of availability; an unannounced GetText limit alone
                    // does not revoke already cached data or an edit cookie.
                    store.get().read_limit.set(0);
                    store.get().dynamic_status.set(TS_SD_LOADING);
                    let sink = store
                        .get()
                        .sink
                        .borrow()
                        .clone()
                        .ok_or_else(|| windows::core::Error::from_hresult(E_FAIL))?;
                    sink.OnStatusChange(TS_SD_LOADING)?;
                    pump_tsf_messages(&messages)?;
                    if case == "read-restored" {
                        store.get().read_limit.set(usize::MAX);
                        store.get().dynamic_status.set(0);
                        sink.OnStatusChange(0)?;
                        pump_tsf_messages(&messages)?;
                    }
                }
                _ => {}
            }
            let before = store.get().text.borrow().clone();
            let completion = wait_completion(&messages, &job)?;
            if case == "cancel-after-result" {
                assert_eq!(completion.outcome(), Ok("日本語"));
                job.cancel()?;
            }
            let result = match completion.outcome() {
                Ok(text) => {
                    let reply_id = if case == "wrong-request" {
                        completion.id + 1
                    } else {
                        completion.id
                    };
                    let mut revision = Revision {
                        input: completion.revision.input,
                        settings: completion.revision.settings,
                        authorization: completion.revision.authorization,
                    };
                    if case == "wrong-revision" {
                        revision.input += 1;
                    }
                    // Route before consuming the owner ticket. An old job's
                    // completion must not retire the current/newer request.
                    if ticket
                        .as_ref()
                        .is_some_and(|current| current.request() == RequestId(reply_id.into()))
                    {
                        ticket
                            .take()
                            .ok_or("missing request")?
                            .finish(
                                manager,
                                client,
                                RequestId(reply_id.into()),
                                revision,
                                text.into(),
                            )
                            .map_err(|_| "target rejected result")
                    } else {
                        Err("unmatched completion")
                    }
                }
                Err(error) => {
                    if case == "disabled" || case == "ready-policy" {
                        assert_eq!(error, sumibi_ipc::Error::PolicyDenied);
                    } else if matches!(
                        case,
                        "cancel" | "cancel-after-result" | "close" | "focus-restored"
                    ) {
                        assert_eq!(error, sumibi_ipc::Error::Cancelled);
                    } else if case == "timeout" {
                        assert_eq!(error, sumibi_ipc::Error::Timeout);
                    } else {
                        return Err(format!("broker fixture / {case}: {error}").into());
                    }
                    Err("worker rejected result")
                }
            };
            if case == "new-request" {
                assert_eq!(
                    ticket
                        .as_ref()
                        .ok_or("stale reply consumed the new request")?
                        .request(),
                    RequestId(u128::from(id) + 1000)
                );
                assert!(ticket
                    .as_ref()
                    .ok_or("missing new request")?
                    .source()
                    .is_ok());
            }
            drop(ticket);
            job.shutdown(Duration::from_secs(3))?;
            if case == "apply" {
                result?;
                assert_eq!(
                    *store.get().text.borrow(),
                    "prefix 日本語 suffix".encode_utf16().collect::<Vec<_>>()
                );
                assert_eq!(store.get().undo.borrow().len(), 1);
                store.get().undo_once()?;
                assert_eq!(*store.get().text.borrow(), before);
            } else {
                assert!(result.is_err(), "{case}");
                assert_eq!(*store.get().text.borrow(), before, "{case}");
                assert!(store.get().undo.borrow().is_empty(), "{case}");
            }
            store.get().readonly.set(false);
            store.get().deny_lock.set(false);
            store.get().read_limit.set(usize::MAX);
            store.get().dynamic_status.set(0);
            association.close()?;
            manager.SetFocus(None)?;
            drop(other);
            println!("Broker / protocol / Core / TSF / {case}: OK");
        }
    }
    exercise_job_capacity(&messages)?;
    Ok(())
}

/// Real child processes have finished and joined, but no result was received.
/// Used by both the worker lifetime and DLL pre-capture admission checks.
pub(super) fn completed_jobs(
    messages: &ITfMessagePump,
    mode: Mode,
) -> std::result::Result<Vec<BrokerJob>, Failure> {
    let executable = std::env::current_exe()?
        .parent()
        .ok_or("missing host directory")?
        .join("sumibi-host.exe");
    let mut jobs = Vec::new();
    for index in 0..sumibi_ipc::job::MAX_JOBS {
        jobs.push(Reservation::acquire()?.start(
            executable.clone(),
            Request {
                id: index as u64 + 1,
                revision: sumibi_protocol::message::Revision {
                    input: 1,
                    settings: 1,
                    authorization: 1,
                },
                text: "fixed diagnostic source".into(),
                deadline: Instant::now() + Duration::from_secs(8),
                mode,
            },
        )?);
    }
    for job in &mut jobs {
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            pump_tsf_messages(messages)?;
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err("undrained job completion timed out".into());
            }
            let wait = unsafe {
                MsgWaitForMultipleObjectsEx(
                    Some(&[HANDLE(job.completion_handle().as_raw_handle())]),
                    remaining.as_millis() as u32,
                    QS_ALLINPUT,
                    MWMO_INPUTAVAILABLE,
                )
            };
            if wait == WAIT_OBJECT_0 {
                break;
            }
            if wait.0 != WAIT_OBJECT_0.0 + 1 {
                return Err("undrained job completion wait failed".into());
            }
        }
        job.shutdown(Duration::from_secs(3))?;
    }
    Ok(jobs)
}

fn exercise_job_capacity(messages: &ITfMessagePump) -> std::result::Result<(), Failure> {
    for mode in [Mode::DiagnosticFixed { delay_ms: 0 }, Mode::Disabled] {
        let label = match mode {
            Mode::DiagnosticFixed { .. } => "success",
            Mode::Disabled => "policy-denied",
            Mode::DiagnosticRevoke => unreachable!("fixture is not used by one-shot jobs"),
            Mode::DiagnosticRevokeIdle => unreachable!("fixture is not used by one-shot jobs"),
            Mode::DiagnosticRevokeCoalesced => unreachable!("fixture is not used by one-shot jobs"),
            Mode::DiagnosticRevokeAfterResult => {
                unreachable!("fixture is not used by one-shot jobs")
            }
            Mode::DiagnosticSettingsAfterResult => {
                unreachable!("fixture is not used by one-shot jobs")
            }
            Mode::DiagnosticRegrantAfterResult => {
                unreachable!("fixture is not used by one-shot jobs")
            }
            Mode::DiagnosticRevokeAtAccept => {
                unreachable!("fixture is not used by one-shot jobs")
            }
            Mode::DiagnosticRegrantAtAccept => {
                unreachable!("fixture is not used by one-shot jobs")
            }
            Mode::DiagnosticFutureReply => unreachable!("fixture is not used by one-shot jobs"),
            Mode::DiagnosticFanoutFailedNotice => {
                unreachable!("fixture is not used by one-shot jobs")
            }
            Mode::DiagnosticFanoutLaggingNotice => {
                unreachable!("fixture is not used by one-shot jobs")
            }
        };
        let jobs = completed_jobs(messages, mode)?;
        assert!(matches!(
            Reservation::acquire(),
            Err(sumibi_ipc::Error::Capacity)
        ));
        println!("Broker / admission / completed-undrained / {label}: OK");
        let mut replies = Vec::new();
        for job in &jobs {
            let reply = job.try_result().ok_or("missing retained completion")?;
            match mode {
                Mode::DiagnosticFixed { .. } => assert_eq!(reply.outcome(), Ok("日本語")),
                Mode::Disabled => assert_eq!(reply.outcome(), Err(sumibi_ipc::Error::PolicyDenied)),
                Mode::DiagnosticRevoke => unreachable!("fixture is not used by one-shot jobs"),
                Mode::DiagnosticRevokeIdle => unreachable!("fixture is not used by one-shot jobs"),
                Mode::DiagnosticRevokeCoalesced => {
                    unreachable!("fixture is not used by one-shot jobs")
                }
                Mode::DiagnosticRevokeAfterResult => {
                    unreachable!("fixture is not used by one-shot jobs")
                }
                Mode::DiagnosticSettingsAfterResult => {
                    unreachable!("fixture is not used by one-shot jobs")
                }
                Mode::DiagnosticRegrantAfterResult => {
                    unreachable!("fixture is not used by one-shot jobs")
                }
                Mode::DiagnosticRevokeAtAccept => {
                    unreachable!("fixture is not used by one-shot jobs")
                }
                Mode::DiagnosticRegrantAtAccept => {
                    unreachable!("fixture is not used by one-shot jobs")
                }
                Mode::DiagnosticFutureReply => {
                    unreachable!("fixture is not used by one-shot jobs")
                }
                Mode::DiagnosticFanoutFailedNotice => {
                    unreachable!("fixture is not used by one-shot jobs")
                }
                Mode::DiagnosticFanoutLaggingNotice => {
                    unreachable!("fixture is not used by one-shot jobs")
                }
            }
            assert!(job.try_result().is_none());
            replies.push(reply);
        }
        drop(jobs);
        assert!(matches!(
            Reservation::acquire(),
            Err(sumibi_ipc::Error::Capacity)
        ));
        println!("Broker / admission / received-result-retained / {label}: OK");
        drop(replies.pop());
        let slot = Reservation::acquire()?;
        assert!(matches!(
            Reservation::acquire(),
            Err(sumibi_ipc::Error::Capacity)
        ));
        // A failed start also releases exactly its own reservation.
        assert!(slot
            .start(
                std::path::PathBuf::from("relative-host.exe"),
                Request {
                    id: 100,
                    revision: sumibi_protocol::message::Revision {
                        input: 1,
                        settings: 1,
                        authorization: 1
                    },
                    text: "fixed".into(),
                    deadline: Instant::now() + Duration::from_secs(2),
                    mode,
                },
            )
            .is_err());
        let slot = Reservation::acquire()?;
        assert!(matches!(
            Reservation::acquire(),
            Err(sumibi_ipc::Error::Capacity)
        ));
        assert!(matches!(
            slot.start(
                std::env::current_exe()?.with_file_name("sumibi-host.exe"),
                Request {
                    id: 101,
                    revision: sumibi_protocol::message::Revision {
                        input: 1,
                        settings: 1,
                        authorization: 1,
                    },
                    text: "a\u{fffc}b".into(),
                    deadline: Instant::now() + Duration::from_secs(2),
                    mode,
                },
            ),
            Err(sumibi_ipc::Error::InvalidArgument)
        ));
        let slot = Reservation::acquire()?;
        assert!(matches!(
            Reservation::acquire(),
            Err(sumibi_ipc::Error::Capacity)
        ));
        drop((slot, replies));
        let slots = (0..sumibi_ipc::job::MAX_JOBS)
            .map(|_| Reservation::acquire())
            .collect::<std::result::Result<Vec<_>, _>>()?;
        drop(slots);
        println!("Broker / admission / release-and-failed-start / {label}: OK");
    }
    Ok(())
}

fn wait_completion(
    messages: &ITfMessagePump,
    job: &BrokerJob,
) -> std::result::Result<Completion, Failure> {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        pump_tsf_messages(messages)?;
        if let Some(completion) = job.try_result() {
            return Ok(completion);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err("broker completion did not arrive".into());
        }
        // SAFETY: owned event remains live; messages are handled by the owner
        // loop, not from a worker callback or while an edit lock is held.
        let wait = unsafe {
            MsgWaitForMultipleObjectsEx(
                Some(&[HANDLE(job.completion_handle().as_raw_handle())]),
                remaining.as_millis() as u32,
                QS_ALLINPUT,
                MWMO_INPUTAVAILABLE,
            )
        };
        if wait == WAIT_TIMEOUT {
            return Err("broker completion timed out".into());
        }
        if wait.0 != WAIT_OBJECT_0.0 && wait.0 != WAIT_OBJECT_0.0 + 1 {
            return Err("broker completion wait failed".into());
        }
    }
}

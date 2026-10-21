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
            "close",
            "new-request",
            "text",
            "text-restored",
            "selection",
            "selection-restored",
            "focus-restored",
            "revoke",
            "settings",
            "readonly",
            "write-denied",
            "read-denied",
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
            let window = crate::window_host::Window::new()?;
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
            let mut association =
                crate::window_host::Association::new(manager, &window, &document.0)?;
            manager.SetFocus(&document.0)?;
            pump_tsf_messages()?;
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
            let mut other = None;
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
                    pump_tsf_messages()?;
                    if case == "text-restored" {
                        store
                            .get()
                            .external_text("prefix nihongo suffix".encode_utf16().collect())?;
                        pump_tsf_messages()?;
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
                    pump_tsf_messages()?;
                    if case == "selection-restored" {
                        store.get().selection.set(original);
                        sink.OnSelectionChange()?;
                        pump_tsf_messages()?;
                    }
                }
                "focus-restored" => {
                    let next = Document(manager.CreateDocumentMgr()?);
                    manager.SetFocus(&next.0)?;
                    manager.SetFocus(&document.0)?;
                    other = Some(next);
                }
                "revoke" => {
                    let mut changed = policy.get();
                    changed.allowed = false;
                    changed.revision.authorization += 1;
                    policy.set(changed);
                }
                "settings" => {
                    let mut changed = policy.get();
                    changed.revision.settings += 1;
                    policy.set(changed);
                }
                "readonly" => store.get().readonly.set(true),
                "write-denied" => store.get().deny_lock.set(true),
                "read-denied" => store.get().read_limit.set(0),
                _ => {}
            }
            let before = store.get().text.borrow().clone();
            let completion = wait_completion(&job)?;
            let result = match completion.outcome {
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
                            .finish(manager, client, RequestId(reply_id.into()), revision, text)
                            .map_err(|_| "target rejected result")
                    } else {
                        Err("unmatched completion")
                    }
                }
                Err(error) => {
                    if case == "disabled" || case == "ready-policy" {
                        assert_eq!(error, sumibi_ipc::Error::PolicyDenied);
                    } else if case == "cancel" || case == "close" {
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
            association.close()?;
            manager.SetFocus(None)?;
            drop(other);
            println!("Broker / protocol / Core / TSF / {case}: OK");
        }
    }
    Ok(())
}

fn wait_completion(job: &BrokerJob) -> std::result::Result<Completion, Failure> {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        pump_tsf_messages()?;
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

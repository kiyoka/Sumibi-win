//! Real DLL preserved key -> EXE worker -> separate Host -> DLL Core/TSF write.
use super::*;
use crate::broker_bridge::{Bridge, Delivery};
use std::time::{Duration, Instant};
use sumibi_ipc::bootstrap::Mode;
use sumibi_tip::spike::{
    ISpikeBroker, ISpikeBrokerTransport, ISpikeControl, COMMAND_CONVERT, FUNCTION_TEST_BROKER,
    FUNCTION_TEST_CONTROL, FUNCTION_TEST_KEY_SINK, FUNCTION_TEST_MANAGER_SINK,
};
use windows::Win32::Foundation::{LPARAM, WAIT_OBJECT_0, WAIT_TIMEOUT, WPARAM};
use windows::Win32::System::Threading::WaitForSingleObject;
use windows::Win32::UI::WindowsAndMessaging::{
    DestroyWindow, MsgWaitForMultipleObjectsEx, MWMO_INPUTAVAILABLE, QS_ALLINPUT,
};

type Failure = Box<dyn std::error::Error>;
pub(crate) fn exercise_broker_keys(
    manager: &ITfThreadMgr,
    client: u32,
) -> std::result::Result<(), Failure> {
    let messages: ITfMessagePump = manager.cast()?;
    unsafe {
        for case in [
            "apply",
            "shared-ready",
            "shared-revoke",
            "shared-inflight-revoke",
            "shared-text-restored",
            "shared-selection-restored",
            "shared-focus-restored",
            "reuse",
            "reuse-late-cancel",
            "reuse-delivery-reentry",
            "reuse-escape",
            "reuse-text-restored",
            "restart-after-cancel",
            "double-key",
            "capacity",
            "capacity-completed",
            "capacity-result",
            "escape",
            "enter",
            "text-restored",
            "selection-restored",
            "focus-restored",
            "readonly",
            "write-denied",
            "revoke",
            "stale-reply",
            "replace-transport",
            "wrong-revision",
            "invalid-result",
            "disabled",
            "close",
        ] {
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
            let other = if matches!(case, "focus-restored" | "shared-focus-restored") {
                Some(scope_fixture::other_document(manager, client)?)
            } else {
                None
            };
            manager.SetFocus(&document.0)?;
            let provider = manager.GetFunctionProvider(&sumibi_tip::CLSID_SUMIBI_TIP)?;
            let control: ISpikeControl = provider
                .GetFunction(&FUNCTION_TEST_CONTROL, &ISpikeControl::IID)?
                .cast()?;
            let broker: ISpikeBroker = provider
                .GetFunction(&FUNCTION_TEST_BROKER, &ISpikeBroker::IID)?
                .cast()?;
            let sink: ITfKeyEventSink = provider
                .GetFunction(&FUNCTION_TEST_KEY_SINK, &ITfKeyEventSink::IID)?
                .cast()?;
            let (shared, bridge) = if case.starts_with("shared-") {
                let (fixture, worker) = crate::broker_shared_fixture::Fixture::start(
                    case == "shared-inflight-revoke",
                    matches!(
                        case,
                        "shared-text-restored"
                            | "shared-selection-restored"
                            | "shared-focus-restored"
                    ),
                )?;
                (Some(fixture), Bridge::new_prepared(worker)?)
            } else {
                (
                    None,
                    Bridge::new(if case == "disabled" {
                        Mode::Disabled
                    } else {
                        Mode::DiagnosticFixed { delay_ms: 200 }
                    })?,
                )
            };
            let transport = bridge.to_interface::<ISpikeBrokerTransport>();
            let reentry = Rc::new(Cell::new(false));
            if case == "reuse-delivery-reentry" {
                let nested = bridge.clone();
                let nested_broker = broker.clone();
                let checked = reentry.clone();
                *store.get().write_hook.borrow_mut() = Some(Box::new(move || {
                    let transport = nested.to_interface::<ISpikeBrokerTransport>();
                    let mut id = u64::MAX;
                    let reserved = transport.Reserve(&mut id);
                    let drained = nested.get().drain(&nested_broker);
                    let shutdown = nested.get().shutdown();
                    let refused =
                        reserved.is_err() && id == 0 && drained.is_err() && shutdown.is_err();
                    checked.set(refused);
                    if refused {
                        Ok(())
                    } else {
                        Err(E_FAIL.into())
                    }
                }));
            }
            broker.Attach(context.as_raw(), transport.as_raw()).ok()?;
            let keys: ITfKeystrokeMgr = manager.cast()?;
            let capacity = if case == "capacity" {
                (0..sumibi_ipc::job::MAX_JOBS)
                    .map(|_| sumibi_ipc::job::Reservation::acquire())
                    .collect::<std::result::Result<Vec<_>, _>>()?
            } else {
                Vec::new()
            };
            let completed_jobs = if matches!(case, "capacity-completed" | "capacity-result") {
                super::broker_host::completed_jobs(
                    &messages,
                    Mode::DiagnosticFixed { delay_ms: 0 },
                )?
            } else {
                Vec::new()
            };
            let mut retained_results = Vec::new();
            let completed_jobs = if case == "capacity-result" {
                for job in &completed_jobs {
                    retained_results.push(job.try_result().ok_or("missing retained result")?);
                }
                drop(completed_jobs);
                Vec::new()
            } else {
                completed_jobs
            };
            let at_capacity = matches!(case, "capacity" | "capacity-completed" | "capacity-result");
            let before_reads = store.get().reads.get();
            if case == "shared-revoke" {
                shared.as_ref().ok_or("missing shared fixture")?.revoke()?;
                let handle = bridge
                    .get()
                    .wait_handle()
                    .ok_or("missing idle worker event")?;
                if WaitForSingleObject(handle, 5000) != WAIT_OBJECT_0 {
                    return Err("shared policy notification timed out".into());
                }
                assert!(!sink.OnPreservedKey(&context, &COMMAND_CONVERT)?.as_bool());
                assert_eq!(store.get().reads.get(), before_reads);
                assert!(!bridge.get().active());
                assert!(store.get().undo.borrow().is_empty());
                let mut pending = u64::MAX;
                broker.Pending(&mut pending).ok()?;
                assert_eq!(pending, 0);
                bridge.get().shutdown()?;
                shared.ok_or("missing shared fixture")?.finish()?;
                control.RevokeContext().ok()?;
                association.close()?;
                manager.SetFocus(None)?;
                println!("DLL key / shared Host / revoke before capture: OK");
                continue;
            }
            // SimulatePreservedKey itself may ask the store for TSF metadata.
            // Isolate the DLL callback when measuring its pre-capture reads.
            let eaten = if at_capacity {
                sink.OnPreservedKey(&context, &COMMAND_CONVERT)?.as_bool()
            } else {
                keys.SimulatePreservedKey(&context, &COMMAND_CONVERT)?
                    .as_bool()
            };
            if at_capacity {
                assert!(!eaten);
                assert_eq!(store.get().reads.get(), before_reads);
                assert!(!bridge.get().active());
                assert!(store.get().undo.borrow().is_empty());
                let mut pending = u64::MAX;
                broker.Pending(&mut pending).ok()?;
                assert_eq!(pending, 0);
                drop((capacity, completed_jobs, retained_results));
                // Releasing the retained work permits the same key exactly
                // once without reattaching or changing the input selection.
                assert!(sink.OnPreservedKey(&context, &COMMAND_CONVERT)?.as_bool());
                assert_eq!(wait_bridge(&messages, &bridge, &broker)?, Delivery::Applied);
                assert_eq!(store.get().undo.borrow().len(), 1);
                store.get().undo_once()?;
                bridge.get().shutdown()?;
                control.RevokeContext().ok()?;
                association.close()?;
                manager.SetFocus(None)?;
                println!("DLL key / broker / {case} before capture and reuse: OK");
                continue;
            }
            assert!(eaten, "{case}");
            let mut id = 0;
            broker.Pending(&mut id).ok()?;
            assert_ne!(id, 0);
            assert!(bridge.get().active());
            let mut replacement = None;
            if matches!(
                case,
                "shared-inflight-revoke"
                    | "shared-text-restored"
                    | "shared-selection-restored"
                    | "shared-focus-restored"
            ) {
                let fixture = shared.as_ref().ok_or("missing shared fixture")?;
                let deadline = Instant::now() + Duration::from_secs(3);
                while !fixture.submitted() {
                    if Instant::now() >= deadline {
                        return Err("shared request was not submitted".into());
                    }
                    pump_tsf_messages(&messages)?;
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
            match case {
                "shared-inflight-revoke" => {
                    let fixture = shared.as_ref().ok_or("missing shared fixture")?;
                    fixture.revoke()?;
                }
                "double-key" => {
                    let reads = store.get().reads.get();
                    assert!(!keys
                        .SimulatePreservedKey(&context, &COMMAND_CONVERT)?
                        .as_bool());
                    let mut pending = 0;
                    broker.Pending(&mut pending).ok()?;
                    assert_eq!(pending, id);
                    assert_eq!(store.get().reads.get(), reads);
                }
                "escape" | "enter" | "restart-after-cancel" => {
                    let escape = case != "enter";
                    let key = WPARAM(if escape { 0x1b } else { 0x0d });
                    // Same DLL callback as normal routing; explicit diagnostic
                    // interface avoids depending on OS foreground permission.
                    assert!(sink.OnTestKeyDown(&context, key, LPARAM(1))?.as_bool());
                    assert_eq!(sink.OnKeyDown(&context, key, LPARAM(1))?.as_bool(), escape);
                    let mut pending = u64::MAX;
                    broker.Pending(&mut pending).ok()?;
                    assert_eq!(pending, 0);
                }
                "text-restored" | "shared-text-restored" => {
                    store
                        .get()
                        .external_text("prefix NIHONGO suffix".encode_utf16().collect())?;
                    pump_tsf_messages(&messages)?;
                    store
                        .get()
                        .external_text("prefix nihongo suffix".encode_utf16().collect())?;
                    pump_tsf_messages(&messages)?;
                }
                "selection-restored" | "shared-selection-restored" => {
                    let original = store.get().selection.get();
                    let mut changed = original;
                    changed.acpEnd = 10;
                    let sink = store
                        .get()
                        .sink
                        .borrow()
                        .clone()
                        .ok_or_else(|| windows::core::Error::from_hresult(E_FAIL))?;
                    store.get().selection.set(changed);
                    sink.OnSelectionChange()?;
                    pump_tsf_messages(&messages)?;
                    store.get().selection.set(original);
                    sink.OnSelectionChange()?;
                    pump_tsf_messages(&messages)?;
                }
                "focus-restored" | "shared-focus-restored" => {
                    let next = other.as_ref().ok_or("missing second input field")?;
                    manager.SetFocus(&next.0)?;
                    assert!(manager.GetFocus()? == next.0);
                    // Headless logical SetFocus does not reliably deliver a
                    // desktop focus notification. Exercise the same real DLL
                    // callback explicitly, as in key-test's focus fixture.
                    let focus_sink: ITfThreadMgrEventSink = provider
                        .GetFunction(&FUNCTION_TEST_MANAGER_SINK, &ITfThreadMgrEventSink::IID)?
                        .cast()?;
                    focus_sink.OnSetFocus(&next.0, &document.0)?;
                    pump_tsf_messages(&messages)?;
                    manager.SetFocus(&document.0)?;
                    pump_tsf_messages(&messages)?;
                }
                "readonly" => store.get().readonly.set(true),
                "write-denied" => store.get().deny_lock.set(true),
                "revoke" | "close" => {
                    control.RevokeContext().ok()?;
                    if case == "close" {
                        DestroyWindow(window.hwnd())?;
                    }
                }
                "stale-reply" => {
                    assert!(broker
                        .Complete(id + 100, 1, 1, 1, std::ptr::null(), 0)
                        .is_err());
                    let mut pending = 0;
                    broker.Pending(&mut pending).ok()?;
                    assert_eq!(pending, id);
                }
                "replace-transport" => {
                    control.RevokeContext().ok()?;
                    let next = Bridge::new(Mode::DiagnosticFixed { delay_ms: 200 })?;
                    let next_transport = next.to_interface::<ISpikeBrokerTransport>();
                    broker
                        .Attach(context.as_raw(), next_transport.as_raw())
                        .ok()?;
                    assert!(keys
                        .SimulatePreservedKey(&context, &COMMAND_CONVERT)?
                        .as_bool());
                    let mut current = 0;
                    broker.Pending(&mut current).ok()?;
                    assert!(current > id, "reattach must not reuse a request ID");
                    assert!(broker.Complete(id, 1, 1, 1, std::ptr::null(), 0).is_err());
                    assert_eq!(
                        wait_bridge(&messages, &bridge, &broker)?,
                        Delivery::Discarded
                    );
                    let mut still_current = 0;
                    broker.Pending(&mut still_current).ok()?;
                    assert_eq!(
                        still_current, current,
                        "old completion must preserve the new ticket"
                    );
                    replacement = Some(next);
                }
                "wrong-revision" => {
                    let text = [0x65e5, 0x672c, 0x8a9e];
                    assert!(broker.Complete(id, 2, 1, 1, text.as_ptr(), 3).is_err());
                }
                "invalid-result" => {
                    let text = [0xd800];
                    assert!(broker.Complete(id, 1, 1, 1, text.as_ptr(), 1).is_err());
                    let mut pending = 1;
                    broker.Pending(&mut pending).ok()?;
                    assert_eq!(pending, 0);
                }
                _ => {}
            }
            let delivery =
                wait_bridge(&messages, replacement.as_ref().unwrap_or(&bridge), &broker)?;
            if case == "reuse-delivery-reentry" {
                assert!(reentry.get());
            }
            let apply = matches!(
                case,
                "apply" | "shared-ready" | "double-key" | "stale-reply" | "replace-transport"
            ) || case.starts_with("reuse");
            assert_eq!(
                delivery,
                if apply {
                    Delivery::Applied
                } else {
                    Delivery::Discarded
                },
                "{case}"
            );
            if apply {
                assert_eq!(
                    *store.get().text.borrow(),
                    "prefix 日本語 suffix".encode_utf16().collect::<Vec<_>>()
                );
                assert_eq!(store.get().undo.borrow().len(), 1);
                assert!(broker.Complete(id, 1, 1, 1, std::ptr::null(), 0).is_err());
                store.get().undo_once()?;
            } else {
                assert!(store.get().undo.borrow().is_empty(), "{case}");
            }
            assert_eq!(
                *store.get().text.borrow(),
                "prefix nihongo suffix".encode_utf16().collect::<Vec<_>>(),
                "{case}"
            );
            if case.starts_with("reuse") || case == "restart-after-cancel" {
                let first = bridge.get().last_exchange();
                assert_eq!(first.is_some(), case != "restart-after-cancel");
                assert!(sink.OnPreservedKey(&context, &COMMAND_CONVERT)?.as_bool());
                let mut next = 0;
                broker.Pending(&mut next).ok()?;
                assert!(next > id);
                match case {
                    "reuse-late-cancel" => {
                        transport.Cancel(id).ok()?;
                    }
                    "reuse-escape" => {
                        assert!(sink.OnKeyDown(&context, WPARAM(0x1b), LPARAM(1))?.as_bool());
                    }
                    "reuse-text-restored" => {
                        store
                            .get()
                            .external_text("prefix NIHONGO suffix".encode_utf16().collect())?;
                        pump_tsf_messages(&messages)?;
                        store
                            .get()
                            .external_text("prefix nihongo suffix".encode_utf16().collect())?;
                        pump_tsf_messages(&messages)?;
                    }
                    _ => {}
                }
                let second_applied = !matches!(case, "reuse-escape" | "reuse-text-restored");
                assert_eq!(
                    wait_bridge(&messages, &bridge, &broker)?,
                    if second_applied {
                        Delivery::Applied
                    } else {
                        Delivery::Discarded
                    }
                );
                if second_applied {
                    let (pid, wire) = bridge
                        .get()
                        .last_exchange()
                        .ok_or("missing reused exchange")?;
                    if let Some((previous, previous_wire)) = first {
                        assert_eq!(pid, previous, "same launched child must be reused");
                        assert_eq!(wire, previous_wire + 1);
                    } else {
                        assert_eq!(wire, 1, "explicit retry after cancel uses a new connection");
                    }
                    assert_eq!(
                        *store.get().text.borrow(),
                        "prefix 日本語 suffix".encode_utf16().collect::<Vec<_>>()
                    );
                    assert_eq!(store.get().undo.borrow().len(), 1);
                    store.get().undo_once()?;
                } else {
                    assert!(store.get().undo.borrow().is_empty());
                }
                assert_eq!(
                    *store.get().text.borrow(),
                    "prefix nihongo suffix".encode_utf16().collect::<Vec<_>>()
                );
            }
            store.get().readonly.set(false);
            store.get().write_hook.borrow_mut().take();
            store.get().deny_lock.set(false);
            bridge.get().shutdown()?;
            if let Some(fixture) = shared {
                fixture.finish()?;
            }
            if let Some(bridge) = &replacement {
                bridge.get().shutdown()?;
            }
            control.RevokeContext().ok()?;
            association.close()?;
            manager.SetFocus(None)?;
            drop(other);
            println!("DLL key / broker / Core / TSF / {case}: OK");
        }
    }
    Ok(())
}
fn wait_bridge(
    messages: &ITfMessagePump,
    bridge: &ComObject<Bridge>,
    control: &ISpikeBroker,
) -> std::result::Result<Delivery, Failure> {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        pump_tsf_messages(messages)?;
        if let Some(delivery) = bridge.get().drain(control)? {
            return Ok(delivery);
        }
        let handle = bridge.get().wait_handle().ok_or("missing broker job")?;
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err("DLL broker completion timed out".into());
        }
        let wait = unsafe {
            MsgWaitForMultipleObjectsEx(
                Some(&[handle]),
                remaining.as_millis() as u32,
                QS_ALLINPUT,
                MWMO_INPUTAVAILABLE,
            )
        };
        if wait == WAIT_TIMEOUT || (wait.0 != WAIT_OBJECT_0.0 && wait.0 != WAIT_OBJECT_0.0 + 1) {
            return Err("DLL broker wait failed".into());
        }
    }
}

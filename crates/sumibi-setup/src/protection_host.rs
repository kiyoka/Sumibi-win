//! Real TSF metadata gate, including the actual DLL and bounded EXE worker.
use super::*;
use crate::broker_bridge::{Bridge, Delivery};
use std::time::{Duration, Instant};
use sumibi_ipc::bootstrap::Mode;
use sumibi_tip::spike::{
    ISpikeBroker, ISpikeBrokerTransport, ISpikeControl, COMMAND_CONVERT, FUNCTION_TEST_BROKER,
    FUNCTION_TEST_CONTROL, FUNCTION_TEST_KEY_SINK,
};
use windows::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::WindowsAndMessaging::{
    MsgWaitForMultipleObjectsEx, MWMO_INPUTAVAILABLE, QS_ALLINPUT,
};
type Failure = Box<dyn std::error::Error>;
fn cases() -> Vec<(&'static str, ScopeSpec)> {
    let mut cases = vec![
        ("text", ScopeSpec::List(vec![IS_TEXT])),
        ("chat", ScopeSpec::List(vec![IS_CHAT])),
        (
            "chat-no-emoji",
            ScopeSpec::List(vec![IS_CHAT_WITHOUT_EMOJI]),
        ),
        ("mixed-allowed", ScopeSpec::List(vec![IS_TEXT, IS_CHAT])),
        ("missing", ScopeSpec::Absent),
        ("wrong-type", ScopeSpec::WrongType),
        ("scope-error", ScopeSpec::Failure),
        ("null", ScopeSpec::Null),
        ("empty", ScopeSpec::Empty),
        ("over-limit", ScopeSpec::OverLimit),
        ("allocated-error", ScopeSpec::AllocatedFailure),
    ];
    for (name, scope) in [
        ("password", IS_PASSWORD),
        ("numeric-password", IS_NUMERIC_PASSWORD),
        ("private", IS_PRIVATE),
        ("numeric-pin", IS_NUMERIC_PIN),
        ("alpha-pin", IS_ALPHANUMERIC_PIN),
        ("alpha-pin-set", IS_ALPHANUMERIC_PIN_SET),
        ("login", IS_LOGINNAME),
        ("default", IS_DEFAULT),
        ("new-scope", InputScope(1000)),
        ("grammar", IS_XML),
    ] {
        cases.push((name, ScopeSpec::List(vec![scope])));
    }
    cases.push((
        "mixed-password",
        ScopeSpec::List(vec![IS_TEXT, IS_PASSWORD]),
    ));
    for name in [
        "readonly",
        "loading",
        "hidden-possible",
        "unknown-status",
        "unsupported-static",
        "keyboard-disabled",
        "empty-context",
        "keyboard-unset",
        "empty-unset",
        "compartment-wrong-type",
        "scope-during-read",
        "private-after-send",
        "allowed-scope-change",
        "private-restored",
        "keyboard-restored",
        "empty-restored",
        "source-recheck",
    ] {
        cases.push((name, ScopeSpec::List(vec![IS_TEXT])));
    }
    cases
}
pub(crate) fn exercise_protection(
    manager: &ITfThreadMgr,
    client: u32,
) -> std::result::Result<(), Failure> {
    let cases = cases();
    let total = cases.len();
    // Probe owns this module. Count DLL calls separately from MSCTF's own
    // internal body queries during application-property fallback processing.
    let module = unsafe {
        windows::Win32::System::LibraryLoader::GetModuleHandleW(windows::core::w!(
            "sumibi_tip.dll"
        ))?
    };
    let counter: unsafe extern "system" fn() -> u64 = unsafe {
        std::mem::transmute(
            windows::Win32::System::LibraryLoader::GetProcAddress(
                module,
                windows::core::s!("SumibiSpikeBodyReadCount"),
            )
            .ok_or("missing spike body counter")?,
        )
    };
    let mut framework_copies = 0;
    unsafe {
        for (index, (case, spec)) in cases.into_iter().enumerate() {
            let window = crate::window_host::Window::new()?;
            let store = ComObject::new(TextStore::new("nihongo"));
            *store.get().scopes.borrow_mut() = spec;
            match case {
                "readonly" => store.get().readonly.set(true),
                "loading" => store.get().dynamic_status.set(TS_SD_LOADING),
                "hidden-possible" => store.get().static_status.set(0),
                "unknown-status" => store.get().dynamic_status.set(0x8000_0000),
                "unsupported-static" => store
                    .get()
                    .static_status
                    .set(TS_SS_NOHIDDENTEXT | TS_SS_REGIONS),
                _ => {}
            }
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
            pump_tsf_messages()?;
            let compartments: ITfCompartmentMgr = context.cast()?;
            match case {
                "keyboard-disabled" => compartments
                    .GetCompartment(&GUID_COMPARTMENT_KEYBOARD_DISABLED)?
                    .SetValue(client, &VARIANT::from(1i32))?,
                "empty-context" => compartments
                    .GetCompartment(&GUID_COMPARTMENT_EMPTYCONTEXT)?
                    .SetValue(client, &VARIANT::from(1i32))?,
                "keyboard-unset" => {
                    compartments.ClearCompartment(client, &GUID_COMPARTMENT_KEYBOARD_DISABLED)?
                }
                "empty-unset" => {
                    compartments.ClearCompartment(client, &GUID_COMPARTMENT_EMPTYCONTEXT)?
                }
                "compartment-wrong-type" => {
                    // The native compartment enforces VT_I4 before our gate
                    // can observe an invalid type. Record that boundary honestly.
                    assert!(compartments
                        .GetCompartment(&GUID_COMPARTMENT_KEYBOARD_DISABLED)?
                        .SetValue(client, &VARIANT::from(true))
                        .is_err());
                    association.close()?;
                    manager.SetFocus(None)?;
                    println!("Input protection / invalid compartment type rejected by TSF: OK");
                    continue;
                }
                _ => {}
            }
            if case == "source-recheck" {
                let policy = Rc::new(Cell::new(EditPolicy {
                    allowed: true,
                    revision: Revision {
                        input: 1,
                        settings: 1,
                        authorization: 1,
                    },
                }));
                let ticket = sumibi_tip::edit::manual::ManualEdit::capture(
                    manager,
                    &context,
                    client,
                    policy,
                    sumibi_core::RequestId(index as u128 + 1),
                    index as u128 + 1,
                    Instant::now() + Duration::from_secs(5),
                )?;
                let reads = store.get().body_reads.get();
                *store.get().scopes.borrow_mut() = ScopeSpec::List(vec![IS_PRIVATE]); // Deliberately no notification.
                assert!(ticket.source().is_err());
                assert_eq!(store.get().body_reads.get(), reads);
                drop(ticket);
                association.close()?;
                manager.SetFocus(None)?;
                println!("Input protection / source metadata recheck: OK");
                continue;
            }
            let provider = manager.GetFunctionProvider(&sumibi_tip::CLSID_SUMIBI_TIP)?;
            let control: ISpikeControl = provider
                .GetFunction(&FUNCTION_TEST_CONTROL, &ISpikeControl::IID)?
                .cast()?;
            let broker: ISpikeBroker = provider
                .GetFunction(&FUNCTION_TEST_BROKER, &ISpikeBroker::IID)?
                .cast()?;
            let keys: ITfKeyEventSink = provider
                .GetFunction(&FUNCTION_TEST_KEY_SINK, &ITfKeyEventSink::IID)?
                .cast()?;
            let bridge = Bridge::new(Mode::DiagnosticFixed { delay_ms: 100 })?;
            let transport = bridge.to_interface::<ISpikeBrokerTransport>();
            broker.Attach(context.as_raw(), transport.as_raw()).ok()?;
            let reads = store.get().body_reads.get();
            let dll_reads = counter();
            if case == "scope-during-read" {
                store.get().protect_on_read.set(true);
            }
            // Isolate DLL reads from Windows' own key simulation reads.
            let eaten = keys.OnPreservedKey(&context, &COMMAND_CONVERT)?.as_bool();
            let allowed = matches!(case, "text" | "chat" | "chat-no-emoji" | "mixed-allowed");
            let changing = matches!(
                case,
                "private-after-send"
                    | "allowed-scope-change"
                    | "private-restored"
                    | "keyboard-restored"
                    | "empty-restored"
            );
            if !allowed && !changing {
                assert!(!eaten, "{case}");
                assert!(!bridge.get().active(), "{case}: must not start a worker");
                let mut pending = 1;
                broker.Pending(&mut pending).ok()?;
                assert_eq!(pending, 0);
                if case != "scope-during-read" {
                    assert_eq!(counter(), dll_reads, "{case}: DLL must not read text");
                    framework_copies += store.get().body_reads.get() - reads;
                }
            } else {
                assert!(eaten, "{case}");
                assert!(
                    counter() > dll_reads,
                    "positive cases must exercise the actual DLL reader"
                );
                if matches!(
                    case,
                    "private-after-send" | "private-restored" | "allowed-scope-change"
                ) {
                    *store.get().scopes.borrow_mut() =
                        ScopeSpec::List(vec![if case == "allowed-scope-change" {
                            IS_CHAT
                        } else {
                            IS_PRIVATE
                        }]);
                    notify_scope(&store)?;
                    pump_tsf_messages()?;
                    if case == "private-restored" {
                        *store.get().scopes.borrow_mut() = ScopeSpec::List(vec![IS_TEXT]);
                        notify_scope(&store)?;
                        pump_tsf_messages()?;
                    }
                }
                if matches!(case, "keyboard-restored" | "empty-restored") {
                    let guid = if case == "keyboard-restored" {
                        GUID_COMPARTMENT_KEYBOARD_DISABLED
                    } else {
                        GUID_COMPARTMENT_EMPTYCONTEXT
                    };
                    let compartment = compartments.GetCompartment(&guid)?;
                    compartment.SetValue(client, &VARIANT::from(1i32))?;
                    compartment.SetValue(client, &VARIANT::from(0i32))?;
                }
                assert_eq!(
                    wait(&bridge, &broker)?,
                    if allowed {
                        Delivery::Applied
                    } else {
                        Delivery::Discarded
                    },
                    "{case}"
                );
            }
            assert_eq!(
                *store.get().text.borrow(),
                if allowed { "日本語" } else { "nihongo" }
                    .encode_utf16()
                    .collect::<Vec<_>>(),
                "{case}"
            );
            assert_eq!(
                store.get().undo.borrow().len(),
                usize::from(allowed),
                "{case}"
            );
            bridge.get().shutdown()?;
            control.RevokeContext().ok()?;
            association.close()?;
            manager.SetFocus(None)?;
            println!("Input protection / {case}: OK");
        }
    }
    println!("Input protection: {total} scenarios passed");
    println!("MSCTF-internal body copies during rejected synthetic metadata probes: {framework_copies}; DLL body reads and broker jobs: zero");
    Ok(())
}
fn notify_scope(store: &ComObject<TextStore>) -> Result<()> {
    let sink = store.get().sink.borrow().clone().ok_or(E_FAIL)?;
    let end = store.get().text.borrow().len() as i32;
    unsafe {
        sink.OnAttrsChange(0, end, &[GUID_PROP_INPUTSCOPE])?;
    }
    Ok(())
}
fn wait(
    bridge: &ComObject<Bridge>,
    broker: &ISpikeBroker,
) -> std::result::Result<Delivery, Failure> {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        pump_tsf_messages()?;
        if let Some(delivery) = bridge.get().drain(broker)? {
            return Ok(delivery);
        }
        let handle = bridge
            .get()
            .wait_handle()
            .ok_or("missing protected request")?;
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err("protected request timed out".into());
        }
        let status = unsafe {
            MsgWaitForMultipleObjectsEx(
                Some(&[handle]),
                remaining.as_millis() as u32,
                QS_ALLINPUT,
                MWMO_INPUTAVAILABLE,
            )
        };
        if status == WAIT_TIMEOUT
            || (status.0 != WAIT_OBJECT_0.0 && status.0 != WAIT_OBJECT_0.0 + 1)
        {
            return Err("protected request wait failed".into());
        }
    }
}

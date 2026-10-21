//! Real DLL-owned selection composition. No user documents or physical keys.
use super::*;
use sumibi_tip::spike::{
    ISpikeControl, ISpikeDelayed, COMMAND_CONVERT, FUNCTION_TEST_CONTROL, FUNCTION_TEST_DELAYED,
    FUNCTION_TEST_KEY_SINK, FUNCTION_TEST_MANAGER_SINK,
};
type Failure = Box<dyn std::error::Error>;
fn views(context: &ITfContext) -> Result<Vec<ITfCompositionView>> {
    let service: ITfContextComposition = context.cast()?;
    let enumerator = unsafe { service.EnumCompositions()? };
    let mut views = Vec::new();
    for _ in 0..4 {
        let mut slot = [None];
        let mut fetched = 0;
        unsafe {
            enumerator.Next(&mut slot, &mut fetched)?;
        }
        if fetched == 0 {
            return Ok(views);
        }
        assert_eq!(fetched, 1);
        views.push(slot[0].take().ok_or(E_FAIL)?);
    }
    Err(E_FAIL.into())
}
pub(crate) fn exercise_owned_composition(
    manager: &ITfThreadMgr,
    client: u32,
) -> std::result::Result<(), Failure> {
    let cases = [
        "cancel",
        "commit",
        "forced",
        "forced-restart",
        "revoke",
        "focus",
        "external-restored",
        "cleanup-denied",
        "readonly-cleanup",
        "host-denied",
        "begin-denied",
        "missing-sink",
        "wrong-sink",
        "foreign-after-begin",
        "protected-after-start",
        "delayed",
        "direct-force",
        "direct-force-with-foreign",
    ];
    // SAFETY: this STA owns every synthetic document and composition below.
    unsafe {
        for (index, case) in cases.into_iter().enumerate() {
            let window = crate::window_host::Window::new()?;
            let store = ComObject::new(TextStore::new("prefix nihongo suffix"));
            let mut selection = store.get().selection.get();
            selection.acpStart = 7;
            selection.acpEnd = 14;
            store.get().selection.set(selection);
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
            let context = context.ok_or_else(|| windows::core::Error::from(E_FAIL))?;
            document.0.Push(&context)?;
            scope_fixture::initialise_context(&context, client)?;
            let other = if case == "focus" {
                Some(scope_fixture::other_document(manager, client)?)
            } else {
                None
            };
            let mut association =
                crate::window_host::Association::new(manager, &window, &document.0)?;
            manager.SetFocus(&document.0)?;
            pump_tsf_messages()?;
            let provider = manager.GetFunctionProvider(&sumibi_tip::CLSID_SUMIBI_TIP)?;
            let control: ISpikeControl = provider
                .GetFunction(&FUNCTION_TEST_CONTROL, &ISpikeControl::IID)?
                .cast()?;
            let delayed: ISpikeDelayed = provider
                .GetFunction(&FUNCTION_TEST_DELAYED, &ISpikeDelayed::IID)?
                .cast()?;
            let keys: ITfKeyEventSink = provider
                .GetFunction(&FUNCTION_TEST_KEY_SINK, &ITfKeyEventSink::IID)?
                .cast()?;
            let manager_sink: ITfThreadMgrEventSink = provider
                .GetFunction(&FUNCTION_TEST_MANAGER_SINK, &ITfThreadMgrEventSink::IID)?
                .cast()?;
            control.GrantContext(context.as_raw()).ok()?;
            if case.starts_with("direct-force") {
                // Direct adapter has no Controller observer. Prove that object
                // identity alone rejects forced termination, even if another
                // composition now covers exactly the same text and range.
                let policy = Rc::new(Cell::new(EditPolicy {
                    allowed: true,
                    revision: Revision {
                        input: 1,
                        settings: 1,
                        authorization: 1,
                    },
                }));
                let snapshot = capture_selection(manager, &context, client, policy)?;
                let mut preview =
                    sumibi_tip::edit::preview::Preview::begin(manager, client, snapshot)?;
                let current = views(&context)?;
                assert_eq!(current.len(), 1);
                context
                    .cast::<ITfContextOwnerCompositionServices>()?
                    .TerminateComposition(&current[0])?;
                let mut foreign = if case == "direct-force-with-foreign" {
                    Some(composition_host::Composition::start(
                        &context, client, 7, 7,
                    )?)
                } else {
                    None
                };
                assert!(preview.commit("日本語").is_err());
                preview.cancel()?;
                preview.verify_attribute(false)?;
                assert_eq!(views(&context)?.len(), usize::from(foreign.is_some()));
                if let Some(foreign) = &mut foreign {
                    foreign.end()?;
                }
                assert_eq!(
                    *store.get().text.borrow(),
                    "prefix nihongo suffix".encode_utf16().collect::<Vec<_>>()
                );
                assert!(store.get().undo.borrow().is_empty());
                control.RevokeContext().ok()?;
                association.close()?;
                manager.SetFocus(None)?;
                println!("Owned selection composition / {case}: OK");
                continue;
            }
            let request = index as u64 * 2 + 1;
            let asynchronous = matches!(
                case,
                "forced"
                    | "forced-restart"
                    | "external-restored"
                    | "protected-after-start"
                    | "delayed"
            );
            if matches!(
                case,
                "host-denied" | "begin-denied" | "missing-sink" | "wrong-sink"
            ) {
                let compartments: ITfCompartmentMgr = context.cast()?;
                if case == "missing-sink" {
                    compartments.ClearCompartment(
                        client,
                        &sumibi_tip::spike::GUID_SPIKE_COMPOSITION_SINK,
                    )?;
                } else if case == "wrong-sink" {
                    compartments
                        .GetCompartment(&sumibi_tip::spike::GUID_SPIKE_COMPOSITION_SINK)?
                        .SetValue(
                            client,
                            &windows::Win32::System::Variant::VARIANT::from(
                                store.to_interface::<IUnknown>(),
                            ),
                        )?;
                }
                store.get().reject_composition.set(case == "host-denied");
                store.get().deny_lock.set(case == "begin-denied");
                for _ in 0..if case == "host-denied" { 32 } else { 1 } {
                    assert!(control.BeginPreview().is_err());
                }
                assert!(views(&context)?.is_empty());
                assert!(store.get().undo.borrow().is_empty());
                store.get().reject_composition.set(false);
                store.get().deny_lock.set(false);
                if matches!(case, "missing-sink" | "wrong-sink") {
                    scope_fixture::initialise_context(&context, client)?;
                    control.GrantContext(context.as_raw()).ok()?;
                }
                control.BeginPreview().ok()?; // Fresh explicit operation after cleanup.
            } else if asynchronous {
                delayed.BeginRequest(request, 2000).ok()?;
            } else {
                control.BeginPreview().ok()?;
            }
            let current = views(&context)?;
            assert_eq!(current.len(), 1, "{case}: exactly one composition");
            assert_eq!(
                current[0].GetOwnerClsid()?,
                sumibi_tip::CLSID_SUMIBI_TIP,
                "composition must belong to the real DLL client"
            );
            control.VerifyPreview().ok()?;
            let mut foreign = None;
            let applied = matches!(case, "commit" | "delayed" | "forced-restart");
            match case {
                "commit" => assert!(keys.OnPreservedKey(&context, &COMMAND_CONVERT)?.as_bool()),
                "delayed" => delayed.CompleteRequest(request).ok()?,
                "forced" | "forced-restart" => {
                    let owner: ITfContextOwnerCompositionServices = context.cast()?;
                    owner.TerminateComposition(&current[0])?;
                    pump_tsf_messages()?;
                    assert!(views(&context)?.is_empty());
                    if case == "forced-restart" {
                        delayed.BeginRequest(request + 1, 2000).ok()?;
                        assert!(delayed.CompleteRequest(request).is_err());
                        assert_eq!(
                            views(&context)?.len(),
                            1,
                            "old reply must not consume the new composition"
                        );
                        delayed.CompleteRequest(request + 1).ok()?;
                    } else {
                        assert!(delayed.CompleteRequest(request).is_err());
                    }
                }
                "revoke" => {
                    control.RevokeContext().ok()?;
                }
                "focus" => {
                    let other = &other.as_ref().unwrap().0;
                    manager.SetFocus(other)?;
                    // Headless focus need not deliver a desktop notification.
                    // Call the same actual DLL focus callback explicitly.
                    manager_sink.OnSetFocus(other, &document.0)?;
                    pump_tsf_messages()?;
                    manager.SetFocus(&document.0)?;
                }
                "external-restored" => {
                    store
                        .get()
                        .external_text("prefix NIHONGO suffix".encode_utf16().collect())?;
                    store
                        .get()
                        .external_text("prefix nihongo suffix".encode_utf16().collect())?;
                    pump_tsf_messages()?;
                    assert!(delayed.CompleteRequest(request).is_err());
                }
                "cleanup-denied" | "readonly-cleanup" => {
                    store.get().deny_lock.set(case == "cleanup-denied");
                    store.get().readonly.set(case == "readonly-cleanup");
                    assert!(control.CancelPreview().is_err());
                    assert_eq!(
                        views(&context)?.len(),
                        1,
                        "denied cleanup must retain the composition"
                    );
                    assert!(store.get().undo.borrow().is_empty());
                    store.get().deny_lock.set(false);
                    store.get().readonly.set(false);
                    control.CancelPreview().ok()?;
                }
                "foreign-after-begin" => {
                    foreign = Some(composition_host::Composition::start(
                        &context, client, 0, 2,
                    )?);
                    pump_tsf_messages()?;
                    control.CancelPreview().ok()?;
                    let remaining = views(&context)?;
                    assert_eq!(remaining.len(), 1);
                    assert_ne!(
                        remaining[0].GetOwnerClsid()?,
                        sumibi_tip::CLSID_SUMIBI_TIP,
                        "foreign composition must survive"
                    );
                    assert!(!keys.OnPreservedKey(&context, &COMMAND_CONVERT)?.as_bool());
                }
                "protected-after-start" => {
                    *store.get().scopes.borrow_mut() = ScopeSpec::List(vec![IS_PRIVATE]);
                    let sink = store
                        .get()
                        .sink
                        .borrow()
                        .clone()
                        .ok_or_else(|| windows::core::Error::from(E_FAIL))?;
                    sink.OnAttrsChange(0, 21, &[GUID_PROP_INPUTSCOPE])?;
                    pump_tsf_messages()?;
                    assert!(delayed.CompleteRequest(request).is_err());
                    *store.get().scopes.borrow_mut() = ScopeSpec::List(vec![IS_TEXT]);
                }
                _ => {
                    control.CancelPreview().ok()?;
                }
            }
            pump_tsf_messages()?;
            if let Some(foreign) = &mut foreign {
                foreign.end()?;
            }
            control.CancelPreview().ok()?;
            assert!(
                views(&context)?.is_empty(),
                "{case}: no composition may leak"
            );
            assert_eq!(
                *store.get().text.borrow(),
                if applied {
                    "prefix 日本語 suffix"
                } else {
                    "prefix nihongo suffix"
                }
                .encode_utf16()
                .collect::<Vec<_>>(),
                "{case}"
            );
            assert_eq!(
                store.get().undo.borrow().len(),
                usize::from(applied),
                "metadata must not create body undo entries"
            );
            if applied {
                store.get().undo_once()?;
                assert_eq!(
                    *store.get().text.borrow(),
                    "prefix nihongo suffix".encode_utf16().collect::<Vec<_>>()
                );
            }
            control.RevokeContext().ok()?;
            association.close()?;
            manager.SetFocus(None)?;
            println!("Owned selection composition / {case}: OK");
        }
    }
    println!(
        "Owned selection composition: {} scenarios passed",
        cases.len()
    );
    Ok(())
}

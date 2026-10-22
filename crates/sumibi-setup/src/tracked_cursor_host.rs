//! Caret/deletion through the actual DLL, confined to recorded synthetic input.
use super::*;
pub(super) fn test(
    window: &crate::window_host::Window,
    editor: &Editor,
    live: &crate::live_render::Binding,
    client: u32,
    case: &str,
) -> Result<()> {
    if case != "delete" {
        return test_other(window, editor, live, client, case);
    }
    let store = editor.store.get();
    store.external_text("prefix  suffix".encode_utf16().collect())?;
    store.host_selection(7, 7)?;
    pump(window)?;
    editor.insert(&"ae\u{301}👨‍👩‍👧‍👦🇯🇵b".encode_utf16().collect::<Vec<_>>())?;
    assert_eq!(editor.input_disposition.get(), INPUT_APPLIED);
    pump(window)?;
    let identity = owned_view(&editor.context)?.cast::<IUnknown>()?;
    let original = store.text.borrow().clone();
    let reads = input_notify_host::body_reads()?;
    let writes = store.set_text_calls.get();
    store.undo.borrow_mut().clear();
    editor.navigate(0x25, false, 3)?;
    assert_eq!(
        editor.input_disposition.get(),
        INPUT_APPLIED,
        "recorded movement"
    );
    assert_eq!(store.selection.get().acpStart, 10);
    assert_eq!(store.set_text_calls.get(), writes);
    assert!(store.undo.borrow().is_empty());
    pump(window)?;
    assert!(owned_view(&editor.context)?.cast::<IUnknown>()? == identity);
    assert_eq!(live.scene()?.underlines, vec![7..26]);
    editor.delete_forward(2)?;
    assert_eq!(
        editor.input_disposition.get(),
        INPUT_APPLIED,
        "recorded forward deletion"
    );
    assert_eq!(
        *store.text.borrow(),
        "prefix ae\u{301}b suffix"
            .encode_utf16()
            .collect::<Vec<_>>()
    );
    assert_eq!(store.selection.get().acpStart, 10);
    assert_eq!(store.set_text_calls.get(), writes + 1);
    assert_eq!(store.undo.borrow().len(), 1);
    pump(window)?;
    assert!(owned_view(&editor.context)?.cast::<IUnknown>()? == identity);
    assert_eq!(live.scene()?.underlines, vec![7..11]);
    store.undo_once()?;
    assert_eq!(*store.text.borrow(), original);
    store.redo_once()?;
    assert_eq!(input_notify_host::body_reads()?, reads);
    editor.clear_tracking();
    pump(window)?;
    assert!(no_composition(&editor.context)?);
    println!("Tracked cursor / {case}: OK");
    Ok(())
}

fn test_other(
    window: &crate::window_host::Window,
    editor: &Editor,
    live: &crate::live_render::Binding,
    client: u32,
    case: &str,
) -> Result<()> {
    use sumibi_core::recovery::Operation;
    use sumibi_tip::spike::ISpikeTrackedEditor;
    let store = editor.store.get();
    store.external_text("prefix  suffix".encode_utf16().collect())?;
    store.host_selection(7, 7)?;
    pump(window)?;
    let text = if case == "rejoin" {
        "🇯x🇵b"
    } else {
        "ae\u{301}👨‍👩‍👧‍👦🇯🇵b"
    };
    editor.insert(&text.encode_utf16().collect::<Vec<_>>())?;
    assert_eq!(editor.input_disposition.get(), INPUT_APPLIED);
    pump(window)?;
    let composing = editor.composing.as_ref().ok_or(E_FAIL)?;
    let control: ISpikeTrackedEditor = composing.control.cast()?;
    let identity = owned_view(&editor.context)?.cast::<IUnknown>()?;
    let original = store.text.borrow().clone();
    let writes = store.set_text_calls.get();
    let reads = input_notify_host::body_reads()?;
    store.undo.borrow_mut().clear();
    let mut foreign = None;
    if matches!(
        case,
        "overshoot" | "nav-budget" | "nav-fail" | "nav-fail-after" | "shift" | "invalid"
    ) {
        match case {
            "invalid" => {
                for (action, count) in [(3, 1), (0, 0), (2, 1025), (1, u32::MAX)] {
                    let mut outcome = INPUT_UNCERTAIN;
                    assert!(unsafe {
                        control.Edit(editor.context.as_raw(), action, count, &mut outcome)
                    }
                    .is_err());
                    assert_eq!(outcome, INPUT_NOT_ATTEMPTED);
                }
                let mut outcome = INPUT_UNCERTAIN;
                assert!(unsafe { control.Edit(std::ptr::null_mut(), 0, 1, &mut outcome) }.is_err());
                assert_eq!(outcome, INPUT_NOT_ATTEMPTED);
                assert!(unsafe {
                    control.Edit(editor.context.as_raw(), 0, 1, std::ptr::null_mut())
                }
                .is_err());
                assert!(owned_view(&editor.context)?.cast::<IUnknown>()? == identity);
            }
            "nav-fail" | "nav-fail-after" => {
                store.fail_selection.set(case == "nav-fail");
                store.fail_selection_after.set(case == "nav-fail-after");
                editor.navigate(0x25, false, 3)?;
                assert_eq!(editor.input_disposition.get(), INPUT_UNCERTAIN);
                assert_eq!(
                    store.selection.get().acpStart,
                    if case == "nav-fail" { 26 } else { 10 },
                    "failed caret move is not replayed"
                );
                let metadata = editor.rescue.metadata().ok_or(E_FAIL)?;
                assert_eq!(
                    metadata.operation,
                    Operation::MoveCaret {
                        backward: true,
                        count: 3
                    }
                );
                assert!(!metadata.copyable);
                let mut outcome = INPUT_UNCERTAIN;
                assert!(
                    unsafe { control.Edit(editor.context.as_raw(), 0, 3, &mut outcome) }.is_err()
                );
                assert_eq!(outcome, INPUT_NOT_ATTEMPTED);
            }
            "shift" => {
                editor.navigate(0x25, true, 3)?;
                let selected = store.selection.get();
                assert_eq!((selected.acpStart, selected.acpEnd), (10, 26));
                assert_eq!(selected.style.ase, TS_AE_START);
            }
            _ => {
                let ticket = if case == "nav-budget" {
                    Some(editor.rescue.reserve(Operation::Insert(1)).ok_or(E_FAIL)?)
                } else {
                    None
                };
                editor.navigate(0x25, false, if case == "overshoot" { 6 } else { 3 })?;
                if let Some(ticket) = ticket {
                    editor.rescue.finish(ticket, INPUT_NOT_ATTEMPTED, &[]);
                }
                assert_eq!(editor.input_disposition.get(), INPUT_NOT_ATTEMPTED);
                assert_eq!(
                    store.selection.get().acpStart,
                    if case == "overshoot" { 6 } else { 10 }
                );
            }
        }
        assert_eq!(*store.text.borrow(), original);
        assert_eq!(store.set_text_calls.get(), writes);
        assert!(store.undo.borrow().is_empty());
    } else {
        editor.navigate(0x25, false, if case == "all" { 5 } else { 3 })?;
        assert_eq!(
            editor.input_disposition.get(),
            INPUT_APPLIED,
            "{case}: movement remains tracked"
        );
        assert_eq!(store.set_text_calls.get(), writes);
        assert!(store.undo.borrow().is_empty());
        if case == "tail-append" {
            editor.navigate(0x27, false, 3)?;
            assert_eq!(editor.input_disposition.get(), INPUT_APPLIED);
            editor.insert(&['z' as u16])?;
            assert_eq!(editor.input_disposition.get(), INPUT_APPLIED);
            editor.input(WM_CHAR, WPARAM(8), LPARAM(1))?;
            assert_eq!(editor.input_disposition.get(), INPUT_APPLIED);
            assert_eq!(*store.text.borrow(), original);
            assert_eq!(store.set_text_calls.get(), writes + 2);
            assert!(owned_view(&editor.context)?.cast::<IUnknown>()? == identity);
        } else if case == "move" {
            editor.navigate(0x27, false, 2)?;
            assert_eq!(editor.input_disposition.get(), INPUT_APPLIED);
            assert_eq!(store.selection.get().acpStart, 25);
            assert!(owned_view(&editor.context)?.cast::<IUnknown>()? == identity);
            assert_eq!(*store.text.borrow(), original);
            assert_eq!(store.set_text_calls.get(), writes);
            assert!(store.undo.borrow().is_empty());
        } else if matches!(case, "readonly" | "foreign") {
            if case == "readonly" {
                store.readonly.set(true);
            } else {
                foreign = Some(crate::edit_host::composition_host::Composition::start(
                    &editor.context,
                    client,
                    0,
                    2,
                )?);
            }
            let mut outcome = INPUT_UNCERTAIN;
            assert!(unsafe { control.Edit(editor.context.as_raw(), 2, 1, &mut outcome) }.is_err());
            assert_eq!(outcome, INPUT_NOT_ATTEMPTED);
            assert_eq!(*store.text.borrow(), original);
            assert_eq!(store.set_text_calls.get(), writes);
            assert!(store.undo.borrow().is_empty());
        } else if case == "interior-typing" {
            editor.insert(&['z' as u16])?;
            assert_eq!(editor.input_disposition.get(), INPUT_NOT_ATTEMPTED);
            assert_eq!(store.set_text_calls.get(), writes);
            assert_eq!(
                *store.text.borrow(),
                "prefix ae\u{301}z👨‍👩‍👧‍👦🇯🇵b suffix"
                    .encode_utf16()
                    .collect::<Vec<_>>()
            );
            assert_eq!(store.undo.borrow().len(), 1);
        } else {
            if case != "convert" {
                match case {
                    "text-fail-before" => store.fail_text_before.set(true),
                    "text-fail-after" => store.fail_text_after.set(true),
                    "selection-fail" => store.fail_selection.set(true),
                    "status-restored" => store.status_cycle_on_write.set(true),
                    "scope-restored" => store.scope_cycle_on_write.set(true),
                    "private" => set_scope(editor, IS_PRIVATE)?,
                    "compartment-restored" => unsafe {
                        *store.compartment_cycle_on_write.borrow_mut() = Some((
                            editor
                                .context
                                .cast::<ITfCompartmentMgr>()?
                                .GetCompartment(&GUID_COMPARTMENT_KEYBOARD_DISABLED)?,
                            client,
                        ));
                    },
                    _ => {}
                }
                let ticket = if case == "delete-budget" {
                    Some(editor.rescue.reserve(Operation::Insert(1)).ok_or(E_FAIL)?)
                } else {
                    None
                };
                let count = match case {
                    "repeat" => 3,
                    "all" => 5,
                    "delete-overshoot" => 4,
                    "rejoin" => 1,
                    _ => 2,
                };
                editor.delete_forward(count)?;
                if let Some(ticket) = ticket {
                    editor.rescue.finish(ticket, INPUT_NOT_ATTEMPTED, &[]);
                }
                let failed = matches!(
                    case,
                    "text-fail-before"
                        | "text-fail-after"
                        | "selection-fail"
                        | "status-restored"
                        | "scope-restored"
                        | "compartment-restored"
                );
                let fallback = matches!(case, "private" | "delete-budget" | "delete-overshoot");
                assert_eq!(store.set_text_calls.get(), writes + usize::from(!fallback));
                assert_eq!(
                    store.undo.borrow().len(),
                    usize::from(case != "text-fail-before")
                );
                if failed {
                    assert_eq!(
                        editor.input_disposition.get(),
                        if case.starts_with("text-fail") {
                            INPUT_UNCERTAIN
                        } else {
                            INPUT_APPLIED_UNTRACKED
                        }
                    );
                    let metadata = editor.rescue.metadata().ok_or(E_FAIL)?;
                    assert_eq!(metadata.operation, Operation::ForwardDelete(2));
                    assert!(!metadata.copyable);
                    let after = store.text.borrow().clone();
                    let mut outcome = INPUT_UNCERTAIN;
                    assert!(
                        unsafe { control.Edit(editor.context.as_raw(), 2, 2, &mut outcome) }
                            .is_err()
                    );
                    assert_eq!(outcome, INPUT_NOT_ATTEMPTED);
                    assert_eq!(*store.text.borrow(), after);
                    assert_eq!(store.set_text_calls.get(), writes + 1);
                } else {
                    assert_eq!(
                        editor.input_disposition.get(),
                        if fallback {
                            INPUT_NOT_ATTEMPTED
                        } else {
                            INPUT_APPLIED
                        }
                    );
                }
                let expected = match case {
                    "all" => "prefix  suffix",
                    "repeat" => "prefix ae\u{301} suffix",
                    "rejoin" => "prefix 🇯🇵b suffix",
                    "delete-overshoot" => "prefix ae\u{301}suffix",
                    "text-fail-before" => "prefix ae\u{301}👨‍👩‍👧‍👦🇯🇵b suffix",
                    _ => "prefix ae\u{301}b suffix",
                };
                assert_eq!(
                    *store.text.borrow(),
                    expected.encode_utf16().collect::<Vec<_>>()
                );
                if case == "rejoin" {
                    editor.delete_forward(1)?;
                    assert_eq!(editor.input_disposition.get(), INPUT_APPLIED);
                    assert_eq!(
                        *store.text.borrow(),
                        "prefix b suffix".encode_utf16().collect::<Vec<_>>()
                    );
                    assert_eq!(store.set_text_calls.get(), writes + 2);
                }
                if !failed && !fallback && case != "all" {
                    assert!(owned_view(&editor.context)?.cast::<IUnknown>()? == identity);
                    assert_eq!(
                        store.last_text_lock.get(),
                        store.last_composition_lock.get()
                    );
                }
                if !failed && case != "convert-after-delete" && case != "rejoin" {
                    let deleted = store.text.borrow().clone();
                    store.undo_once()?;
                    assert_eq!(*store.text.borrow(), original);
                    store.redo_once()?;
                    assert_eq!(*store.text.borrow(), deleted);
                }
            }
            if case.starts_with("convert") {
                assert_eq!(input_notify_host::body_reads()?, reads);
                let before_conversion = store.text.borrow().clone();
                store.undo.borrow_mut().clear();
                editor.input(WM_KEYDOWN, WPARAM(0x76), LPARAM(1))?;
                let (broker, bridge) = editor.broker.as_ref().ok_or(E_FAIL)?;
                assert!(bridge.get().active());
                tracking_host::wait_result(window, broker, bridge, true)?;
                assert_eq!(
                    *store.text.borrow(),
                    "prefix 日本語 suffix".encode_utf16().collect::<Vec<_>>()
                );
                assert_eq!(store.undo.borrow().len(), 1);
                store.undo_once()?;
                assert_eq!(*store.text.borrow(), before_conversion);
            }
        }
    }
    if !case.starts_with("convert") {
        assert_eq!(input_notify_host::body_reads()?, reads);
    }
    store.readonly.set(false);
    store.fail_selection.set(false);
    set_scope(editor, IS_TEXT)?;
    if let Some(foreign) = &mut foreign {
        foreign.end()?;
    }
    editor.clear_tracking();
    pump(window)?;
    assert!(no_composition(&editor.context)?);
    assert!(live.scene()?.underlines.is_empty());
    println!("Tracked cursor / {case}: OK");
    Ok(())
}

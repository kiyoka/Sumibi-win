//! Internal Backspace through the real DLL on recorded, synthetic text only.
use super::*;
use sumibi_core::recovery::{Operation, Outcome};

pub(super) fn test(
    window: &crate::window_host::Window,
    editor: &Editor,
    live: &crate::live_render::Binding,
    client: u32,
    case: &str,
) -> Result<()> {
    let store = editor.store.get();
    store.external_text("prefix  suffix".encode_utf16().collect())?;
    store.host_selection(7, 7)?;
    pump(window)?;
    editor.insert(
        &if case == "rejoin" {
            "🇯x🇵b"
        } else {
            "ae\u{301}👨‍👩‍👧‍👦🇯🇵b"
        }
        .encode_utf16()
        .collect::<Vec<_>>(),
    )?;
    assert_eq!(editor.input_disposition.get(), INPUT_APPLIED);
    editor.navigate(
        0x25,
        false,
        if case == "start-boundary" {
            5
        } else if case == "rejoin" {
            3
        } else {
            2
        },
    )?;
    assert_eq!(editor.input_disposition.get(), INPUT_APPLIED);
    if case == "rejoin" {
        editor.delete_forward(1)?;
        assert_eq!(editor.input_disposition.get(), INPUT_APPLIED);
    } else if case == "all" {
        editor.navigate(0x27, false, 2)?;
        assert_eq!(editor.input_disposition.get(), INPUT_APPLIED);
    }
    pump(window)?;
    let identity = owned_view(&editor.context)?.cast::<IUnknown>()?;
    let original = store.text.borrow().clone();
    let selection = store.selection.get();
    let reads = super::super::input_notify_host::body_reads()?;
    let writes = store.set_text_calls.get();
    store.undo.borrow_mut().clear();
    store.redo.borrow_mut().clear();
    let eraser: ISpikeInputEraser = editor.composing.as_ref().ok_or(E_FAIL)?.control.cast()?;
    let mut foreign = None;
    match case {
        "text-fail-before" => store.fail_text_before.set(true),
        "text-fail-after" => store.fail_text_after.set(true),
        "caret-fail" => store.fail_selection.set(true),
        "status-restored" => store.status_cycle_on_write.set(true),
        "scope-restored" => store.scope_cycle_on_write.set(true),
        "compartment-restored" => unsafe {
            *store.compartment_cycle_on_write.borrow_mut() = Some((
                editor
                    .context
                    .cast::<ITfCompartmentMgr>()?
                    .GetCompartment(&GUID_COMPARTMENT_KEYBOARD_DISABLED)?,
                client,
            ));
        },
        "private" => set_scope(editor, IS_PRIVATE)?,
        "readonly" => store.readonly.set(true),
        "foreign" => {
            foreign = Some(crate::edit_host::composition_host::Composition::start(
                &editor.context,
                client,
                0,
                2,
            )?);
        }
        _ => {}
    }
    if matches!(case, "readonly" | "foreign") {
        let mut disposition = INPUT_UNCERTAIN;
        assert!(unsafe { eraser.Backspace(editor.context.as_raw(), 1, &mut disposition) }.is_err());
        assert_eq!(disposition, INPUT_NOT_ATTEMPTED);
        assert_eq!(*store.text.borrow(), original);
        assert_eq!(store.set_text_calls.get(), writes);
        assert!(store.undo.borrow().is_empty());
    } else {
        let ticket = if case == "budget" {
            Some(editor.rescue.reserve(Operation::Insert(1)).ok_or(E_FAIL)?)
        } else {
            None
        };
        let count = match case {
            "repeat" | "convert" => 2,
            "to-start" => 3,
            "all" => 5,
            "cross-start" => 4,
            _ => 1,
        };
        editor.input(WM_CHAR, WPARAM(8), LPARAM(count))?;
        if let Some(ticket) = ticket {
            editor.rescue.finish(ticket, INPUT_NOT_ATTEMPTED, &[]);
        }
        let failed = matches!(
            case,
            "text-fail-before"
                | "text-fail-after"
                | "caret-fail"
                | "status-restored"
                | "scope-restored"
                | "compartment-restored"
        );
        let fallback = matches!(
            case,
            "budget" | "private" | "start-boundary" | "cross-start"
        );
        let expected = match case {
            "repeat" | "convert" => "prefix a🇯🇵b suffix",
            "to-start" => "prefix 🇯🇵b suffix",
            "all" => "prefix  suffix",
            "start-boundary" => "prefixae\u{301}👨‍👩‍👧‍👦🇯🇵b suffix",
            "cross-start" => "prefix🇯🇵b suffix",
            "rejoin" => "prefix b suffix",
            "text-fail-before" => "prefix ae\u{301}👨‍👩‍👧‍👦🇯🇵b suffix",
            _ => "prefix ae\u{301}🇯🇵b suffix",
        }
        .encode_utf16()
        .collect::<Vec<_>>();
        assert_eq!(
            *store.text.borrow(),
            expected,
            "{case}: exact deletion, never replayed"
        );
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
            assert_eq!(metadata.operation, Operation::Backspace(count as u32));
            assert_eq!(
                metadata.outcome,
                if case.starts_with("text-fail") {
                    Outcome::Uncertain
                } else {
                    Outcome::AppliedUntracked
                }
            );
            assert!(!metadata.copyable);
            let mut disposition = INPUT_UNCERTAIN;
            assert!(unsafe {
                eraser.Backspace(editor.context.as_raw(), count as u32, &mut disposition)
            }
            .is_err());
            assert_eq!(disposition, INPUT_NOT_ATTEMPTED);
            assert_eq!(*store.text.borrow(), expected);
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
            if !fallback {
                assert_eq!(
                    store.last_text_lock.get(),
                    store.last_composition_lock.get()
                );
                pump(window)?;
                if case == "all" {
                    assert!(no_composition(&editor.context)?);
                } else {
                    assert!(owned_view(&editor.context)?.cast::<IUnknown>()? == identity);
                    assert_eq!(live.scene()?.underlines, vec![7..7 + expected.len() - 14]);
                }
            }
            assert!(editor.rescue.metadata().is_none());
        }
        assert_eq!(super::super::input_notify_host::body_reads()?, reads);
        if case == "convert" {
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
            assert_eq!(*store.text.borrow(), expected);
        } else if case != "text-fail-before" {
            store.fail_selection.set(false);
            store.undo_once()?;
            assert_eq!(
                *store.text.borrow(),
                original,
                "one Undo restores the whole repeated deletion"
            );
            let restored = store.selection.get();
            assert_eq!(
                (restored.acpStart, restored.acpEnd),
                (selection.acpStart, selection.acpEnd)
            );
            store.redo_once()?;
            assert_eq!(*store.text.borrow(), expected);
        }
    }
    if case != "convert" {
        assert_eq!(super::super::input_notify_host::body_reads()?, reads);
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
    println!("Internal Backspace / {case}: OK");
    Ok(())
}

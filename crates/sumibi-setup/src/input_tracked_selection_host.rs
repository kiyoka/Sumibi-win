//! Recorded Shift selection through the actual DLL on synthetic text only.
use super::*;
use sumibi_core::recovery::Operation;
use sumibi_tip::spike::{ISpikeInputPreparer, ISpikeTrackedSelector};

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
    let text = if case == "rejoin" {
        "🇯x🇵b"
    } else {
        "ae\u{301}👨‍👩‍👧‍👦🇯🇵b"
    };
    editor.insert(&text.encode_utf16().collect::<Vec<_>>())?;
    assert_eq!(editor.input_disposition.get(), INPUT_APPLIED);
    if matches!(case, "right" | "convert-forward") {
        editor.navigate(0x25, false, 5)?;
    } else if case == "reverse" {
        editor.navigate(0x25, false, 3)?;
    } else if case == "rejoin" {
        editor.navigate(0x25, false, 3)?;
        editor.delete_forward(1)?;
    }
    assert_eq!(editor.input_disposition.get(), INPUT_APPLIED);
    pump(window)?;
    let identity = owned_view(&editor.context)?.cast::<IUnknown>()?;
    let original = store.text.borrow().clone();
    let reads = input_notify_host::body_reads()?;
    let writes = store.set_text_calls.get();
    store.undo.borrow_mut().clear();
    store.redo.borrow_mut().clear();
    let composing = editor.composing.as_ref().ok_or(E_FAIL)?;
    let selector: ISpikeTrackedSelector = composing.control.cast()?;
    let mut foreign = None;
    match case {
        "readonly" => store.readonly.set(true),
        "private" => set_scope(editor, IS_PRIVATE)?,
        "foreign" => {
            foreign = Some(crate::edit_host::composition_host::Composition::start(
                &editor.context,
                client,
                0,
                2,
            )?)
        }
        _ => {}
    }
    if matches!(case, "readonly" | "private" | "foreign" | "invalid") {
        if case == "invalid" {
            for (backward, count) in [(2, 1), (1, 0), (0, 1025), (1, u32::MAX)] {
                let mut disposition = INPUT_UNCERTAIN;
                assert!(unsafe {
                    selector.Extend(editor.context.as_raw(), backward, count, &mut disposition)
                }
                .is_err());
                assert_eq!(disposition, INPUT_NOT_ATTEMPTED);
            }
            let mut disposition = INPUT_UNCERTAIN;
            assert!(
                unsafe { selector.Extend(std::ptr::null_mut(), 1, 3, &mut disposition) }.is_err()
            );
            assert_eq!(disposition, INPUT_NOT_ATTEMPTED);
            assert!(unsafe {
                selector.Extend(editor.context.as_raw(), 1, 3, std::ptr::null_mut())
            }
            .is_err());
        } else {
            let mut disposition = INPUT_UNCERTAIN;
            assert!(
                unsafe { selector.Extend(editor.context.as_raw(), 1, 3, &mut disposition) }
                    .is_err()
            );
            assert_eq!(disposition, INPUT_NOT_ATTEMPTED);
        }
        assert_eq!(store.selection.get().acpStart, 26);
    } else {
        let ticket = if case == "budget" {
            Some(editor.rescue.reserve(Operation::Insert(1)).ok_or(E_FAIL)?)
        } else {
            None
        };
        store.fail_selection.set(case == "fail-before");
        store.fail_selection_after.set(case == "fail-after");
        let right = matches!(case, "right" | "convert-forward");
        let count = match case {
            "reverse" => 2,
            "overshoot" => 6,
            "rejoin" => 1,
            _ => 3,
        };
        editor.navigate(if right { 0x27 } else { 0x25 }, true, count)?;
        if let Some(ticket) = ticket {
            editor.rescue.finish(ticket, INPUT_NOT_ATTEMPTED, &[]);
        }
        if matches!(case, "fail-before" | "fail-after") {
            assert_eq!(editor.input_disposition.get(), INPUT_UNCERTAIN);
            let selected = store.selection.get();
            assert_eq!(
                (selected.acpStart, selected.acpEnd),
                if case == "fail-before" {
                    (26, 26)
                } else {
                    (10, 26)
                }
            );
            let metadata = editor.rescue.metadata().ok_or(E_FAIL)?;
            assert_eq!(
                metadata.operation,
                Operation::ExtendSelection {
                    backward: true,
                    count: 3
                }
            );
            assert!(!metadata.copyable);
            let mut disposition = INPUT_UNCERTAIN;
            assert!(
                unsafe { selector.Extend(editor.context.as_raw(), 1, 3, &mut disposition) }
                    .is_err()
            );
            assert_eq!(disposition, INPUT_NOT_ATTEMPTED);
            assert_eq!(
                store.selection.get().acpStart,
                selected.acpStart,
                "failed selection is never repeated"
            );
        } else if matches!(case, "budget" | "overshoot") {
            assert_eq!(editor.input_disposition.get(), INPUT_NOT_ATTEMPTED);
            let selected = store.selection.get();
            assert_eq!(
                (selected.acpStart, selected.acpEnd),
                (if case == "overshoot" { 6 } else { 10 }, 26)
            );
            assert_eq!(selected.style.ase, TS_AE_START);
            assert!(no_composition(&editor.context)?);
        } else {
            assert_eq!(editor.input_disposition.get(), INPUT_APPLIED);
            let expected = if right {
                (7, 21, TS_AE_END)
            } else if case == "reverse" {
                (7, 10, TS_AE_START)
            } else if case == "rejoin" {
                (7, 9, TS_AE_START)
            } else {
                (10, 26, TS_AE_START)
            };
            assert_selection(editor, expected)?;
            assert!(owned_view(&editor.context)?.cast::<IUnknown>()? == identity);
            pump(window)?;
            assert_eq!(live.scene()?.underlines, vec![7..original.len() - 7]);
            match case {
                "shrink" => {
                    editor.navigate(0x27, true, 1)?;
                    assert_selection(editor, (21, 26, TS_AE_START))?;
                }
                "reverse" => {
                    editor.navigate(0x27, true, 3)?;
                    assert_selection(editor, (10, 21, TS_AE_END))?;
                }
                "collapse-left" | "collapse-repeat" | "type-after-collapse" => {
                    editor.navigate(0x25, false, if case == "collapse-repeat" { 2 } else { 1 })?;
                    let caret = if case == "collapse-repeat" { 8 } else { 10 };
                    assert_selection(editor, (caret, caret, TS_AE_END))?;
                }
                "collapse-right" => {
                    editor.navigate(0x27, false, 1)?;
                    assert_selection(editor, (26, 26, TS_AE_END))?;
                }
                "rejoin" => {
                    editor.navigate(0x27, false, 1)?;
                    assert_selection(editor, (11, 11, TS_AE_END))?;
                }
                "direction-no-notify"
                | "direction-restored"
                | "ready-direction"
                | "unknown-active" => {
                    if case == "direction-restored" {
                        store.select_caret(10, 26)?;
                        store.select_caret(26, 10)?;
                    } else {
                        if case == "ready-direction" {
                            let preparer: ISpikeInputPreparer = composing.control.cast()?;
                            unsafe {
                                preparer.Prepare(editor.context.as_raw()).ok()?;
                            }
                        }
                        let mut selection = store.selection.get();
                        selection.style.ase = if case == "unknown-active" {
                            TS_AE_NONE
                        } else {
                            TS_AE_END
                        };
                        store.selection.set(selection);
                    }
                    if case == "ready-direction" {
                        let units: Vec<_> = "👨‍👩‍👧‍👦🇯🇵b".encode_utf16().collect();
                        assert!(unsafe {
                            composing
                                .control
                                .CheckSource(units.as_ptr(), units.len() as u32)
                        }
                        .is_err());
                    } else {
                        let selected = store.selection.get();
                        let mut disposition = INPUT_UNCERTAIN;
                        assert!(unsafe {
                            selector.Extend(editor.context.as_raw(), 1, 1, &mut disposition)
                        }
                        .is_err());
                        assert_eq!(disposition, INPUT_NOT_ATTEMPTED);
                        assert_selection(
                            editor,
                            (selected.acpStart, selected.acpEnd, selected.style.ase),
                        )?;
                    }
                }
                "home-retire" => {
                    editor.navigate(0x24, true, 1)?;
                    assert_selection(editor, (0, 26, TS_AE_START))?;
                    assert!(no_composition(&editor.context)?);
                }
                _ => {}
            }
            if matches!(case, "convert" | "convert-forward") {
                assert_eq!(input_notify_host::body_reads()?, reads);
                editor.input(WM_KEYDOWN, WPARAM(0x76), LPARAM(1))?;
                let (broker, bridge) = editor.broker.as_ref().ok_or(E_FAIL)?;
                assert!(bridge.get().active());
                tracking_host::wait_result(window, broker, bridge, true)?;
                let expected = if right {
                    "prefix 日本語🇯🇵b suffix"
                } else {
                    "prefix ae\u{301}日本語 suffix"
                };
                assert_eq!(
                    *store.text.borrow(),
                    expected.encode_utf16().collect::<Vec<_>>()
                );
                assert_eq!(store.undo.borrow().len(), 1);
                store.undo_once()?;
                assert_eq!(*store.text.borrow(), original);
                assert_selection(
                    editor,
                    if right {
                        (7, 21, TS_AE_END)
                    } else {
                        (10, 26, TS_AE_START)
                    },
                )?;
            } else if matches!(
                case,
                "type-after-collapse" | "replace-tracked" | "backspace-tracked" | "delete-tracked"
            ) {
                let selection = store.selection.get();
                if matches!(case, "type-after-collapse" | "replace-tracked") {
                    editor.insert(&['z' as u16])?;
                } else if case == "backspace-tracked" {
                    editor.input(WM_CHAR, WPARAM(8), LPARAM(3))?;
                } else {
                    editor.delete_forward(3)?;
                }
                assert_eq!(editor.input_disposition.get(), INPUT_APPLIED);
                assert_eq!(store.set_text_calls.get(), writes + 1);
                let expected = match case {
                    "type-after-collapse" => "prefix ae\u{301}z👨‍👩‍👧‍👦🇯🇵b suffix",
                    "replace-tracked" => "prefix ae\u{301}z suffix",
                    _ => "prefix ae\u{301} suffix",
                }
                .encode_utf16()
                .collect::<Vec<_>>();
                assert_eq!(*store.text.borrow(), expected);
                assert_eq!(store.undo.borrow().len(), 1);
                assert!(owned_view(&editor.context)?.cast::<IUnknown>()? == identity);
                store.undo_once()?;
                assert_eq!(*store.text.borrow(), original);
                assert_selection(
                    editor,
                    (selection.acpStart, selection.acpEnd, selection.style.ase),
                )?;
                store.redo_once()?;
                assert_eq!(*store.text.borrow(), expected);
            } else {
                assert_eq!(*store.text.borrow(), original);
                assert_eq!(store.set_text_calls.get(), writes);
                assert!(store.undo.borrow().is_empty());
            }
        }
    }
    if !matches!(case, "convert" | "convert-forward") {
        assert_eq!(input_notify_host::body_reads()?, reads);
    }
    if matches!(
        case,
        "readonly"
            | "private"
            | "foreign"
            | "invalid"
            | "budget"
            | "overshoot"
            | "fail-before"
            | "fail-after"
    ) {
        assert_eq!(*store.text.borrow(), original);
        assert_eq!(store.set_text_calls.get(), writes);
        assert!(store.undo.borrow().is_empty());
    }
    store.readonly.set(false);
    store.fail_selection.set(false);
    store.fail_selection_after.set(false);
    set_scope(editor, IS_TEXT)?;
    if let Some(foreign) = &mut foreign {
        foreign.end()?;
    }
    editor.clear_tracking();
    pump(window)?;
    assert!(no_composition(&editor.context)?);
    assert!(live.scene()?.underlines.is_empty());
    println!("Tracked selection / {case}: OK");
    Ok(())
}
fn assert_selection(editor: &Editor, expected: (i32, i32, TsActiveSelEnd)) -> Result<()> {
    let selected = editor.store.get().selection.get();
    assert_eq!(
        (selected.acpStart, selected.acpEnd, selected.style.ase),
        expected
    );
    Ok(())
}

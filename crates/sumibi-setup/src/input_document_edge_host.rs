//! Document-edge navigation for the owned single-line host, not visual lines.
use super::*;
use sumibi_core::recovery::Operation;
use sumibi_tip::spike::ISpikeDocumentNavigator;

pub(super) fn test(
    window: &crate::window_host::Window,
    editor: &Editor,
    live: &crate::live_render::Binding,
    client: u32,
    case: &str,
) -> Result<()> {
    let store = editor.store.get();
    let prefix = if case.starts_with("prefix-") || case == "foreign" {
        "prefix "
    } else {
        ""
    };
    let suffix = if case.starts_with("suffix-") {
        " suffix"
    } else {
        ""
    };
    store.external_text(format!("{prefix}{suffix}").encode_utf16().collect())?;
    store.host_selection(prefix.len(), prefix.len())?;
    pump(window)?;
    let text = if case == "rejoin" {
        "🇯x🇵b"
    } else {
        "ae\u{301}👨‍👩‍👧‍👦🇯🇵b"
    };
    editor.insert(&text.encode_utf16().collect::<Vec<_>>())?;
    assert_eq!(editor.input_disposition.get(), INPUT_APPLIED);
    let original_end = prefix.len() + text.encode_utf16().count();
    if case == "rejoin" {
        editor.navigate(0x25, false, 3)?;
        editor.delete_forward(1)?;
    } else if case == "noop-home" {
        editor.navigate(0x25, false, 5)?;
    } else if case != "noop-end" {
        editor.navigate(0x25, false, 2)?;
    }
    assert_eq!(editor.input_disposition.get(), INPUT_APPLIED);
    pump(window)?;
    let identity = owned_view(&editor.context)?.cast::<IUnknown>()?;
    let original = store.text.borrow().clone();
    let before = store.selection.get();
    let reads = input_notify_host::body_reads()?;
    let writes = store.set_text_calls.get();
    let selections = store.set_selection_calls.get();
    let host_selections = store.host_selection_calls.get();
    store.undo.borrow_mut().clear();
    store.redo.borrow_mut().clear();
    let navigator: ISpikeDocumentNavigator =
        editor.composing.as_ref().ok_or(E_FAIL)?.control.cast()?;
    let to_end = matches!(
        case,
        "end"
            | "shift-end"
            | "prefix-end"
            | "suffix-end"
            | "repeat"
            | "noop-end"
            | "backspace"
            | "fail-before"
            | "fail-after"
            | "readonly"
            | "private"
            | "foreign"
            | "invalid"
            | "external"
    );
    let extend = matches!(
        case,
        "shift-home"
            | "shift-end"
            | "reverse"
            | "collapse"
            | "select-convert"
            | "fail-after"
            | "rejoin"
    );
    let count = if case == "repeat" { 1024 } else { 3 };
    let target = if to_end { original.len() as i32 } else { 0 };
    let anchor = if extend { before.acpStart } else { target };
    let expected = (
        anchor.min(target),
        anchor.max(target),
        if target < anchor {
            TS_AE_START
        } else {
            TS_AE_END
        },
    );
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
        "external" => {
            store.select_caret(0, original.len())?;
            store.select_caret(before.acpStart as usize, before.acpEnd as usize)?;
        }
        "fail-before" => store.fail_selection.set(true),
        "fail-after" => store.fail_selection_after.set(true),
        _ => {}
    }
    if matches!(
        case,
        "readonly" | "private" | "foreign" | "invalid" | "external"
    ) {
        if case == "invalid" {
            for (end, shift, count) in [
                (2, 0, 1),
                (1, 2, 1),
                (0, 0, 0),
                (1, 1, 1025),
                (0, 1, u32::MAX),
            ] {
                let mut outcome = INPUT_UNCERTAIN;
                assert!(unsafe {
                    navigator.MoveEdge(editor.context.as_raw(), end, shift, count, &mut outcome)
                }
                .is_err());
                assert_eq!(outcome, INPUT_NOT_ATTEMPTED);
            }
            let mut outcome = INPUT_UNCERTAIN;
            assert!(
                unsafe { navigator.MoveEdge(std::ptr::null_mut(), 1, 0, 1, &mut outcome) }.is_err()
            );
            assert_eq!(outcome, INPUT_NOT_ATTEMPTED);
            assert!(unsafe {
                navigator.MoveEdge(editor.context.as_raw(), 1, 0, 1, std::ptr::null_mut())
            }
            .is_err());
        } else {
            let mut outcome = INPUT_UNCERTAIN;
            assert!(unsafe {
                navigator.MoveEdge(
                    editor.context.as_raw(),
                    u32::from(to_end),
                    u32::from(extend),
                    count,
                    &mut outcome,
                )
            }
            .is_err());
            assert_eq!(outcome, INPUT_NOT_ATTEMPTED);
        }
        assert_selection(editor, (before.acpStart, before.acpEnd, before.style.ase));
        assert_eq!(store.set_selection_calls.get(), selections);
        assert_eq!(
            store.host_selection_calls.get(),
            host_selections + if case == "external" { 2 } else { 0 }
        );
    } else {
        let ticket = if case == "budget" {
            Some(editor.rescue.reserve(Operation::Insert(1)).ok_or(E_FAIL)?)
        } else {
            None
        };
        editor.navigate(if to_end { 0x23 } else { 0x24 }, extend, count as usize)?;
        if let Some(ticket) = ticket {
            editor.rescue.finish(ticket, INPUT_NOT_ATTEMPTED, &[]);
        }
        let failed = matches!(case, "fail-before" | "fail-after");
        let fallback = matches!(case, "prefix-home" | "suffix-end" | "budget");
        if failed {
            assert_eq!(editor.input_disposition.get(), INPUT_UNCERTAIN);
            assert_selection(
                editor,
                if case == "fail-before" {
                    (before.acpStart, before.acpEnd, before.style.ase)
                } else {
                    expected
                },
            );
            assert_eq!(store.set_selection_calls.get(), selections + 1);
            assert_eq!(
                store.host_selection_calls.get(),
                host_selections,
                "never repeat a failed edge move in the host"
            );
            let metadata = editor.rescue.metadata().ok_or(E_FAIL)?;
            assert_eq!(
                metadata.operation,
                Operation::MoveToEdge {
                    to_end,
                    extend,
                    count
                }
            );
            assert!(!metadata.copyable);
            let mut outcome = INPUT_UNCERTAIN;
            assert!(unsafe {
                navigator.MoveEdge(
                    editor.context.as_raw(),
                    u32::from(to_end),
                    u32::from(extend),
                    count,
                    &mut outcome,
                )
            }
            .is_err());
            assert_eq!(outcome, INPUT_NOT_ATTEMPTED);
            assert_eq!(store.set_selection_calls.get(), selections + 1);
            assert_eq!(store.host_selection_calls.get(), host_selections);
        } else {
            assert_eq!(
                editor.input_disposition.get(),
                if fallback {
                    INPUT_NOT_ATTEMPTED
                } else {
                    INPUT_APPLIED
                }
            );
            assert_selection(editor, expected);
            assert_eq!(
                store.set_selection_calls.get(),
                selections + usize::from(!fallback)
            );
            assert_eq!(
                store.host_selection_calls.get(),
                host_selections + usize::from(fallback)
            );
            assert!(editor.rescue.metadata().is_none());
            if fallback {
                assert!(no_composition(&editor.context)?);
            } else {
                assert!(owned_view(&editor.context)?.cast::<IUnknown>()? == identity);
                pump(window)?;
                assert_eq!(
                    live.scene()?.underlines,
                    vec![prefix.len()..if case == "rejoin" { 5 } else { original_end }]
                );
            }
        }
        assert_eq!(*store.text.borrow(), original);
        assert_eq!(store.set_text_calls.get(), writes);
        assert!(store.undo.borrow().is_empty());
        assert_eq!(input_notify_host::body_reads()?, reads);
        if case == "reverse" {
            editor.navigate(0x23, true, 1)?;
            assert_eq!(editor.input_disposition.get(), INPUT_APPLIED);
            assert_selection(editor, (before.acpStart, original.len() as i32, TS_AE_END));
        } else if case == "collapse" {
            editor.navigate(0x23, false, 1)?;
            assert_eq!(editor.input_disposition.get(), INPUT_APPLIED);
            assert_selection(
                editor,
                (original.len() as i32, original.len() as i32, TS_AE_END),
            );
        } else if case == "repeat" {
            editor.navigate(0x23, false, 1024)?;
            assert_eq!(editor.input_disposition.get(), INPUT_APPLIED);
            assert_selection(editor, expected);
        } else if case == "rejoin" {
            assert_selection(editor, (0, 2, TS_AE_START));
            editor.navigate(0x23, false, 1)?;
            assert_eq!(editor.input_disposition.get(), INPUT_APPLIED);
            assert_selection(editor, (5, 5, TS_AE_END));
        }
        if matches!(case, "convert" | "select-convert") {
            editor.input(WM_KEYDOWN, WPARAM(0x76), LPARAM(1))?;
            let (broker, bridge) = editor.broker.as_ref().ok_or(E_FAIL)?;
            assert!(bridge.get().active());
            tracking_host::wait_result(window, broker, bridge, true)?;
            let converted = if case == "select-convert" {
                "日本語🇯🇵b"
            } else {
                "日本語"
            };
            assert_eq!(
                *store.text.borrow(),
                converted.encode_utf16().collect::<Vec<_>>()
            );
            assert_eq!(store.undo.borrow().len(), 1);
            store.undo_once()?;
            assert_eq!(*store.text.borrow(), original);
        } else if matches!(case, "insert" | "backspace" | "rejoin") {
            let selection = store.selection.get();
            let mut changed = original.clone();
            if case == "insert" {
                editor.insert(&['z' as u16])?;
                changed.insert(0, 'z' as u16);
            } else {
                editor.input(WM_CHAR, WPARAM(8), LPARAM(1))?;
                changed.pop();
            }
            assert_eq!(editor.input_disposition.get(), INPUT_APPLIED);
            assert_eq!(*store.text.borrow(), changed);
            assert!(owned_view(&editor.context)?.cast::<IUnknown>()? == identity);
            assert_eq!(store.undo.borrow().len(), 1);
            store.undo_once()?;
            assert_eq!(*store.text.borrow(), original);
            assert_selection(
                editor,
                (selection.acpStart, selection.acpEnd, selection.style.ase),
            );
            store.redo_once()?;
            assert_eq!(*store.text.borrow(), changed);
        }
    }
    if !matches!(case, "convert" | "select-convert") {
        assert_eq!(input_notify_host::body_reads()?, reads);
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
    println!("Document edge / {case}: OK");
    Ok(())
}
fn assert_selection(editor: &Editor, expected: (i32, i32, TsActiveSelEnd)) {
    let selected = editor.store.get().selection.get();
    assert_eq!(
        (selected.acpStart, selected.acpEnd, selected.style.ase),
        expected
    );
}

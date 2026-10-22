//! Exact selected edits of recorded synthetic input through the actual DLL.
use super::*;
use sumibi_core::recovery::{Operation, Outcome};
use sumibi_tip::spike::ISpikeSelectionEraser;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Edit {
    Replace,
    Delete,
    Backspace,
}
pub(super) fn test(
    window: &crate::window_host::Window,
    editor: &Editor,
    live: &crate::live_render::Binding,
    client: u32,
    case: &str,
) -> Result<()> {
    let store = editor.store.get();
    let maximum = matches!(case, "maximum" | "overflow");
    let run_start = if maximum { 0 } else { 7 };
    store.external_text(if maximum {
        Vec::new()
    } else {
        "prefix  suffix".encode_utf16().collect()
    })?;
    store.host_selection(run_start, run_start)?;
    pump(window)?;
    let initial = if maximum {
        "漢".repeat(1024)
    } else if case == "rejoin" {
        "🇯x🇵b".into()
    } else {
        "ae\u{301}👨‍👩‍👧‍👦🇯🇵b".into()
    };
    editor.insert(&initial.encode_utf16().collect::<Vec<_>>())?;
    assert_eq!(editor.input_disposition.get(), INPUT_APPLIED);
    if maximum {
        editor.navigate(0x25, false, 512)?;
        editor.navigate(0x25, true, 2)?;
    } else if case.starts_with("all-") {
        editor.navigate(0x25, true, 5)?;
    } else if case == "forward" {
        editor.navigate(0x25, false, 3)?;
        editor.navigate(0x27, true, 2)?;
    } else if case == "prefix" {
        editor.navigate(0x25, false, 3)?;
        editor.navigate(0x25, true, 2)?;
    } else if case == "suffix" {
        editor.navigate(0x25, true, 3)?;
    } else if case == "rejoin" {
        editor.navigate(0x25, false, 2)?;
        editor.navigate(0x25, true, 1)?;
    } else {
        editor.navigate(0x25, false, 1)?;
        editor.navigate(0x25, true, 2)?;
    }
    assert_eq!(editor.input_disposition.get(), INPUT_APPLIED);
    pump(window)?;
    let identity = owned_view(&editor.context)?.cast::<IUnknown>()?;
    let original = store.text.borrow().clone();
    let selection = store.selection.get();
    let expected_range = if maximum {
        (510, 512)
    } else if case.starts_with("all-") {
        (7, 26)
    } else if case == "prefix" {
        (7, 10)
    } else if case == "suffix" {
        (10, 26)
    } else if case == "rejoin" {
        (9, 10)
    } else {
        (10, 25)
    };
    assert_eq!((selection.acpStart, selection.acpEnd), expected_range);
    assert_eq!(
        selection.style.ase,
        if case == "forward" {
            TS_AE_END
        } else {
            TS_AE_START
        }
    );
    let reads = input_notify_host::body_reads()?;
    let writes = store.set_text_calls.get();
    store.undo.borrow_mut().clear();
    store.redo.borrow_mut().clear();
    let kind = if case.contains("backspace") {
        Edit::Backspace
    } else if case.contains("delete") || case == "rejoin" {
        Edit::Delete
    } else {
        Edit::Replace
    };
    let count = if case == "backspace-repeat" { 1024 } else { 3 };
    let supplied: Vec<_> = if kind != Edit::Replace {
        vec![]
    } else {
        match case {
            "maximum" => "😀",
            "repeat" | "overflow" => "zzz",
            _ => "z",
        }
        .encode_utf16()
        .collect()
    };
    let mut foreign = None;
    match case {
        "text-before" => store.fail_text_before.set(true),
        "text-after" | "delete-text-after" | "backspace-text-after" => {
            store.fail_text_after.set(true)
        }
        "caret-before" => store.fail_selection.set(true),
        "caret-after" => store.fail_selection_after.set(true),
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
        "external-replace" | "external-delete" => {
            store.select_caret(selection.acpStart as usize, selection.acpEnd as usize)?;
            store.select_caret(selection.acpEnd as usize, selection.acpStart as usize)?;
        }
        _ => {}
    }
    if matches!(case, "readonly" | "foreign" | "overflow") {
        let mut disposition = INPUT_UNCERTAIN;
        assert!(direct(editor, kind, &supplied, count, &mut disposition).is_err());
        assert_eq!(disposition, INPUT_NOT_ATTEMPTED);
        assert_eq!(*store.text.borrow(), original);
        assert_eq!(store.set_text_calls.get(), writes);
        assert!(store.undo.borrow().is_empty());
    } else {
        let fallback = case.ends_with("budget") || case == "private";
        let ticket = if case.ends_with("budget") {
            Some(editor.rescue.reserve(Operation::Insert(1)).ok_or(E_FAIL)?)
        } else {
            None
        };
        if case == "repeat" {
            editor.input(WM_CHAR, WPARAM('z' as usize), LPARAM(3))?;
        } else {
            apply(editor, kind, &supplied, count)?;
        }
        if let Some(ticket) = ticket {
            editor.rescue.finish(ticket, INPUT_NOT_ATTEMPTED, &[]);
        }
        let failed = matches!(
            case,
            "text-before"
                | "text-after"
                | "delete-text-after"
                | "backspace-text-after"
                | "caret-before"
                | "caret-after"
                | "status-restored"
                | "scope-restored"
                | "compartment-restored"
        );
        let mut expected = original.clone();
        if case != "text-before" {
            expected.splice(
                selection.acpStart as usize..selection.acpEnd as usize,
                supplied.iter().copied(),
            );
        }
        assert_eq!(
            *store.text.borrow(),
            expected,
            "{case}: only the exact selection is changed once"
        );
        assert_eq!(store.set_text_calls.get(), writes + usize::from(!fallback));
        assert_eq!(
            store.undo.borrow().len(),
            usize::from(case != "text-before")
        );
        if failed {
            let uncertain = case.contains("text-");
            assert_eq!(
                editor.input_disposition.get(),
                if uncertain {
                    INPUT_UNCERTAIN
                } else {
                    INPUT_APPLIED_UNTRACKED
                }
            );
            let metadata = editor.rescue.metadata().ok_or(E_FAIL)?;
            assert_eq!(
                metadata.operation,
                match kind {
                    Edit::Replace => Operation::Insert(supplied.len()),
                    Edit::Delete => Operation::DeleteSelection,
                    Edit::Backspace => Operation::Backspace(count),
                }
            );
            assert_eq!(
                metadata.outcome,
                if uncertain {
                    Outcome::Uncertain
                } else {
                    Outcome::AppliedUntracked
                }
            );
            assert_eq!(
                metadata.copyable,
                kind == Edit::Replace && !case.ends_with("restored")
            );
            let mut disposition = INPUT_UNCERTAIN;
            assert!(direct(editor, kind, &supplied, count, &mut disposition).is_err());
            assert_eq!(disposition, INPUT_NOT_ATTEMPTED);
            assert_eq!(*store.text.borrow(), expected);
            assert_eq!(store.set_text_calls.get(), writes + 1);
            editor.input(WM_KEYDOWN, WPARAM(0x76), LPARAM(1))?;
            assert!(!editor.broker.as_ref().ok_or(E_FAIL)?.1.get().active());
        } else {
            assert_eq!(
                editor.input_disposition.get(),
                if fallback {
                    INPUT_NOT_ATTEMPTED
                } else {
                    INPUT_APPLIED
                }
            );
            if fallback
                || case == "external-delete"
                || matches!(case, "all-delete" | "all-backspace")
            {
                assert!(no_composition(&editor.context)?);
            } else {
                let view = owned_view(&editor.context)?;
                if case == "external-replace" {
                    assert!(view.cast::<IUnknown>()? != identity);
                } else {
                    assert!(view.cast::<IUnknown>()? == identity);
                }
                let range = unsafe { view.GetRange()? };
                let (mut start, mut length) = (0, 0);
                unsafe {
                    range
                        .cast::<ITfRangeACP>()?
                        .GetExtent(&mut start, &mut length)?;
                }
                let end = expected.len() - run_start;
                assert_eq!(
                    (start, start + length),
                    if case == "external-replace" {
                        (
                            selection.acpStart,
                            selection.acpStart + supplied.len() as i32,
                        )
                    } else {
                        (run_start as i32, end as i32)
                    }
                );
                assert_eq!(
                    store.last_text_lock.get(),
                    store.last_composition_lock.get()
                );
                pump(window)?;
                if !maximum {
                    assert_eq!(
                        live.scene()?.underlines,
                        vec![start as usize..(start + length) as usize]
                    );
                }
            }
            assert!(editor.rescue.metadata().is_none());
        }
        assert_eq!(input_notify_host::body_reads()?, reads);
        if matches!(case, "convert" | "delete-convert" | "external-replace") {
            store.undo.borrow_mut().clear();
            editor.input(WM_KEYDOWN, WPARAM(0x76), LPARAM(1))?;
            let (broker, bridge) = editor.broker.as_ref().ok_or(E_FAIL)?;
            assert!(bridge.get().active());
            tracking_host::wait_result(window, broker, bridge, true)?;
            let converted = if case == "external-replace" {
                "prefix ae\u{301}日本語b suffix"
            } else {
                "prefix 日本語 suffix"
            };
            assert_eq!(
                *store.text.borrow(),
                converted.encode_utf16().collect::<Vec<_>>()
            );
            assert_eq!(store.undo.borrow().len(), 1);
            store.undo_once()?;
            assert_eq!(*store.text.borrow(), expected);
        } else if case == "rejoin" {
            editor.input(WM_CHAR, WPARAM(8), LPARAM(1))?;
            assert_eq!(editor.input_disposition.get(), INPUT_APPLIED);
            assert_eq!(
                *store.text.borrow(),
                "prefix b suffix".encode_utf16().collect::<Vec<_>>()
            );
            assert_eq!(store.set_text_calls.get(), writes + 2);
            assert_eq!(store.undo.borrow().len(), 2);
            assert!(owned_view(&editor.context)?.cast::<IUnknown>()? == identity);
            store.undo_once()?;
            assert_eq!(*store.text.borrow(), expected);
            store.undo_once()?;
            assert_eq!(*store.text.borrow(), original);
        } else if case != "text-before" {
            store.fail_selection.set(false);
            store.fail_selection_after.set(false);
            store.undo_once()?;
            assert_eq!(*store.text.borrow(), original);
            let restored = store.selection.get();
            assert_eq!(
                (restored.acpStart, restored.acpEnd, restored.style.ase),
                (selection.acpStart, selection.acpEnd, selection.style.ase)
            );
            store.redo_once()?;
            assert_eq!(*store.text.borrow(), expected);
        }
    }
    if !matches!(case, "convert" | "delete-convert" | "external-replace") {
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
    println!("Recorded selection edit / {case}: OK");
    Ok(())
}
fn apply(editor: &Editor, kind: Edit, text: &[u16], count: u32) -> Result<()> {
    match kind {
        Edit::Replace => editor.insert(text),
        Edit::Delete => editor.delete_forward(count as usize),
        Edit::Backspace => editor
            .input(WM_CHAR, WPARAM(8), LPARAM(count as isize))
            .map(|_| ()),
    }
}
fn direct(
    editor: &Editor,
    kind: Edit,
    text: &[u16],
    count: u32,
    disposition: &mut u32,
) -> Result<()> {
    let control = &editor.composing.as_ref().ok_or(E_FAIL)?.control;
    unsafe {
        match kind {
            Edit::Replace => control.cast::<ISpikeInputWriter>()?.Insert(
                editor.context.as_raw(),
                text.as_ptr(),
                text.len() as u32,
                disposition,
            ),
            Edit::Delete => control
                .cast::<ISpikeSelectionEraser>()?
                .DeleteSelection(editor.context.as_raw(), disposition),
            Edit::Backspace => control.cast::<ISpikeInputEraser>()?.Backspace(
                editor.context.as_raw(),
                count,
                disposition,
            ),
        }
        .ok()
    }
}

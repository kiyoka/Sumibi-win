//! Real DLL suffix deletion. All data belongs to this diagnostic ACP store.
use super::*;
use sumibi_tip::spike::{INPUT_APPLIED, INPUT_APPLIED_UNTRACKED};

pub(crate) fn exercise_backspace(
    manager: &ITfThreadMgr,
    client: u32,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    for case in [
        "erase/partial",
        "erase/supplementary",
        "erase/repeat",
        "erase/all",
        "erase/undo",
        "erase/redo",
        "erase/apply",
        "erase/new-run",
        "erase/close",
        "erase/untracked",
        "erase/empty",
        "erase/selection",
        "erase/moved",
        "erase/private",
        "erase/readonly",
        "erase/lock-denied",
        "erase/foreign",
        "erase/forced",
        "erase/cross-start",
        "erase/text-fail-before",
        "erase/text-fail-after",
        "erase/caret-fail",
        "erase/status-restored",
        "erase/scope-restored",
        "erase/compartment-restored",
        "erase/invalid-count",
        "erase/capacity",
        "erase/grapheme-combining",
        "erase/grapheme-family",
        "erase/grapheme-modifier",
        "erase/grapheme-flag",
        "erase/grapheme-repeat",
        "erase/grapheme-all",
        "erase/grapheme-fragments",
        "erase/grapheme-uncertain",
        "erase/grapheme-run-start",
        "erase/grapheme-host-fallback",
    ] {
        run(manager, client, RunMode::TransactionTest(case))?;
    }
    Ok(())
}
pub(in crate::edit_host::input_host) fn test_backspace(
    window: &crate::window_host::Window,
    editor: &Editor,
    live: &crate::live_render::Binding,
    client: u32,
    case: &str,
) -> Result<()> {
    if let Some(kind) = case.strip_prefix("grapheme-") {
        return test_graphemes(window, editor, live, kind);
    }
    let store = editor.store.get();
    let composing = editor.composing.as_ref().ok_or(E_FAIL)?;
    let eraser: ISpikeInputEraser = composing.control.cast()?;
    store.external_text(if matches!(case, "empty" | "capacity") {
        Vec::new()
    } else {
        "prefix  suffix".encode_utf16().collect()
    })?;
    store.host_selection(
        if matches!(case, "empty" | "capacity") {
            0
        } else {
            7
        },
        if matches!(case, "empty" | "capacity") {
            0
        } else {
            7
        },
    )?;
    pump(window)?;
    if !matches!(case, "untracked" | "empty") {
        editor.insert(&if case == "capacity" {
            vec!['x' as u16; 1024]
        } else {
            "a😀b".encode_utf16().collect()
        })?;
        assert_eq!(
            editor.input_disposition.get(),
            INPUT_APPLIED,
            "{case}: prefill"
        );
    }
    pump(window)?;
    let identity = if matches!(case, "untracked" | "empty") {
        None
    } else {
        Some(owned_view(&editor.context)?.cast::<IUnknown>()?)
    };
    let original = store.text.borrow().clone();
    store.undo.borrow_mut().clear();
    store.redo.borrow_mut().clear();
    store.set_text_calls.set(0);
    let reads = super::input_notify_host::body_reads()?;
    let mut foreign = None;
    match case {
        "private" => set_scope(editor, IS_PRIVATE)?,
        "readonly" => store.readonly.set(true),
        "lock-denied" => store.deny_lock.set(true),
        "selection" => store.host_selection(7, 11)?,
        "moved" => store.host_selection(6, 6)?,
        "foreign" => {
            foreign = Some(crate::edit_host::composition_host::Composition::start(
                &editor.context,
                client,
                0,
                2,
            )?);
        }
        "forced" => unsafe {
            editor
                .context
                .cast::<ITfContextOwnerCompositionServices>()?
                .TerminateComposition(&owned_view(&editor.context)?)?;
        },
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
        _ => {}
    }
    if case == "invalid-count" || case == "readonly" {
        for count in if case == "readonly" {
            vec![1]
        } else {
            vec![0, 1025, u32::MAX]
        } {
            let mut disposition = INPUT_UNCERTAIN;
            let hr = unsafe { eraser.Backspace(editor.context.as_raw(), count, &mut disposition) };
            assert!(hr.is_err());
            assert_eq!(disposition, INPUT_NOT_ATTEMPTED);
        }
        assert_eq!(*store.text.borrow(), original);
        assert_eq!(store.set_text_calls.get(), 0);
        assert!(store.undo.borrow().is_empty());
    } else {
        let count = match case {
            "repeat" | "undo" | "redo" => 2,
            "all" | "new-run" => 3,
            "cross-start" => 4,
            "capacity" => 1024,
            _ => 1,
        };
        editor.input(WM_CHAR, WPARAM(0x08), LPARAM(count))?;
        if matches!(
            case,
            "text-fail-before"
                | "text-fail-after"
                | "caret-fail"
                | "status-restored"
                | "scope-restored"
                | "compartment-restored"
        ) {
            assert_eq!(
                store.set_text_calls.get(),
                1,
                "{case}: attempted deletion must never be repeated"
            );
            assert_eq!(
                editor.input_disposition.get(),
                if case.starts_with("text-fail") {
                    INPUT_UNCERTAIN
                } else {
                    INPUT_APPLIED_UNTRACKED
                }
            );
            assert!(store.input_problem.get());
            assert_eq!(
                store.undo.borrow().len(),
                usize::from(case != "text-fail-before")
            );
            assert_eq!(
                *store.text.borrow(),
                if case == "text-fail-before" {
                    "prefix a😀b suffix"
                } else {
                    "prefix a😀 suffix"
                }
                .encode_utf16()
                .collect::<Vec<_>>()
            );
            let after = store.text.borrow().clone();
            let mut disposition = INPUT_UNCERTAIN;
            unsafe {
                assert!(eraser
                    .Backspace(editor.context.as_raw(), 1, &mut disposition)
                    .is_err());
                assert!(composing.control.PrepareConversion().is_err());
            }
            assert_eq!(disposition, INPUT_NOT_ATTEMPTED);
            assert_eq!(*store.text.borrow(), after);
            assert_eq!(store.set_text_calls.get(), 1);
            editor.input(WM_KEYDOWN, WPARAM(0x76), LPARAM(1))?;
            assert!(!editor.broker.as_ref().ok_or(E_FAIL)?.1.get().active());
            editor.input(WM_KEYDOWN, WPARAM(0x1b), LPARAM(1))?;
            assert!(!store.input_problem.get());
            editor.input(WM_KEYDOWN, WPARAM(0x78), LPARAM(1))?;
            editor.service_recovery(window, &crate::recovery_view::SystemClipboard)?;
            editor.insert(&['z' as u16])?;
            assert_eq!(editor.input_disposition.get(), INPUT_APPLIED);
            pump(window)?;
            let end = store.selection.get().acpEnd as usize;
            assert_eq!(live.scene()?.underlines, vec![end - 1..end]);
        } else if matches!(
            case,
            "untracked"
                | "empty"
                | "selection"
                | "moved"
                | "private"
                | "lock-denied"
                | "foreign"
                | "forced"
                | "cross-start"
        ) {
            assert_eq!(
                editor.input_disposition.get(),
                INPUT_NOT_ATTEMPTED,
                "{case}"
            );
            assert_eq!(store.set_text_calls.get(), 0, "{case}: host fallback only");
            assert_eq!(
                store.undo.borrow().len(),
                match case {
                    "empty" => 0,
                    "cross-start" => 4,
                    _ => 1,
                }
            );
            let expected = match case {
                "empty" => "",
                "untracked" => "prefix suffix",
                "selection" => "prefix  suffix",
                "moved" => "prefi a😀b suffix",
                "cross-start" => "prefix suffix",
                _ => "prefix a😀 suffix",
            };
            assert_eq!(
                *store.text.borrow(),
                expected.encode_utf16().collect::<Vec<_>>()
            );
            store.deny_lock.set(false);
            store.readonly.set(false);
            set_scope(editor, IS_TEXT)?;
            editor.clear_tracking();
            if foreign.is_some() {
                assert!(!no_composition(&editor.context)?);
            }
            editor.input(WM_KEYDOWN, WPARAM(0x76), LPARAM(1))?;
            assert!(
                !editor.broker.as_ref().ok_or(E_FAIL)?.1.get().active(),
                "{case}: no inferred conversion range"
            );
        } else {
            assert_eq!(editor.input_disposition.get(), INPUT_APPLIED, "{case}");
            assert_eq!(store.set_text_calls.get(), 1);
            assert_eq!(store.undo.borrow().len(), 1);
            assert_eq!(
                store.last_text_lock.get(),
                store.last_composition_lock.get(),
                "{case}: same write lock"
            );
            pump(window)?;
            if matches!(case, "all" | "capacity" | "new-run") {
                assert!(no_composition(&editor.context)?);
                assert!(live.scene()?.underlines.is_empty());
                assert_eq!(
                    *store.text.borrow(),
                    if case == "capacity" {
                        Vec::new()
                    } else {
                        "prefix  suffix".encode_utf16().collect::<Vec<_>>()
                    }
                );
                editor.input(WM_KEYDOWN, WPARAM(0x76), LPARAM(1))?;
                assert!(!editor.broker.as_ref().ok_or(E_FAIL)?.1.get().active());
                if case == "new-run" {
                    editor.insert(&['z' as u16])?;
                    assert_eq!(editor.input_disposition.get(), INPUT_APPLIED);
                    pump(window)?;
                    assert_eq!(live.scene()?.underlines, vec![7..8]);
                    assert!(Some(owned_view(&editor.context)?.cast::<IUnknown>()?) != identity);
                }
            } else {
                assert!(Some(owned_view(&editor.context)?.cast::<IUnknown>()?) == identity);
                assert_eq!(
                    live.scene()?.underlines,
                    vec![7..if count == 2 { 8 } else { 10 }]
                );
                match case {
                    "supplementary" => {
                        editor.input(WM_CHAR, WPARAM(0x08), LPARAM(1))?;
                        assert_eq!(editor.input_disposition.get(), INPUT_APPLIED);
                        assert_eq!(
                            *store.text.borrow(),
                            "prefix a suffix".encode_utf16().collect::<Vec<_>>()
                        );
                        assert_eq!(store.set_text_calls.get(), 2);
                        pump(window)?;
                        assert_eq!(live.scene()?.underlines, vec![7..8]);
                        assert!(Some(owned_view(&editor.context)?.cast::<IUnknown>()?) == identity);
                    }
                    "undo" | "redo" => {
                        let deleted = store.text.borrow().clone();
                        editor.input(WM_CHAR, WPARAM(0x1a), LPARAM(1))?;
                        assert_eq!(
                            *store.text.borrow(),
                            original,
                            "{case}: one Undo restores whole repeated deletion"
                        );
                        if case == "redo" {
                            editor.input(WM_CHAR, WPARAM(0x19), LPARAM(1))?;
                            assert_eq!(*store.text.borrow(), deleted);
                        }
                    }
                    "apply" => {
                        let deleted = store.text.borrow().clone();
                        store.undo.borrow_mut().clear();
                        editor.input(WM_KEYDOWN, WPARAM(0x76), LPARAM(1))?;
                        let (broker, bridge) = editor.broker.as_ref().ok_or(E_FAIL)?;
                        tracking_host::wait_result(window, broker, bridge, true)?;
                        assert_eq!(
                            *store.text.borrow(),
                            "prefix 日本語 suffix".encode_utf16().collect::<Vec<_>>()
                        );
                        assert_eq!(store.undo.borrow().len(), 1);
                        editor.input(WM_CHAR, WPARAM(0x1a), LPARAM(1))?;
                        assert_eq!(*store.text.borrow(), deleted);
                    }
                    "close" => test_broker_sent_close(window, editor)?,
                    _ => {}
                }
            }
        }
    }
    if case != "apply" {
        assert_eq!(
            super::input_notify_host::body_reads()?,
            reads,
            "{case}: no DLL body read for deletion"
        );
    }
    store.deny_lock.set(false);
    store.readonly.set(false);
    store.fail_selection.set(false);
    editor.clear_tracking();
    if let Some(foreign) = &mut foreign {
        foreign.end()?;
    }
    if window.is_open() {
        pump(window)?;
        assert!(no_composition(&editor.context)?, "{case}: cleanup");
        assert!(live.scene()?.underlines.is_empty());
    }
    println!("Backspace transaction / {case}: OK");
    Ok(())
}

fn test_graphemes(
    window: &crate::window_host::Window,
    editor: &Editor,
    live: &crate::live_render::Binding,
    kind: &str,
) -> Result<()> {
    let (prefix, fragments, remaining, count): (&str, &[&str], &str, isize) = match kind {
        "combining" => ("prefix ", &["ae\u{301}"], "a", 1),
        "family" | "uncertain" | "host-fallback" => ("prefix ", &["a👨‍👩‍👧‍👦"], "a", 1),
        "modifier" => ("prefix ", &["a👍🏽"], "a", 1),
        "flag" => ("prefix ", &["a🇯🇵"], "a", 1),
        "repeat" => ("prefix ", &["ae\u{301}🇯🇵👨‍👩‍👧‍👦"], "a", 3),
        "all" => ("prefix ", &["e\u{301}🇯🇵"], "", 2),
        "fragments" => ("prefix ", &["a", "e", "\u{301}"], "a", 1),
        "run-start" => ("prefix e", &["\u{301}"], "", 1),
        _ => return Err(E_INVALIDARG.into()),
    };
    let store = editor.store.get();
    let start = prefix.encode_utf16().count();
    store.external_text(format!("{prefix} suffix").encode_utf16().collect())?;
    store.host_selection(start, start)?;
    pump(window)?;
    for fragment in fragments {
        editor.insert(&fragment.encode_utf16().collect::<Vec<_>>())?;
        assert_eq!(editor.input_disposition.get(), INPUT_APPLIED);
    }
    pump(window)?;
    let identity = owned_view(&editor.context)?.cast::<IUnknown>()?;
    if kind == "host-fallback" {
        editor.clear_tracking();
        pump(window)?;
        assert!(no_composition(&editor.context)?);
    }
    let original = store.text.borrow().clone();
    store.undo.borrow_mut().clear();
    store.redo.borrow_mut().clear();
    store.set_text_calls.set(0);
    let reads = super::input_notify_host::body_reads()?;
    store.fail_text_after.set(kind == "uncertain");
    editor.input(WM_CHAR, WPARAM(8), LPARAM(count))?;
    let expected: Vec<_> = format!("{prefix}{remaining} suffix")
        .encode_utf16()
        .collect();
    assert_eq!(*store.text.borrow(), expected);
    let dll_writes = usize::from(kind != "host-fallback");
    assert_eq!(
        store.set_text_calls.get(),
        dll_writes,
        "whole deletion once; untracked input stays with host"
    );
    assert_eq!(store.undo.borrow().len(), 1);
    if kind == "uncertain" {
        assert_eq!(editor.input_disposition.get(), INPUT_UNCERTAIN);
        let eraser: ISpikeInputEraser = editor.composing.as_ref().ok_or(E_FAIL)?.control.cast()?;
        let mut disposition = INPUT_UNCERTAIN;
        unsafe {
            assert!(eraser
                .Backspace(editor.context.as_raw(), 1, &mut disposition)
                .is_err());
        }
        assert_eq!(disposition, INPUT_NOT_ATTEMPTED);
        assert_eq!(
            store.set_text_calls.get(),
            1,
            "no repeat after uncertain grapheme write"
        );
        assert_eq!(*store.text.borrow(), expected);
    } else if kind == "host-fallback" {
        assert_eq!(editor.input_disposition.get(), INPUT_NOT_ATTEMPTED);
        assert!(no_composition(&editor.context)?);
        assert!(live.scene()?.underlines.is_empty());
    } else {
        assert_eq!(editor.input_disposition.get(), INPUT_APPLIED);
        assert_eq!(
            store.last_text_lock.get(),
            store.last_composition_lock.get()
        );
        pump(window)?;
        let end = start + remaining.encode_utf16().count();
        assert_eq!(store.selection.get().acpStart, end as i32);
        assert_eq!(store.selection.get().acpEnd, end as i32);
        if remaining.is_empty() {
            assert!(no_composition(&editor.context)?);
            assert!(live.scene()?.underlines.is_empty());
        } else {
            assert_eq!(live.scene()?.underlines, vec![start..end]);
            assert!(owned_view(&editor.context)?.cast::<IUnknown>()? == identity);
        }
    }
    assert_eq!(super::input_notify_host::body_reads()?, reads);
    store.fail_text_after.set(false);
    editor.input(WM_CHAR, WPARAM(0x1a), LPARAM(1))?;
    assert_eq!(
        *store.text.borrow(),
        original,
        "one Undo restores the entire cluster deletion"
    );
    assert_eq!(store.set_text_calls.get(), dll_writes);
    editor.clear_tracking();
    pump(window)?;
    assert!(no_composition(&editor.context)?);
    assert!(live.scene()?.underlines.is_empty());
    assert_eq!(super::input_notify_host::body_reads()?, reads);
    println!("Backspace transaction / grapheme-{kind}: OK");
    Ok(())
}

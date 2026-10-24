//! Verified insertion inside synthetic recorded input through the actual DLL.
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
    let run_start = if matches!(case, "maximum" | "overflow") {
        0
    } else {
        7
    };
    store.external_text(if run_start == 0 {
        Vec::new()
    } else {
        "prefix  suffix".encode_utf16().collect()
    })?;
    store.host_selection(run_start, run_start)?;
    pump(window)?;
    let initial = match case {
        "rejoin" => "🇯x🇵b".to_owned(),
        "combining" => "ab".to_owned(),
        "maximum" => "漢".repeat(1023),
        "overflow" => "漢".repeat(1024),
        _ => "a😀b".to_owned(),
    };
    editor.insert(&initial.encode_utf16().collect::<Vec<_>>())?;
    assert_eq!(editor.input_disposition.get(), INPUT_APPLIED);
    editor.navigate(
        0x25,
        false,
        match case {
            "start" | "rejoin" => 3,
            "combining" => 1,
            "maximum" | "overflow" => 512,
            _ => 2,
        },
    )?;
    assert_eq!(editor.input_disposition.get(), INPUT_APPLIED);
    if case == "rejoin" {
        editor.delete_forward(1)?;
        assert_eq!(editor.input_disposition.get(), INPUT_APPLIED);
    }
    pump(window)?;
    let identity = owned_view(&editor.context)?.cast::<IUnknown>()?;
    let original = store.text.borrow().clone();
    let selection = store.selection.get();
    let reads = input_notify_host::body_reads()?;
    let writes = store.set_text_calls.get();
    store.undo.borrow_mut().clear();
    store.redo.borrow_mut().clear();
    let text: Vec<_> = match case {
        "multi" => "e\u{301}👩‍💻",
        "repeat" => "zzz",
        "combining" => "\u{301}",
        "rejoin" => "x",
        _ => "z",
    }
    .encode_utf16()
    .collect();
    let writer: ISpikeInputWriter = editor.composing.as_ref().ok_or(E_FAIL)?.control.cast()?;
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
        "lock-denied" => store.deny_lock.set(true),
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
    if matches!(case, "readonly" | "foreign" | "lock-denied" | "overflow") {
        let mut disposition = INPUT_UNCERTAIN;
        assert!(unsafe {
            writer.Insert(
                editor.context.as_raw(),
                text.as_ptr(),
                text.len() as u32,
                &mut disposition,
            )
        }
        .is_err());
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
        if case == "repeat" {
            editor.input(WM_CHAR, WPARAM('z' as usize), LPARAM(3))?;
        } else {
            editor.insert(&text)?;
        }
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
        let fallback = matches!(case, "private" | "budget");
        let mut expected = original.clone();
        if case != "text-fail-before" {
            expected.splice(
                selection.acpStart as usize..selection.acpStart as usize,
                text.iter().copied(),
            );
        }
        assert_eq!(
            *store.text.borrow(),
            expected,
            "{case}: input appears exactly once"
        );
        assert_eq!(store.set_text_calls.get(), writes + usize::from(!fallback));
        assert_eq!(
            store.undo.borrow().len(),
            usize::from(case != "text-fail-before")
        );
        if failed {
            let outcome = if case.starts_with("text-fail") {
                INPUT_UNCERTAIN
            } else {
                INPUT_APPLIED_UNTRACKED
            };
            assert_eq!(editor.input_disposition.get(), outcome);
            let metadata = editor.rescue.metadata().ok_or(E_FAIL)?;
            assert_eq!(metadata.operation, Operation::Insert(text.len()));
            assert_eq!(
                metadata.outcome,
                if outcome == INPUT_UNCERTAIN {
                    Outcome::Uncertain
                } else {
                    Outcome::AppliedUntracked
                }
            );
            // Only the supplied fragment is eligible for rescue, never the run
            // prefix/suffix. Protection history redacts even that fragment.
            assert_eq!(metadata.copyable, !case.ends_with("restored"));
            let mut disposition = INPUT_UNCERTAIN;
            assert!(unsafe {
                writer.Insert(
                    editor.context.as_raw(),
                    text.as_ptr(),
                    text.len() as u32,
                    &mut disposition,
                )
            }
            .is_err());
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
            assert_eq!(
                store.selection.get().acpStart,
                selection.acpStart + text.len() as i32
            );
            if !fallback {
                assert_eq!(
                    store.last_text_lock.get(),
                    store.last_composition_lock.get()
                );
                pump(window)?;
                assert!(owned_view(&editor.context)?.cast::<IUnknown>()? == identity);
                let range = unsafe { owned_view(&editor.context)?.GetRange()? };
                let (mut start, mut length) = (0, 0);
                unsafe {
                    range
                        .cast::<ITfRangeACP>()?
                        .GetExtent(&mut start, &mut length)?;
                }
                assert_eq!(
                    (start, length),
                    (run_start as i32, (expected.len() - 2 * run_start) as i32)
                );
                // The fixed-width diagnostic renderer cannot lay out a full
                // 1024-unit line. Check its exact TSF extent directly instead.
                if case != "maximum" {
                    assert_eq!(
                        live.scene()?.underlines,
                        vec![run_start..expected.len() - run_start]
                    );
                }
            }
            assert!(editor.rescue.metadata().is_none());
        }
        assert_eq!(input_notify_host::body_reads()?, reads);
        if case == "combining" {
            editor.input(WM_CHAR, WPARAM(8), LPARAM(1))?;
            assert_eq!(editor.input_disposition.get(), INPUT_APPLIED);
            assert_eq!(
                *store.text.borrow(),
                "prefix b suffix".encode_utf16().collect::<Vec<_>>()
            );
            assert_eq!(store.set_text_calls.get(), writes + 2);
            assert_eq!(store.undo.borrow().len(), 2);
            assert!(owned_view(&editor.context)?.cast::<IUnknown>()? == identity);
            pump(window)?;
            assert_eq!(live.scene()?.underlines, vec![7..8]);
            store.undo_once()?;
            assert_eq!(*store.text.borrow(), expected);
            store.undo_once()?;
            assert_eq!(*store.text.borrow(), original);
            store.redo_once()?;
            assert_eq!(*store.text.borrow(), expected);
        } else if case == "convert" {
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
            assert_eq!(*store.text.borrow(), original);
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
        assert_eq!(input_notify_host::body_reads()?, reads);
    }
    store.readonly.set(false);
    store.deny_lock.set(false);
    store.fail_selection.set(false);
    set_scope(editor, IS_TEXT)?;
    if let Some(foreign) = &mut foreign {
        foreign.end()?;
    }
    editor.clear_tracking();
    pump(window)?;
    assert!(no_composition(&editor.context)?);
    assert!(live.scene()?.underlines.is_empty());
    println!("Internal insertion / {case}: OK");
    Ok(())
}

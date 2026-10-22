//! Explicit selected Delete through the real DLL, on synthetic owned text.
use super::*;
use sumibi_core::recovery::{Operation, Outcome};
use sumibi_tip::spike::ISpikeSelectionEraser;

struct NoClipboard(Cell<usize>);
impl crate::recovery_view::Clipboard for NoClipboard {
    fn write(&self, _: HWND, _: &[u16], _: &dyn Fn() -> bool) -> Result<()> {
        self.0.set(self.0.get() + 1);
        Err(E_FAIL.into())
    }
}
pub(super) fn test(
    window: &crate::window_host::Window,
    editor: &Editor,
    live: &crate::live_render::Binding,
    client: u32,
    case: &str,
) -> Result<()> {
    let store = editor.store.get();
    let composing = editor.composing.as_ref().ok_or(E_FAIL)?;
    let eraser: ISpikeSelectionEraser = composing.control.cast()?;
    store.external_text("prefix a😀b suffix".encode_utf16().collect())?;
    store.host_selection(7, 11)?;
    if case == "partial" {
        store.external_text("ae\u{301}b".encode_utf16().collect())?;
        store.host_selection(2, 3)?;
    } else if case == "maximum" {
        store.external_text(vec!['x' as u16; 1024])?;
        store.host_selection(0, 1024)?;
    } else if matches!(case, "tracked" | "pending") {
        store.external_text("prefix  suffix".encode_utf16().collect())?;
        store.host_selection(7, 7)?;
        pump(window)?;
        editor.insert(&"a😀b".encode_utf16().collect::<Vec<_>>())?;
        assert_eq!(editor.input_disposition.get(), INPUT_APPLIED);
        if case == "tracked" {
            store.host_selection(7, 11)?;
        }
    } else if case == "empty" {
        store.host_selection(7, 7)?;
    }
    pump(window)?;
    let original = store.text.borrow().clone();
    let mut selection = store.selection.get();
    store.undo.borrow_mut().clear();
    store.redo.borrow_mut().clear();
    let writes = store.set_text_calls.get();
    let mut reads = input_notify_host::body_reads()?;
    assert_eq!(editor.delete_forward(0).unwrap_err().code(), E_INVALIDARG);
    assert_eq!(store.set_text_calls.get(), writes);
    assert_eq!(*store.text.borrow(), original);
    let mut foreign = None;
    match case {
        "pending" => {
            editor.input(WM_KEYDOWN, WPARAM(0x76), LPARAM(1))?;
            assert!(editor.broker.as_ref().ok_or(E_FAIL)?.1.get().active());
            selection = store.selection.get();
            reads = input_notify_host::body_reads()?;
        }
        "readonly" => store.readonly.set(true),
        "private" => set_scope(editor, IS_PRIVATE)?,
        "foreign" => {
            foreign = Some(crate::edit_host::composition_host::Composition::start(
                &editor.context,
                client,
                0,
                2,
            )?);
        }
        "lock-denied" => store.deny_lock.set(true),
        "owner-rejected" => store.reject_composition.set(true),
        "missing-sink" => unsafe {
            editor
                .context
                .cast::<ITfCompartmentMgr>()?
                .ClearCompartment(client, &sumibi_tip::spike::GUID_SPIKE_COMPOSITION_SINK)?;
        },
        "text-fail-before" => store.fail_text_before.set(true),
        "text-fail-after" => store.fail_text_after.set(true),
        "selection-fail" => store.fail_selection.set(true),
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
    if matches!(case, "empty" | "readonly" | "foreign") {
        let mut outcome = INPUT_UNCERTAIN;
        let hr = unsafe { eraser.DeleteSelection(editor.context.as_raw(), &mut outcome) };
        assert!(hr.is_err());
        assert_eq!(outcome, INPUT_NOT_ATTEMPTED);
        assert_eq!(*store.text.borrow(), original);
        assert_eq!(store.set_text_calls.get(), writes);
        assert!(store.undo.borrow().is_empty());
        // Null pointers fail before a write, without altering an existing ABI.
        assert!(
            unsafe { eraser.DeleteSelection(editor.context.as_raw(), std::ptr::null_mut()) }
                .is_err()
        );
        assert!(unsafe { eraser.DeleteSelection(std::ptr::null_mut(), &mut outcome) }.is_err());
        assert_eq!(outcome, INPUT_NOT_ATTEMPTED);
    } else {
        let ticket = if case == "budget" {
            Some(editor.rescue.reserve(Operation::Insert(1)).ok_or(E_FAIL)?)
        } else {
            None
        };
        let result = editor.delete_forward(if case == "repeat" { 1024 } else { 1 });
        if let Some(ticket) = ticket {
            editor.rescue.finish(ticket, INPUT_NOT_ATTEMPTED, &[]);
        }
        result?;
        let failed = matches!(
            case,
            "text-fail-before"
                | "text-fail-after"
                | "selection-fail"
                | "status-restored"
                | "scope-restored"
                | "compartment-restored"
        );
        let fallback = matches!(
            case,
            "private" | "lock-denied" | "owner-rejected" | "missing-sink" | "budget"
        );
        assert_eq!(store.set_text_calls.get(), writes + usize::from(!fallback));
        assert_eq!(
            store.undo.borrow().len(),
            usize::from(case != "text-fail-before")
        );
        let expected: Vec<u16> = if case == "text-fail-before" {
            original.clone()
        } else {
            match case {
                "partial" => "aeb",
                "maximum" => "",
                _ => "prefix  suffix",
            }
            .encode_utf16()
            .collect()
        };
        assert_eq!(
            *store.text.borrow(),
            expected,
            "{case}: no repeated deletion"
        );
        if case == "pending" {
            let (broker, bridge) = editor.broker.as_ref().ok_or(E_FAIL)?;
            tracking_host::wait_result(window, broker, bridge, false)?;
            assert_eq!(*store.text.borrow(), expected);
            assert_eq!(store.set_text_calls.get(), writes + 1);
            assert_eq!(store.undo.borrow().len(), 1);
        }
        if failed {
            assert_eq!(
                editor.input_disposition.get(),
                if case.starts_with("text-fail") {
                    INPUT_UNCERTAIN
                } else {
                    INPUT_APPLIED_UNTRACKED
                }
            );
            assert!(store.input_problem.get());
            let metadata = editor.rescue.metadata().ok_or(E_FAIL)?;
            assert_eq!(metadata.operation, Operation::DeleteSelection);
            assert_eq!(
                metadata.outcome,
                if case.starts_with("text-fail") {
                    Outcome::Uncertain
                } else {
                    Outcome::AppliedUntracked
                }
            );
            assert!(!metadata.copyable);
            let mut outcome = INPUT_UNCERTAIN;
            assert!(
                unsafe { eraser.DeleteSelection(editor.context.as_raw(), &mut outcome) }.is_err()
            );
            assert_eq!(outcome, INPUT_NOT_ATTEMPTED);
            assert_eq!(store.set_text_calls.get(), writes + 1);
            let clipboard = NoClipboard(Cell::new(0));
            for key in [0x77, 0x79] {
                editor.rescue.request(key);
                editor.service_recovery(window, &clipboard)?;
            }
            assert_eq!(
                clipboard.0.get(),
                0,
                "deleted text is never captured or copied"
            );
            assert_eq!(*store.text.borrow(), expected);
        } else {
            assert_eq!(
                editor.input_disposition.get(),
                if fallback {
                    INPUT_NOT_ATTEMPTED
                } else {
                    INPUT_APPLIED
                }
            );
            assert!(editor.rescue.metadata().is_none());
            let current = store.selection.get();
            assert_eq!(
                (current.acpStart, current.acpEnd),
                (selection.acpStart, selection.acpStart)
            );
            if !fallback {
                assert_eq!(
                    store.last_text_lock.get(),
                    store.last_composition_lock.get(),
                    "body and composition use one write cookie"
                );
            }
            store.deny_lock.set(false);
            store.reject_composition.set(false);
            set_scope(editor, IS_TEXT)?;
            if case == "missing-sink" {
                scope_fixture::initialise_context(&editor.context, client)?;
            }
            assert!(
                unsafe { composing.control.PrepareConversion() }.is_err(),
                "no tracked source survives deletion"
            );
        }
        if case != "text-fail-before" {
            store.fail_selection.set(false);
            store.undo_once()?;
            assert_eq!(
                *store.text.borrow(),
                original,
                "one Undo restores selection deletion"
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
    store.readonly.set(false);
    store.deny_lock.set(false);
    store.fail_selection.set(false);
    if case == "tracked" {
        editor.insert(&['z' as u16])?;
        assert_eq!(editor.input_disposition.get(), INPUT_APPLIED);
        assert_eq!(
            *store.text.borrow(),
            "prefix z suffix".encode_utf16().collect::<Vec<_>>()
        );
        pump(window)?;
        assert_eq!(live.scene()?.underlines, vec![7..8]);
    }
    editor.clear_tracking();
    if let Some(foreign) = &mut foreign {
        foreign.end()?;
    }
    pump(window)?;
    assert!(no_composition(&editor.context)?);
    assert!(live.scene()?.underlines.is_empty());
    assert_eq!(
        input_notify_host::body_reads()?,
        reads,
        "selected deletion never obtains document body in DLL"
    );
    println!("Selection Delete transaction / {case}: OK");
    Ok(())
}

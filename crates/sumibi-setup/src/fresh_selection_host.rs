//! Select pre-existing synthetic text without fabricating a preceding input.
use super::*;
use sumibi_tip::spike::ISpikeInputPreparer;

pub(super) fn test(
    window: &crate::window_host::Window,
    editor: &Editor,
    live: &crate::live_render::Binding,
    client: u32,
    case: &str,
) -> Result<()> {
    let store = editor.store.get();
    if case == "maximum" {
        store.external_text(vec!['x' as u16; 1024])?;
        store.host_selection(0, 1024)?;
    } else {
        store.external_text("prefix old suffix".encode_utf16().collect())?;
        if case == "backward" {
            store.select_caret(10, 7)?;
        } else {
            store.host_selection(7, if case == "collapsed" { 7 } else { 10 })?;
        }
    }
    pump(window)?;
    assert!(no_composition(&editor.context)?);
    assert!(live.scene()?.underlines.is_empty());
    let original = store.text.borrow().clone();
    let selected = store.selection.get();
    let reads = input_notify_host::body_reads()?;
    let writes = store.set_text_calls.get();
    let composing = editor.composing.as_ref().ok_or(E_FAIL)?;
    let preparer: ISpikeInputPreparer = composing.control.cast()?;
    let (broker, bridge) = editor.broker.as_ref().ok_or(E_FAIL)?;
    let mut foreign = None;
    match case {
        "missing-sink" => unsafe {
            editor
                .context
                .cast::<ITfCompartmentMgr>()?
                .ClearCompartment(client, &sumibi_tip::spike::GUID_SPIKE_COMPOSITION_SINK)?;
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
            )?);
        }
        _ => {}
    }
    if case == "null-context" {
        assert!(unsafe { preparer.Prepare(std::ptr::null_mut()) }.is_err());
        // The old context-free ABI remains callable and does not invent an owner.
        assert!(unsafe { composing.control.PrepareConversion() }.is_err());
    } else if case.starts_with("gate-") || case == "one-shot" {
        assert!(unsafe { preparer.Prepare(editor.context.as_raw()) }.is_ok());
        assert_eq!(
            input_notify_host::body_reads()?,
            reads,
            "preparation is metadata only"
        );
        match case {
            "gate-selection" => {
                store.host_selection(8, 10)?;
                store.host_selection(7, 10)?;
            }
            "gate-protection" => {
                set_scope(editor, IS_PRIVATE)?;
                set_scope(editor, IS_TEXT)?;
            }
            "gate-body-restored" => {
                store.external_text("prefix new suffix".encode_utf16().collect())?;
                store.external_text(original.clone())?;
            }
            _ => {}
        }
        let units: Vec<u16> = "old".encode_utf16().collect();
        let checked = unsafe {
            composing
                .control
                .CheckSource(units.as_ptr(), units.len() as u32)
        };
        assert_eq!(checked.is_ok(), case == "one-shot");
        assert!(
            unsafe {
                composing
                    .control
                    .CheckSource(units.as_ptr(), units.len() as u32)
            }
            .is_err(),
            "a consumed or rejected source cannot be retried"
        );
    } else {
        editor.input(WM_KEYDOWN, WPARAM(0x76), LPARAM(1))?;
        if matches!(
            case,
            "collapsed" | "private" | "readonly" | "lock-denied" | "foreign"
        ) {
            assert!(!bridge.get().active(), "{case}: no source submitted");
        } else {
            assert!(
                bridge.get().active(),
                "fresh selection starts without a preceding character input"
            );
            let captured = input_notify_host::body_reads()?;
            assert!(captured > reads);
            let current = store.selection.get();
            assert_eq!(
                (current.acpStart, current.acpEnd, current.style.ase),
                (selected.acpStart, selected.acpEnd, selected.style.ase),
                "preparation preserves explicit selection direction"
            );
            let applied = !matches!(case, "cancel" | "pending-edit" | "pending-protection");
            match case {
                "cancel" => {
                    editor.input(WM_KEYDOWN, WPARAM(0x1b), LPARAM(1))?;
                }
                "pending-edit" => store.type_text(&['z' as u16])?,
                "pending-protection" => {
                    set_scope(editor, IS_PRIVATE)?;
                    set_scope(editor, IS_TEXT)?;
                }
                _ => {}
            }
            tracking_host::wait_result(window, broker, bridge, applied)?;
            if applied {
                let expected: Vec<u16> = if case == "maximum" {
                    "日本語"
                } else {
                    "prefix 日本語 suffix"
                }
                .encode_utf16()
                .collect();
                assert_eq!(*store.text.borrow(), expected);
                assert_eq!(store.set_text_calls.get(), writes + 1);
                assert_eq!(store.undo.borrow().len(), 1);
                store.undo_once()?;
                assert_eq!(*store.text.borrow(), original);
                let restored = store.selection.get();
                assert_eq!(
                    (restored.acpStart, restored.acpEnd, restored.style.ase),
                    (selected.acpStart, selected.acpEnd, selected.style.ase)
                );
                store.redo_once()?;
                assert_eq!(*store.text.borrow(), expected);
            } else {
                assert_eq!(store.set_text_calls.get(), writes);
                assert_eq!(
                    input_notify_host::body_reads()?,
                    captured,
                    "cancelled/stale delivery cannot read document body again"
                );
                if case == "pending-edit" {
                    assert_eq!(
                        *store.text.borrow(),
                        "prefix z suffix".encode_utf16().collect::<Vec<_>>()
                    );
                    assert_eq!(store.undo.borrow().len(), 1);
                } else {
                    assert_eq!(*store.text.borrow(), original);
                    assert!(store.undo.borrow().is_empty());
                }
            }
        }
    }
    if case == "null-context"
        || case.starts_with("gate-")
        || case == "one-shot"
        || matches!(
            case,
            "collapsed" | "private" | "readonly" | "lock-denied" | "foreign"
        )
    {
        assert!(!bridge.get().active());
        assert_eq!(input_notify_host::body_reads()?, reads);
        assert_eq!(store.set_text_calls.get(), writes);
        assert_eq!(*store.text.borrow(), original);
        assert!(store.undo.borrow().is_empty());
    }
    store.readonly.set(false);
    store.deny_lock.set(false);
    if case == "missing-sink" {
        scope_fixture::initialise_context(&editor.context, client)?;
    }
    set_scope(editor, IS_TEXT)?;
    if let Some(foreign) = &mut foreign {
        foreign.end()?;
    }
    editor.clear_tracking();
    pump(window)?;
    assert!(no_composition(&editor.context)?);
    assert!(live.scene()?.underlines.is_empty());
    println!("Fresh selection / {case}: OK");
    Ok(())
}

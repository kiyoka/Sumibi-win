//! Same host operations used by the editor, on a dedicated native TSF fixture.
//! This does not substitute for the foreground-dependent key routing tests.
use super::*;
use sumibi_tip::spike::INPUT_APPLIED;

pub(crate) fn exercise_graphemes(
    manager: &ITfThreadMgr,
    client: u32,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    for case in [
        "grapheme/arrows",
        "grapheme/shift",
        "grapheme/collapse-repeat",
        "grapheme/delete-repeat",
        "grapheme/backspace-interior",
        "grapheme/delete-interior",
        "grapheme/selection",
        "grapheme/ends",
        "grapheme/readonly",
        "grapheme/locked",
        "grapheme/pending",
        "grapheme/interior-navigation",
    ] {
        run(manager, client, RunMode::TransactionTest(case))?;
    }
    Ok(())
}

pub(super) fn test_graphemes(
    window: &crate::window_host::Window,
    editor: &Editor,
    live: &crate::live_render::Binding,
    case: &str,
) -> Result<()> {
    const TEXT: &str = "ae\u{301}👨‍👩‍👧‍👦🇯🇵b";
    let store = editor.store.get();
    store.external_text(Vec::new())?;
    store.host_selection(0, 0)?;
    pump(window)?;
    editor.insert(&TEXT.encode_utf16().collect::<Vec<_>>())?;
    assert_eq!(editor.input_disposition.get(), INPUT_APPLIED);
    pump(window)?;
    let original = store.text.borrow().clone();
    assert_eq!(original.len(), 19); // grapheme edges: 0,1,3,14,18,19
    store.undo.borrow_mut().clear();
    store.redo.borrow_mut().clear();
    let writes = store.set_text_calls.get();
    let mut reads = composing_host::input_notify_host::body_reads()?;
    let mut edited = false;
    match case {
        "arrows" => {
            store.host_selection(0, 0)?;
            for next in [1, 3, 14, 18, 19] {
                store.navigate(0x27, false, 1)?;
                caret(store, next, next, false);
            }
            for next in [18, 14, 3, 1, 0] {
                store.navigate(0x25, false, 1)?;
                caret(store, next, next, false);
            }
        }
        "shift" => {
            store.host_selection(3, 3)?;
            store.navigate(0x27, true, 2)?;
            caret(store, 3, 18, false);
            store.navigate(0x25, true, 3)?;
            caret(store, 1, 3, true);
            store.navigate(0x27, true, 1)?;
            caret(store, 3, 3, false);
        }
        "collapse-repeat" => {
            store.host_selection(1, 18)?;
            store.navigate(0x25, false, 1)?;
            caret(store, 1, 1, false);
            store.host_selection(1, 18)?;
            store.navigate(0x27, false, 2)?;
            caret(store, 19, 19, false);
            store.host_selection(1, 18)?;
            store.navigate(0x25, false, 2)?;
            caret(store, 0, 0, false);
        }
        "delete-repeat" => {
            store.host_selection(1, 1)?;
            store.delete_forward(2)?;
            body(store, "a🇯🇵b");
            caret(store, 1, 1, false);
            edited = true;
        }
        "backspace-interior" => {
            store.host_selection(5, 5)?; // after first scalar within ZWJ family
            store.backspace()?;
            body(store, "ae\u{301}🇯🇵b");
            caret(store, 3, 3, false);
            edited = true;
        }
        "delete-interior" => {
            store.host_selection(2, 2)?; // between base and combining accent
            store.delete_forward(1)?;
            body(store, "a👨‍👩‍👧‍👦🇯🇵b");
            caret(store, 1, 1, false);
            edited = true;
        }
        "selection" => {
            store.host_selection(2, 3)?; // explicit selection of the accent only
            store.delete_forward(1024)?;
            body(store, "ae👨‍👩‍👧‍👦🇯🇵b");
            caret(store, 2, 2, false);
            edited = true;
        }
        "ends" => {
            store.host_selection(0, 0)?;
            store.backspace()?;
            store.navigate(0x27, false, 1024)?;
            caret(store, 19, 19, false);
            store.delete_forward(1024)?;
            store.navigate(0x25, false, 1024)?;
            caret(store, 0, 0, false);
            store.navigate(0x23, true, 1)?;
            caret(store, 0, 19, false);
            store.navigate(0x24, false, 1)?;
            caret(store, 0, 0, false);
        }
        "readonly" => {
            store.host_selection(1, 1)?;
            store.readonly.set(true);
            let deleted = store.delete_forward(2);
            let backspaced = store.backspace();
            store.readonly.set(false);
            assert_eq!(deleted.unwrap_err().code(), E_ACCESSDENIED);
            assert_eq!(backspaced.unwrap_err().code(), E_ACCESSDENIED);
        }
        "locked" => {
            store.host_selection(1, 1)?;
            let previous = store.lock.replace(TS_LF_READ.0);
            let deleted = store.delete_forward(2);
            let moved = store.navigate(0x27, false, 1);
            store.lock.set(previous);
            assert_eq!(deleted.unwrap_err().code(), TS_E_NOLOCK);
            assert_eq!(moved.unwrap_err().code(), TS_E_NOLOCK);
            caret(store, 1, 1, false);
        }
        "pending" => {
            editor.input(WM_KEYDOWN, WPARAM(0x76), LPARAM(1))?;
            let (broker, bridge) = editor.broker.as_ref().ok_or(E_FAIL)?;
            assert!(bridge.get().active());
            reads = composing_host::input_notify_host::body_reads()?;
            // Use the host operation and its real TSF notifications, without
            // an explicit Cancel or a direct DLL notification callback.
            // F7 selected the conversion run; first collapse it to its end.
            store.navigate(0x27, false, 1)?;
            store.navigate(0x25, false, 1)?;
            store.delete_forward(1)?;
            body(store, "ae\u{301}👨‍👩‍👧‍👦🇯🇵");
            tracking_host::wait_result(window, broker, bridge, false)?;
            body(store, "ae\u{301}👨‍👩‍👧‍👦🇯🇵");
            edited = true;
        }
        "interior-navigation" => {
            for (inside, left, right) in [(2, 1, 3), (5, 3, 14)] {
                store.host_selection(inside, inside)?;
                store.navigate(0x25, false, 1)?;
                caret(store, left, left, false);
                store.host_selection(inside, inside)?;
                store.navigate(0x27, false, 1)?;
                caret(store, right, right, false);
            }
            store.host_selection(2, 5)?;
            store.navigate(0x25, false, 1)?;
            caret(store, 1, 1, false);
        }
        _ => return Err(E_INVALIDARG.into()),
    }
    assert_eq!(
        store.set_text_calls.get(),
        writes,
        "host editing does not enter DLL SetText"
    );
    assert_eq!(store.undo.borrow().len(), usize::from(edited));
    if edited {
        let deleted = store.text.borrow().clone();
        store.undo_once()?;
        assert_eq!(
            *store.text.borrow(),
            original,
            "one Undo restores the entire host operation"
        );
        store.redo_once()?;
        assert_eq!(*store.text.borrow(), deleted);
    } else {
        assert_eq!(*store.text.borrow(), original);
    }
    for _ in 0..4 {
        pump(window)?;
        if !editor.composing.as_ref().ok_or(E_FAIL)?.poll(editor)? {
            break;
        }
    }
    assert!(composing_host::no_composition(&editor.context)?);
    assert!(live.scene()?.underlines.is_empty());
    assert_eq!(
        composing_host::input_notify_host::body_reads()?,
        reads,
        "navigation/deletion and stale result retirement cannot read body in DLL"
    );
    println!("Grapheme host / {case}: OK");
    Ok(())
}
fn caret(store: &TextStore, start: i32, end: i32, backwards: bool) {
    let selection = store.selection.get();
    assert_eq!((selection.acpStart, selection.acpEnd), (start, end));
    assert_eq!(selection.style.ase == TS_AE_START, backwards);
}
fn body(store: &TextStore, text: &str) {
    assert_eq!(
        *store.text.borrow(),
        text.encode_utf16().collect::<Vec<_>>()
    );
}

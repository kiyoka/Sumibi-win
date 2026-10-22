//! Real DLL insertion; never edit user documents or replay attempted writes.
use super::*;
use sumibi_tip::spike::{INPUT_APPLIED, INPUT_APPLIED_UNTRACKED};
#[path = "input_selection_delete_host.rs"]
mod selection_delete;
#[path = "tracked_cursor_host.rs"]
mod tracked_cursor;

pub(crate) fn run_transaction_demo(
    manager: &ITfThreadMgr,
    client: u32,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    run(manager, client, RunMode::TransactionDemo)
}
pub(crate) fn exercise_transactions(
    manager: &ITfThreadMgr,
    client: u32,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    for case in [
        "grow",
        "replace",
        "repeat",
        "backspace",
        "undo",
        "apply",
        "lock-denied",
        "private",
        "owner-rejected",
        "missing-sink",
        "text-fail-before",
        "text-fail-after",
        "selection-fail",
        "status-restored",
        "scope-restored",
        "compartment-restored",
        "foreign",
        "external",
        "forced",
        "close",
        "invalid",
        "capacity",
        "delete/selection",
        "delete/repeat",
        "delete/partial",
        "delete/maximum",
        "delete/tracked",
        "delete/pending",
        "delete/empty",
        "delete/readonly",
        "delete/private",
        "delete/foreign",
        "delete/lock-denied",
        "delete/owner-rejected",
        "delete/missing-sink",
        "delete/budget",
        "delete/text-fail-before",
        "delete/text-fail-after",
        "delete/selection-fail",
        "delete/status-restored",
        "delete/scope-restored",
        "delete/compartment-restored",
        "cursor/delete",
        "cursor/repeat",
        "cursor/move",
        "cursor/all",
        "cursor/convert",
        "cursor/convert-after-delete",
        "cursor/overshoot",
        "cursor/delete-overshoot",
        "cursor/nav-budget",
        "cursor/delete-budget",
        "cursor/nav-fail",
        "cursor/nav-fail-after",
        "cursor/tail-append",
        "cursor/text-fail-before",
        "cursor/text-fail-after",
        "cursor/selection-fail",
        "cursor/status-restored",
        "cursor/scope-restored",
        "cursor/compartment-restored",
        "cursor/private",
        "cursor/readonly",
        "cursor/foreign",
        "cursor/invalid",
        "cursor/shift",
        "cursor/interior-typing",
        "cursor/rejoin",
    ] {
        run(manager, client, RunMode::TransactionTest(case))?;
    }
    Ok(())
}
pub(in crate::edit_host::input_host) fn test_transactions(
    window: &crate::window_host::Window,
    editor: &Editor,
    live: &crate::live_render::Binding,
    client: u32,
    case: &str,
) -> Result<()> {
    if let Some(case) = case.strip_prefix("cursor/") {
        return tracked_cursor::test(window, editor, live, client, case);
    }
    if let Some(case) = case.strip_prefix("delete/") {
        return selection_delete::test(window, editor, live, client, case);
    }
    let store = editor.store.get();
    let composing = editor.composing.as_ref().ok_or(E_FAIL)?;
    let writer: ISpikeInputWriter = composing.control.cast()?;
    store.external_text("prefix  suffix".encode_utf16().collect())?;
    store.host_selection(7, 7)?;
    pump(window)?;
    let reads = super::input_notify_host::body_reads()?;
    let mut foreign = None;
    match case {
        "lock-denied" => store.deny_lock.set(true),
        "private" => set_scope(editor, IS_PRIVATE)?,
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
        "replace" => store.host_selection(0, 6)?,
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
    if case == "invalid" {
        for units in [
            vec![0xd800],
            vec![0xdc00],
            vec![0],
            vec![0xfffc],
            vec![b'a' as u16; 1025],
            vec![],
        ] {
            let mut outcome = INPUT_UNCERTAIN;
            let hr = unsafe {
                writer.Insert(
                    editor.context.as_raw(),
                    units.as_ptr(),
                    units.len() as u32,
                    &mut outcome,
                )
            };
            assert!(hr.is_err());
            assert_eq!(outcome, INPUT_NOT_ATTEMPTED);
        }
        assert_eq!(store.set_text_calls.get(), 0);
        assert_eq!(
            *store.text.borrow(),
            "prefix  suffix".encode_utf16().collect::<Vec<_>>()
        );
    } else if case == "capacity" {
        store.external_text(Vec::new())?;
        store.host_selection(0, 0)?;
        pump(window)?;
        let mut outcome = INPUT_UNCERTAIN;
        let text = vec![b'x' as u16; 1024];
        let status = unsafe {
            writer.Insert(
                editor.context.as_raw(),
                text.as_ptr(),
                text.len() as u32,
                &mut outcome,
            )
        };
        assert_eq!(
            status,
            windows::Win32::Foundation::S_OK,
            "capacity: outcome {outcome}"
        );
        assert_eq!(outcome, INPUT_APPLIED);
        let before = store.set_text_calls.get();
        let hr = unsafe { writer.Insert(editor.context.as_raw(), text.as_ptr(), 1, &mut outcome) };
        assert!(hr.is_err());
        assert_eq!(outcome, INPUT_NOT_ATTEMPTED);
        assert_eq!(store.set_text_calls.get(), before);
    } else {
        let text = "a😀b".encode_utf16().collect::<Vec<_>>();
        editor.insert(&text)?;
        if matches!(
            case,
            "text-fail-before"
                | "text-fail-after"
                | "selection-fail"
                | "status-restored"
                | "scope-restored"
                | "compartment-restored"
        ) {
            assert_eq!(
                store.set_text_calls.get(),
                1,
                "{case}: never replay attempted SetText"
            );
            assert!(store.input_problem.get());
            assert_eq!(
                editor.input_disposition.get(),
                if case.starts_with("text-fail") {
                    INPUT_UNCERTAIN
                } else {
                    INPUT_APPLIED_UNTRACKED
                }
            );
            assert_eq!(
                store.undo.borrow().len(),
                usize::from(case != "text-fail-before")
            );
            assert_eq!(
                *store.text.borrow(),
                if case == "text-fail-before" {
                    "prefix  suffix"
                } else {
                    "prefix a😀b suffix"
                }
                .encode_utf16()
                .collect::<Vec<_>>()
            );
            unsafe {
                assert!(composing.control.PrepareConversion().is_err());
            }
            editor.input(WM_KEYDOWN, WPARAM(0x76), LPARAM(1))?;
            assert!(!editor.broker.as_ref().ok_or(E_FAIL)?.1.get().active());
            // A second real input is ordinary host input once; it never blesses
            // the uncertain edit as a tracked conversion source.
            editor.insert(&['z' as u16])?;
            assert_eq!(store.set_text_calls.get(), 1);
            editor.input(WM_KEYDOWN, WPARAM(0x1b), LPARAM(1))?;
            assert!(!store.input_problem.get());
            editor.input(WM_KEYDOWN, WPARAM(0x78), LPARAM(1))?;
            editor.service_recovery(window, &crate::recovery_view::SystemClipboard)?;
            editor.insert(&['q' as u16])?;
            assert_eq!(editor.input_disposition.get(), INPUT_APPLIED);
        } else if matches!(
            case,
            "lock-denied" | "private" | "owner-rejected" | "missing-sink" | "foreign"
        ) {
            assert_eq!(editor.input_disposition.get(), INPUT_NOT_ATTEMPTED);
            assert_eq!(store.set_text_calls.get(), 0);
            assert_eq!(store.undo.borrow().len(), 1, "{case}: original input once");
            assert_eq!(
                *store.text.borrow(),
                "prefix a😀b suffix".encode_utf16().collect::<Vec<_>>()
            );
            store.deny_lock.set(false);
            store.reject_composition.set(false);
            set_scope(editor, IS_TEXT)?;
            if case == "missing-sink" {
                scope_fixture::initialise_context(&editor.context, client)?;
            }
            editor.clear_tracking();
        } else {
            assert_eq!(editor.input_disposition.get(), INPUT_APPLIED, "{case}");
            assert_eq!(store.set_text_calls.get(), 1);
            assert_eq!(store.undo.borrow().len(), 1);
            assert_eq!(
                store.last_text_lock.get(),
                store.last_composition_lock.get(),
                "text and composition use the same write lock"
            );
            let expected_range = if case == "replace" { 0..4 } else { 7..11 };
            pump(window)?;
            assert_eq!(live.scene()?.underlines, vec![expected_range]);
            let identity = owned_view(&editor.context)?.cast::<IUnknown>()?;
            match case {
                "grow" | "repeat" => {
                    editor.input(
                        WM_CHAR,
                        WPARAM('x' as usize),
                        LPARAM(if case == "repeat" { 3 } else { 1 }),
                    )?;
                    assert_eq!(editor.input_disposition.get(), INPUT_APPLIED);
                    assert!(owned_view(&editor.context)?.cast::<IUnknown>()? == identity);
                    pump(window)?;
                    assert_eq!(
                        live.scene()?.underlines,
                        vec![7..if case == "repeat" { 14 } else { 12 }]
                    );
                    assert_eq!(store.undo.borrow().len(), 2);
                }
                "backspace" => {
                    editor.input(WM_CHAR, WPARAM(0x08), LPARAM(1))?;
                    pump(window)?;
                    assert!(owned_view(&editor.context)?.cast::<IUnknown>()? == identity);
                    assert_eq!(live.scene()?.underlines, vec![7..10]);
                }
                "undo" => {
                    editor.input(WM_CHAR, WPARAM(0x1a), LPARAM(1))?;
                    assert_eq!(
                        *store.text.borrow(),
                        "prefix  suffix".encode_utf16().collect::<Vec<_>>()
                    );
                }
                "apply" => {
                    let original = store.text.borrow().clone();
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
                    assert_eq!(*store.text.borrow(), original);
                }
                "external" => {
                    store.host_selection(0, 0)?;
                    editor.insert(&['z' as u16])?;
                    assert_eq!(editor.input_disposition.get(), INPUT_APPLIED);
                    pump(window)?;
                    assert_eq!(live.scene()?.underlines, vec![0..1]);
                    assert!(owned_view(&editor.context)?.cast::<IUnknown>()? != identity);
                }
                "forced" => {
                    unsafe {
                        editor
                            .context
                            .cast::<ITfContextOwnerCompositionServices>()?
                            .TerminateComposition(&owned_view(&editor.context)?)?;
                    }
                    editor.insert(&['z' as u16])?;
                    assert_eq!(editor.input_disposition.get(), INPUT_APPLIED);
                    assert!(owned_view(&editor.context)?.cast::<IUnknown>()? != identity);
                    pump(window)?;
                    assert_eq!(live.scene()?.underlines, vec![11..12]);
                }
                "close" => {
                    test_broker_sent_close(window, editor)?;
                }
                _ => {}
            }
        }
    }
    if case != "apply" {
        assert_eq!(
            super::input_notify_host::body_reads()?,
            reads,
            "{case}: no DLL body capture for insertion"
        );
    }
    store.deny_lock.set(false);
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
    println!("Input transaction / {case}: OK");
    Ok(())
}

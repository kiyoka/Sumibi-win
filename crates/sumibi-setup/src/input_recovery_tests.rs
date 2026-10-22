//! Native rescue tests use an injected clipboard, never the user's clipboard.
use super::*;
use crate::recovery_view::Clipboard;
use sumibi_core::recovery::RETENTION;
use windows::Win32::Foundation::{HWND, WAIT_FAILED};

struct TestClipboard<'a> {
    calls: Cell<usize>,
    text: RefCell<Vec<u16>>,
    fail: Cell<bool>,
    before: RefCell<Option<Box<dyn Fn() + 'a>>>,
}
impl TestClipboard<'_> {
    fn new() -> Self {
        Self {
            calls: Cell::new(0),
            text: RefCell::new(Vec::new()),
            fail: Cell::new(false),
            before: RefCell::new(None),
        }
    }
}
impl Clipboard for TestClipboard<'_> {
    fn write(&self, _: HWND, text: &[u16], permitted: &dyn Fn() -> bool) -> Result<()> {
        self.calls.set(self.calls.get() + 1);
        let callback = self.before.borrow_mut().take();
        if let Some(callback) = callback {
            callback();
        }
        if self.fail.get() || !permitted() {
            return Err(E_FAIL.into());
        }
        *self.text.borrow_mut() = text.to_vec();
        Ok(())
    }
}
pub(crate) fn exercise_recovery(
    manager: &ITfThreadMgr,
    client: u32,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    for case in [
        "rescue/success",
        "rescue/private",
        "rescue/uncertain-before",
        "rescue/uncertain-after",
        "rescue/applied-untracked",
        "rescue/backspace",
        "rescue/capacity",
        "rescue/escape",
        "rescue/copy",
        "rescue/copy-failed",
        "rescue/discard",
        "rescue/hide",
        "rescue/expired-copy",
        "rescue/expire-wake",
        "rescue/close",
        "rescue/focus",
        "rescue/maximum",
        "rescue/late-ticket",
        "rescue/no-preview-copy",
        "rescue/keyboard-copy",
        "rescue/protected-failure",
        "rescue/protection",
        "rescue/protection-restored",
        "rescue/late-command",
        "rescue/copy-invalidated",
    ] {
        run(manager, client, RunMode::TransactionTest(case))?;
    }
    Ok(())
}
pub(crate) fn run_recovery_demo(
    manager: &ITfThreadMgr,
    client: u32,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    run(manager, client, RunMode::TransactionTest("rescue/demo"))
}
pub(in crate::edit_host::input_host) fn test_recovery(
    window: &crate::window_host::Window,
    editor: &Editor,
    _live: &crate::live_render::Binding,
    _client: u32,
    case: &str,
) -> Result<()> {
    let clipboard = TestClipboard::new();
    let store = editor.store.get();
    let reads = super::super::composing_host::input_notify_host::body_reads()?;
    let text: Vec<_> = if case == "maximum" {
        vec!['x' as u16; 1024]
    } else {
        "rescue😀".encode_utf16().collect()
    };
    match case {
        "success" => {}
        "private" => {
            *store.scopes.borrow_mut() = ScopeSpec::List(vec![IS_PRIVATE]);
        }
        "uncertain-after" => store.fail_text_after.set(true),
        "protected-failure" => {
            store.scope_cycle_on_write.set(true);
            store.fail_text_after.set(true);
        }
        "applied-untracked" => store.fail_selection.set(true),
        "backspace" => {
            editor.insert(&['a' as u16])?;
            store.fail_text_before.set(true);
            editor.input(WM_CHAR, WPARAM(8), LPARAM(1))?;
        }
        _ => store.fail_text_before.set(true),
    }
    if case != "backspace" {
        editor.insert(&text)?;
    }
    let after = store.text.borrow().clone();
    let writes = store.set_text_calls.get();
    let history = store.undo.borrow().len();
    if case == "demo" {
        println!("Recovery demo: synthetic failed input is retained for up to 10 minutes. F8 inspect, F10 explicit copy while open, F9 discard. No automatic insertion.");
        return broker_message_loop(window, editor);
    }
    if matches!(case, "success" | "private") {
        assert!(editor.rescue.metadata().is_none());
        editor.input(WM_KEYDOWN, WPARAM(0x77), LPARAM(1))?;
        editor.service_recovery(window, &clipboard)?;
        assert!(editor.rescue.view.borrow().is_none());
    } else {
        let original = editor.rescue.metadata().ok_or(E_FAIL)?;
        assert_eq!(
            original.outcome,
            if case == "applied-untracked" {
                Outcome::AppliedUntracked
            } else {
                Outcome::Uncertain
            }
        );
        if matches!(case, "backspace" | "protected-failure") {
            assert!(!original.copyable);
        } else {
            assert_eq!(
                editor
                    .rescue
                    .slot
                    .borrow()
                    .text(original.ticket, editor.rescue.now()),
                Some(text.as_slice())
            );
        }
        assert!(
            editor.rescue.view.borrow().is_none(),
            "no automatic disclosure UI"
        );
        if case == "capacity" || case == "escape" {
            if case == "escape" {
                editor.input(WM_KEYDOWN, WPARAM(0x1b), LPARAM(1))?;
            }
            editor.insert(&['z' as u16])?;
            assert_eq!(
                store.set_text_calls.get(),
                writes,
                "full rescue capacity routes new input through host once"
            );
            assert_eq!(store.text.borrow().as_slice(), &['z' as u16]);
            assert_eq!(store.undo.borrow().len(), history + 1);
            assert_eq!(
                editor.rescue.metadata().ok_or(E_FAIL)?.ticket,
                original.ticket
            );
            assert_eq!(
                editor
                    .rescue
                    .slot
                    .borrow()
                    .text(original.ticket, editor.rescue.now()),
                Some(text.as_slice())
            );
        } else if case == "focus" {
            editor.input(WM_KILLFOCUS, WPARAM(0), LPARAM(0))?;
            assert!(!editor.rescue.metadata().ok_or(E_FAIL)?.copyable);
            assert_eq!(
                editor.rescue.metadata().ok_or(E_FAIL)?.ticket,
                original.ticket
            );
        } else if case == "no-preview-copy" {
            editor.input(WM_KEYDOWN, WPARAM(0x79), LPARAM(1))?;
            editor.service_recovery(window, &clipboard)?;
            assert_eq!(clipboard.calls.get(), 0);
            assert!(editor.rescue.metadata().is_some());
        } else {
            editor.input(WM_KEYDOWN, WPARAM(0x77), LPARAM(1))?;
            editor.service_recovery(window, &clipboard)?;
            assert!(editor.rescue.view.borrow().is_some());
            assert_eq!(clipboard.calls.get(), 0);
            match case {
                "copy" | "copy-failed" | "maximum" | "keyboard-copy" => {
                    clipboard.fail.set(case == "copy-failed");
                    if case == "keyboard-copy" {
                        editor.input(WM_KEYDOWN, WPARAM(0x79), LPARAM(1))?;
                    } else {
                        editor
                            .rescue
                            .view
                            .borrow()
                            .as_ref()
                            .ok_or(E_FAIL)?
                            .click_copy();
                    }
                    editor.service_recovery(window, &clipboard)?;
                    if case == "copy-failed" {
                        assert_eq!(
                            editor.rescue.metadata().ok_or(E_FAIL)?.ticket,
                            original.ticket
                        );
                        assert!(
                            editor.rescue.metadata().ok_or(E_FAIL)?.remaining <= original.remaining
                        );
                        clipboard.fail.set(false);
                        editor
                            .rescue
                            .view
                            .borrow()
                            .as_ref()
                            .ok_or(E_FAIL)?
                            .click_copy();
                        editor.service_recovery(window, &clipboard)?;
                    }
                    assert_eq!(
                        clipboard.calls.get(),
                        if case == "copy-failed" { 2 } else { 1 },
                        "copy action: current metadata present {}, copyable {}",
                        editor.rescue.metadata().is_some(),
                        editor.rescue.metadata().is_some_and(|m| m.copyable)
                    );
                    assert_eq!(*clipboard.text.borrow(), text);
                    assert!(editor.rescue.metadata().is_none());
                    assert!(editor.rescue.view.borrow().is_none());
                }
                "backspace" => {
                    editor.input(WM_KEYDOWN, WPARAM(0x79), LPARAM(1))?;
                    editor.service_recovery(window, &clipboard)?;
                    assert_eq!(clipboard.calls.get(), 0);
                    assert!(editor.rescue.metadata().is_some());
                }
                "discard" => {
                    editor
                        .rescue
                        .view
                        .borrow()
                        .as_ref()
                        .ok_or(E_FAIL)?
                        .click_discard();
                    editor.service_recovery(window, &clipboard)?;
                    assert!(editor.rescue.metadata().is_none());
                    assert!(editor.rescue.view.borrow().is_none());
                }
                "protection" | "protection-restored" => {
                    let sink = store.sink.borrow().clone().ok_or(E_FAIL)?;
                    if case == "protection" {
                        *store.scopes.borrow_mut() = ScopeSpec::List(vec![IS_PRIVATE]);
                        unsafe {
                            sink.OnAttrsChange(0, 0, &[GUID_PROP_INPUTSCOPE])?;
                        }
                    } else {
                        store.readonly.set(true);
                        unsafe {
                            sink.OnStatusChange(TS_SD_READONLY)?;
                        }
                        store.readonly.set(false);
                        unsafe {
                            sink.OnStatusChange(0)?;
                        }
                    }
                    pump(window)?;
                    editor.service_recovery(window, &clipboard)?;
                    assert!(!editor.rescue.metadata().ok_or(E_FAIL)?.copyable);
                    assert!(editor.rescue.view.borrow().is_none());
                    editor.input(WM_KEYDOWN, WPARAM(0x79), LPARAM(1))?;
                    editor.service_recovery(window, &clipboard)?;
                    assert_eq!(clipboard.calls.get(), 0);
                }
                "copy-invalidated" => {
                    *clipboard.before.borrow_mut() = Some(Box::new(|| editor.rescue.invalidate()));
                    editor
                        .rescue
                        .view
                        .borrow()
                        .as_ref()
                        .ok_or(E_FAIL)?
                        .click_copy();
                    editor.service_recovery(window, &clipboard)?;
                    assert_eq!(clipboard.calls.get(), 1);
                    assert!(clipboard.text.borrow().is_empty());
                    assert!(!editor.rescue.metadata().ok_or(E_FAIL)?.copyable);
                    assert!(editor.rescue.view.borrow().is_none());
                }
                "late-command" => {
                    editor.input(WM_KEYDOWN, WPARAM(0x79), LPARAM(1))?;
                    editor.input(WM_KEYDOWN, WPARAM(0x78), LPARAM(1))?;
                    editor.rescue.slot.borrow_mut().clear();
                    let next = editor.rescue.reserve(Operation::Insert(1)).ok_or(E_FAIL)?;
                    editor
                        .rescue
                        .finish(next, sumibi_tip::spike::INPUT_UNCERTAIN, &['n' as u16]);
                    editor.service_recovery(window, &clipboard)?;
                    assert_eq!(clipboard.calls.get(), 0);
                    assert_eq!(editor.rescue.metadata().ok_or(E_FAIL)?.ticket, next);
                    editor.input(WM_KEYDOWN, WPARAM(0x77), LPARAM(1))?;
                    editor.service_recovery(window, &clipboard)?;
                    assert_eq!(
                        clipboard.calls.get(),
                        0,
                        "old copy is drained even when discard wins"
                    );
                    assert_eq!(editor.rescue.metadata().ok_or(E_FAIL)?.ticket, next);
                }
                "hide" => {
                    editor
                        .rescue
                        .view
                        .borrow()
                        .as_ref()
                        .ok_or(E_FAIL)?
                        .close_request();
                    editor.service_recovery(window, &clipboard)?;
                    assert!(editor.rescue.view.borrow().is_none());
                    assert_eq!(
                        editor.rescue.metadata().ok_or(E_FAIL)?.ticket,
                        original.ticket
                    );
                    editor.input(WM_KEYDOWN, WPARAM(0x77), LPARAM(1))?;
                    editor.service_recovery(window, &clipboard)?;
                    assert!(editor.rescue.view.borrow().is_some());
                }
                "expired-copy" | "late-ticket" => {
                    editor
                        .rescue
                        .view
                        .borrow()
                        .as_ref()
                        .ok_or(E_FAIL)?
                        .click_copy();
                    editor.rescue.clock.set(Some(Instant::now() + RETENTION));
                    editor.service_recovery(window, &clipboard)?;
                    assert!(editor.rescue.metadata().is_none());
                    assert!(editor.rescue.view.borrow().is_none());
                    assert_eq!(clipboard.calls.get(), 0);
                    if case == "late-ticket" {
                        let ticket = editor.rescue.reserve(Operation::Insert(1)).ok_or(E_FAIL)?;
                        editor.rescue.finish(
                            ticket,
                            sumibi_tip::spike::INPUT_UNCERTAIN,
                            &['n' as u16],
                        );
                        assert!(!editor.rescue.slot.borrow_mut().discard(original.ticket));
                        assert!(editor
                            .rescue
                            .slot
                            .borrow()
                            .text(original.ticket, editor.rescue.now())
                            .is_none());
                        assert_eq!(editor.rescue.metadata().ok_or(E_FAIL)?.ticket, ticket);
                    }
                }
                "expire-wake" => {
                    let deadline = Instant::now() + Duration::from_secs(3);
                    while editor.rescue.metadata().is_some() {
                        pump(window)?;
                        let wait = unsafe {
                            MsgWaitForMultipleObjectsEx(
                                None,
                                editor.rescue.wait_ms(),
                                QS_ALLINPUT,
                                MWMO_INPUTAVAILABLE,
                            )
                        };
                        assert_ne!(wait, WAIT_FAILED);
                        editor.service_recovery(window, &clipboard)?;
                        assert!(Instant::now() < deadline);
                    }
                    editor.service_recovery(window, &clipboard)?;
                    assert!(editor.rescue.view.borrow().is_none());
                    assert_eq!(clipboard.calls.get(), 0);
                }
                "close" => {
                    test_broker_sent_close(window, editor)?;
                    assert!(editor.rescue.metadata().is_none());
                    assert!(editor.rescue.view.borrow().is_none());
                }
                _ => {}
            }
        }
        if !matches!(case, "capacity" | "escape") {
            assert_eq!(
                *store.text.borrow(),
                after,
                "rescue never inserts/undoes document text"
            );
            assert_eq!(store.set_text_calls.get(), writes);
            assert_eq!(store.undo.borrow().len(), history);
        }
    }
    assert_eq!(
        super::super::composing_host::input_notify_host::body_reads()?,
        reads,
        "rescue never reads document text"
    );
    editor.rescue.close();
    assert!(editor.rescue.metadata().is_none());
    assert!(editor.rescue.view.borrow().is_none());
    println!("Input recovery / {case}: OK");
    Ok(())
}

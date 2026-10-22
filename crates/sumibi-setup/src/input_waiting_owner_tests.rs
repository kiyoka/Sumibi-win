//! The demo's route_message/WndProc path with synthetic MSGs. No SendInput,
//! physical modifier assertion, foreground bypass, or system clipboard writes.
use super::*;
use windows::Win32::Foundation::HWND;

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
        if let Some(before) = self.before.borrow_mut().take() {
            before();
        }
        if self.fail.get() || !permitted() {
            return Err(E_FAIL.into());
        }
        *self.text.borrow_mut() = text.to_vec();
        Ok(())
    }
}
fn route(window: &crate::window_host::Window, message: u32, key: usize) -> Result<()> {
    window.route_message(&MSG {
        hwnd: window.hwnd(),
        message,
        wParam: WPARAM(key),
        lParam: LPARAM(1),
        ..Default::default()
    })
}
fn drive(
    window: &crate::window_host::Window,
    editor: &Editor,
    clipboard: &dyn Clipboard,
) -> Result<()> {
    let owner = editor.waiting.as_ref().ok_or(E_FAIL)?;
    let deadline = Instant::now() + Duration::from_secs(8);
    while owner.active()
        || editor
            .broker
            .as_ref()
            .is_some_and(|(_, b)| b.get().active())
    {
        pump(window)?;
        if let Some(composing) = &editor.composing {
            composing.poll(editor)?;
        }
        owner.service(window, editor, clipboard)?;
        if !owner.active() {
            if let Some((broker, bridge)) = &editor.broker {
                let _ = bridge.get().drain(broker)?;
            }
        }
        if Instant::now() >= deadline {
            return Err(E_FAIL.into());
        }
        if let Some(handle) = editor
            .broker
            .as_ref()
            .and_then(|(_, b)| b.get().wait_handle())
        {
            let status = unsafe {
                MsgWaitForMultipleObjectsEx(Some(&[handle]), 20, QS_ALLINPUT, MWMO_INPUTAVAILABLE)
            };
            if status == windows::Win32::Foundation::WAIT_FAILED {
                return Err(E_FAIL.into());
            }
        }
    }
    Ok(())
}
pub(crate) fn exercise_waiting_demo(
    manager: &ITfThreadMgr,
    client: u32,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    for case in [
        "fifo",
        "enter",
        "capacity",
        "copy",
        "discard",
        "cycle",
        "focus",
        "escape",
        "unicode",
        "convert",
        "deadline",
        "copy-failed",
        "copy-boundary",
        "stale-copy",
        "hide",
        "close-busy",
        "deactivate",
        "disable",
        "cancel-mode",
        "stale-session",
        "copy-close",
        "ui-expire",
    ] {
        run(manager, client, RunMode::WaitingTest(case))?;
    }
    Ok(())
}
pub(in super::super::super) fn test_waiting_demo(
    window: &crate::window_host::Window,
    editor: &Editor,
    case: &str,
) -> Result<()> {
    let owner = editor.waiting.as_ref().ok_or(E_FAIL)?;
    let store = editor.store.get();
    let clipboard = TestClipboard::new();
    editor.insert(&"nihongo".encode_utf16().collect::<Vec<_>>())?;
    pump(window)?;
    store.undo.borrow_mut().clear();
    route(window, WM_KEYDOWN, 0x76)?;
    assert!(owner.active());
    let recovery_case = matches!(
        case,
        "enter"
            | "copy"
            | "discard"
            | "cycle"
            | "copy-failed"
            | "copy-boundary"
            | "stale-copy"
            | "hide"
            | "stale-session"
            | "copy-close"
            | "ui-expire"
    );
    if recovery_case {
        route(window, WM_KEYDOWN, 0x0d)?;
    }
    let text = if case == "unicode" { "😀" } else { "ab" };
    for unit in text.encode_utf16() {
        route(window, WM_CHAR, unit as usize)?;
    }
    if case == "fifo" {
        route(window, WM_KEYDOWN, 8)?;
    }
    if case == "capacity" {
        for unit in "cde".encode_utf16() {
            route(window, WM_CHAR, unit as usize)?;
        }
    }
    if case == "convert" {
        route(window, WM_KEYDOWN, 0x76)?;
        route(window, WM_CHAR, 'z' as usize)?;
    }
    if case == "focus" {
        route(window, WM_KILLFOCUS, 0)?;
    }
    if matches!(case, "deactivate" | "disable" | "cancel-mode") {
        test_window_boundary(window, case)?;
    }
    if case == "escape" {
        route(window, WM_KEYDOWN, 0x1b)?;
    }
    if case == "deadline" {
        owner.session.borrow_mut().as_mut().unwrap().deadline = Instant::now();
    }
    if case == "close-busy" {
        owner.state(editor, |_| {
            owner.close();
            Ok(())
        })?;
        assert!(owner.session.borrow().is_none());
        assert!(!owner.active());
    }
    drive(window, editor, &clipboard)?;
    let expected = match case {
        "fifo" => "日本語a",
        "unicode" => "日本語😀",
        "capacity" => "e",
        "convert" => "日本語日本語z",
        "focus" | "escape" | "deadline" | "close-busy" | "deactivate" | "disable"
        | "cancel-mode" => "nihongo",
        _ => "日本語",
    };
    assert_eq!(
        *store.text.borrow(),
        expected.encode_utf16().collect::<Vec<_>>(),
        "{case}"
    );
    assert_eq!(
        editor.submits.get(),
        0,
        "consumed Enter must not generate a second WM_CHAR"
    );
    if recovery_case {
        route(window, WM_KEYDOWN, 0x77)?;
        owner.service(window, editor, &clipboard)?;
        assert_eq!(owner.view.borrow().as_ref().unwrap().key.index, 0);
        if case == "cycle" {
            for index in [1, 2, 0] {
                route(window, WM_KEYDOWN, 0x77)?;
                owner.service(window, editor, &clipboard)?;
                assert_eq!(owner.view.borrow().as_ref().unwrap().key.index, index);
            }
        }
        if case == "discard" {
            owner.view.borrow().as_ref().unwrap().view.click_discard();
            owner.service(window, editor, &clipboard)?;
            assert!(owner
                .session
                .borrow()
                .as_ref()
                .unwrap()
                .metadata(0, Instant::now())
                .is_none());
            assert!(owner
                .session
                .borrow()
                .as_ref()
                .unwrap()
                .metadata(1, Instant::now())
                .is_some());
        }
        if case == "hide" {
            owner.view.borrow().as_ref().unwrap().view.close_request();
            owner.service(window, editor, &clipboard)?;
            assert!(owner.view.borrow().is_none());
            route(window, WM_KEYDOWN, 0x79)?;
            owner.service(window, editor, &clipboard)?;
            assert_eq!(clipboard.calls.get(), 0);
            assert_eq!(owner.session.borrow().as_ref().unwrap().ready.len(), 3);
        }
        if matches!(
            case,
            "copy"
                | "copy-failed"
                | "copy-boundary"
                | "stale-copy"
                | "stale-session"
                | "copy-close"
                | "ui-expire"
        ) {
            route(window, WM_KEYDOWN, 0x77)?;
            owner.service(window, editor, &clipboard)?;
            let old = owner.view.borrow().as_ref().unwrap().key;
            assert_eq!(old.index, 1);
            if matches!(case, "ui-expire" | "stale-session") {
                owner
                    .session
                    .borrow_mut()
                    .as_mut()
                    .unwrap()
                    .expire(Instant::now() + sumibi_core::recovery::RETENTION);
                owner.copy.set(Some(old));
                owner.service(window, editor, &clipboard)?;
                assert!(owner.view.borrow().is_none());
                assert_eq!(clipboard.calls.get(), 0);
                if case == "stale-session" {
                    editor.navigation_boundary();
                    let end = store.text.borrow().len();
                    store.host_selection(0, end)?;
                    pump(window)?;
                    route(window, WM_KEYDOWN, 0x76)?;
                    route(window, WM_KEYDOWN, 0x0d)?;
                    route(window, WM_CHAR, 'a' as usize)?;
                    drive(window, editor, &clipboard)?;
                    let current = owner.session.borrow().as_ref().unwrap().target.target;
                    assert_ne!(current, old.session);
                    owner.copy.set(Some(old));
                    owner.service(window, editor, &clipboard)?;
                    assert_eq!(clipboard.calls.get(), 0);
                    assert!(
                        owner
                            .session
                            .borrow()
                            .as_ref()
                            .unwrap()
                            .metadata(1, Instant::now())
                            .unwrap()
                            .copyable
                    );
                }
            } else if case == "stale-copy" {
                owner.discard.set(Some(old));
                owner.copy.set(Some(old));
                owner.service(window, editor, &clipboard)?;
                owner.service(window, editor, &clipboard)?;
                assert_eq!(clipboard.calls.get(), 0);
                assert!(owner
                    .session
                    .borrow()
                    .as_ref()
                    .unwrap()
                    .metadata(2, Instant::now())
                    .is_some());
            } else {
                if case == "copy-failed" {
                    clipboard.fail.set(true);
                }
                if case == "copy-boundary" {
                    *clipboard.before.borrow_mut() = Some(Box::new(|| owner.boundary()));
                }
                if case == "copy-close" {
                    *clipboard.before.borrow_mut() = Some(Box::new(|| owner.close()));
                }
                owner.view.borrow().as_ref().unwrap().view.click_copy();
                owner.service(window, editor, &clipboard)?;
                if case == "copy" {
                    assert_eq!(*clipboard.text.borrow(), vec!['a' as u16]);
                    assert!(owner
                        .session
                        .borrow()
                        .as_ref()
                        .unwrap()
                        .metadata(1, Instant::now())
                        .is_none());
                    assert!(owner
                        .session
                        .borrow()
                        .as_ref()
                        .unwrap()
                        .metadata(2, Instant::now())
                        .is_some());
                } else if case == "copy-failed" {
                    assert!(
                        owner
                            .session
                            .borrow()
                            .as_ref()
                            .unwrap()
                            .metadata(1, Instant::now())
                            .unwrap()
                            .copyable
                    );
                    clipboard.fail.set(false);
                    route(window, WM_KEYDOWN, 0x79)?;
                    owner.service(window, editor, &clipboard)?;
                    assert_eq!(*clipboard.text.borrow(), vec!['a' as u16]);
                } else if case == "copy-close" {
                    assert!(clipboard.text.borrow().is_empty());
                    assert!(owner.view.borrow().is_none());
                    assert!(owner.session.borrow().is_none());
                } else {
                    assert!(clipboard.text.borrow().is_empty());
                    assert!(owner.view.borrow().is_none());
                    assert!(
                        !owner
                            .session
                            .borrow()
                            .as_ref()
                            .unwrap()
                            .metadata(1, Instant::now())
                            .unwrap()
                            .copyable
                    );
                }
            }
        }
    }
    if case == "fifo" {
        assert_eq!(store.undo.borrow().len(), 4);
        for _ in 0..4 {
            store.undo_once()?;
        }
        assert_eq!(
            *store.text.borrow(),
            "nihongo".encode_utf16().collect::<Vec<_>>()
        );
    }
    owner.close();
    assert!(owner.view.borrow().is_none());
    let mut records = Vec::new();
    for _ in 0..sumibi_core::recovery::MAX_RECORDS {
        let mut slot = Recovery::with_budget(input_recovery_host::shared_budget());
        slot.reserve(Operation::DeferredEnter, Instant::now())
            .map_err(|_| E_FAIL)?;
        records.push(slot);
    }
    println!("Waiting demo / {case}: OK");
    Ok(())
}

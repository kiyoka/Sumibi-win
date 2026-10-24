//! The demo's route_message/WndProc path with synthetic MSGs. No SendInput,
//! physical modifier assertion, foreground bypass, or system clipboard writes.
use super::*;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::GetWindowTextW;

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
fn title(window: &crate::window_host::Window) -> String {
    let mut units = [0u16; 256];
    let length = unsafe { GetWindowTextW(window.hwnd(), &mut units) } as usize;
    String::from_utf16_lossy(&units[..length])
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
    exercise_waiting_demo_with_activation(manager, client, false)
}
pub(crate) fn exercise_waiting_demo_interactive(
    manager: &ITfThreadMgr,
    client: u32,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    exercise_waiting_demo_with_activation(manager, client, true)
}
fn exercise_waiting_demo_with_activation(
    manager: &ITfThreadMgr,
    client: u32,
    interactive: bool,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    for (index, case) in [
        "fifo",
        "enter",
        "capacity",
        "shared-capacity",
        "invalid-full",
        "copy",
        "discard",
        "cycle",
        "focus",
        "escape",
        "escape-empty-text",
        "escape-queued-text",
        "unicode",
        "convert",
        "convert-key",
        "convert-key-alt",
        "deadline",
        "deadline-empty",
        "deadline-idle-text",
        "deadline-idle-backspace",
        "deadline-idle-enter",
        "deadline-queued-text",
        "deadline-admit",
        "deadline-backspace",
        "deadline-enter",
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
        "mixed-cycle",
        "mixed-copy",
        "mixed-discard",
        "mixed-stale",
        "mixed-block",
        "mixed-last-ack",
        "mixed-native-failure",
        "mixed-redacted-discard",
        "ordinary-only",
        "ordinary-expiry",
        "ordinary-copy-boundary",
        "ordinary-copy-close",
        "ordinary-idle-protection",
        "ordinary-visible-wake",
        "start-readonly",
        "start-private",
        "start-error",
        "start-boundary",
        "start-close",
        "start-cancel",
        "start-newer",
        "start-duplicate",
        "start-retained",
        "start-no-request",
        "start-retry",
    ]
    .into_iter()
    .enumerate()
    {
        run(
            manager,
            client,
            if interactive && index == 0 {
                RunMode::WaitingDemoInteractiveTest(case)
            } else {
                RunMode::WaitingTest(case)
            },
        )?;
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
    if case.starts_with("start-") {
        test_start(window, editor, &clipboard, case)?;
        owner.close();
        editor.rescue.close();
        assert!(!owner.active());
        assert!(owner.session.borrow().is_none());
        let mut reserved = Vec::new();
        for _ in 0..sumibi_core::recovery::MAX_RECORDS {
            let mut record = Recovery::with_budget(input_recovery_host::shared_budget());
            record
                .reserve(Operation::DeferredEnter, Instant::now())
                .map_err(|_| E_FAIL)?;
            reserved.push(record);
        }
        println!("Waiting demo / {case}: OK");
        return Ok(());
    }
    if case.starts_with("mixed-") || case.starts_with("ordinary-") {
        test_mixed(window, editor, &clipboard, case)?;
        owner.close();
        editor.rescue.close();
        assert!(owner.view.borrow().is_none());
        assert!(owner.session.borrow().is_none());
        assert!(editor.rescue.metadata().is_none());
        println!("Waiting demo / {case}: OK");
        return Ok(());
    }
    editor.insert(&"nihongo".encode_utf16().collect::<Vec<_>>())?;
    pump(window)?;
    store.undo.borrow_mut().clear();
    if case == "convert-key-alt" {
        assert!(!editor.input(WM_KEYDOWN, WPARAM(0x1c), LPARAM(1 | (1 << 29)))?);
        assert!(!owner.active());
        assert!(editor
            .broker
            .as_ref()
            .ok_or(E_FAIL)?
            .1
            .get()
            .pending_id()
            .is_none());
    }
    route(
        window,
        WM_KEYDOWN,
        if matches!(case, "convert-key" | "convert-key-alt") {
            0x1c
        } else {
            0x76
        },
    )?;
    assert!(owner.active());
    if case == "convert-key-alt" {
        assert!(!editor.input(WM_KEYDOWN, WPARAM(0x1c), LPARAM(1 | (1 << 29)))?);
        assert!(owner.active());
        assert_eq!(owner.offered.get(), 0);
    }
    let mut peers = Vec::new();
    if matches!(case, "shared-capacity" | "invalid-full") {
        for _ in 0..sumibi_core::recovery::MAX_RECORDS {
            let mut record = Recovery::with_budget(input_recovery_host::shared_budget());
            record
                .reserve(Operation::DeferredEnter, Instant::now())
                .map_err(|_| E_FAIL)?;
            peers.push(record);
        }
    }
    if case == "invalid-full" {
        assert!(!owner.admit(editor, Input::Text(&[]))?);
        assert!(!owner.capacity_limited.get());
    }
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
    if matches!(
        case,
        "deadline-empty"
            | "deadline-idle-text"
            | "deadline-idle-backspace"
            | "deadline-idle-enter"
            | "deadline-admit"
            | "deadline-backspace"
            | "deadline-enter"
    ) {
        owner.session.borrow_mut().as_mut().unwrap().deadline = Instant::now();
    }
    if matches!(
        case,
        "deadline-idle-text" | "deadline-idle-backspace" | "deadline-idle-enter"
    ) {
        owner.service(window, editor, &clipboard)?;
        assert!(!owner.active(), "idle timeout retires the queue");
        assert!(title(window).contains("変換待機が期限切れ"));
    }
    let text = match case {
        "unicode" => "😀",
        "shared-capacity" => "e",
        "invalid-full" => "",
        "escape-empty-text" => "",
        "deadline-empty"
        | "deadline-idle-backspace"
        | "deadline-idle-enter"
        | "deadline-backspace"
        | "deadline-enter" => "",
        _ => "ab",
    };
    for unit in text.encode_utf16() {
        route(window, WM_CHAR, unit as usize)?;
    }
    if case == "fifo" {
        route(window, WM_KEYDOWN, 8)?;
    }
    if matches!(case, "deadline-backspace" | "deadline-idle-backspace") {
        route(window, WM_KEYDOWN, 8)?;
    }
    if matches!(case, "deadline-enter" | "deadline-idle-enter") {
        route(window, WM_KEYDOWN, 0x0d)?;
    }
    if case == "capacity" {
        for unit in "cde".encode_utf16() {
            route(window, WM_CHAR, unit as usize)?;
        }
    }
    if case == "deadline-queued-text" {
        owner.session.borrow_mut().as_mut().unwrap().deadline = Instant::now();
        owner.service(window, editor, &clipboard)?;
        assert!(!owner.active(), "idle timeout retires accepted input");
        let records = Owner::records(editor, &owner.session.borrow());
        assert_eq!(records.len(), 2);
        assert!(records.iter().all(|(_, metadata)| metadata.copyable));
        route(window, WM_CHAR, 'c' as usize)?;
    }
    if matches!(case, "convert" | "convert-key") {
        route(
            window,
            WM_KEYDOWN,
            if case == "convert-key" { 0x1c } else { 0x76 },
        )?;
        route(window, WM_CHAR, 'z' as usize)?;
    }
    if case == "focus" {
        route(window, WM_KILLFOCUS, 0)?;
    }
    if matches!(case, "deactivate" | "disable" | "cancel-mode") {
        test_window_boundary(window, case)?;
    }
    if matches!(case, "escape" | "escape-empty-text" | "escape-queued-text") {
        route(window, WM_KEYDOWN, 0x1b)?;
    }
    if matches!(case, "escape-empty-text" | "escape-queued-text") {
        assert!(!owner.active());
        if case == "escape-queued-text" {
            let records = Owner::records(editor, &owner.session.borrow());
            assert_eq!(records.len(), 2);
            assert!(records.iter().all(|(_, metadata)| metadata.copyable));
        }
        route(window, WM_CHAR, 'c' as usize)?;
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
        "capacity" => "nihongoe",
        "shared-capacity" => "nihongoe",
        "invalid-full" => "nihongo",
        "convert" | "convert-key" => "日本語日本語z",
        "convert-key-alt" => "日本語ab",
        "focus"
        | "escape"
        | "deadline"
        | "deadline-empty"
        | "deadline-enter"
        | "deadline-idle-enter"
        | "close-busy"
        | "deactivate"
        | "disable"
        | "cancel-mode" => "nihongo",
        "deadline-admit" | "deadline-idle-text" => "nihongoab",
        "deadline-queued-text" => "nihongoc",
        "escape-empty-text" | "escape-queued-text" => "nihongoc",
        "deadline-backspace" | "deadline-idle-backspace" => "nihong",
        _ => "日本語",
    };
    assert_eq!(
        *store.text.borrow(),
        expected.encode_utf16().collect::<Vec<_>>(),
        "{case}: foreground={}, owner_active={}, boundary={:?}, start_failed={}, queue_active={:?}, last_stop={:?}, broker_active={}, pending_id={:?}, broker_failure={:?}",
        window.require_foreground().is_ok(),
        owner.active(),
        owner.boundary.get(),
        owner.start_failed.get(),
        owner.session.borrow().as_ref().map(|session| session.queue.is_some()),
        owner.session.borrow().as_ref().and_then(|session| session.last_stop),
        editor.broker.as_ref().ok_or(E_FAIL)?.1.get().active(),
        editor.broker.as_ref().ok_or(E_FAIL)?.1.get().pending_id(),
        editor.broker.as_ref().ok_or(E_FAIL)?.1.get().last_failure(),
    );
    assert_eq!(
        editor.submits.get(),
        usize::from(matches!(case, "deadline-enter" | "deadline-idle-enter")),
        "consumed Enter must not generate a second WM_CHAR"
    );
    if case == "capacity" {
        assert_eq!(store.undo.borrow().len(), 1, "unadmitted key edits once");
        assert_eq!(Owner::records(editor, &owner.session.borrow()).len(), 4);
        assert!(owner.capacity_limited.get());
        assert!(title(window).contains("容量上限で新しい入力は通常経路へ"));
        route(window, WM_KEYDOWN, 0x77)?;
        owner.service(window, editor, &clipboard)?;
        assert!(owner.view.borrow().as_ref().unwrap().view.copyable);
    }
    if case == "shared-capacity" {
        assert_eq!(store.undo.borrow().len(), 1, "unadmitted key edits once");
        assert!(Owner::records(editor, &owner.session.borrow()).is_empty());
        assert!(owner.capacity_limited.get());
        assert!(title(window).contains("容量上限で新しい入力は通常経路へ"));
        owner.service(window, editor, &clipboard)?;
        assert!(title(window).contains("容量上限で新しい入力は通常経路へ"));
        route(window, WM_KEYDOWN, 0x1b)?;
        owner.service(window, editor, &clipboard)?;
        assert!(!title(window).contains("容量上限で新しい入力は通常経路へ"));
    }
    if case == "invalid-full" {
        assert_eq!(store.undo.borrow().len(), 0);
        assert!(Owner::records(editor, &owner.session.borrow()).is_empty());
        assert!(!owner.capacity_limited.get());
        assert!(!title(window).contains("容量上限で新しい入力は通常経路へ"));
    }
    if case == "deadline-queued-text" {
        assert_eq!(store.undo.borrow().len(), 1, "later key edits once");
        assert_eq!(Owner::records(editor, &owner.session.borrow()).len(), 2);
        route(window, WM_KEYDOWN, 0x77)?;
        owner.service(window, editor, &clipboard)?;
        assert!(!owner.view.borrow().as_ref().unwrap().view.copyable);
    }
    if matches!(case, "escape-empty-text" | "escape-queued-text") {
        assert_eq!(store.undo.borrow().len(), 1, "later key edits once");
        assert_eq!(
            Owner::records(editor, &owner.session.borrow()).len(),
            if case == "escape-queued-text" { 2 } else { 0 }
        );
    }
    if matches!(
        case,
        "deadline-empty"
            | "deadline-idle-text"
            | "deadline-idle-backspace"
            | "deadline-idle-enter"
            | "deadline-admit"
            | "deadline-backspace"
            | "deadline-enter"
    ) {
        assert!(
            !owner.capacity_limited.get(),
            "deadline is not capacity denial"
        );
        assert!(Owner::records(editor, &owner.session.borrow()).is_empty());
        assert_eq!(
            store.undo.borrow().len(),
            match case {
                "deadline-admit" | "deadline-idle-text" => 2,
                "deadline-backspace" | "deadline-idle-backspace" => 1,
                _ => 0,
            },
            "each unadmitted key edits once"
        );
        assert!(title(window).contains("変換待機が期限切れ"));
        owner.service(window, editor, &clipboard)?;
        assert!(title(window).contains("変換待機が期限切れ"));
        route(window, WM_KEYDOWN, 0x1b)?;
        owner.service(window, editor, &clipboard)?;
        assert!(!title(window).contains("変換待機が期限切れ"));
    }
    if recovery_case {
        route(window, WM_KEYDOWN, 0x77)?;
        owner.service(window, editor, &clipboard)?;
        assert!(matches!(
            owner.view.borrow().as_ref().unwrap().key.source,
            Source::Waiting { index: 0, .. }
        ));
        if case == "cycle" {
            for index in [1, 2, 0] {
                route(window, WM_KEYDOWN, 0x77)?;
                owner.service(window, editor, &clipboard)?;
                assert!(
                    matches!(owner.view.borrow().as_ref().unwrap().key.source, Source::Waiting { index: current, .. } if current == index)
                );
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
            assert!(matches!(old.source, Source::Waiting { index: 1, .. }));
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
                if case == "ui-expire" {
                    assert!(
                        title(window).contains("救済は残っていません"),
                        "expired waiting records need an explicit warning"
                    );
                    owner.service(window, editor, &clipboard)?;
                    assert!(
                        title(window).contains("救済は残っていません"),
                        "warning must survive the next owner loop"
                    );
                    route(window, WM_KEYDOWN, 0x1b)?;
                    owner.service(window, editor, &clipboard)?;
                    assert!(!title(window).contains("救済は残っていません"));
                }
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
                    assert!(
                        matches!(old.source, Source::Waiting { session, .. } if session != current)
                    );
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
    drop(peers);
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

fn test_start(
    window: &crate::window_host::Window,
    editor: &Editor,
    clipboard: &dyn Clipboard,
    case: &str,
) -> Result<()> {
    let owner = editor.waiting.as_ref().ok_or(E_FAIL)?;
    let store = editor.store.get();
    let (broker, bridge) = editor.broker.as_ref().ok_or(E_FAIL)?;
    editor.insert(&"nihongo".encode_utf16().collect::<Vec<_>>())?;
    pump(window)?;
    store.undo.borrow_mut().clear();
    if case == "start-no-request" {
        owner.started_with(editor, |_| panic!("initialise without a request"))?;
        assert!(!owner.start_failed.get());
        assert!(!bridge.get().active());
        return Ok(());
    }
    if matches!(case, "start-duplicate" | "start-retained") {
        route(window, WM_KEYDOWN, 0x76)?;
        if case == "start-retained" {
            route(window, WM_KEYDOWN, 0x0d)?;
        }
        route(window, WM_CHAR, 'a' as usize)?;
        let identity = owner.session.borrow().as_ref().unwrap().target.target;
        if case == "start-duplicate" {
            owner.started_with(editor, |_| panic!("duplicate initialisation"))?;
            assert_eq!(
                owner
                    .session
                    .borrow()
                    .as_ref()
                    .unwrap()
                    .queue
                    .as_ref()
                    .unwrap()
                    .len(),
                1
            );
            assert_eq!(
                owner.session.borrow().as_ref().unwrap().target.target,
                identity
            );
            drive(window, editor, clipboard)?;
            assert_eq!(
                *store.text.borrow(),
                "日本語a".encode_utf16().collect::<Vec<_>>()
            );
            return Ok(());
        }
        drive(window, editor, clipboard)?;
        assert_eq!(
            owner.session.borrow().as_ref().unwrap().ready.len(),
            2,
            "start-retained: starts={}, offered={}, admitted={}, peak_recoveries={}, views_opened={}",
            owner.starts.get(),
            owner.offered.get(),
            owner.admitted.get(),
            owner.peak_recoveries.get(),
            owner.views_opened.get(),
        );
        editor.navigation_boundary();
        store.host_selection(0, store.text.borrow().len())?;
        pump(window)?;
        owner.state(editor, |_| {
            editor.input(WM_KEYDOWN, WPARAM(0x76), LPARAM(1))?;
            Ok(())
        })?;
        assert!(bridge.get().pending_id().is_some());
        owner.started_with(editor, |_| panic!("unresolved recovery overwritten"))?;
        assert!(owner.start_failed.get());
        assert_eq!(
            owner.session.borrow().as_ref().unwrap().target.target,
            identity
        );
        assert_eq!(Owner::records(editor, &owner.session.borrow()).len(), 2);
        let mut pending = 1;
        unsafe {
            broker.Pending(&mut pending).ok()?;
        }
        assert_eq!(pending, 0);
        tracking_host::wait_result(window, broker, bridge, false)?;
        return Ok(());
    }
    // Start the real DLL/worker under the existing owner busy guard, then
    // interleave a controlled initialisation boundary before publication.
    owner.state(editor, |_| {
        editor.input(WM_KEYDOWN, WPARAM(0x76), LPARAM(1))?;
        Ok(())
    })?;
    let request = bridge.get().pending_id().ok_or(E_FAIL)?;
    let writes = store.set_text_calls.get();
    let reads = composing_host::input_notify_host::body_reads()?;
    let newer = Cell::new(None);
    owner.started_with(editor, |expected| {
        assert_eq!(expected, request);
        if case == "start-error" {
            return Err(E_FAIL.into());
        }
        if matches!(case, "start-readonly" | "start-retry") {
            store.readonly.set(true);
        }
        if case == "start-private" {
            tracking_host::set_scope(editor, windows::Win32::UI::TextServices::IS_PRIVATE)?;
        }
        let waiting = Waiting::begin(&owner.manager, editor, owner.client, expected)?;
        match case {
            "start-boundary" => owner.boundary(),
            "start-close" => owner.close(),
            "start-cancel" => unsafe {
                broker.Cancel(expected).ok()?;
            },
            "start-newer" => {
                unsafe {
                    broker.Cancel(expected).ok()?;
                }
                tracking_host::wait_result(window, broker, bridge, false)?;
                editor.navigation_boundary();
                store.host_selection(0, store.text.borrow().len())?;
                pump(window)?;
                editor.input(WM_KEYDOWN, WPARAM(0x76), LPARAM(1))?;
                newer.set(bridge.get().pending_id());
            }
            _ => return Err(E_FAIL.into()),
        }
        Ok(waiting)
    })?;
    store.readonly.set(false);
    if case == "start-private" {
        tracking_host::set_scope(editor, windows::Win32::UI::TextServices::IS_DEFAULT)?;
    }
    assert!(owner.start_failed.get());
    assert!(!owner.active());
    assert!(owner.session.borrow().is_none());
    let mut pending = 0;
    unsafe {
        broker.Pending(&mut pending).ok()?;
    }
    if case == "start-newer" {
        let newer = newer.get().ok_or(E_FAIL)?;
        assert_ne!(newer, request);
        assert_eq!(
            pending, newer,
            "failed old attempt cannot cancel the newer request"
        );
        tracking_host::wait_result(window, broker, bridge, true)?;
        assert_eq!(
            *store.text.borrow(),
            "日本語".encode_utf16().collect::<Vec<_>>()
        );
        return Ok(());
    }
    assert_eq!(
        pending, 0,
        "failed initialisation cancels the captured request"
    );
    tracking_host::wait_result(window, broker, bridge, false)?;
    assert_eq!(
        *store.text.borrow(),
        "nihongo".encode_utf16().collect::<Vec<_>>()
    );
    assert_eq!(store.set_text_calls.get(), writes);
    assert_eq!(composing_host::input_notify_host::body_reads()?, reads);
    assert!(store.undo.borrow().is_empty());
    if case == "start-retry" {
        editor.navigation_boundary();
        store.host_selection(0, store.text.borrow().len())?;
        pump(window)?;
        route(window, WM_KEYDOWN, 0x76)?;
        assert!(owner.active());
        assert!(!owner.start_failed.get());
        drive(window, editor, clipboard)?;
        assert_eq!(
            *store.text.borrow(),
            "日本語".encode_utf16().collect::<Vec<_>>()
        );
    }
    Ok(())
}

fn test_mixed<'a>(
    window: &crate::window_host::Window,
    editor: &'a Editor,
    clipboard: &TestClipboard<'a>,
    case: &str,
) -> Result<()> {
    let owner = editor.waiting.as_ref().ok_or(E_FAIL)?;
    let store = editor.store.get();
    store.type_text(&"nihongo".encode_utf16().collect::<Vec<_>>())?;
    store.host_selection(0, 7)?;
    pump(window)?;
    if case.starts_with("mixed-") {
        route(window, WM_KEYDOWN, 0x76)?;
        route(window, WM_KEYDOWN, 0x0d)?;
        route(window, WM_CHAR, 'a' as usize)?;
        route(window, WM_CHAR, 'b' as usize)?;
        drive(window, editor, clipboard)?;
    }
    // The real failed insertion and a fixture completion both share the pool
    // with deferred input. Neither injects desktop keys or uses the clipboard.
    let ticket = if case == "mixed-native-failure" {
        store.fail_text_after.set(true);
        editor.insert(&['x' as u16])?;
        store.fail_text_after.set(false);
        assert!(store.text.borrow().ends_with(&['x' as u16]));
        editor.rescue.metadata().ok_or(E_FAIL)?.ticket
    } else {
        let ticket = editor.rescue.reserve(Operation::Insert(1)).ok_or(E_FAIL)?;
        editor
            .rescue
            .finish(ticket, sumibi_tip::spike::INPUT_UNCERTAIN, &['x' as u16]);
        ticket
    };
    editor.handled_transaction(sumibi_tip::spike::INPUT_UNCERTAIN);
    assert!(editor.rescue.metadata().ok_or(E_FAIL)?.copyable);
    owner.service(window, editor, clipboard)?;
    let initial = store.text.borrow().clone();
    let writes = store.set_text_calls.get();
    let count = if case.starts_with("mixed-") { 4 } else { 1 };
    assert_eq!(Owner::records(editor, &owner.session.borrow()).len(), count);
    route(window, WM_KEYDOWN, 0x77)?;
    owner.service(window, editor, clipboard)?;
    let first = owner.view.borrow().as_ref().unwrap().key;
    if count == 4 {
        assert!(matches!(first.source, Source::Waiting { index: 0, .. }));
        for _ in 0..3 {
            route(window, WM_KEYDOWN, 0x77)?;
            owner.service(window, editor, clipboard)?;
        }
    }
    let ordinary = owner.view.borrow().as_ref().unwrap().key;
    assert!(matches!(ordinary.source, Source::Ordinary));
    assert_eq!(ordinary.ticket, ticket);
    match case {
        "mixed-cycle" => {
            assert_eq!(
                first.ticket, ordinary.ticket,
                "source distinguishes equal slot tickets"
            );
            route(window, WM_KEYDOWN, 0x77)?;
            owner.service(window, editor, clipboard)?;
            assert!(owner.view.borrow().as_ref().is_some_and(|v| v.key == first));
        }
        "mixed-block" => {
            owner.view.borrow().as_ref().unwrap().view.close_request();
            owner.service(window, editor, clipboard)?;
            route(window, WM_KEYDOWN, 0x76)?;
            owner.service(window, editor, clipboard)?;
            assert!(!owner.active());
            assert!(owner.view.borrow().is_some());
            assert!(editor
                .broker
                .as_ref()
                .unwrap()
                .1
                .get()
                .pending_id()
                .is_none());
            assert_eq!(Owner::records(editor, &owner.session.borrow()).len(), 4);
        }
        "mixed-discard" => {
            route(window, WM_KEYDOWN, 0x78)?;
            owner.service(window, editor, clipboard)?;
            assert!(editor.rescue.metadata().is_none());
            assert_eq!(Owner::records(editor, &owner.session.borrow()).len(), 3);
        }
        "mixed-stale" => {
            owner.discard.set(Some(ordinary));
            owner.copy.set(Some(ordinary));
            owner.service(window, editor, clipboard)?;
            let next = editor.rescue.reserve(Operation::Insert(1)).ok_or(E_FAIL)?;
            editor
                .rescue
                .finish(next, sumibi_tip::spike::INPUT_UNCERTAIN, &['y' as u16]);
            owner.copy.set(Some(ordinary));
            owner.service(window, editor, clipboard)?;
            assert_eq!(clipboard.calls.get(), 0);
            assert_eq!(editor.rescue.metadata().unwrap().ticket, next);
            assert_ne!(next, ordinary.ticket);
        }
        "mixed-redacted-discard" => {
            store.readonly.set(true);
            owner.boundary();
            owner.service(window, editor, clipboard)?;
            assert!(owner.view.borrow().is_none());
            assert!(Owner::records(editor, &owner.session.borrow())
                .iter()
                .all(|(_, m)| !m.copyable));
            for remaining in (0..4).rev() {
                route(window, WM_KEYDOWN, 0x77)?;
                owner.service(window, editor, clipboard)?;
                assert!(!owner.view.borrow().as_ref().unwrap().view.copyable);
                owner.service(window, editor, clipboard)?;
                assert!(
                    owner.view.borrow().is_some(),
                    "metadata-only view stays open"
                );
                route(window, WM_KEYDOWN, 0x79)?;
                owner.service(window, editor, clipboard)?;
                assert_eq!(clipboard.calls.get(), 0);
                route(window, WM_KEYDOWN, 0x78)?;
                owner.service(window, editor, clipboard)?;
                assert_eq!(
                    Owner::records(editor, &owner.session.borrow()).len(),
                    remaining
                );
            }
        }
        "ordinary-expiry" => {
            editor
                .rescue
                .expire(Instant::now() + sumibi_core::recovery::RETENTION);
            route(window, WM_KEYDOWN, 0x79)?;
            owner.service(window, editor, clipboard)?;
            assert_eq!(clipboard.calls.get(), 0);
            assert!(owner.view.borrow().is_none());
            assert!(editor.rescue.metadata().is_none());
            assert!(store.input_problem.get(), "expiry is not acknowledgement");
            assert!(
                title(window).contains("救済は残っていません"),
                "expired ordinary rescue needs an explicit warning"
            );
            owner.service(window, editor, clipboard)?;
            assert!(title(window).contains("救済は残っていません"));
            route(window, WM_KEYDOWN, 0x1b)?;
            owner.service(window, editor, clipboard)?;
            assert!(!title(window).contains("救済は残っていません"));
        }
        "ordinary-idle-protection" => {
            owner.view.borrow().as_ref().unwrap().view.close_request();
            owner.service(window, editor, clipboard)?;
            assert!(owner.view.borrow().is_none());
            assert!(
                owner.wait_ms(editor) <= 250,
                "hidden rescue needs a protection check"
            );
            store.readonly.set(true);
            owner.service(window, editor, clipboard)?;
            assert!(!editor.rescue.metadata().unwrap().copyable);
            route(window, WM_KEYDOWN, 0x77)?;
            owner.service(window, editor, clipboard)?;
            assert!(!owner.view.borrow().as_ref().unwrap().view.copyable);
            route(window, WM_KEYDOWN, 0x79)?;
            owner.service(window, editor, clipboard)?;
            assert_eq!(clipboard.calls.get(), 0);
            route(window, WM_KEYDOWN, 0x78)?;
            owner.service(window, editor, clipboard)?;
            assert!(editor.rescue.metadata().is_none());
            store.readonly.set(false);
        }
        "ordinary-visible-wake" => {
            assert!(
                owner.wait_ms(editor) <= 1000,
                "visible countdown needs a wake"
            );
            assert!(owner.wait_ms(editor) > 0);
            route(window, WM_KEYDOWN, 0x78)?;
            owner.service(window, editor, clipboard)?;
            assert!(editor.rescue.metadata().is_none());
            assert_eq!(owner.wait_ms(editor), u32::MAX);
        }
        "ordinary-copy-boundary" | "ordinary-copy-close" => {
            if case == "ordinary-copy-close" {
                *clipboard.before.borrow_mut() = Some(Box::new(|| owner.close()));
            } else {
                *clipboard.before.borrow_mut() = Some(Box::new(|| owner.boundary()));
            }
            route(window, WM_KEYDOWN, 0x79)?;
            owner.service(window, editor, clipboard)?;
            assert!(clipboard.text.borrow().is_empty());
            assert!(owner.view.borrow().is_none());
            assert!(editor.rescue.metadata().is_none_or(|m| !m.copyable));
        }
        "mixed-copy" | "ordinary-only" | "mixed-last-ack" | "mixed-native-failure" => {
            route(window, WM_KEYDOWN, 0x79)?;
            owner.service(window, editor, clipboard)?;
            assert_eq!(*clipboard.text.borrow(), vec!['x' as u16]);
            assert!(editor.rescue.metadata().is_none());
            assert_eq!(
                Owner::records(editor, &owner.session.borrow()).len(),
                count - 1
            );
            assert_eq!(store.input_problem.get(), count > 1);
            if case == "mixed-last-ack" {
                for remaining in (0..3).rev() {
                    route(window, WM_KEYDOWN, 0x77)?;
                    owner.service(window, editor, clipboard)?;
                    route(window, WM_KEYDOWN, 0x78)?;
                    owner.service(window, editor, clipboard)?;
                    assert_eq!(
                        Owner::records(editor, &owner.session.borrow()).len(),
                        remaining
                    );
                }
                store.readonly.set(false);
                assert!(owner.session.borrow().is_none());
                assert!(owner.can_start());
                assert!(
                    !store.input_problem.get(),
                    "last explicit acknowledgement clears warning"
                );
            }
        }
        _ => return Err(E_FAIL.into()),
    }
    assert_eq!(
        *store.text.borrow(),
        initial,
        "recovery never edits the document"
    );
    assert_eq!(store.set_text_calls.get(), writes);
    Ok(())
}

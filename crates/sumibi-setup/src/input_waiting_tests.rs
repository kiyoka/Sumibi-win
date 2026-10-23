//! Synthetic owner store + actual DLL/worker/TSF. No physical key injection or
//! user's clipboard. Semantic admissions are explicitly called by this fixture.
use super::*;
use crate::recovery_view::Clipboard;
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
pub(crate) fn exercise_waiting(
    manager: &ITfThreadMgr,
    client: u32,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    exercise_waiting_with_activation(manager, client, false)
}
pub(crate) fn exercise_waiting_interactive(
    manager: &ITfThreadMgr,
    client: u32,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    exercise_waiting_with_activation(manager, client, true)
}
fn exercise_waiting_with_activation(
    manager: &ITfThreadMgr,
    client: u32,
    interactive: bool,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    for (index, case) in [
        "waiting/fifo",
        "waiting/unicode",
        "waiting/convert",
        "waiting/enter",
        "waiting/budget",
        "waiting/capacity",
        "waiting/maximum",
        "waiting/invalid",
        "waiting/private",
        "waiting/readonly",
        "waiting/external",
        "waiting/scope-restored",
        "waiting/cancel",
        "waiting/deadline",
        "waiting/fail-before",
        "waiting/fail-after",
        "waiting/applied-untracked",
        "waiting/probe",
        "waiting/probe-foreign",
        "waiting/copy",
        "waiting/copy-failed",
        "waiting/copy-protected",
        "waiting/copy-expired-during",
        "waiting/expiry",
        "waiting/drop",
        "waiting/drop-newer",
    ]
    .into_iter()
    .enumerate()
    {
        run(
            manager,
            client,
            if interactive && index == 0 {
                RunMode::WaitingInteractiveTest(case)
            } else {
                RunMode::TransactionTest(case)
            },
        )?;
    }
    Ok(())
}
fn drive(
    window: &crate::window_host::Window,
    editor: &Editor,
    waiting: &mut Waiting,
) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(8);
    while waiting.queue.is_some() {
        pump(window)?;
        waiting.service(editor)?;
        if Instant::now() >= deadline {
            return Err(E_FAIL.into());
        }
        if let Some(handle) = editor
            .broker
            .as_ref()
            .and_then(|(_, b)| b.get().wait_handle())
        {
            let result = unsafe {
                MsgWaitForMultipleObjectsEx(Some(&[handle]), 50, QS_ALLINPUT, MWMO_INPUTAVAILABLE)
            };
            if result == windows::Win32::Foundation::WAIT_FAILED {
                return Err(E_FAIL.into());
            }
        }
    }
    Ok(())
}
pub(in super::super) fn test_waiting(
    manager: &ITfThreadMgr,
    window: &crate::window_host::Window,
    editor: &Editor,
    live: &crate::live_render::Binding,
    client: u32,
    case: &str,
) -> Result<()> {
    let store = editor.store.get();
    editor.insert(&"nihongo".encode_utf16().collect::<Vec<_>>())?;
    pump(window)?;
    store.undo.borrow_mut().clear();
    if case.starts_with("probe") {
        let probe = editor.composing.as_ref().ok_or(E_FAIL)?.state_probe()?;
        let point = store.selection.get();
        let reads = composing_host::input_notify_host::body_reads()?;
        let writes = store.set_text_calls.get();
        unsafe {
            probe
                .Validate(editor.context.as_raw(), point.acpStart, point.acpEnd, 0)
                .ok()?;
            assert!(probe
                .Validate(editor.context.as_raw(), point.acpStart, point.acpEnd, 2)
                .is_err());
        }
        if case == "probe-foreign" {
            let mut foreign = crate::edit_host::composition_host::Composition::start(
                &editor.context,
                client,
                0,
                2,
            )?;
            assert!(unsafe {
                probe.Validate(editor.context.as_raw(), point.acpStart, point.acpEnd, 0)
            }
            .is_err());
            foreign.end()?;
        }
        assert_eq!(composing_host::input_notify_host::body_reads()?, reads);
        assert_eq!(store.set_text_calls.get(), writes);
        assert!(store.undo.borrow().is_empty());
        editor.clear_tracking();
        println!("Waiting input / {case}: OK");
        return Ok(());
    }
    editor.input(WM_KEYDOWN, WPARAM(0x76), LPARAM(1))?;
    let (broker, bridge) = editor.broker.as_ref().ok_or(E_FAIL)?;
    let request = bridge.get().pending_id().ok_or(E_FAIL)?;
    let mut waiting = Waiting::begin(manager, editor, client, request)?;
    let initial = store.text.borrow().clone();
    let writes = store.set_text_calls.get();
    let reads = composing_host::input_notify_host::body_reads()?;
    let mut peers = Vec::new();
    if case == "budget" {
        for _ in 0..3 {
            let mut recovery = Recovery::with_budget(input_recovery_host::shared_budget());
            let ticket = recovery
                .reserve(Operation::Backspace(1), Instant::now())
                .map_err(|_| E_FAIL)?;
            recovery.finish(ticket, Outcome::Uncertain, &[], Instant::now());
            peers.push((recovery, ticket));
        }
    }
    let entered: Vec<_> = if case == "unicode" {
        "a👨‍👩‍👧‍👦🇯🇵"
    } else {
        "です"
    }
    .encode_utf16()
    .collect();
    if matches!(
        case,
        "enter"
            | "copy"
            | "copy-failed"
            | "copy-protected"
            | "copy-expired-during"
            | "expiry"
            | "drop"
    ) {
        waiting.admit(editor, Input::Enter)?;
        waiting.admit(editor, Input::Text(&entered))?;
    } else if case == "convert" {
        waiting.admit(editor, Input::Text(&['a' as u16]))?;
        waiting.admit(editor, Input::Convert)?;
        waiting.admit(editor, Input::Text(&['z' as u16]))?;
    } else if case == "maximum" {
        let block = vec!['漢' as u16; sumibi_core::recovery::MAX_UNITS];
        for _ in 0..4 {
            waiting.admit(editor, Input::Text(&block))?;
        }
        assert!(waiting.admit(editor, Input::Text(&['z' as u16])).is_err());
        assert_eq!(waiting.queue.as_ref().unwrap().text_units(), 4096);
    } else if case == "capacity" {
        for _ in 0..4 {
            waiting.admit(editor, Input::Text(&['a' as u16]))?;
        }
        assert!(waiting.admit(editor, Input::Text(&['z' as u16])).is_err());
        assert_eq!(waiting.queue.as_ref().unwrap().len(), 4);
    } else {
        waiting.admit(editor, Input::Text(&entered))?;
        if matches!(case, "fifo" | "unicode") {
            waiting.admit(editor, Input::Backspace(1))?;
        }
        if matches!(case, "fail-before" | "fail-after" | "applied-untracked") {
            waiting.admit(editor, Input::Text(&['z' as u16]))?;
        }
    }
    if case == "invalid" {
        for value in [
            vec![],
            vec![0xd800],
            vec![0x0a],
            vec![0xfffc],
            vec![b'x' as u16; 1025],
        ] {
            assert!(waiting.admit(editor, Input::Text(&value)).is_err());
        }
        assert_eq!(waiting.queue.as_ref().unwrap().len(), 1);
    }
    if case == "budget" {
        assert!(waiting.admit(editor, Input::Text(&['z' as u16])).is_err());
        assert_eq!(waiting.queue.as_ref().unwrap().len(), 1);
        for (peer, ticket) in &peers {
            assert_eq!(peer.metadata(Instant::now()).unwrap().ticket, *ticket);
        }
    }
    assert_eq!(*store.text.borrow(), initial, "admission must not edit");
    assert_eq!(store.set_text_calls.get(), writes);
    assert_eq!(composing_host::input_notify_host::body_reads()?, reads);
    assert!(store.undo.borrow().is_empty());
    if case == "drop" {
        drop(waiting);
        let mut pending = 1;
        unsafe {
            broker.Pending(&mut pending).ok()?;
        }
        assert_eq!(pending, 0, "owner Drop cancels its exact request");
        tracking_host::wait_result(window, broker, bridge, false)?;
        assert_eq!(*store.text.borrow(), initial);
    } else if case == "drop-newer" {
        waiting.stop(editor, Boundary::Cancelled, Outcome::Deferred);
        tracking_host::wait_result(window, broker, bridge, false)?;
        editor.navigation_boundary();
        store.host_selection(0, initial.len())?;
        pump(window)?;
        editor.input(WM_KEYDOWN, WPARAM(0x76), LPARAM(1))?;
        let new_request = bridge.get().pending_id().ok_or(E_FAIL)?;
        assert_ne!(new_request, waiting.request);
        drop(waiting);
        let mut pending = 0;
        unsafe {
            broker.Pending(&mut pending).ok()?;
        }
        assert_eq!(
            pending, new_request,
            "old owner Drop must not cancel a newer request"
        );
        tracking_host::wait_result(window, broker, bridge, true)?;
        assert_eq!(
            *store.text.borrow(),
            "日本語".encode_utf16().collect::<Vec<_>>()
        );
    } else {
        match case {
            "private" | "scope-restored" => {
                tracking_host::set_scope(editor, IS_PRIVATE)?;
                if case == "scope-restored" {
                    tracking_host::set_scope(editor, IS_DEFAULT)?;
                }
            }
            "readonly" => store.readonly.set(true),
            "external" => {
                store.external_text("external".encode_utf16().collect())?;
            }
            "cancel" | "maximum" => waiting.stop(editor, Boundary::Cancelled, Outcome::Deferred),
            "deadline" => waiting.deadline = Instant::now(),
            _ => {}
        }
        if matches!(case, "fail-before" | "fail-after" | "applied-untracked") {
            // Apply the initial conversion first, but leave the first queued
            // native insertion for the subsequent failing service call.
            tracking_host::wait_result(window, broker, bridge, true)?;
            waiting.settled_conversion(editor)?;
            if case == "fail-before" {
                store.fail_text_before.set(true);
            } else if case == "fail-after" {
                store.fail_text_after.set(true);
            } else {
                store.fail_selection.set(true);
            }
        }
        drive(window, editor, &mut waiting)?;
        if matches!(
            case,
            "private"
                | "readonly"
                | "external"
                | "scope-restored"
                | "cancel"
                | "deadline"
                | "maximum"
        ) && bridge.get().active()
        {
            tracking_host::wait_result(window, broker, bridge, false)?;
        }
        pump(window)?;
        let expected = match case {
            "fifo" => "日本語で",
            "unicode" => "日本語a👨‍👩‍👧‍👦",
            "convert" => "日本語日本語z",
            "capacity" => "日本語aaaa",
            "budget" | "invalid" | "fail-after" | "applied-untracked" => "日本語です",
            "fail-before" => "日本語",
            "private" | "readonly" | "scope-restored" | "cancel" | "deadline" | "maximum" => {
                "nihongo"
            }
            "external" => "external",
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
            "deferred Enter is never sent to host"
        );
        if matches!(case, "fifo" | "unicode") {
            assert_eq!(store.undo.borrow().len(), 3);
            assert_eq!(
                live.scene()?.text,
                expected.encode_utf16().collect::<Vec<_>>()
            );
            store.undo_once()?;
            store.undo_once()?;
            store.undo_once()?;
            assert_eq!(*store.text.borrow(), initial);
        }
        if matches!(case, "fail-before" | "fail-after" | "applied-untracked") {
            let metadata = waiting.metadata(0, Instant::now()).ok_or(E_FAIL)?;
            assert_eq!(
                metadata.outcome,
                if case == "applied-untracked" {
                    Outcome::AppliedUntracked
                } else {
                    Outcome::Uncertain
                }
            );
            assert!(metadata.copyable);
            assert_eq!(
                waiting.metadata(1, Instant::now()).unwrap().outcome,
                Outcome::Deferred
            );
            assert_eq!(
                store.set_text_calls.get(),
                writes + 2,
                "one result write and one head attempt; never the tail"
            );
            let native_writes = store.set_text_calls.get();
            assert!(!waiting.service(editor)?);
            assert_eq!(store.set_text_calls.get(), native_writes);
        }
        if case == "maximum" {
            assert_eq!(waiting.ready.len(), 4);
            let clipboard = TestClipboard::new();
            waiting.copy(editor, 0, &clipboard)?;
            assert_eq!(clipboard.text.borrow().len(), 1024);
            assert_eq!(store.set_text_calls.get(), writes);
        }
        if matches!(
            case,
            "enter" | "copy" | "copy-failed" | "copy-protected" | "copy-expired-during" | "expiry"
        ) {
            assert_eq!(
                waiting.metadata(0, Instant::now()).unwrap().operation,
                Operation::DeferredEnter
            );
            assert_eq!(
                waiting.metadata(0, Instant::now()).unwrap().outcome,
                Outcome::Deferred
            );
            assert!(!waiting.metadata(0, Instant::now()).unwrap().copyable);
            assert_eq!(
                waiting.metadata(1, Instant::now()).unwrap().outcome,
                Outcome::Deferred
            );
            let copy_now = Cell::new(Instant::now());
            let clipboard = TestClipboard::new();
            let before_copy = store.set_text_calls.get();
            if case == "copy-protected" {
                tracking_host::set_scope(editor, IS_PRIVATE)?;
            }
            if case == "copy-failed" {
                clipboard.fail.set(true);
            }
            if case == "expiry" {
                waiting.expire(Instant::now() + sumibi_core::recovery::RETENTION);
            }
            if case == "copy-expired-during" {
                *clipboard.before.borrow_mut() = Some(Box::new(|| {
                    copy_now.set(copy_now.get() + sumibi_core::recovery::RETENTION);
                }));
            }
            let copied = waiting.copy_with_clock(editor, 1, &clipboard, &|| copy_now.get());
            assert_eq!(
                copied.is_ok(),
                !matches!(
                    case,
                    "copy-protected" | "copy-failed" | "copy-expired-during" | "expiry"
                )
            );
            if copied.is_ok() {
                assert_eq!(*clipboard.text.borrow(), entered);
            }
            if case == "copy-protected" {
                assert_eq!(clipboard.calls.get(), 0);
            }
            if case == "copy-failed" {
                assert!(waiting.metadata(1, Instant::now()).unwrap().copyable);
            }
            if case == "copy-expired-during" {
                assert_eq!(clipboard.calls.get(), 1);
                assert!(clipboard.text.borrow().is_empty());
                assert!(waiting.metadata(1, copy_now.get()).is_none());
                waiting.expire(copy_now.get());
            }
            assert_eq!(store.set_text_calls.get(), before_copy);
        }
        drop(waiting);
    }
    store.readonly.set(false);
    store.fail_text_before.set(false);
    store.fail_text_after.set(false);
    store.fail_selection.set(false);
    tracking_host::set_scope(editor, IS_DEFAULT)?;
    editor.clear_tracking();
    drop(peers);
    let mut available = Vec::new();
    for _ in 0..sumibi_core::recovery::MAX_RECORDS {
        let mut recovery = Recovery::with_budget(input_recovery_host::shared_budget());
        recovery
            .reserve(Operation::DeferredEnter, Instant::now())
            .map_err(|_| E_FAIL)?;
        available.push(recovery);
    }
    println!("Waiting input / {case}: OK");
    Ok(())
}

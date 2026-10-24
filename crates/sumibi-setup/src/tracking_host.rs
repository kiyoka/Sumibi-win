//! Cooperating host input-run demo; no text is inferred from an existing field.
use super::*;
use std::sync::atomic::{AtomicU64, Ordering};
use sumibi_core::input::{InputPoint, InputSession};
use sumibi_tip::edit::{inspect_input, manual::InputWatch, InputMetadata};

pub(super) struct Tracking {
    manager: ITfThreadMgr,
    client: u32,
    identity: u128,
    watch: Rc<InputWatch>,
    session: RefCell<InputSession>,
    metadata: Cell<Option<InputMetadata>>,
    auto_selected: Cell<bool>,
}
static NEXT_EDITOR: AtomicU64 = AtomicU64::new(0);
impl Tracking {
    pub(super) fn new(manager: &ITfThreadMgr, context: &ITfContext, client: u32) -> Result<Self> {
        let identity = NEXT_EDITOR
            .try_update(Ordering::AcqRel, Ordering::Acquire, |id| id.checked_add(1))
            .map_err(|_| windows::core::Error::from(E_FAIL))?
            + 1;
        Ok(Self {
            manager: manager.clone(),
            client,
            identity: identity.into(),
            watch: Rc::new(InputWatch::attach(manager, context)?),
            session: RefCell::new(InputSession::default()),
            metadata: Cell::new(None),
            auto_selected: Cell::new(false),
        })
    }
    pub(super) fn clear(&self) {
        self.session.borrow_mut().clear();
        self.metadata.set(None);
    }
    pub(super) fn user_boundary(&self) {
        self.clear();
        self.auto_selected.set(false);
    }
    pub(super) fn blocks_selection(&self) -> bool {
        self.auto_selected.get()
    }
    pub(super) fn observe(&self, editor: &Editor) -> Option<(InputPoint, InputMetadata)> {
        let generation = self.watch.generation().ok()?;
        let composition_generation = u64::from(editor.store.get().composition_events.get());
        if composition_generation == u64::from(u32::MAX) {
            return None;
        }
        let metadata = inspect_input(&self.manager, &editor.context, self.client).ok()?;
        let selected = editor.store.get().selection.get();
        if metadata.selection != (selected.acpStart, selected.acpEnd)
            || self.watch.generation().ok()? != generation
            || u64::from(editor.store.get().composition_events.get()) != composition_generation
        {
            return None;
        }
        Some((
            InputPoint {
                context: self.identity,
                generation,
                composition_generation,
                selection: metadata.selection,
                allowed: true,
            },
            metadata,
        ))
    }
    pub(super) fn inserted(
        &self,
        editor: &Editor,
        before: Option<(InputPoint, InputMetadata)>,
        units: &[u16],
    ) {
        self.auto_selected.set(false);
        let after = self.observe(editor);
        if let (Some((before, first)), Some((after, last))) = (before, after) {
            if !first.same_protection(&last) {
                self.clear();
                return;
            }
            if self
                .metadata
                .get()
                .is_some_and(|old| !old.same_protection(&first))
            {
                self.clear();
            }
            self.session.borrow_mut().inserted(before, after, units);
            self.metadata.set(Some(last));
        } else {
            self.clear();
        }
    }
    pub(super) fn select(&self, editor: &Editor, bridge: &ComObject<Bridge>) -> Result<bool> {
        if self.auto_selected.get() {
            return Ok(false);
        }
        let Some((point, metadata)) = self.observe(editor) else {
            self.clear();
            return Ok(false);
        };
        if !self
            .metadata
            .get()
            .is_some_and(|old| old.same_protection(&metadata))
        {
            self.clear();
            return Ok(false);
        }
        let run = self.session.borrow_mut().take(point);
        self.metadata.set(None);
        let Some(run) = run else {
            return Ok(false);
        };
        let range = run.range();
        self.auto_selected.set(true);
        editor
            .store
            .get()
            .host_selection(range.0 as usize, range.1 as usize)?;
        let Some((selected, protection)) = self.observe(editor) else {
            return Ok(false);
        };
        if selected.selection != range || !metadata.same_protection(&protection) {
            return Ok(false);
        }
        let manager = self.manager.clone();
        let client = self.client;
        let context = editor.context.clone();
        let store = editor.store.clone();
        let watch = self.watch.clone();
        bridge.get().check_next_input(Box::new(move |units| {
            if !run.matches(units)
                || watch.generation()? != selected.generation
                || u64::from(store.get().composition_events.get())
                    != selected.composition_generation
            {
                return Err(windows::Win32::Foundation::E_ABORT.into());
            }
            let current = inspect_input(&manager, &context, client)?;
            if current.selection != range
                || !current.same_protection(&protection)
                || watch.generation()? != selected.generation
                || u64::from(store.get().composition_events.get())
                    != selected.composition_generation
            {
                return Err(windows::Win32::Foundation::E_ABORT.into());
            }
            Ok(())
        }))?;
        Ok(true)
    }
}

pub(crate) fn run_tracking_demo(
    manager: &ITfThreadMgr,
    client: u32,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    run(manager, client, RunMode::TrackingDemo)
}
pub(crate) fn exercise_tracking(
    manager: &ITfThreadMgr,
    client: u32,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    for case in [
        "empty",
        "prefix",
        "unicode",
        "replacement",
        "explicit",
        "navigate",
        "selection-restored",
        "external-restored",
        "composition",
        "private",
        "private-restored",
        "private-input",
        "readonly",
        "denied",
        "focus",
        "capacity",
        "overflow",
        "escape",
        "enter",
        "edit",
        "gate-text",
        "gate-reentry",
        "gate-range",
        "gate-composition",
        "gate-scope",
    ] {
        run(manager, client, RunMode::TrackingTest(case))?;
    }
    Ok(())
}
pub(super) fn test_tracking(
    window: &crate::window_host::Window,
    editor: &Editor,
    live: &crate::live_render::Binding,
    client: u32,
    case: &str,
) -> Result<()> {
    let store = editor.store.get();
    let (broker, bridge) = editor.broker.as_ref().ok_or(E_FAIL)?;
    let tracking = editor.tracking.as_ref().ok_or(E_FAIL)?;
    let initial = match case {
        "empty" | "capacity" | "overflow" => "",
        "replacement" => "prefix OLD suffix",
        _ => "prefix ",
    };
    store.external_text(initial.encode_utf16().collect())?;
    let start = if initial.is_empty() { 0 } else { 7 };
    store.host_selection(start, if case == "replacement" { 10 } else { start })?;
    pump(window)?;
    if case == "private-input" {
        set_scope(editor, IS_PRIVATE)?;
    }
    let typed = if case == "unicode" {
        "に😀本"
    } else {
        "nihongo"
    };
    if matches!(case, "capacity" | "overflow") {
        editor.input(WM_CHAR, WPARAM('a' as usize), LPARAM(1024))?;
        if case == "overflow" {
            editor.input(WM_CHAR, WPARAM('b' as usize), LPARAM(1))?;
        }
    } else {
        for unit in typed.encode_utf16() {
            editor.input(WM_CHAR, WPARAM(unit.into()), LPARAM(1))?;
        }
    }
    pump(window)?;
    let original = store.text.borrow().clone();
    let caret = store.selection.get().acpEnd as usize;
    assert_eq!(
        caret,
        start
            + if matches!(case, "capacity" | "overflow") {
                // The host's own field limit rejects the extra input once;
                // tracking must clear without changing/replaying existing text.
                1024
            } else {
                typed.encode_utf16().count()
            }
    );
    store.undo.borrow_mut().clear();
    match case {
        "navigate" => {
            editor.input(WM_KEYDOWN, WPARAM(0x24), LPARAM(1))?;
        }
        "selection-restored" => {
            store.host_selection(0, 0)?;
            pump(window)?;
            store.host_selection(caret, caret)?;
        }
        "external-restored" => {
            let mut changed = original.clone();
            changed[0] = 'P' as u16;
            store.external_text(changed)?;
            pump(window)?;
            store.external_text(original.clone())?;
        }
        "composition" => {
            super::super::composition_host::empty_cycle(&editor.context, client)?;
        }
        "private" => {
            set_scope(editor, IS_PRIVATE)?;
        }
        "private-restored" => {
            set_scope(editor, IS_PRIVATE)?;
            pump(window)?;
            set_scope(editor, IS_TEXT)?;
        }
        "private-input" => {
            set_scope(editor, IS_TEXT)?;
        }
        "readonly" => store.readonly.set(true),
        "denied" => store.deny_lock.set(true),
        "focus" => {
            editor.input(WM_KILLFOCUS, WPARAM(0), LPARAM(0))?;
        }
        "explicit" => {
            editor.input(WM_KEYDOWN, WPARAM(0x24), LPARAM(1))?;
            store.host_selection(start, caret)?;
        }
        _ => {}
    }
    pump(window)?;
    if case.starts_with("gate-") {
        assert!(tracking.select(editor, bridge)?, "{case}");
        if matches!(case, "gate-text" | "gate-reentry") {
            // The real transport must retire both checker and reservation on
            // rejection, so a second Start cannot use that reservation.
            let transport = bridge.to_interface::<ISpikeBrokerTransport>();
            let mut id = 0;
            let wrong: Vec<u16> = "NIHONGO".encode_utf16().collect();
            let right: Vec<u16> = typed.encode_utf16().collect();
            if case == "gate-reentry" {
                bridge.get().clear_input_check();
                let checking_bridge = bridge.clone();
                let request = Rc::new(Cell::new(0));
                let nested_request = request.clone();
                let called = Rc::new(Cell::new(false));
                let nested_called = called.clone();
                bridge.get().check_next_input(Box::new(move |units| {
                    let transport = checking_bridge.to_interface::<ISpikeBrokerTransport>();
                    let mut new_id = 0;
                    unsafe {
                        let reserved = transport.Reserve(&mut new_id);
                        let started = transport.Start(
                            nested_request.get(),
                            1,
                            1,
                            1,
                            3000,
                            units.as_ptr(),
                            units.len() as u32,
                        );
                        nested_called.set(reserved.is_err() && new_id == 0 && started.is_err());
                    }
                    Err(windows::Win32::Foundation::E_ABORT.into())
                }))?;
                unsafe {
                    transport.Reserve(&mut id).ok()?;
                    request.set(id);
                    assert!(transport
                        .Start(id, 1, 1, 1, 3000, right.as_ptr(), right.len() as u32)
                        .is_err());
                }
                assert!(
                    called.get(),
                    "reentrant Reserve and Start must both be rejected"
                );
            } else {
                unsafe {
                    transport.Reserve(&mut id).ok()?;
                    assert!(transport
                        .Start(id, 1, 1, 1, 3000, wrong.as_ptr(), wrong.len() as u32)
                        .is_err());
                }
            }
            unsafe {
                assert!(!bridge.get().active());
                assert!(transport
                    .Start(id, 1, 1, 1, 3000, right.as_ptr(), right.len() as u32)
                    .is_err());
            }
        } else {
            match case {
                "gate-range" => store.host_selection(start + 1, caret)?,
                "gate-composition" => {
                    super::super::composition_host::empty_cycle(&editor.context, client)?
                }
                "gate-scope" => set_scope(editor, IS_CHAT)?,
                _ => unreachable!(),
            }
            let transport = bridge.to_interface::<ISpikeBrokerTransport>();
            unsafe {
                broker
                    .Attach(editor.context.as_raw(), transport.as_raw())
                    .ok()?;
                let _ = editor
                    .keys
                    .SimulatePreservedKey(&editor.context, &COMMAND_CONVERT)?;
            }
        }
        bridge.get().clear_input_check();
        assert!(!bridge.get().active(), "{case}: no worker may start");
        editor.input(WM_KEYDOWN, WPARAM(0x76), LPARAM(1))?;
        assert!(
            !bridge.get().active(),
            "{case}: no automatic-selection fallback"
        );
        assert_eq!(*store.text.borrow(), original, "{case}");
        assert!(store.undo.borrow().is_empty(), "{case}");
        set_scope(editor, IS_TEXT)?;
        println!("Tracked input / {case}: OK");
        return Ok(());
    }
    editor.input(WM_KEYDOWN, WPARAM(0x76), LPARAM(1))?;
    let rejected = matches!(
        case,
        "navigate"
            | "selection-restored"
            | "external-restored"
            | "composition"
            | "private"
            | "private-restored"
            | "private-input"
            | "readonly"
            | "denied"
            | "focus"
            | "overflow"
    );
    // Restore fixtures before assertions/teardown; original input is never
    // undone or replayed merely because metadata/tracking was unavailable.
    store.readonly.set(false);
    store.deny_lock.set(false);
    if rejected {
        set_scope(editor, IS_TEXT)?;
        assert!(
            !bridge.get().active(),
            "{case}: stale input must not be submitted"
        );
        editor.input(WM_KEYDOWN, WPARAM(0x76), LPARAM(1))?;
        assert!(
            !bridge.get().active(),
            "{case}: retry must not infer whole field"
        );
        assert_eq!(*store.text.borrow(), original, "{case}");
        assert!(store.undo.borrow().is_empty(), "{case}");
        println!("Tracked input / {case}: OK");
        return Ok(());
    }
    assert!(
        bridge.get().active(),
        "{case}: typed run must start a broker request"
    );
    // Route pending input through the owner message loop, including the
    // host's normal TranslateMessage -> WM_CHAR path for Enter.
    let pending_input = match case {
        "escape" => Some((WM_KEYDOWN, 0x1b)),
        "enter" => Some((WM_KEYDOWN, 0x0d)),
        "edit" => Some((WM_CHAR, 'x' as usize)),
        _ => None,
    };
    if let Some((message, key)) = pending_input {
        unsafe {
            PostMessageW(Some(window.hwnd()), message, WPARAM(key), LPARAM(1))?;
        }
    }
    let applied = !matches!(case, "escape" | "enter" | "edit");
    wait_result(window, broker, bridge, applied)?;
    let expected = if applied {
        let mut next = initial.encode_utf16().collect::<Vec<_>>();
        next.splice(
            start..if case == "replacement" { 10 } else { start },
            "日本語".encode_utf16(),
        );
        next
    } else if case == "edit" {
        let mut next = original.clone();
        next.splice(start..caret, ['x' as u16]);
        next
    } else {
        original.clone()
    };
    pump(window)?;
    assert_eq!(*store.text.borrow(), expected, "{case}");
    assert_eq!(live.scene()?.text, expected, "{case}");
    assert_eq!(editor.submits.get(), usize::from(case == "enter"));
    if applied {
        assert_eq!(store.undo.borrow().len(), 1, "{case}: one conversion undo");
        // Auto-selected text cannot become a new explicit selection by retry.
        if case != "explicit" {
            editor.input(WM_KEYDOWN, WPARAM(0x76), LPARAM(1))?;
            assert!(!bridge.get().active(), "{case}: single-use input run");
        }
        editor.input(WM_CHAR, WPARAM(0x1a), LPARAM(1))?;
        pump(window)?;
        assert_eq!(
            *store.text.borrow(),
            original,
            "{case}: undo preserves prefix/suffix"
        );
    } else if case != "edit" {
        assert!(store.undo.borrow().is_empty(), "{case}");
    }
    println!("Tracked input / {case}: OK");
    Ok(())
}

pub(super) fn set_scope(editor: &Editor, scope: InputScope) -> Result<()> {
    *editor.store.get().scopes.borrow_mut() = ScopeSpec::List(vec![scope]);
    let sink = editor.store.get().sink.borrow().clone().ok_or(E_FAIL)?;
    let length = editor.store.get().text.borrow().len() as i32;
    unsafe { sink.OnAttrsChange(0, length, &[GUID_PROP_INPUTSCOPE]) }
}

pub(super) fn wait_result(
    window: &crate::window_host::Window,
    broker: &ISpikeBroker,
    bridge: &ComObject<Bridge>,
    applied: bool,
) -> Result<()> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
    loop {
        pump(window)?;
        if let Some(delivery) = bridge.get().drain(broker)? {
            assert_eq!(delivery == crate::broker_bridge::Delivery::Applied, applied);
            return Ok(());
        }
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return Err(E_FAIL.into());
        }
        let handle = bridge.get().wait_handle().ok_or(E_FAIL)?;
        let wait = unsafe {
            MsgWaitForMultipleObjectsEx(
                Some(&[handle]),
                remaining.as_millis() as u32,
                QS_ALLINPUT,
                MWMO_INPUTAVAILABLE,
            )
        };
        if wait == windows::Win32::Foundation::WAIT_FAILED
            || wait == windows::Win32::Foundation::WAIT_TIMEOUT
        {
            return Err(E_FAIL.into());
        }
    }
}

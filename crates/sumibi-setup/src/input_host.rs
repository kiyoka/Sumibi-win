//! Keyboard-only, single-line synthetic editor. Never attached to user documents.
use super::*;
use crate::broker_bridge::Bridge;
use std::ops::Range;
use sumibi_core::grapheme::{Boundaries, Direction};
use sumibi_tip::spike::{ISpikeBroker, ISpikeBrokerTransport, FUNCTION_TEST_BROKER};
use sumibi_tip::spike::{ISpikeControl, COMMAND_CONVERT, FUNCTION_TEST_CONTROL};
use windows::Win32::Foundation::{E_ACCESSDENIED, LPARAM, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL, VK_MENU, VK_SHIFT};
use windows::Win32::UI::WindowsAndMessaging::*;

#[path = "tracking_host.rs"]
mod tracking_host;
pub(crate) use tracking_host::{exercise_tracking, run_tracking_demo};
#[path = "composing_host.rs"]
mod composing_host;
pub(crate) use composing_host::{exercise_backspace, exercise_notifications};
pub(crate) use composing_host::{exercise_composing, run_composing_demo};
pub(crate) use composing_host::{exercise_transactions, run_transaction_demo};
#[path = "input_recovery_host.rs"]
mod input_recovery_host;
pub(crate) use input_recovery_host::{exercise_recovery, run_recovery_demo};
#[path = "grapheme_host.rs"]
mod grapheme_host;
pub(crate) use grapheme_host::exercise_graphemes;

impl TextStore {
    fn boundary(&self, position: usize) -> bool {
        let text = self.text.borrow();
        position <= text.len()
            && (position == 0
                || position == text.len()
                || !((0xd800..=0xdbff).contains(&text[position - 1])
                    && (0xdc00..=0xdfff).contains(&text[position])))
    }
    fn host_selection(&self, start: usize, end: usize) -> Result<()> {
        self.select_caret(start, end)
    }
    fn select_caret(&self, anchor: usize, caret: usize) -> Result<()> {
        if self.lock.get() != 0 {
            return Err(TS_E_NOLOCK.into());
        }
        if !self.boundary(anchor) || !self.boundary(caret) {
            return Err(E_INVALIDARG.into());
        }
        let mut selection = self.selection.get();
        selection.acpStart = anchor.min(caret) as i32;
        selection.acpEnd = anchor.max(caret) as i32;
        selection.style.ase = if caret < anchor {
            TS_AE_START
        } else {
            TS_AE_END
        };
        self.selection.set(selection);
        let sink = self.sink.borrow().clone();
        // SAFETY: host-originated selection change, no RefCell borrow held.
        if let Some(sink) = sink {
            unsafe {
                sink.OnSelectionChange()?;
            }
        }
        Ok(())
    }
    fn navigate(&self, key: usize, extend: bool, repeats: usize) -> Result<()> {
        let selected = self.selection.get();
        let start = selected.acpStart as usize;
        let end = selected.acpEnd as usize;
        let backwards = selected.style.ase == TS_AE_START;
        let caret = if backwards { start } else { end };
        let anchor = if backwards { end } else { start };
        let map = Boundaries::new(&self.text.borrow()).ok_or(E_INVALIDARG)?;
        let repeats = repeats.min(MAX_SELECTION_UNITS);
        if repeats == 0 {
            return Err(E_INVALIDARG.into());
        }
        let caret = match key {
            0x24 => 0,
            0x23 => map.end(),
            0x25 | 0x27 => {
                let direction = if key == 0x25 {
                    Direction::Backward
                } else {
                    Direction::Forward
                };
                if !extend && start != end {
                    let edge = if key == 0x25 { start } else { end };
                    let snapped = map.snap(edge, direction).ok_or(E_INVALIDARG)?;
                    if repeats == 1 {
                        snapped
                    } else {
                        map.move_caret(snapped, direction, repeats - 1)
                            .ok_or(E_INVALIDARG)?
                    }
                } else {
                    map.move_caret(caret, direction, repeats)
                        .ok_or(E_INVALIDARG)?
                }
            }
            _ => return Err(E_INVALIDARG.into()),
        };
        self.select_caret(if extend { anchor } else { caret }, caret)
    }
    fn delete_forward(&self, repeats: usize) -> Result<()> {
        self.delete(Direction::Forward, repeats)
    }
    fn backspace(&self, repeats: usize) -> Result<()> {
        self.delete(Direction::Backward, repeats)
    }
    fn delete(&self, direction: Direction, repeats: usize) -> Result<()> {
        let selected = self.selection.get();
        // Plan the entire original operation once. Recomputing after each
        // repeat could delete beyond an explicit selection and split Undo.
        // No text borrow survives into the edit or external notifications.
        let map = Boundaries::new(&self.text.borrow()).ok_or(E_INVALIDARG)?;
        let (start, end) = map
            .deletion(
                (selected.acpStart as usize, selected.acpEnd as usize),
                direction,
                repeats.min(MAX_SELECTION_UNITS),
            )
            .ok_or(E_INVALIDARG)?;
        if start == end {
            return Ok(());
        }
        self.user_edit(start..end, &[])
    }
    fn user_edit(&self, range: Range<usize>, replacement: &[u16]) -> Result<()> {
        if self.readonly.get() {
            return Err(E_ACCESSDENIED.into());
        }
        if self.lock.get() != 0 {
            return Err(TS_E_NOLOCK.into());
        }
        if range.start > range.end
            || !self.boundary(range.start)
            || !self.boundary(range.end)
            || String::from_utf16(replacement).is_err()
        {
            return Err(E_INVALIDARG.into());
        }
        let previous = self.text.borrow().clone();
        if previous.len() - range.len() + replacement.len() > MAX_SELECTION_UNITS {
            return Err(E_INVALIDARG.into());
        }
        let selection = self.selection.get();
        let mut next = previous.clone();
        next.splice(range.clone(), replacement.iter().copied());
        self.undo.borrow_mut().push((previous, selection));
        self.redo.borrow_mut().clear();
        *self.text.borrow_mut() = next;
        let caret = range.start + replacement.len();
        let mut selected = selection;
        selected.acpStart = caret as i32;
        selected.acpEnd = caret as i32;
        selected.style.ase = TS_AE_END;
        self.selection.set(selected);
        let sink = self.sink.borrow().clone();
        // SAFETY: host changed its own text outside a TSF lock. Notify only
        // after text and selection are consistent and all borrows are released.
        if let Some(sink) = sink {
            unsafe {
                sink.OnTextChange(
                    TS_ST_NONE,
                    &TS_TEXTCHANGE {
                        acpStart: range.start as i32,
                        acpOldEnd: range.end as i32,
                        acpNewEnd: caret as i32,
                    },
                )?;
                sink.OnSelectionChange()?;
            }
        }
        Ok(())
    }
    fn type_text(&self, text: &[u16]) -> Result<()> {
        let selected = self.selection.get();
        self.user_edit(selected.acpStart as usize..selected.acpEnd as usize, text)
    }
}

struct Editor {
    store: ComObject<TextStore>,
    context: ITfContext,
    keys: ITfKeystrokeMgr,
    control: ISpikeControl,
    high: Cell<Option<(u16, usize)>>,
    submits: Cell<usize>,
    broker: Option<(ISpikeBroker, ComObject<Bridge>)>,
    tracking: Option<tracking_host::Tracking>,
    composing: Option<composing_host::Composing>,
    transaction: bool,
    input_disposition: Cell<u32>,
    rescue: input_recovery_host::Rescue,
}
impl Editor {
    fn delete_forward(&self, repeats: usize) -> Result<()> {
        if repeats == 0 {
            return Err(E_INVALIDARG.into());
        }
        let selected = self.store.get().selection.get();
        if self.transaction && selected.acpStart != selected.acpEnd {
            let disposition = if let Some(ticket) = self
                .rescue
                .reserve(sumibi_core::recovery::Operation::DeleteSelection)
            {
                let disposition = self
                    .composing
                    .as_ref()
                    .ok_or(E_FAIL)?
                    .delete_selection(self)
                    .unwrap_or(sumibi_tip::spike::INPUT_UNCERTAIN);
                self.rescue.finish(ticket, disposition, &[]);
                disposition
            } else {
                self.clear_tracking();
                sumibi_tip::spike::INPUT_NOT_ATTEMPTED
            };
            if self.handled_transaction(disposition) {
                return Ok(());
            }
        }
        self.store.get().delete_forward(repeats)
    }
    fn handled_transaction(&self, disposition: u32) -> bool {
        self.input_disposition.set(disposition);
        if disposition == sumibi_tip::spike::INPUT_NOT_ATTEMPTED {
            return false;
        }
        if disposition != sumibi_tip::spike::INPUT_APPLIED {
            self.store.get().input_problem.set(true);
        }
        true
    }
    fn clear_tracking(&self) {
        if let Some(composing) = &self.composing {
            composing.cancel(self, false);
        }
        if let Some(tracking) = &self.tracking {
            tracking.clear();
        }
    }
    fn insert(&self, text: &[u16]) -> Result<()> {
        if self.transaction {
            let Some(ticket) = self
                .rescue
                .reserve(sumibi_core::recovery::Operation::Insert(text.len()))
            else {
                self.clear_tracking();
                self.input_disposition
                    .set(sumibi_tip::spike::INPUT_NOT_ATTEMPTED);
                return self.store.get().type_text(text);
            };
            let disposition = self
                .composing
                .as_ref()
                .ok_or(E_FAIL)?
                .insert(self, text)
                .unwrap_or(sumibi_tip::spike::INPUT_UNCERTAIN);
            self.rescue.finish(ticket, disposition, text);
            if self.handled_transaction(disposition) {
                return Ok(());
            }
            // Only an explicit not-attempted result permits the original host
            // input once. Do not attach tracking to a denied transaction.
            return self.store.get().type_text(text);
        }
        let before = self
            .tracking
            .as_ref()
            .and_then(|tracking| tracking.observe(self));
        let selection = self.store.get().selection.get();
        let (start, end) = self
            .store
            .get()
            .bounds(selection.acpStart, selection.acpEnd)?;
        let length = self.store.get().text.borrow().len();
        if length - (end - start) + text.len() > MAX_SELECTION_UNITS {
            self.clear_tracking();
            return Ok(());
        }
        let recorded = self
            .composing
            .as_ref()
            .is_some_and(|composing| composing.before(self, false));
        let result = self.store.get().type_text(text);
        if let Some(composing) = &self.composing {
            composing.after(self, text, recorded && result.is_ok());
        }
        if let Some(tracking) = &self.tracking {
            if result.is_ok() {
                tracking.inserted(self, before, text);
            } else {
                tracking.clear();
            }
        }
        result
    }
    fn input(&self, message: u32, key: WPARAM, flags: LPARAM) -> Result<bool> {
        if message == WM_CLOSE {
            self.rescue.close();
            self.clear_tracking();
            self.high.set(None);
            // SAFETY: revoke this editor's owner-thread grant before its view
            // is destroyed, so cleanup still has a live native input target.
            unsafe {
                self.control.RevokeContext().ok()?;
            }
            return Ok(false);
        }
        if crate::window_host::input_boundary(message, key) {
            self.rescue.invalidate();
            let boundary = self
                .composing
                .as_ref()
                .map_or(Ok(()), |composing| composing.owner_boundary());
            if let Some(tracking) = &self.tracking {
                tracking.clear();
            }
            self.high.set(None);
            if self.broker.is_some() {
                unsafe {
                    self.control.RevokeContext().ok()?;
                }
            }
            boundary?;
            return Ok(false);
        }
        if message == WM_KEYUP {
            // SAFETY: normal TSF application key routing on the owning thread.
            return Ok(unsafe {
                self.keys.TestKeyUp(key, flags).and_then(|test| {
                    if test.as_bool() {
                        self.keys.KeyUp(key, flags)
                    } else {
                        Ok(false.into())
                    }
                })
            }
            .map(|value| value.as_bool())
            .unwrap_or(false));
        }
        if message == WM_KEYDOWN {
            if self.transaction && matches!(key.0, 0x77..=0x79) {
                self.rescue.request(key.0);
                return Ok(true);
            }
            if self.high.take().is_some() {
                self.clear_tracking();
            }
            // SAFETY: test first; only invoke KeyDown when TSF requests it.
            let consumed = unsafe {
                self.keys.TestKeyDown(key, flags).and_then(|test| {
                    if test.as_bool() {
                        self.keys.KeyDown(key, flags)
                    } else {
                        Ok(false.into())
                    }
                })
            }
            .map(|value| value.as_bool())
            .unwrap_or(false);
            if consumed {
                return Ok(true);
            }
            if let Some((broker, _)) = &self.broker {
                if matches!(key.0, 0x1b | 0x0d) {
                    self.clear_tracking();
                    if let Some(composing) = &self.composing {
                        composing.cancel(self, true);
                    }
                    let mut pending = 0;
                    unsafe {
                        let _ = broker.Pending(&mut pending);
                        let _ = broker.Cancel(0);
                    }
                    if key.0 == 0x1b && pending != 0 {
                        return Ok(true);
                    }
                }
            }
            if matches!(key.0, 0x23..=0x25 | 0x27 | 0x2e) {
                self.clear_tracking();
                if let Some(composing) = &self.composing {
                    composing.cancel(self, true);
                }
                if let Some(tracking) = &self.tracking {
                    tracking.user_boundary();
                }
                // SAFETY: read the calling thread's keyboard state; never modify it.
                let (shift, modified) = unsafe {
                    (
                        GetKeyState(VK_SHIFT.0 as i32) < 0,
                        GetKeyState(VK_CONTROL.0 as i32) < 0 || GetKeyState(VK_MENU.0 as i32) < 0,
                    )
                };
                if modified {
                    return Ok(false);
                }
                let repeats = (flags.0 as usize & 0xffff).max(1);
                if key.0 == 0x2e {
                    if !shift && !self.store.get().readonly.get() {
                        self.delete_forward(repeats)?;
                    }
                } else {
                    self.store.get().navigate(key.0, shift, repeats)?;
                }
                return Ok(true);
            }
            match key.0 {
                0x75 => {
                    if self.broker.is_some() {
                        return Ok(true);
                    }
                    // F6: explicitly grant only this editor's own context.
                    if self.store.get().text.borrow().is_empty() {
                        return Ok(true);
                    }
                    let selected = self.store.get().selection.get();
                    if selected.acpStart == selected.acpEnd {
                        let length = self.store.get().text.borrow().len();
                        self.store.get().host_selection(0, length)?;
                    }
                    unsafe {
                        self.control.GrantContext(self.context.as_raw()).ok()?;
                        self.control.BeginPreview().ok()?;
                    }
                    return Ok(true);
                }
                0x76 => {
                    if let Some((broker, bridge)) = &self.broker {
                        if bridge.get().active() || self.store.get().text.borrow().is_empty() {
                            return Ok(true);
                        }
                        let selected = self.store.get().selection.get();
                        if let Some(composing) = &self.composing {
                            if !composing.prepare(self, bridge)? {
                                return Ok(true);
                            }
                        } else if selected.acpStart == selected.acpEnd {
                            if let Some(tracking) = &self.tracking {
                                if !tracking.select(self, bridge)? {
                                    return Ok(true);
                                }
                            } else {
                                let length = self.store.get().text.borrow().len();
                                self.store.get().host_selection(0, length)?;
                            }
                        } else {
                            if self
                                .tracking
                                .as_ref()
                                .is_some_and(|tracking| tracking.blocks_selection())
                            {
                                return Ok(true);
                            }
                            self.clear_tracking();
                        }
                        let transport = bridge.to_interface::<ISpikeBrokerTransport>();
                        // Explicit F7 opt-in for this owned ACP context only.
                        unsafe {
                            let attached = broker
                                .Attach(self.context.as_raw(), transport.as_raw())
                                .ok();
                            if let Err(error) = attached {
                                bridge.get().clear_input_check();
                                if let Some(composing) = &self.composing {
                                    composing.cancel(self, false);
                                }
                                return Err(error);
                            }
                        }
                    }
                    // SAFETY: same-thread command for this editor's context;
                    // no keyboard event is sent to the desktop or other apps.
                    let result = unsafe {
                        self.keys
                            .SimulatePreservedKey(&self.context, &COMMAND_CONVERT)
                    };
                    if let Some((_, bridge)) = &self.broker {
                        bridge.get().clear_input_check();
                    }
                    if let Some(composing) = &self.composing {
                        composing.cancel(self, false);
                    }
                    let _ = result?;
                    return Ok(true);
                }
                _ => return Ok(false),
            }
        }
        if message != WM_CHAR {
            return Ok(false);
        }
        if let Some((broker, _)) = &self.broker {
            unsafe {
                let _ = broker.Cancel(0);
            }
        }
        let Ok(unit) = u16::try_from(key.0) else {
            self.clear_tracking();
            self.high.set(None);
            return Ok(true);
        };
        let repeats = (flags.0 as usize & 0xffff).max(1);
        if repeats > MAX_SELECTION_UNITS {
            self.clear_tracking();
            self.high.set(None);
            return Ok(true);
        }
        if (0xd800..=0xdbff).contains(&unit) {
            if self.high.get().is_some() {
                self.clear_tracking();
            }
            self.high.set(Some((unit, repeats)));
            return Ok(true);
        }
        let previous = self.high.take();
        if self.store.get().readonly.get() {
            self.clear_tracking();
            return Ok(true);
        }
        if (0xdc00..=0xdfff).contains(&unit) {
            if let Some((high, count)) = previous {
                if count == repeats && count <= MAX_SELECTION_UNITS / 2 {
                    self.insert(&[high, unit].repeat(count))?;
                } else {
                    self.clear_tracking();
                }
            } else {
                self.clear_tracking();
            }
            return Ok(true);
        }
        if (unit < 0x20 || unit == 0x7f) && !(unit == 0x08 && self.composing.is_some()) {
            self.clear_tracking();
            if let Some(composing) = &self.composing {
                composing.cancel(self, true);
            }
            if let Some(tracking) = &self.tracking {
                tracking.user_boundary();
            }
        }
        if previous.is_some() {
            self.clear_tracking();
        }
        match unit {
            0x01 => {
                let length = self.store.get().text.borrow().len();
                self.store.get().host_selection(0, length)?;
            }
            0x08 => {
                if self.transaction {
                    let ticket = self
                        .rescue
                        .reserve(sumibi_core::recovery::Operation::Backspace(repeats as u32));
                    let disposition = if let Some(ticket) = ticket {
                        let disposition = self
                            .composing
                            .as_ref()
                            .ok_or(E_FAIL)?
                            .backspace(self, repeats as u32)
                            .unwrap_or(sumibi_tip::spike::INPUT_UNCERTAIN);
                        self.rescue.finish(ticket, disposition, &[]);
                        disposition
                    } else {
                        self.clear_tracking();
                        sumibi_tip::spike::INPUT_NOT_ATTEMPTED
                    };
                    if !self.handled_transaction(disposition) {
                        // Only an explicit no-write outcome permits this
                        // original host operation once, including its repeat.
                        self.store.get().backspace(repeats)?;
                    }
                    return Ok(true);
                }
                let recorded = self
                    .composing
                    .as_ref()
                    .is_some_and(|composing| composing.before(self, true));
                self.store.get().backspace(repeats)?;
                if let Some(composing) = &self.composing {
                    composing.after(self, &[], recorded);
                }
            }
            0x0d => {
                let end = self.store.get().selection.get().acpEnd as usize;
                self.store.get().host_selection(end, end)?;
                self.submits.set(self.submits.get().saturating_add(1));
            } // Single-line confirmation.
            0x19 => {
                if !self.store.get().redo.borrow().is_empty() {
                    self.store.get().redo_once()?;
                }
            }
            0x1a => {
                if !self.store.get().undo.borrow().is_empty() {
                    self.store.get().undo_once()?;
                }
            }
            0..=0x1f | 0x7f => {}
            _ => self.insert(&vec![unit; repeats])?,
        }
        Ok(true)
    }
}

pub(crate) fn exercise_input(
    manager: &ITfThreadMgr,
    client: u32,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    run(manager, client, RunMode::InputTest)
}
pub(crate) fn run_demo(
    manager: &ITfThreadMgr,
    client: u32,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    run(manager, client, RunMode::Demo)
}
pub(crate) fn run_broker_demo(
    manager: &ITfThreadMgr,
    client: u32,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    run(manager, client, RunMode::BrokerDemo)
}
pub(crate) fn exercise_broker_input(
    manager: &ITfThreadMgr,
    client: u32,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    for case in [
        "apply",
        "escape",
        "enter",
        "edit",
        "focus",
        "composition",
        "close",
        "sent-close",
        "window-deactivate",
        "window-disable",
        "window-cancel-mode",
    ] {
        run(manager, client, RunMode::BrokerInputTest(case))?;
    }
    Ok(())
}

pub(crate) fn exercise_close(
    manager: &ITfThreadMgr,
    client: u32,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    for case in ["idle", "preview", "revoked", "cleanup-error"] {
        run(manager, client, RunMode::CloseTest(case))?;
    }
    Ok(())
}

enum RunMode {
    InputTest,
    Demo,
    BrokerDemo,
    BrokerInputTest(&'static str),
    CloseTest(&'static str),
    TrackingDemo,
    TrackingTest(&'static str),
    ComposingDemo,
    ComposingTest(&'static str),
    NotificationTest(&'static str),
    TransactionDemo,
    TransactionTest(&'static str),
}

fn run(
    manager: &ITfThreadMgr,
    client: u32,
    mode: RunMode,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    // SAFETY: this process owns the view, text store, apartment and all interfaces.
    unsafe {
        let window = crate::window_host::Window::new(manager)?;
        window
            .set_title("Sumibi input demo | F6 preview | F7 convert | Ctrl+Z undo | Ctrl+Y redo")?;
        let store = ComObject::new(TextStore::new(""));
        store.get().window.set(window.hwnd());
        let document = Document(manager.CreateDocumentMgr()?);
        let mut context = None;
        let mut cookie = 0;
        document.0.CreateContext(
            client,
            0,
            &store.to_interface::<IUnknown>(),
            &mut context,
            &mut cookie,
        )?;
        let context = context.ok_or_else(|| windows::core::Error::from_hresult(E_FAIL))?;
        document.0.Push(&context)?;
        scope_fixture::initialise_context(&context, client)?;
        let mut association = crate::window_host::Association::new(manager, &window, &document.0)?;
        window
            .focus()
            .map_err(|error| format!("input window focus: {error}"))?;
        manager.SetFocus(&document.0)?;
        let control: ISpikeControl = manager
            .GetFunctionProvider(&sumibi_tip::CLSID_SUMIBI_TIP)?
            .GetFunction(&FUNCTION_TEST_CONTROL, &ISpikeControl::IID)?
            .cast()?;
        let tracking = if matches!(mode, RunMode::TrackingDemo | RunMode::TrackingTest(_)) {
            Some(tracking_host::Tracking::new(manager, &context, client)?)
        } else {
            None
        };
        let broker = if matches!(
            mode,
            RunMode::BrokerDemo
                | RunMode::BrokerInputTest(_)
                | RunMode::TrackingDemo
                | RunMode::TrackingTest(_)
                | RunMode::ComposingDemo
                | RunMode::ComposingTest(_)
                | RunMode::NotificationTest(_)
                | RunMode::TransactionDemo
                | RunMode::TransactionTest(_)
        ) {
            window.set_title("Sumibi 非同期変換デモ（固定結果） | F7 変換 | Esc 取消 | Enter 原文確定 | Ctrl+Z Undo")?;
            let broker: ISpikeBroker = manager
                .GetFunctionProvider(&sumibi_tip::CLSID_SUMIBI_TIP)?
                .GetFunction(&FUNCTION_TEST_BROKER, &ISpikeBroker::IID)?
                .cast()?;
            Some((
                broker,
                Bridge::new(sumibi_ipc::bootstrap::Mode::DiagnosticFixed { delay_ms: 400 })?,
            ))
        } else {
            None
        };
        *store.get().composition_broker.borrow_mut() =
            broker.as_ref().map(|(broker, _)| broker.clone());
        let editor = Rc::new(Editor {
            store: store.clone(),
            context: context.clone(),
            keys: manager.cast()?,
            control,
            high: Cell::new(None),
            submits: Cell::new(0),
            broker,
            tracking,
            transaction: matches!(mode, RunMode::TransactionDemo | RunMode::TransactionTest(_)),
            input_disposition: Cell::new(sumibi_tip::spike::INPUT_NOT_ATTEMPTED),
            rescue: input_recovery_host::Rescue::new(
                manager,
                &context,
                client,
                matches!(mode, RunMode::TransactionDemo | RunMode::TransactionTest(_)),
                matches!(mode, RunMode::TransactionTest("rescue/expire-wake")),
            )?,
            composing: if matches!(
                mode,
                RunMode::ComposingDemo
                    | RunMode::ComposingTest(_)
                    | RunMode::NotificationTest(_)
                    | RunMode::TransactionDemo
                    | RunMode::TransactionTest(_)
            ) {
                Some(composing_host::Composing::new(manager)?)
            } else {
                None
            },
        });
        let input = editor.clone();
        if let Some(composing) = &editor.composing {
            composing.connect(&editor);
        }
        window.set_input(Rc::new(move |message, key, flags| {
            input.input(message, key, flags)
        }))?;
        let mut live = crate::live_render::Binding::attach(&window, &context, client)?;
        let outcome = if matches!(mode, RunMode::Demo) {
            println!("Type in the single-line demo. F6 previews, F7 converts, Enter confirms the original. Close the window to exit.");
            message_loop(&window)
        } else if matches!(
            mode,
            RunMode::BrokerDemo
                | RunMode::TrackingDemo
                | RunMode::ComposingDemo
                | RunMode::TransactionDemo
        ) {
            if matches!(mode, RunMode::TrackingDemo) {
                window.set_title(
                    "Sumibi 入力範囲の変換デモ | F7 入力範囲を変換 | 範囲選択可 | Ctrl+Z Undo",
                )?;
            }
            if matches!(mode, RunMode::ComposingDemo) {
                window.set_title(
                    "Sumibi 入力composition | F7 変換 | Backspace 範囲縮小 | Esc 取消",
                )?;
            }
            if matches!(mode, RunMode::TransactionDemo) {
                window.set_title("Sumibi 同期入力取引 | F7 変換 | Esc 取消 | Ctrl+Z Undo")?;
            }
            if matches!(mode, RunMode::TrackingDemo | RunMode::ComposingDemo) {
                println!("Tracked input demo: F7 converts the recent typed run or an explicit selection. Esc cancels; Enter confirms the original. Close to exit.");
            } else {
                println!("Async fixed-result demo: F7 converts the selection (or all text), Esc cancels, Enter confirms the original. Close to exit.");
            }
            broker_message_loop(&window, &editor)
        } else if let RunMode::BrokerInputTest(case) = mode {
            test_broker_input(&window, &editor, &live, client, case)
        } else if let RunMode::TrackingTest(case) = mode {
            tracking_host::test_tracking(&window, &editor, &live, client, case)
        } else if let RunMode::ComposingTest(case) = mode {
            if let Some(case) = case.strip_prefix("grapheme/") {
                grapheme_host::test_graphemes(&window, &editor, &live, case)
            } else {
                composing_host::test_composing(&window, &editor, &live, client, case)
            }
        } else if let RunMode::NotificationTest(case) = mode {
            composing_host::test_notifications(&window, &editor, &live, client, case)
        } else if let RunMode::TransactionTest(case) = mode {
            if let Some(case) = case.strip_prefix("grapheme/") {
                grapheme_host::test_graphemes(&window, &editor, &live, case)
            } else if let Some(case) = case.strip_prefix("erase/") {
                composing_host::test_backspace(&window, &editor, &live, client, case)
            } else if let Some(case) = case.strip_prefix("rescue/") {
                input_recovery_host::test_recovery(&window, &editor, &live, client, case)
            } else {
                composing_host::test_transactions(&window, &editor, &live, client, case)
            }
        } else if let RunMode::CloseTest(case) = mode {
            test_close(&window, &editor, case)
        } else {
            test_input(&window, &editor, &live)
        };
        window.clear_input();
        editor.rescue.close();
        editor.clear_tracking();
        if let Some(composing) = &editor.composing {
            composing.close(&editor)?;
        }
        store.get().composition_broker.borrow_mut().take();
        if let Some((broker, bridge)) = &editor.broker {
            let _ = broker.Cancel(0);
            bridge.get().shutdown()?;
        }
        live.close()
            .map_err(|error| format!("input render cleanup: {error}"))?;
        // WM_CLOSE already revoked before destruction. A desktop TSF manager
        // can deactivate the TIP during destruction, expiring this weak control.
        // Only revoke here when exiting while the native view is still alive.
        if window.is_open() {
            editor
                .control
                .RevokeContext()
                .ok()
                .map_err(|error| format!("input grant cleanup: {error}"))?;
        }
        association
            .close()
            .map_err(|error| format!("input association cleanup: {error}"))?;
        outcome?;
    }
    Ok(())
}

fn broker_message_loop(window: &crate::window_host::Window, editor: &Editor) -> Result<()> {
    let (control, bridge) = editor.broker.as_ref().ok_or(E_FAIL)?;
    let mut pending = false;
    while window.is_open() {
        pump(window)?;
        if !window.is_open() {
            break;
        }
        if let Some(composing) = &editor.composing {
            composing.poll(editor)?;
        }
        if let Some(delivery) = bridge.get().drain(control)? {
            window.set_title(if delivery == crate::broker_bridge::Delivery::Applied {
                "Sumibi 変換完了（固定結果） | F7 変換 | Ctrl+Z Undo"
            } else {
                "Sumibi 原文を保持しました | F7 変換 | Esc 取消"
            })?;
            pending = false;
        }
        let active = bridge.get().active();
        if active && !pending {
            window.set_title("Sumibi 変換中（固定結果） | Esc 取消 | Enter 原文確定")?;
        }
        pending = active;
        if editor.store.get().input_problem.get() {
            window.set_title(
                "Sumibi 入力取引に失敗しました。入力結果を確認してください（自動再入力なし）",
            )?;
        }
        editor.service_recovery(window, &crate::recovery_view::SystemClipboard)?;
        let handles: Vec<_> = bridge
            .get()
            .wait_handle()
            .into_iter()
            .chain(
                editor
                    .composing
                    .as_ref()
                    .and_then(|composing| composing.wait_handle()),
            )
            .collect();
        // No IPC wait or result callback inside WndProc. Sent close/focus
        // messages are handled by the owner and wake the ordinary message path.
        let wait = unsafe {
            MsgWaitForMultipleObjectsEx(
                Some(&handles),
                editor.rescue.wait_ms(),
                QS_ALLINPUT,
                MWMO_INPUTAVAILABLE,
            )
        };
        if wait == windows::Win32::Foundation::WAIT_FAILED {
            return Err(windows::core::Error::from_thread());
        }
    }
    window.input_status()
}

fn test_broker_input(
    window: &crate::window_host::Window,
    editor: &Editor,
    live: &crate::live_render::Binding,
    client: u32,
    case: &str,
) -> Result<()> {
    use std::time::{Duration, Instant};
    let (control, bridge) = editor.broker.as_ref().ok_or(E_FAIL)?;
    editor.insert(&"nihongo".encode_utf16().collect::<Vec<_>>())?;
    editor.store.get().undo.borrow_mut().clear();
    pump(window)?;
    editor.input(WM_KEYDOWN, WPARAM(0x76), LPARAM(1))?;
    assert!(bridge.get().active());
    let captured = composing_host::input_notify_host::body_reads()?;
    if let Some(kind) = case.strip_prefix("window-") {
        test_window_boundary(window, kind)?;
        let mut pending = 1;
        unsafe {
            control.Pending(&mut pending).ok()?;
        }
        assert_eq!(
            pending, 0,
            "window boundary revokes pending target before delivery"
        );
        assert_eq!(composing_host::input_notify_host::body_reads()?, captured);
    }
    if case == "composition" {
        super::composition_host::empty_cycle(&editor.context, client)?;
        assert!(editor.store.get().composition_events.get() >= 2);
        let mut pending = 1;
        unsafe {
            control.Pending(&mut pending).ok()?;
        }
        assert_eq!(
            pending, 0,
            "owner callback must retire even an empty composition cycle"
        );
    }
    if case == "sent-close" {
        test_broker_sent_close(window, editor)?;
        assert_eq!(
            *editor.store.get().text.borrow(),
            "nihongo".encode_utf16().collect::<Vec<_>>()
        );
        assert!(editor.store.get().undo.borrow().is_empty());
        println!("Async input loop / pending F7 / sent SC_CLOSE: OK");
        return Ok(());
    }
    let post = |message, key| unsafe {
        PostMessageW(Some(window.hwnd()), message, WPARAM(key), LPARAM(1))
    };
    match case {
        "escape" => post(WM_KEYDOWN, 0x1b)?,
        "enter" => post(WM_KEYDOWN, 0x0d)?,
        "edit" => post(WM_CHAR, 'x' as usize)?,
        "focus" => post(WM_KILLFOCUS, 0)?,
        "close" => post(WM_CLOSE, 0)?,
        _ => {}
    }
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        pump(window)?;
        if !window.is_open() {
            bridge.get().shutdown()?;
            break;
        }
        if let Some(delivery) = bridge.get().drain(control)? {
            assert_eq!(
                delivery == crate::broker_bridge::Delivery::Applied,
                case == "apply",
                "{case}"
            );
            break;
        }
        let handle = bridge.get().wait_handle().ok_or(E_FAIL)?;
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(E_FAIL.into());
        }
        let wait = unsafe {
            MsgWaitForMultipleObjectsEx(
                Some(&[handle]),
                remaining.as_millis() as u32,
                QS_ALLINPUT,
                MWMO_INPUTAVAILABLE,
            )
        };
        if wait == windows::Win32::Foundation::WAIT_TIMEOUT
            || wait == windows::Win32::Foundation::WAIT_FAILED
        {
            return Err(E_FAIL.into());
        }
    }
    pump(window)?;
    let expected = match case {
        "apply" => "日本語",
        "edit" => "x",
        _ => "nihongo",
    };
    assert_eq!(
        *editor.store.get().text.borrow(),
        expected.encode_utf16().collect::<Vec<_>>(),
        "{case}"
    );
    assert_eq!(
        editor.submits.get(),
        usize::from(case == "enter"),
        "Enter must reach host exactly once"
    );
    if case == "apply" {
        assert_eq!(editor.store.get().undo.borrow().len(), 1);
        assert_eq!(
            live.scene()?.text,
            "日本語".encode_utf16().collect::<Vec<_>>()
        );
        editor.input(WM_CHAR, WPARAM(0x1a), LPARAM(1))?;
        pump(window)?;
        assert_eq!(
            *editor.store.get().text.borrow(),
            "nihongo".encode_utf16().collect::<Vec<_>>()
        );
    } else if case != "edit" {
        assert!(editor.store.get().undo.borrow().is_empty());
    }
    if case.starts_with("window-") {
        assert_eq!(
            composing_host::input_notify_host::body_reads()?,
            captured,
            "late response after a restored window boundary cannot reread the body"
        );
    }
    println!("Async input loop / F7 / {case}: OK");
    Ok(())
}

fn message_loop(window: &crate::window_host::Window) -> Result<()> {
    let mut message = MSG::default();
    while window.is_open() {
        if !window.get_message(&mut message)? {
            break;
        }
        window.route_message(&message)?;
    }
    window.input_status()
}

fn test_broker_sent_close(window: &crate::window_host::Window, editor: &Editor) -> Result<()> {
    use std::sync::mpsc::sync_channel;
    use std::time::Duration;
    use windows::Win32::Foundation::{E_ABORT, HWND};
    use windows::Win32::System::Threading::GetCurrentThreadId;
    let hwnd = window.hwnd().0 as usize;
    let thread = unsafe { GetCurrentThreadId() };
    std::thread::scope(|scope| {
        let (finished, wait) = sync_channel(1);
        let worker = scope.spawn(move || unsafe {
            let sent = SendMessageTimeoutW(
                HWND(hwnd as *mut _),
                WM_SYSCOMMAND,
                WPARAM(SC_CLOSE as usize),
                LPARAM(0),
                SMTO_ABORTIFHUNG,
                1000,
                None,
            );
            let timed_out = wait.recv_timeout(Duration::from_secs(2)).is_err();
            if timed_out {
                let _ = PostThreadMessageW(thread, WM_QUIT, WPARAM(1), LPARAM(0));
            }
            (sent.0 != 0, timed_out)
        });
        let result = broker_message_loop(window, editor);
        let _ = finished.send(());
        let (sent, timed_out) = worker
            .join()
            .map_err(|_| windows::core::Error::from(E_ABORT))?;
        assert!(
            sent && !timed_out,
            "sent close must release the async demo wait"
        );
        assert!(!window.is_open());
        result
    })
}

fn test_close(window: &crate::window_host::Window, editor: &Rc<Editor>, case: &str) -> Result<()> {
    use std::sync::mpsc::sync_channel;
    use std::time::Duration;
    use windows::Win32::Foundation::{E_ABORT, HWND};
    use windows::Win32::System::Threading::GetCurrentThreadId;
    editor.insert(&"nihongo".encode_utf16().collect::<Vec<_>>())?;
    if matches!(case, "preview" | "revoked") {
        editor.input(WM_KEYDOWN, WPARAM(0x75), LPARAM(1))?;
        if case == "revoked" {
            unsafe {
                editor.control.RevokeContext().ok()?;
            }
        }
    }
    if case == "cleanup-error" {
        window.clear_input();
        let input = editor.clone();
        window.set_input(Rc::new(move |message, key, flags| {
            input.input(message, key, flags)?;
            if message == WM_CLOSE {
                Err(E_ABORT.into())
            } else {
                Ok(false)
            }
        }))?;
    }
    pump(window)?;
    // A system command alone bypasses mouse hit testing. Check the native
    // caption button first so a click-through window cannot pass this test.
    unsafe {
        let mut title = TITLEBARINFOEX {
            cbSize: std::mem::size_of::<TITLEBARINFOEX>() as u32,
            ..Default::default()
        };
        SendMessageW(
            window.hwnd(),
            WM_GETTITLEBARINFOEX,
            None,
            Some(LPARAM((&mut title as *mut TITLEBARINFOEX) as isize)),
        );
        let close = title.rgrect[5];
        assert!(close.right > close.left && close.bottom > close.top);
        let x = (close.left + close.right) / 2;
        let y = (close.top + close.bottom) / 2;
        let point = LPARAM(((x as u16 as u32) | ((y as u16 as u32) << 16)) as isize);
        let hit = SendMessageW(window.hwnd(), WM_NCHITTEST, None, Some(point));
        assert_eq!(
            hit.0, HTCLOSE as isize,
            "caption close must accept mouse input"
        );
    }
    let hwnd = window.hwnd().0 as usize;
    let thread = unsafe { GetCurrentThreadId() };
    let loop_result = std::thread::scope(|scope| {
        let (finished, wait) = sync_channel(1);
        let worker = scope.spawn(move || {
            // SAFETY: only the synthetic view, kept alive until join. Simulate
            // the close button's synchronous system command, not posted WM_CLOSE.
            unsafe {
                let sent = SendMessageTimeoutW(
                    HWND(hwnd as *mut _),
                    WM_SYSCOMMAND,
                    WPARAM(SC_CLOSE as usize),
                    LPARAM(0),
                    SMTO_ABORTIFHUNG,
                    1000,
                    None,
                );
                let timed_out = wait.recv_timeout(Duration::from_secs(2)).is_err();
                if timed_out {
                    // Bound regressions: wake the loop rather than hang CI.
                    let _ = PostThreadMessageW(thread, WM_QUIT, WPARAM(1), LPARAM(0));
                }
                (sent.0 != 0, timed_out)
            }
        });
        let result = message_loop(window);
        let _ = finished.send(());
        let (sent, timed_out) = worker.join().expect("close fixture worker");
        assert!(sent, "close command was not dispatched");
        assert!(!timed_out, "close did not release GetMessage");
        result
    });
    assert!(
        !window.is_open(),
        "close must destroy the view even if cleanup fails"
    );
    if case == "cleanup-error" {
        assert_eq!(loop_result.unwrap_err().code(), E_ABORT);
    } else {
        loop_result?;
    }
    println!("Caption HTCLOSE / sent SC_CLOSE / GetMessage shutdown / {case}: OK");
    Ok(())
}

/// Diagnostic delivery through the real WndProc. Deactivation is a synthetic
/// notice; disable/restore uses EnableWindow and its synchronous OS messages.
fn test_window_boundary(window: &crate::window_host::Window, kind: &str) -> Result<()> {
    unsafe {
        match kind {
            "deactivate" => {
                SendMessageW(window.hwnd(), WM_ACTIVATEAPP, Some(WPARAM(0)), None);
                SendMessageW(window.hwnd(), WM_ACTIVATEAPP, Some(WPARAM(1)), None);
            }
            "disable" => {
                use windows::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, IsWindowEnabled};
                let _ = EnableWindow(window.hwnd(), false);
                assert!(!IsWindowEnabled(window.hwnd()).as_bool());
                let _ = EnableWindow(window.hwnd(), true);
                assert!(IsWindowEnabled(window.hwnd()).as_bool());
            }
            "cancel-mode" => {
                SendMessageW(window.hwnd(), WM_CANCELMODE, None, None);
            }
            _ => return Err(E_INVALIDARG.into()),
        }
    }
    window.input_status()
}

fn pump(window: &crate::window_host::Window) -> Result<()> {
    // Process only this STA's messages through TSF, with a finite test bound.
    let mut message = MSG::default();
    for _ in 0..512 {
        if !window.peek_message(&mut message)? {
            return window.input_status();
        }
        if message.message == WM_QUIT {
            return Err(windows::Win32::Foundation::E_ABORT.into());
        }
        window.route_message(&message)?;
    }
    Err(E_FAIL.into())
}

fn test_input(
    window: &crate::window_host::Window,
    editor: &Editor,
    live: &crate::live_render::Binding,
) -> Result<()> {
    window.require_foreground()?;
    // SAFETY: fixture messages go only to our own window. No SendInput, global
    // keyboard-state changes, or messages to another application are used.
    let post = |message, key| unsafe {
        PostMessageW(Some(window.hwnd()), message, WPARAM(key), LPARAM(1))
    };
    pump(window)?;
    assert!(live.scene()?.text.is_empty());
    assert_eq!(live.scene()?.caret, 0);
    let _ = window.frame()?;
    for unit in "nihongo".encode_utf16() {
        post(WM_CHAR, unit as usize)?;
    }
    pump(window)?;
    assert_eq!(
        live.scene()?.text,
        "nihongo".encode_utf16().collect::<Vec<_>>()
    );
    post(WM_CHAR, 0x08)?;
    pump(window)?;
    assert_eq!(
        live.scene()?.text,
        "nihong".encode_utf16().collect::<Vec<_>>()
    );
    post(WM_CHAR, 0x1a)?;
    pump(window)?;
    assert_eq!(
        live.scene()?.text,
        "nihongo".encode_utf16().collect::<Vec<_>>()
    );
    println!("WM_CHAR typing / Backspace / undo / automatic redraw: OK");
    post(WM_KEYDOWN, 0x75)?;
    pump(window)?;
    assert_eq!(live.scene()?.underlines.len(), 1);
    post(WM_KEYDOWN, 0x1b)?;
    pump(window)?;
    assert!(live.scene()?.underlines.is_empty());
    assert_eq!(editor.submits.get(), 0);
    post(WM_KEYDOWN, 0x75)?;
    pump(window)?;
    post(WM_KEYDOWN, 0x0d)?;
    pump(window)?;
    assert_eq!(editor.submits.get(), 1);
    assert_eq!(editor.store.get().selection.get().acpStart, 7);
    assert_eq!(editor.store.get().selection.get().acpEnd, 7);
    assert_eq!(
        *editor.store.get().text.borrow(),
        "nihongo".encode_utf16().collect::<Vec<_>>()
    );
    assert!(live.scene()?.underlines.is_empty());
    println!("Native message loop: Esc consumed; Enter confirms once without conversion: OK");
    post(WM_KEYDOWN, 0x75)?;
    pump(window)?;
    post(WM_KEYDOWN, 0x76)?;
    pump(window)?;
    assert_eq!(
        live.scene()?.text,
        "日本語".encode_utf16().collect::<Vec<_>>()
    );
    post(WM_CHAR, 0x1a)?;
    pump(window)?;
    assert_eq!(
        live.scene()?.text,
        "nihongo".encode_utf16().collect::<Vec<_>>()
    );
    println!("Native input loop -> preview -> fixed conversion -> one-step undo: OK");
    post(WM_CHAR, 0x01)?;
    for unit in "😀".encode_utf16() {
        post(WM_CHAR, unit as usize)?;
    }
    pump(window)?;
    assert_eq!(live.scene()?.text, "😀".encode_utf16().collect::<Vec<_>>());
    post(WM_CHAR, 0x08)?;
    pump(window)?;
    assert!(editor.store.get().text.borrow().is_empty());
    post(WM_CHAR, 0x1a)?;
    pump(window)?;
    assert_eq!(live.scene()?.text, "😀".encode_utf16().collect::<Vec<_>>());
    post(WM_CHAR, 0xdc00)?;
    pump(window)?;
    assert_eq!(live.scene()?.text, "😀".encode_utf16().collect::<Vec<_>>());
    editor.store.get().readonly.set(true);
    post(WM_CHAR, 'x' as usize)?;
    post(WM_CHAR, 0x08)?;
    pump(window)?;
    assert_eq!(live.scene()?.text, "😀".encode_utf16().collect::<Vec<_>>());
    editor.store.get().readonly.set(false);
    println!("Surrogate pair input / grapheme Backspace / invalid low surrogate / read-only: OK");
    post(WM_KEYDOWN, 0x24)?;
    pump(window)?;
    assert_eq!(live.scene()?.caret, 0);
    post(WM_KEYDOWN, 0x23)?;
    pump(window)?;
    assert_eq!(live.scene()?.caret, 2);
    assert!(editor.store.get().user_edit(1..2, &[]).is_err());
    let before = editor.store.get().text.borrow().clone();
    assert!(editor
        .store
        .get()
        .user_edit(0..0, &vec![b'x' as u16; MAX_SELECTION_UNITS + 1])
        .is_err());
    assert_eq!(*editor.store.get().text.borrow(), before);
    println!("Home/End caret, surrogate boundary and size limit guards: OK");
    post(WM_CHAR, 0x01)?;
    // A single WM_CHAR can represent multiple repeated characters.
    unsafe {
        PostMessageW(
            Some(window.hwnd()),
            WM_CHAR,
            WPARAM('a' as usize),
            LPARAM(3),
        )?;
    }
    pump(window)?;
    assert_eq!(live.scene()?.text, "aaa".encode_utf16().collect::<Vec<_>>());
    post(WM_CHAR, 0x1a)?;
    pump(window)?;
    assert_eq!(live.scene()?.text, before);
    println!("WM_CHAR repeat count applies once and undoes as one edit: OK");
    post(WM_KEYDOWN, 0x24)?;
    post(WM_KEYDOWN, 0x27)?;
    pump(window)?;
    assert_eq!(live.scene()?.caret, 2);
    post(WM_KEYDOWN, 0x25)?;
    pump(window)?;
    assert_eq!(live.scene()?.caret, 0);
    editor.store.get().readonly.set(true);
    post(WM_KEYDOWN, 0x2e)?;
    pump(window)?;
    assert_eq!(live.scene()?.text, before);
    editor.store.get().readonly.set(false);
    post(WM_KEYDOWN, 0x2e)?;
    pump(window)?;
    assert!(live.scene()?.text.is_empty());
    let undo_count = editor.store.get().undo.borrow().len();
    post(WM_KEYDOWN, 0x2e)?;
    pump(window)?;
    assert_eq!(editor.store.get().undo.borrow().len(), undo_count);
    post(WM_CHAR, 0x1a)?;
    pump(window)?;
    assert_eq!(live.scene()?.text, before);
    println!("Native Left/Right and Delete preserve surrogate pairs; read-only and no-op undo: OK");

    // Exercise Shift semantics explicitly without altering the desktop keyboard
    // state. Plain navigation/Delete above still travel through the message loop.
    editor.store.get().host_selection(2, 2)?;
    editor.store.get().navigate(0x25, true, 1)?;
    pump(window)?;
    assert_eq!(live.scene()?.selection, 0..2);
    assert_eq!(live.scene()?.caret, 0);
    editor.store.get().navigate(0x27, true, 1)?;
    pump(window)?;
    assert_eq!(live.scene()?.selection, 2..2);
    assert_eq!(live.scene()?.caret, 2);

    editor.store.get().host_selection(0, 2)?;
    editor.insert(&"a😀bc".encode_utf16().collect::<Vec<_>>())?;
    editor.store.get().host_selection(3, 3)?;
    editor.store.get().navigate(0x25, true, 1)?;
    pump(window)?;
    assert_eq!(live.scene()?.selection, 1..3);
    assert_eq!(live.scene()?.caret, 1);
    editor.store.get().navigate(0x23, true, 1)?;
    pump(window)?;
    assert_eq!(live.scene()?.selection, 3..5);
    assert_eq!(live.scene()?.caret, 5);
    editor.store.get().navigate(0x24, true, 1)?;
    pump(window)?;
    assert_eq!(live.scene()?.selection, 0..3);
    assert_eq!(live.scene()?.caret, 0);
    post(WM_KEYDOWN, 0x27)?;
    pump(window)?;
    assert_eq!(live.scene()?.selection, 3..3);
    editor.store.get().navigate(0x25, true, 1)?;
    post(WM_KEYDOWN, 0x75)?;
    pump(window)?;
    assert_eq!(live.scene()?.underlines, vec![1..3]);
    post(WM_KEYDOWN, 0x25)?;
    pump(window)?;
    assert!(live.scene()?.underlines.is_empty());
    assert_eq!(live.scene()?.caret, 1);
    editor.store.get().host_selection(3, 3)?;
    editor.store.get().navigate(0x25, true, 1)?;
    post(WM_KEYDOWN, 0x75)?;
    pump(window)?;
    post(WM_KEYDOWN, 0x76)?;
    pump(window)?;
    assert_eq!(
        live.scene()?.text,
        "a日本語bc".encode_utf16().collect::<Vec<_>>()
    );
    post(WM_CHAR, 0x1a)?;
    pump(window)?;
    assert_eq!(
        live.scene()?.text,
        "a😀bc".encode_utf16().collect::<Vec<_>>()
    );
    assert_eq!(live.scene()?.selection, 1..3);
    assert_eq!(live.scene()?.caret, 1);
    post(WM_KEYDOWN, 0x2e)?;
    pump(window)?;
    assert_eq!(live.scene()?.text, "abc".encode_utf16().collect::<Vec<_>>());
    post(WM_CHAR, 0x1a)?;
    pump(window)?;
    assert_eq!(live.scene()?.caret, 1);
    println!("Shift selection direction / anchor crossing / selected conversion / cancellation / Delete / undo: OK");
    post(WM_CHAR, 0x19)?;
    pump(window)?;
    assert_eq!(live.scene()?.text, "abc".encode_utf16().collect::<Vec<_>>());
    assert_eq!(live.scene()?.selection, 1..1);
    post(WM_CHAR, 0x1a)?;
    pump(window)?;
    assert_eq!(live.scene()?.caret, 1);
    assert_eq!(live.scene()?.selection, 1..3);
    let counts = (
        editor.store.get().undo.borrow().len(),
        editor.store.get().redo.borrow().len(),
    );
    editor.store.get().readonly.set(true);
    post(WM_CHAR, 0x19)?;
    pump(window)?;
    assert!(editor.store.get().redo_once().is_err());
    assert!(editor.store.get().undo_once().is_err());
    editor.store.get().readonly.set(false);
    editor.store.get().lock.set(TS_LF_READ.0);
    assert!(editor.store.get().redo_once().is_err());
    assert!(editor.store.get().undo_once().is_err());
    editor.store.get().lock.set(0);
    assert_eq!(
        counts,
        (
            editor.store.get().undo.borrow().len(),
            editor.store.get().redo.borrow().len()
        )
    );
    assert_eq!(
        live.scene()?.text,
        "a😀bc".encode_utf16().collect::<Vec<_>>()
    );
    post(WM_KEYDOWN, 0x75)?;
    pump(window)?;
    post(WM_CHAR, 0x19)?;
    pump(window)?;
    assert_eq!(live.scene()?.text, "abc".encode_utf16().collect::<Vec<_>>());
    assert!(live.scene()?.underlines.is_empty());
    post(WM_CHAR, 0x1a)?;
    pump(window)?;
    post(WM_KEYDOWN, 0x75)?;
    pump(window)?;
    post(WM_KEYDOWN, 0x76)?;
    pump(window)?;
    assert!(editor.store.get().redo.borrow().is_empty());
    post(WM_CHAR, 0x1a)?;
    pump(window)?;
    assert_eq!(live.scene()?.caret, 1);
    post(WM_CHAR, 0x19)?;
    pump(window)?;
    assert_eq!(
        live.scene()?.text,
        "a日本語bc".encode_utf16().collect::<Vec<_>>()
    );
    assert!(live.scene()?.underlines.is_empty());
    post(WM_CHAR, 0x1a)?;
    pump(window)?;
    post(WM_CHAR, 'x' as usize)?;
    pump(window)?;
    assert!(editor.store.get().redo.borrow().is_empty());
    post(WM_CHAR, 0x19)?;
    pump(window)?;
    assert_eq!(
        live.scene()?.text,
        "axbc".encode_utf16().collect::<Vec<_>>()
    );
    post(WM_CHAR, 0x1a)?;
    pump(window)?;
    post(WM_CHAR, 0x19)?;
    pump(window)?;
    assert_eq!(
        live.scene()?.text,
        "axbc".encode_utf16().collect::<Vec<_>>()
    );
    println!("Redo typing / Delete / conversion; selection restored; stale branch discarded; lock/read-only guards; preview invalidation: OK");
    post(WM_CLOSE, 0)?;
    pump(window)?;
    assert!(!window.is_open());
    println!("WM_CLOSE revokes the grant and closes the native input view: OK");
    window.input_status()
}

//! Explicit waiting-demo routing. Take state out before native/COM/UI calls;
//! reentrant boundaries set a scalar flag and invalidate publication immediately.
use super::*;
use crate::recovery_view::{Action, Clipboard, View};

#[derive(Clone, Copy, PartialEq, Eq)]
struct Key {
    session: u128,
    index: usize,
    ticket: Ticket,
}
struct Shown {
    key: Key,
    view: View,
}
pub(in super::super) struct Owner {
    manager: ITfThreadMgr,
    client: u32,
    session: RefCell<Option<Waiting>>,
    view: RefCell<Option<Shown>>,
    active: Cell<bool>,
    busy: Cell<bool>,
    closed: Cell<bool>,
    epoch: Cell<u64>,
    boundary: Cell<Option<Boundary>>,
    show: Cell<bool>,
    copy: Cell<Option<Key>>,
    discard: Cell<Option<Key>>,
}
struct Busy<'a>(&'a Cell<bool>);
impl Drop for Busy<'_> {
    fn drop(&mut self) {
        self.0.set(false);
    }
}
impl Owner {
    pub(in super::super) fn new(manager: &ITfThreadMgr, client: u32) -> Self {
        Self {
            manager: manager.clone(),
            client,
            session: RefCell::new(None),
            view: RefCell::new(None),
            active: Cell::new(false),
            busy: Cell::new(false),
            closed: Cell::new(false),
            epoch: Cell::new(0),
            boundary: Cell::new(None),
            show: Cell::new(false),
            copy: Cell::new(None),
            discard: Cell::new(None),
        }
    }
    pub(in super::super) fn active(&self) -> bool {
        self.active.get()
    }
    fn state<T>(
        &self,
        editor: &Editor,
        action: impl FnOnce(&mut Option<Waiting>) -> Result<T>,
    ) -> Result<T> {
        if self.busy.replace(true) {
            return Err(E_FAIL.into());
        }
        let _busy = Busy(&self.busy);
        let mut session = self.session.borrow_mut().take();
        if let Some(reason) = self.boundary.take() {
            if let Some(session) = &mut session {
                session.stop(editor, reason, Outcome::Deferred);
                session.redact();
            }
        }
        let result = action(&mut session);
        if let Some(reason) = self.boundary.take() {
            if let Some(session) = &mut session {
                session.stop(editor, reason, Outcome::Deferred);
                session.redact();
            }
        }
        if self.closed.get() {
            session = None;
        }
        self.active
            .set(session.as_ref().is_some_and(|s| s.queue.is_some()));
        *self.session.borrow_mut() = session;
        result
    }
    pub(in super::super) fn boundary(&self) {
        self.epoch.set(self.epoch.get().saturating_add(1));
        self.active.set(false);
        self.boundary.set(Some(Boundary::FocusLost));
        self.copy.set(None);
        self.discard.set(None);
        self.show.set(false);
    }
    pub(in super::super) fn stop(&self, editor: &Editor) -> Result<()> {
        editor.high.set(None);
        self.state(editor, |session| {
            if let Some(session) = session {
                session.stop(editor, Boundary::Cancelled, Outcome::Deferred);
            }
            Ok(())
        })
    }
    pub(in super::super) fn close(&self) {
        self.closed.set(true);
        self.boundary();
        let old_view = self.view.borrow_mut().take();
        let old_session = self.session.borrow_mut().take();
        drop(old_view);
        drop(old_session);
    }
    pub(in super::super) fn can_start(&self) -> bool {
        self.busy.get()
            || self.session.borrow().as_ref().is_none_or(|s| {
                s.queue.is_none()
                    && !s
                        .ready
                        .iter()
                        .any(|r| r.recovery.metadata(Instant::now()).is_some())
            })
    }
    pub(in super::super) fn started(&self, editor: &Editor) -> Result<()> {
        if self.busy.get() {
            return Ok(());
        } // Core Convert is starting the next request.
        self.state(editor, |session| {
            if let Some((_, bridge)) = &editor.broker {
                if bridge.get().pending_id().is_some() {
                    *session = Some(Waiting::begin(&self.manager, editor, self.client)?);
                }
            }
            Ok(())
        })
    }
    pub(in super::super) fn admit(&self, editor: &Editor, input: Input<'_>) -> Result<bool> {
        if !self.active.get() {
            return Ok(false);
        }
        self.state(editor, |session| {
            let Some(session) = session else {
                return Ok(false);
            };
            match session.admit(editor, input) {
                Ok(()) => Ok(true),
                Err(_) => {
                    // This original event was not admitted. Cancel the request
                    // and preserve earlier events before its ordinary host path.
                    session.stop(editor, Boundary::Cancelled, Outcome::Deferred);
                    Ok(false)
                }
            }
        })
    }
    pub(in super::super) fn key(
        &self,
        editor: &Editor,
        key: usize,
        repeats: usize,
    ) -> Result<bool> {
        if self.busy.get() {
            return Ok(false);
        }
        if matches!(key, 0x77..=0x79) {
            if self.session.borrow().is_none() {
                return Ok(false);
            }
            let key_view = self.view.borrow().as_ref().map(|v| v.key);
            match key {
                0x77 => self.show.set(true),
                0x78 => self.discard.set(key_view),
                _ => self.copy.set(key_view),
            }
            return Ok(true);
        }
        if key == 0x76 && !self.active.get() && !self.can_start() {
            self.show.set(true);
            return Ok(true);
        }
        if !self.active.get() {
            return Ok(false);
        }
        if key == 0x1b {
            self.stop(editor)?;
            return Ok(true);
        }
        let modified =
            unsafe { GetKeyState(VK_CONTROL.0 as i32) < 0 || GetKeyState(VK_MENU.0 as i32) < 0 };
        if modified || matches!(key, 0x23..=0x28 | 0x2e) {
            self.stop(editor)?;
            return Ok(false);
        }
        let input = match key {
            0x08 => Some(Input::Backspace(repeats as u32)),
            0x0d => Some(Input::Enter),
            0x76 => Some(Input::Convert),
            _ => None,
        };
        if let Some(input) = input {
            editor.high.set(None);
            return self.admit(editor, input);
        }
        Ok(false)
    }
    fn key_valid(session: &Waiting, key: Key) -> bool {
        session.target.target == key.session
            && session
                .metadata(key.index, Instant::now())
                .is_some_and(|m| m.ticket == key.ticket)
    }
    pub(in super::super) fn service(
        &self,
        window: &crate::window_host::Window,
        editor: &Editor,
        clipboard: &dyn Clipboard,
    ) -> Result<()> {
        let epoch = self.epoch.get();
        let mut shown = self.view.borrow_mut().take();
        let mut action = shown
            .as_ref()
            .and_then(|v| v.view.take_action().map(|(_, a)| (v.key, a)));
        if let Some(key) = self.discard.take() {
            action = Some((key, Action::Discard));
        } else if let Some(key) = self.copy.take() {
            action = Some((key, Action::Copy));
        }
        let next = self.show.replace(false);
        self.state(editor, |session| {
            let Some(waiting) = session else {
                shown = None;
                return Ok(());
            };
            waiting.expire(Instant::now());
            if waiting.queue.is_some() {
                waiting.service(editor)?;
            }
            if waiting.inspect().is_err() {
                waiting.redact();
                shown = None;
            }
            let mut acknowledged = false;
            let mut failed = false;
            if let Some((key, command)) = action {
                if Self::key_valid(waiting, key) && self.boundary.get().is_none() {
                    match command {
                        Action::Hide => shown = None,
                        Action::Discard => {
                            acknowledged = waiting.ready[key.index].recovery.discard(key.ticket);
                            shown = None;
                        }
                        Action::Copy => {
                            let guarded = GuardedClipboard {
                                inner: clipboard,
                                owner: self,
                                epoch,
                            };
                            failed = waiting.copy(editor, key.index, &guarded).is_err();
                            acknowledged = !failed;
                            if acknowledged {
                                shown = None;
                            }
                        }
                    }
                }
            }
            let records: Vec<_> = waiting
                .ready
                .iter()
                .enumerate()
                .filter_map(|(i, r)| r.recovery.metadata(Instant::now()).map(|m| (i, m)))
                .collect();
            if shown.as_ref().is_some_and(|v| {
                !Self::key_valid(waiting, v.key)
                    || waiting
                        .metadata(v.key.index, Instant::now())
                        .is_none_or(|m| m.copyable != v.view.copyable)
            }) {
                shown = None;
            }
            if next && !records.is_empty() && self.boundary.get().is_none() {
                let ordinal = shown
                    .as_ref()
                    .and_then(|v| records.iter().position(|(i, _)| *i == v.key.index))
                    .map_or(0, |i| (i + 1) % records.len());
                let (index, metadata) = records[ordinal];
                shown = None;
                if waiting.inspect().is_ok() {
                    let slot = &waiting.ready[index];
                    let text = slot
                        .recovery
                        .text(slot.ticket, Instant::now())
                        .unwrap_or(&[]);
                    if let Ok(view) = View::new(window.hwnd(), metadata, text) {
                        view.position(ordinal + 1, records.len())?;
                        shown = Some(Shown {
                            key: Key {
                                session: waiting.target.target,
                                index,
                                ticket: metadata.ticket,
                            },
                            view,
                        });
                    }
                }
            }
            if let Some(view) = &shown {
                if let Some(metadata) = waiting.metadata(view.key.index, Instant::now()) {
                    view.view.update(metadata, failed)?;
                    if let Some(ordinal) = records
                        .iter()
                        .position(|(index, _)| *index == view.key.index)
                    {
                        view.view.position(ordinal + 1, records.len())?;
                    }
                }
            }
            if !records.is_empty() {
                window.set_title(
                    "Sumibi 救済あり（最大10分） | F8 順に確認 | 表示中の1件をF9破棄 / F10コピー",
                )?;
            } else if waiting.queue.is_some() {
                window.set_title(
                    "Sumibi 待機入力デモ | 待機は最大8秒・4件 | F7再変換 | Enterは救済へ | Esc取消",
                )?;
            } else {
                if acknowledged {
                    if let Some(composing) = &editor.composing {
                        composing.cancel(editor, true);
                    }
                }
                *session = None;
                window.set_title(
                    "Sumibi 待機入力デモ（固定結果） | F7 変換 | F8 救済確認 | Ctrl+Z Undo",
                )?;
            }
            Ok(())
        })?;
        if self.boundary.get().is_some() || self.epoch.get() != epoch || self.closed.get() {
            shown = None;
        }
        *self.view.borrow_mut() = shown;
        Ok(())
    }
    pub(in super::super) fn wait_ms(&self, editor: &Editor) -> u32 {
        if self.active.get() {
            if editor
                .broker
                .as_ref()
                .is_some_and(|(_, b)| b.get().active())
            {
                return 250;
            }
            return 0;
        }
        if self
            .session
            .borrow()
            .as_ref()
            .is_some_and(|s| !s.ready.is_empty())
        {
            250
        } else {
            u32::MAX
        }
    }
}
struct GuardedClipboard<'a> {
    inner: &'a dyn Clipboard,
    owner: &'a Owner,
    epoch: u64,
}
impl Clipboard for GuardedClipboard<'_> {
    fn write(
        &self,
        hwnd: windows::Win32::Foundation::HWND,
        text: &[u16],
        permitted: &dyn Fn() -> bool,
    ) -> Result<()> {
        self.inner.write(hwnd, text, &|| {
            self.owner.epoch.get() == self.epoch
                && self.owner.boundary.get().is_none()
                && !self.owner.closed.get()
                && permitted()
        })
    }
}

pub(crate) fn run_waiting_demo(
    manager: &ITfThreadMgr,
    client: u32,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    run(manager, client, RunMode::WaitingDemo)
}
#[path = "input_waiting_owner_tests.rs"]
mod tests;
pub(crate) use tests::exercise_waiting_demo;
pub(in super::super) use tests::test_waiting_demo;

//! Explicit waiting-demo routing. Take state out before native/COM/UI calls;
//! reentrant boundaries set a scalar flag and invalidate publication immediately.
use super::*;
use crate::recovery_view::{Action, Clipboard, View};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Source {
    Waiting { session: u128, index: usize },
    Ordinary,
}
#[derive(Clone, Copy, PartialEq, Eq)]
struct Key {
    source: Source,
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
    start_failed: Cell<bool>,
    timed_out: Cell<bool>,
    capacity_limited: Cell<bool>,
    // Expired waiting records must not silently return to the normal F7 title.
    lost: Cell<bool>,
    epoch: Cell<u64>,
    boundary: Cell<Option<Boundary>>,
    show: Cell<bool>,
    copy: Cell<Option<Key>>,
    discard: Cell<Option<Key>>,
    // Diagnostic counts only; never retain text or individual key identities.
    starts: Cell<u32>,
    offered: Cell<u32>,
    admitted: Cell<u32>,
    peak_recoveries: Cell<usize>,
    views_opened: Cell<u32>,
}
struct Busy<'a>(&'a Cell<bool>);
impl Drop for Busy<'_> {
    fn drop(&mut self) {
        self.0.set(false);
    }
}
// Every exit before publication cancels only the request captured at entry.
// Cancel signals the worker; completion draining stays on the owner loop.
struct Starting<'a> {
    broker: &'a ISpikeBroker,
    request: u64,
    accepted: bool,
}
impl Drop for Starting<'_> {
    fn drop(&mut self) {
        if !self.accepted {
            unsafe {
                let _ = self.broker.Cancel(self.request);
            }
        }
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
            start_failed: Cell::new(false),
            timed_out: Cell::new(false),
            capacity_limited: Cell::new(false),
            lost: Cell::new(false),
            epoch: Cell::new(0),
            boundary: Cell::new(None),
            show: Cell::new(false),
            copy: Cell::new(None),
            discard: Cell::new(None),
            starts: Cell::new(0),
            offered: Cell::new(0),
            admitted: Cell::new(0),
            peak_recoveries: Cell::new(0),
            views_opened: Cell::new(0),
        }
    }
    pub(in super::super) fn diagnostics(&self) -> (u32, u32, u32, usize, u32, bool) {
        (
            self.starts.get(),
            self.offered.get(),
            self.admitted.get(),
            self.peak_recoveries.get(),
            self.views_opened.get(),
            self.start_failed.get(),
        )
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
            editor.rescue.invalidate();
            if let Some(session) = &mut session {
                session.stop(editor, reason, Outcome::Deferred);
                session.redact();
            }
        }
        let result = action(&mut session);
        if let Some(reason) = self.boundary.take() {
            editor.rescue.invalidate();
            if let Some(session) = &mut session {
                session.stop(editor, reason, Outcome::Deferred);
                session.redact();
            }
        }
        if self.closed.get() {
            session = None;
            editor.rescue.close();
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
    fn cancel(&self, editor: &Editor) -> Result<()> {
        editor.high.set(None);
        self.state(editor, |session| {
            if let Some(session) = session {
                let fallback_caret = session.fallback_caret_end();
                session.stop(editor, Boundary::Cancelled, Outcome::Deferred);
                session.release_source_selection(editor, fallback_caret)?;
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
        self.started_with(editor, |request| {
            Waiting::begin(&self.manager, editor, self.client, request)
        })
    }
    fn started_with(
        &self,
        editor: &Editor,
        initialise: impl FnOnce(u64) -> Result<Waiting>,
    ) -> Result<()> {
        if self.busy.get() {
            return Ok(());
        } // Core Convert is starting the next request.
        let Some((broker, bridge)) = &editor.broker else {
            return Ok(());
        };
        let Some(request) = bridge.get().pending_id() else {
            return Ok(());
        };
        let epoch = self.epoch.get();
        let eligible = !self.closed.get() && self.boundary.get().is_none();
        self.state(editor, |session| {
            if session
                .as_ref()
                .is_some_and(|s| s.request == request && s.queue.is_some())
            {
                return Ok(()); // Duplicate notification must preserve accepted input.
            }
            let mut attempt = Starting {
                broker,
                request,
                accepted: false,
            };
            if !eligible
                || session.as_ref().is_some_and(|s| {
                    s.queue.is_some()
                        || s.ready
                            .iter()
                            .any(|r| r.recovery.metadata(Instant::now()).is_some())
                })
                || editor.rescue.metadata().is_some()
            {
                self.start_failed.set(true);
                return Ok(());
            }
            let candidate = match initialise(request) {
                Ok(candidate) => candidate,
                Err(_) => {
                    self.start_failed.set(true);
                    return Ok(());
                }
            };
            let stable = candidate.stable();
            let mut pending = 0;
            let current = unsafe { broker.Pending(&mut pending).is_ok() };
            if !current
                || pending != request
                || bridge.get().pending_id() != Some(request)
                || candidate.request != request
                || !stable
                || self.epoch.get() != epoch
                || self.boundary.get().is_some()
                || self.closed.get()
            {
                self.start_failed.set(true);
                return Ok(());
            }
            *session = Some(candidate);
            attempt.accepted = true;
            self.starts.set(self.starts.get().saturating_add(1));
            self.start_failed.set(false);
            self.timed_out.set(false);
            self.capacity_limited.set(false);
            Ok(())
        })
    }
    pub(in super::super) fn admit(&self, editor: &Editor, input: Input<'_>) -> Result<bool> {
        if !self.active.get() {
            return Ok(false);
        }
        self.offered.set(self.offered.get().saturating_add(1));
        self.state(editor, |session| {
            let Some(session) = session else {
                return Ok(false);
            };
            match session.admit(editor, input) {
                Ok(()) => {
                    self.admitted.set(self.admitted.get().saturating_add(1));
                    Ok(true)
                }
                Err(_) => {
                    if session.capacity_denied {
                        self.capacity_limited.set(true);
                    }
                    let fallback_caret = session.fallback_caret_end();
                    if Instant::now() >= session.deadline {
                        self.timed_out.set(true);
                    }
                    // This original event was not admitted. Cancel the request
                    // and preserve earlier events before its ordinary host path.
                    session.stop(editor, Boundary::Cancelled, Outcome::Deferred);
                    session.release_source_selection(editor, fallback_caret)?;
                    Ok(false)
                }
            }
        })
    }
    pub(in super::super) fn key(&self, editor: &Editor, key: usize, flags: LPARAM) -> Result<bool> {
        if self.busy.get() {
            return Ok(false);
        }
        if matches!(key, 0x77..=0x79) {
            let key_view = self.view.borrow().as_ref().map(|v| v.key);
            match key {
                0x77 => self.show.set(true),
                0x78 => self.discard.set(key_view),
                _ => self.copy.set(key_view),
            }
            return Ok(true);
        }
        // F7 and the Japanese keyboard's Convert key are equivalent only in
        // this explicitly owned demo. Leave modified Convert to the host.
        let convert = matches!(key, 0x76 | 0x1c);
        let modified =
            unsafe { GetKeyState(VK_CONTROL.0 as i32) < 0 || GetKeyState(VK_MENU.0 as i32) < 0 };
        if key == 0x1c
            && (flags.0 & (1 << 29) != 0
                || modified
                || unsafe { GetKeyState(VK_SHIFT.0 as i32) < 0 })
        {
            return Ok(false);
        }
        if convert
            && !self.active.get()
            && (!self.can_start() || editor.rescue.metadata().is_some())
        {
            self.show.set(true);
            return Ok(true);
        }
        if !self.active.get() {
            if key == 0x1b {
                self.lost.set(false);
                self.timed_out.set(false);
                self.capacity_limited.set(false);
            }
            return Ok(false);
        }
        if key == 0x1b {
            self.cancel(editor)?;
            return Ok(true);
        }
        if modified || matches!(key, 0x23..=0x28 | 0x2e) {
            self.stop(editor)?;
            return Ok(false);
        }
        let input = match key {
            0x08 => Some(Input::Backspace((flags.0 as usize & 0xffff).max(1) as u32)),
            0x0d => Some(Input::Enter),
            0x76 | 0x1c => Some(Input::Convert),
            _ => None,
        };
        if let Some(input) = input {
            editor.high.set(None);
            return self.admit(editor, input);
        }
        Ok(false)
    }
    fn metadata(editor: &Editor, session: &Option<Waiting>, key: Key) -> Option<Metadata> {
        let metadata = match key.source {
            Source::Waiting {
                session: identity,
                index,
            } => {
                let session = session.as_ref()?;
                if session.target.target != identity {
                    return None;
                }
                session.metadata(index, Instant::now())?
            }
            Source::Ordinary => editor.rescue.metadata()?,
        };
        (metadata.ticket == key.ticket).then_some(metadata)
    }
    fn records(editor: &Editor, session: &Option<Waiting>) -> Vec<(Key, Metadata)> {
        let mut records = Vec::new();
        if let Some(waiting) = session {
            for (index, slot) in waiting.ready.iter().enumerate() {
                if let Some(metadata) = slot.recovery.metadata(Instant::now()) {
                    records.push((
                        Key {
                            source: Source::Waiting {
                                session: waiting.target.target,
                                index,
                            },
                            ticket: metadata.ticket,
                        },
                        metadata,
                    ));
                }
            }
        }
        if let Some(metadata) = editor.rescue.metadata() {
            records.push((
                Key {
                    source: Source::Ordinary,
                    ticket: metadata.ticket,
                },
                metadata,
            ));
        }
        records
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
            let had_waiting_records = session.as_ref().is_some_and(|s| !s.ready.is_empty());
            editor.rescue.maintain();
            if let Some(waiting) = session {
                waiting.expire(Instant::now());
                if waiting.queue.is_some() {
                    let expired = Instant::now() >= waiting.deadline;
                    let fallback_caret = expired.then(|| waiting.fallback_caret_end()).flatten();
                    if expired {
                        self.timed_out.set(true);
                    }
                    waiting.service(editor)?;
                    // The owner loop can retire a request before another key
                    // arrives. Release F7's selection now, or that later
                    // ordinary key would replace the entire source.
                    if waiting.queue.is_none() {
                        waiting.release_source_selection(editor, fallback_caret)?;
                    }
                }
                if waiting.inspect().is_err() {
                    waiting.redact();
                }
            }
            let mut acknowledged = false;
            let mut failed = false;
            if let Some((key, command)) = action {
                if Self::metadata(editor, session, key).is_some() && self.boundary.get().is_none() {
                    match command {
                        Action::Hide => shown = None,
                        Action::Discard => {
                            acknowledged = match key.source {
                                Source::Waiting { index, .. } => session
                                    .as_mut()
                                    .is_some_and(|s| s.ready[index].recovery.discard(key.ticket)),
                                Source::Ordinary => editor.rescue.discard(key.ticket),
                            };
                            shown = None;
                        }
                        Action::Copy => {
                            let guarded = GuardedClipboard {
                                inner: clipboard,
                                owner: self,
                                epoch,
                            };
                            acknowledged = match key.source {
                                Source::Waiting { index, .. } => session
                                    .as_mut()
                                    .is_some_and(|s| s.copy(editor, index, &guarded).is_ok()),
                                Source::Ordinary => editor
                                    .rescue
                                    .copy_to(window, key.ticket, &guarded)
                                    .unwrap_or(false),
                            };
                            failed = !acknowledged;
                            if acknowledged {
                                shown = None;
                            }
                        }
                    }
                }
            }
            editor.rescue.maintain();
            let records = Self::records(editor, session);
            self.peak_recoveries
                .set(self.peak_recoveries.get().max(records.len()));
            if shown.as_ref().is_some_and(|v| {
                Self::metadata(editor, session, v.key).is_none_or(|m| m.copyable != v.view.copyable)
            }) {
                shown = None;
            }
            if next && !records.is_empty() && self.boundary.get().is_none() {
                let ordinal = shown
                    .as_ref()
                    .and_then(|v| records.iter().position(|(key, _)| *key == v.key))
                    .map_or(0, |i| (i + 1) % records.len());
                let (key, metadata) = records[ordinal];
                shown = None;
                let text = match key.source {
                    Source::Waiting { index, .. } => session.as_ref().and_then(|waiting| {
                        if !metadata.copyable {
                            return Some(Vec::new());
                        }
                        waiting.inspect().ok()?;
                        let slot = &waiting.ready[index];
                        Some(
                            slot.recovery
                                .text(slot.ticket, Instant::now())
                                .unwrap_or(&[])
                                .to_vec(),
                        )
                    }),
                    Source::Ordinary => editor.rescue.preview(key.ticket),
                };
                if let Some(text) = text {
                    if let Ok(view) = View::new(window.hwnd(), metadata, &text) {
                        view.position(ordinal + 1, records.len())?;
                        self.views_opened
                            .set(self.views_opened.get().saturating_add(1));
                        shown = Some(Shown { key, view });
                    }
                }
            }
            if let Some(view) = &shown {
                if let Some(metadata) = Self::metadata(editor, session, view.key) {
                    view.view.update(metadata, failed)?;
                    if let Some(ordinal) = records.iter().position(|(key, _)| *key == view.key) {
                        view.view.position(ordinal + 1, records.len())?;
                    }
                }
            }
            let queued = session.as_ref().is_some_and(|s| s.queue.is_some());
            if !queued
                && !records
                    .iter()
                    .any(|(key, _)| matches!(key.source, Source::Waiting { .. }))
            {
                *session = None;
            }
            if !queued && records.is_empty() && had_waiting_records && !acknowledged {
                self.lost.set(true);
            }
            if !records.is_empty() {
                window.set_title(if self.capacity_limited.get() {
                    "Sumibi 容量上限で新しい入力は通常経路へ | 救済あり | F8 順に確認 | F9破棄 / F10コピー"
                } else {
                    "Sumibi 救済あり（最大10分） | F8 順に確認 | 表示中の1件をF9破棄 / F10コピー"
                })?;
            } else if queued {
                window.set_title(
                    "Sumibi 待機入力デモ | 待機は最大8秒・4件 | F7/変換キーで再変換 | Enterは救済へ | Esc取消",
                )?;
            } else {
                if acknowledged {
                    if let Some(composing) = &editor.composing {
                        composing.cancel(editor, true);
                    }
                }
                window.set_title(if self.start_failed.get() {
                    "Sumibi 待機入力を開始できず変換を取り消しました | 原文を保持 | F7 再試行"
                } else if self.lost.get() || (!acknowledged && editor.store.get().input_problem.get())
                {
                    "Sumibi 未確認入力の救済は残っていません。文書の結果を確認してください | Esc 追跡解除"
                } else if self.timed_out.get() {
                    "Sumibi 変換待機が期限切れになりました | 文書の結果を確認 | F7 再試行 / Esc 解除"
                } else if self.capacity_limited.get() {
                    "Sumibi 容量上限で新しい入力は通常経路へ | 文書を確認 | F7 再試行 / Esc 解除"
                } else {
                    "Sumibi 待機入力デモ（固定結果） | F7/変換キー | F8 救済確認 | Ctrl+Z Undo"
                })?;
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
            return 250;
        }
        // The legacy Rescue has no View of its own in waiting-demo. Its
        // original timer can sleep until expiry, leaving the owner's preview
        // and protection check stale for up to ten minutes.
        if editor.rescue.metadata().is_some() {
            if self.view.borrow().is_some() {
                1000
            } else {
                250
            }
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
pub(in super::super) use tests::test_waiting_demo;
pub(crate) use tests::{exercise_waiting_demo, exercise_waiting_demo_interactive};

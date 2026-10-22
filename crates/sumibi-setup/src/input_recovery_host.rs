//! Owner-loop rescue handling; never replays keys or edits document text.
use super::*;
use crate::recovery_view::{Action, Clipboard, View};
use std::time::{Duration, Instant};
use sumibi_core::recovery::{Metadata, Operation, Outcome, Recovery, Ticket};
pub(crate) use tests::exercise_recovery;
pub(crate) use tests::run_recovery_demo;
pub(super) use tests::test_recovery;
#[path = "input_recovery_tests.rs"]
mod tests;

pub(super) struct Rescue {
    slot: RefCell<Recovery>,
    view: RefCell<Option<View>>,
    show: Cell<Option<Option<Ticket>>>,
    copy: Cell<Option<Ticket>>,
    discard: Cell<Option<Ticket>>,
    clock: Cell<Option<Instant>>,
    safety: Option<Safety>,
    permission: Cell<Option<(Ticket, u64)>>,
}
struct Safety {
    manager: ITfThreadMgr,
    context: ITfContext,
    client: u32,
    watch: sumibi_tip::edit::manual::InputWatch,
}
impl Safety {
    fn allowed(&self, revision: u64) -> bool {
        self.watch.boundary_generation().ok() == Some(revision)
            && sumibi_tip::edit::inspect_input(&self.manager, &self.context, self.client).is_ok()
            && self.watch.boundary_generation().ok() == Some(revision)
    }
}
impl Rescue {
    pub(super) fn new(
        manager: &ITfThreadMgr,
        context: &ITfContext,
        client: u32,
        enabled: bool,
        short: bool,
    ) -> Result<Self> {
        Ok(Self {
            slot: RefCell::new(if short {
                Recovery::with_retention(Duration::from_millis(500)).unwrap()
            } else {
                Recovery::default()
            }),
            view: RefCell::new(None),
            show: Cell::new(None),
            copy: Cell::new(None),
            discard: Cell::new(None),
            clock: Cell::new(None),
            safety: if enabled {
                Some(Safety {
                    manager: manager.clone(),
                    context: context.clone(),
                    client,
                    watch: sumibi_tip::edit::manual::InputWatch::attach(manager, context)?,
                })
            } else {
                None
            },
            permission: Cell::new(None),
        })
    }
    fn now(&self) -> Instant {
        self.clock.get().unwrap_or_else(Instant::now)
    }
    pub(super) fn reserve(&self, operation: Operation) -> Option<Ticket> {
        let ticket = self.slot.borrow_mut().reserve(operation, self.now()).ok()?;
        self.permission.set(
            self.safety
                .as_ref()
                .and_then(|s| s.watch.boundary_generation().ok())
                .map(|v| (ticket, v)),
        );
        Some(ticket)
    }
    fn permitted(&self, ticket: Ticket) -> bool {
        self.permission.get().is_some_and(|(old, revision)| {
            old == ticket && self.safety.as_ref().is_some_and(|s| s.allowed(revision))
        })
    }
    pub(super) fn finish(&self, ticket: Ticket, disposition: u32, units: &[u16]) {
        let outcome = match disposition {
            sumibi_tip::spike::INPUT_NOT_ATTEMPTED => Outcome::NotAttempted,
            sumibi_tip::spike::INPUT_APPLIED => Outcome::Applied,
            sumibi_tip::spike::INPUT_APPLIED_UNTRACKED => Outcome::AppliedUntracked,
            _ => Outcome::Uncertain,
        };
        let payload = if matches!(outcome, Outcome::AppliedUntracked | Outcome::Uncertain)
            && self.permitted(ticket)
        {
            units
        } else {
            &[]
        };
        self.slot
            .borrow_mut()
            .finish(ticket, outcome, payload, self.now());
    }
    pub(super) fn request(&self, key: usize) {
        match key {
            0x77 => self.show.set(Some(self.metadata().map(|m| m.ticket))),
            0x78 => self.discard.set(self.metadata().map(|m| m.ticket)),
            0x79 => self.copy.set(self.view.borrow().as_ref().map(|v| v.ticket)),
            _ => {}
        }
    }
    fn metadata(&self) -> Option<Metadata> {
        self.slot.borrow().metadata(self.now())
    }
    pub(super) fn invalidate(&self) {
        let metadata = self.metadata();
        if let Some(metadata) = metadata {
            self.slot.borrow_mut().redact(metadata.ticket);
        }
        self.permission.set(None);
    }
    fn check_payload(&self) {
        if let Some(metadata) = self.metadata() {
            if metadata.copyable && !self.permitted(metadata.ticket) {
                self.slot.borrow_mut().redact(metadata.ticket);
            }
        }
    }
    pub(super) fn wait_ms(&self) -> u32 {
        self.metadata().map_or_else(
            || {
                if self.slot.borrow().needs_expiry(self.now()) {
                    0
                } else {
                    u32::MAX
                }
            },
            |m| {
                let milliseconds = m.remaining.as_millis()
                    + u128::from(m.remaining.subsec_nanos() % 1_000_000 != 0);
                milliseconds.clamp(
                    1,
                    if self.view.borrow().is_some() {
                        1000
                    } else {
                        600_000
                    },
                ) as u32
            },
        )
    }
    pub(super) fn close(&self) {
        self.slot.borrow_mut().clear();
        self.permission.set(None);
        let old = self.view.borrow_mut().take();
        drop(old);
        self.show.set(None);
        self.copy.set(None);
        self.discard.set(None);
    }
    /// Returns true only after explicit acknowledgement. Expiry/hide cannot
    /// reactivate a failed input grant. No RefCell borrow spans native calls.
    pub(super) fn service(
        &self,
        window: &crate::window_host::Window,
        clipboard: &dyn Clipboard,
    ) -> Result<bool> {
        let expired = self.slot.borrow_mut().expire(self.now());
        self.check_payload();
        let mut view = self.view.borrow_mut().take();
        let mut acknowledged = false;
        let metadata = self.metadata();
        if view.as_ref().is_some_and(|v| {
            metadata.is_none_or(|m| m.ticket != v.ticket || m.copyable != v.copyable)
        }) {
            view = None;
        }
        let mut action = view.as_ref().and_then(|v| v.take_action());
        let discard = self.discard.take();
        let copy = self.copy.take();
        if let Some(ticket) = discard {
            action = Some((ticket, Action::Discard));
        } else if let Some(ticket) = copy {
            action = Some((ticket, Action::Copy));
        }
        let mut copy_failed = false;
        if let Some((ticket, operation)) = action {
            match operation {
                Action::Hide => {
                    view = None;
                }
                Action::Discard => {
                    acknowledged = self.slot.borrow_mut().discard(ticket);
                    view = None;
                }
                Action::Copy => {
                    let text = {
                        let slot = self.slot.borrow();
                        slot.text(ticket, self.now()).and_then(|text| {
                            let mut copy = Vec::new();
                            copy.try_reserve_exact(text.len()).ok()?;
                            copy.extend_from_slice(text);
                            Some(copy)
                        })
                    };
                    if let Some(text) = text {
                        let permitted = || {
                            self.permitted(ticket)
                                && self.slot.borrow().text(ticket, self.now()).is_some()
                        };
                        if clipboard.write(window.hwnd(), &text, &permitted).is_ok() {
                            acknowledged = self.slot.borrow_mut().discard(ticket);
                            view = None;
                        } else {
                            copy_failed = true;
                        }
                    }
                }
            }
        }
        self.slot.borrow_mut().expire(self.now());
        self.check_payload();
        let metadata = self.metadata();
        if view.as_ref().is_some_and(|v| {
            metadata.is_none_or(|m| m.ticket != v.ticket || m.copyable != v.copyable)
        }) {
            view = None;
        }
        let requested_show = self.show.take();
        if let Some(metadata) = metadata {
            if requested_show == Some(Some(metadata.ticket)) && view.is_none() {
                let units = {
                    let slot = self.slot.borrow();
                    slot.text(metadata.ticket, self.now())
                        .unwrap_or(&[])
                        .to_vec()
                };
                // A UI creation failure retains the rescue record and deadline.
                if let Ok(created) = View::new(window.hwnd(), metadata, &units) {
                    view = Some(created);
                }
            }
            if let Some(view) = &view {
                view.update(metadata, copy_failed)?;
            }
            window.set_title("Sumibi 未確認入力あり（最大10分で破棄） | F8 確認 | F9 破棄 | 確認画面を開いてF10 コピー")?;
        } else {
            view = None;
            if expired {
                window.set_title("Sumibi 救済データを期限切れで破棄しました。文書を確認してください | Esc 追跡解除")?;
            } else if acknowledged {
                window.set_title("Sumibi 救済の確認を完了しました（本文へ自動適用しません）")?;
            } else if requested_show.is_some() {
                window.set_title("Sumibi 現在の救済データはありません | F7 変換")?;
            }
        }
        *self.view.borrow_mut() = view;
        Ok(acknowledged)
    }
}
impl Editor {
    pub(super) fn service_recovery(
        &self,
        window: &crate::window_host::Window,
        clipboard: &dyn Clipboard,
    ) -> Result<()> {
        if self.rescue.service(window, clipboard)? {
            if let Some(composing) = &self.composing {
                composing.cancel(self, true);
            }
        }
        Ok(())
    }
}

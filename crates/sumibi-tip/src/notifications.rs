//! Weak, owner-thread observers. Notification cookies never authorize writes.
#![allow(non_snake_case)]
use crate::com::{boundary, ObjectLease};
use crate::spike::Controller;
use std::rc::{Rc, Weak};
use windows::core::{implement, Interface, Ref, Result};
use windows::Win32::UI::TextServices::*;

pub(crate) struct Subscription {
    source: ITfSource,
    cookie: Option<u32>,
}
impl Subscription {
    pub(crate) fn close(&mut self) -> Result<()> {
        if let Some(cookie) = self.cookie {
            // SAFETY: exact cookie/source pair returned by AdviseSink.
            unsafe {
                self.source.UnadviseSink(cookie)?;
            }
            self.cookie = None;
        }
        Ok(())
    }
}
impl Drop for Subscription {
    fn drop(&mut self) {
        let _ = boundary(|| self.close());
    }
}

#[implement(ITfTextEditSink, ITfThreadMgrEventSink)]
struct Observer {
    state: Weak<Controller>,
    _lease: ObjectLease,
}

pub(crate) fn manager_sink(state: &Rc<Controller>) -> ITfThreadMgrEventSink {
    Observer {
        state: Rc::downgrade(state),
        _lease: ObjectLease::new(),
    }
    .into()
}

pub(crate) fn subscribe(
    state: &Rc<Controller>,
    manager: &ITfThreadMgr,
    context: &ITfContext,
) -> Result<Vec<Subscription>> {
    let sink = windows::core::ComObject::new(Observer {
        state: Rc::downgrade(state),
        _lease: ObjectLease::new(),
    });
    let mut subscriptions = Vec::new();
    // SAFETY: same-thread sources retain independently counted weak observers.
    unsafe {
        let source: ITfSource = context.cast()?;
        let cookie = source.AdviseSink(
            &ITfTextEditSink::IID,
            &sink.to_interface::<ITfTextEditSink>(),
        )?;
        subscriptions.push(Subscription {
            source,
            cookie: Some(cookie),
        });
        let source: ITfSource = manager.cast()?;
        let cookie = source.AdviseSink(
            &ITfThreadMgrEventSink::IID,
            &sink.to_interface::<ITfThreadMgrEventSink>(),
        )?;
        subscriptions.push(Subscription {
            source,
            cookie: Some(cookie),
        });
    }
    Ok(subscriptions)
}

impl ITfTextEditSink_Impl for Observer_Impl {
    fn OnEndEdit(
        &self,
        context: Ref<ITfContext>,
        _: u32,
        record: Ref<ITfEditRecord>,
    ) -> Result<()> {
        boundary(|| {
            let Some(state) = self.state.upgrade() else {
                return Ok(());
            };
            if !state.observes(context.ok()?)? {
                return Ok(());
            }
            let record = record.ok()?;
            // SAFETY: notification provides read-only metadata. Text is not read.
            let changed = unsafe {
                if record.GetSelectionStatus()?.as_bool() {
                    true
                } else {
                    let ranges = record.GetTextAndPropertyUpdates(TF_GTP_INCL_TEXT, &[])?;
                    let mut slot = [None];
                    let mut fetched = 0;
                    ranges.Next(&mut slot, &mut fetched)?;
                    fetched != 0
                }
            };
            if changed {
                state.invalidate(false)?;
            }
            Ok(())
        })
    }
}
impl ITfThreadMgrEventSink_Impl for Observer_Impl {
    fn OnInitDocumentMgr(&self, _: Ref<ITfDocumentMgr>) -> Result<()> {
        Ok(())
    }
    fn OnUninitDocumentMgr(&self, _: Ref<ITfDocumentMgr>) -> Result<()> {
        self.check_focus()
    }
    fn OnSetFocus(&self, focus: Ref<ITfDocumentMgr>, _: Ref<ITfDocumentMgr>) -> Result<()> {
        boundary(|| {
            if let Some(state) = self.state.upgrade() {
                state.check_thread()?;
                // OnSetFocus may precede the manager's GetFocus update. Use
                // the notification's new document, never the cached old focus.
                let context = focus
                    .as_ref()
                    .and_then(|document| unsafe { document.GetTop().ok() });
                state.focus_notification(context.as_ref())?;
            }
            Ok(())
        })
    }
    fn OnPushContext(&self, _: Ref<ITfContext>) -> Result<()> {
        self.check_focus()
    }
    fn OnPopContext(&self, _: Ref<ITfContext>) -> Result<()> {
        self.check_focus()
    }
}
impl Observer_Impl {
    fn check_focus(&self) -> Result<()> {
        boundary(|| {
            if let Some(state) = self.state.upgrade() {
                state.check_focus_notification()?;
            }
            Ok(())
        })
    }
}

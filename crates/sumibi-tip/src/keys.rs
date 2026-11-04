//! A separate sink avoids a TIP -> manager -> TIP reference cycle.
#![allow(non_snake_case)]

use windows::core::{implement, Ref, Result, BOOL, GUID};
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::TextServices::{ITfContext, ITfKeyEventSink, ITfKeyEventSink_Impl};

use crate::com::ObjectLease;
#[cfg(not(feature = "spike"))]
use crate::session::COMMAND_CONVERT;
#[cfg(feature = "spike")]
use crate::spike::{Controller, COMMAND_CONVERT};
use std::{
    rc::Weak,
    thread::{self, ThreadId},
};

#[implement(ITfKeyEventSink)]
pub(super) struct KeySink {
    #[cfg(feature = "spike")]
    controller: Weak<Controller>,
    session: Weak<crate::session::TipHostSession>,
    owner: ThreadId,
    // A manager can retain this sink even after releasing the TIP.
    // Keep the DLL loaded until its last reference has been released.
    _lease: ObjectLease,
}

impl KeySink {
    /// Passive observation only: the key is never consumed here. Recorded at
    /// OnTestKeyDown, which TSF issues for every key even when it is not eaten.
    fn observe(&self, context: &Ref<ITfContext>, key: WPARAM, flags: LPARAM) {
        if self.owner != thread::current().id() {
            return;
        }
        if let Some(session) = self.session.upgrade() {
            let _ = crate::com::boundary(|| {
                session.observe_key(context.as_ref(), key.0 as u32, flags.0);
                Ok(())
            });
        }
    }

    #[cfg(feature = "spike")]
    fn preview_key(
        &self,
        context: Ref<ITfContext>,
        key: WPARAM,
        flags: LPARAM,
        test: bool,
    ) -> bool {
        if self.owner != thread::current().id() {
            return false;
        }
        let Some(context) = context.as_ref() else {
            return false;
        };
        let Some(controller) = self.controller.upgrade() else {
            return false;
        };
        let mut eaten = false;
        let _ = crate::com::boundary(|| {
            use windows::Win32::UI::Input::KeyboardAndMouse::{
                GetKeyState, VK_CONTROL, VK_MENU, VK_SHIFT,
            };
            // SAFETY: read thread keyboard state only. Modified shortcuts are
            // left to their normal handler; no physical events are injected.
            if flags.0 & (1 << 29) != 0
                || unsafe {
                    [VK_CONTROL, VK_MENU, VK_SHIFT]
                        .iter()
                        .any(|key| GetKeyState(key.0 as i32) < 0)
                }
            {
                return Ok(());
            }
            eaten = if test {
                controller.test_preview_key(context, key.0)?
            } else {
                controller.preview_key(context, key.0)?
            };
            Ok(())
        });
        eaten
    }
    #[allow(dead_code)]
    pub(super) fn create() -> ITfKeyEventSink {
        Self::with_session(Weak::new())
    }
    #[allow(dead_code)]
    pub(super) fn with_session(session: Weak<crate::session::TipHostSession>) -> ITfKeyEventSink {
        Self {
            #[cfg(feature = "spike")]
            controller: Weak::new(),
            session,
            owner: thread::current().id(),
            _lease: ObjectLease::new(),
        }
        .into()
    }
    #[allow(dead_code)]
    #[cfg(feature = "spike")]
    pub(super) fn with_controller(controller: Weak<Controller>) -> ITfKeyEventSink {
        Self::with_controller_and_session(controller, Weak::new())
    }
    #[cfg(feature = "spike")]
    pub(super) fn with_controller_and_session(
        controller: Weak<Controller>,
        session: Weak<crate::session::TipHostSession>,
    ) -> ITfKeyEventSink {
        Self {
            controller,
            session,
            owner: thread::current().id(),
            _lease: ObjectLease::new(),
        }
        .into()
    }
}

impl ITfKeyEventSink_Impl for KeySink_Impl {
    fn OnSetFocus(&self, _foreground: BOOL) -> Result<()> {
        if self.owner == thread::current().id() {
            if let Some(session) = self.session.upgrade() {
                session.clear_run();
            }
        }
        #[cfg(feature = "spike")]
        if self.owner == thread::current().id() && !_foreground.as_bool() {
            if let Some(controller) = self.controller.upgrade() {
                return crate::com::boundary(|| controller.revoke());
            }
        }
        Ok(())
    }

    fn OnTestKeyDown(
        &self,
        _context: Ref<ITfContext>,
        _key: WPARAM,
        _flags: LPARAM,
    ) -> Result<BOOL> {
        self.observe(&_context, _key, _flags);
        #[cfg(feature = "spike")]
        return Ok(self.preview_key(_context, _key, _flags, true).into());
        #[cfg(not(feature = "spike"))]
        Ok(false.into())
    }

    fn OnTestKeyUp(&self, _context: Ref<ITfContext>, _key: WPARAM, _flags: LPARAM) -> Result<BOOL> {
        Ok(false.into())
    }

    fn OnKeyDown(&self, _context: Ref<ITfContext>, _key: WPARAM, _flags: LPARAM) -> Result<BOOL> {
        #[cfg(feature = "spike")]
        return Ok(self.preview_key(_context, _key, _flags, false).into());
        #[cfg(not(feature = "spike"))]
        Ok(false.into())
    }

    fn OnKeyUp(&self, _context: Ref<ITfContext>, _key: WPARAM, _flags: LPARAM) -> Result<BOOL> {
        Ok(false.into())
    }

    fn OnPreservedKey(&self, _context: Ref<ITfContext>, _key: *const GUID) -> Result<BOOL> {
        if self.owner != thread::current().id() || _key.is_null() {
            return Ok(false.into());
        }
        if unsafe { *_key } != COMMAND_CONVERT {
            return Ok(false.into());
        }
        let Some(context) = _context.as_ref() else {
            return Ok(false.into());
        };
        #[cfg(feature = "spike")]
        if let Some(controller) = self.controller.upgrade() {
            if controller.has_grant(context) {
                return Ok(crate::com::boundary(|| controller.convert(context))
                    .is_ok()
                    .into());
            }
        }
        if let Some(session) = self.session.upgrade() {
            return session.on_preserved_key(Some(context), unsafe { &*_key });
        }
        Ok(false.into())
    }
}

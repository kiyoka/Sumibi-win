//! A separate sink avoids a TIP -> manager -> TIP reference cycle.
#![allow(non_snake_case)]

use windows::core::{implement, Ref, Result, BOOL, GUID};
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::TextServices::{ITfContext, ITfKeyEventSink, ITfKeyEventSink_Impl};

use crate::com::ObjectLease;
#[cfg(feature = "spike")]
use crate::spike::{Controller, COMMAND_CONVERT};
#[cfg(feature = "spike")]
use std::{
    rc::Weak,
    thread::{self, ThreadId},
};

#[implement(ITfKeyEventSink)]
pub(super) struct KeySink {
    #[cfg(feature = "spike")]
    controller: Weak<Controller>,
    #[cfg(feature = "spike")]
    owner: ThreadId,
    // A manager can retain this sink even after releasing the TIP.
    // Keep the DLL loaded until its last reference has been released.
    _lease: ObjectLease,
}

impl KeySink {
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
    #[cfg(any(test, not(feature = "spike")))]
    pub(super) fn create() -> ITfKeyEventSink {
        Self {
            #[cfg(feature = "spike")]
            controller: Weak::new(),
            #[cfg(feature = "spike")]
            owner: thread::current().id(),
            _lease: ObjectLease::new(),
        }
        .into()
    }
    #[cfg(feature = "spike")]
    pub(super) fn with_controller(controller: Weak<Controller>) -> ITfKeyEventSink {
        Self {
            controller,
            owner: thread::current().id(),
            _lease: ObjectLease::new(),
        }
        .into()
    }
}

impl ITfKeyEventSink_Impl for KeySink_Impl {
    fn OnSetFocus(&self, _foreground: BOOL) -> Result<()> {
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
        #[cfg(feature = "spike")]
        {
            if self.owner != thread::current().id() || _key.is_null() {
                return Ok(false.into());
            }
            // SAFETY: TSF supplies a valid command GUID for this callback.
            if unsafe { *_key } != COMMAND_CONVERT {
                return Ok(false.into());
            }
            let Some(context) = _context.as_ref() else {
                return Ok(false.into());
            };
            let Some(controller) = self.controller.upgrade() else {
                return Ok(false.into());
            };
            Ok(crate::com::boundary(|| controller.convert(context))
                .is_ok()
                .into())
        }
        #[cfg(not(feature = "spike"))]
        Ok(false.into())
    }
}

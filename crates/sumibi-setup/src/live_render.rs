//! Observe only this synthetic host's document, and defer reads out of TSF notifications.
#![allow(non_snake_case)]
use crate::render::{self, Frame, Scene};
use crate::window_host::{RefreshHandle, Window};
use std::cell::{Cell, Ref as Borrow, RefCell};
use std::rc::{Rc, Weak};
use std::thread::{self, ThreadId};
use windows::core::{implement, Interface, Ref, Result};
use windows::Win32::Foundation::{E_ABORT, E_FAIL, RPC_E_WRONG_THREAD};
use windows::Win32::UI::TextServices::*;

struct State {
    context: ITfContext,
    client: u32,
    owner: ThreadId,
    active: Cell<bool>,
    refresh: RefCell<Option<RefreshHandle>>,
    scene: RefCell<Option<Scene>>,
    updates: Cell<usize>,
}
impl State {
    fn check(&self) -> Result<()> {
        if self.owner != thread::current().id() {
            return Err(RPC_E_WRONG_THREAD.into());
        }
        if !self.active.get() {
            return Err(E_ABORT.into());
        }
        Ok(())
    }
    fn schedule(&self) -> Result<()> {
        self.check()?;
        let handle = self.refresh.borrow().clone().ok_or(E_ABORT)?;
        handle.request()
    }
    fn render(&self) -> Result<Frame> {
        self.check()?;
        self.scene.borrow_mut().take();
        let scene = render::capture(&self.context, self.client)?;
        self.check()?;
        let frame = Frame::draw(&scene, "TSF live display")?;
        *self.scene.borrow_mut() = Some(scene);
        self.updates.set(self.updates.get().saturating_add(1));
        Ok(frame)
    }
}

#[implement(ITfTextEditSink)]
struct Observer {
    state: Weak<State>,
}
impl ITfTextEditSink_Impl for Observer_Impl {
    fn OnEndEdit(&self, context: Ref<ITfContext>, _: u32, _: Ref<ITfEditRecord>) -> Result<()> {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let Some(state) = self.state.upgrade() else {
                return Ok(());
            };
            if state.owner != thread::current().id() {
                return Err(RPC_E_WRONG_THREAD.into());
            }
            if !state.active.get() {
                return Ok(());
            }
            if context.ok()?.cast::<windows::core::IUnknown>()?
                != state.context.cast::<windows::core::IUnknown>()?
            {
                return Err(E_ABORT.into());
            }
            // No edit session or paint is invoked from this read-only callback.
            // Attribute-only edits also request a refresh; queued requests merge.
            match state.schedule() {
                Err(error) if error.code() == E_ABORT => Ok(()),
                result => result,
            }
        }))
        .unwrap_or_else(|_| Err(E_FAIL.into()))
    }
}

pub(crate) struct Binding {
    state: Rc<State>,
    source: ITfSource,
    cookie: Option<u32>,
}
impl Binding {
    pub(crate) fn attach(window: &Window, context: &ITfContext, client: u32) -> Result<Self> {
        let state = Rc::new(State {
            context: context.clone(),
            client,
            owner: thread::current().id(),
            active: Cell::new(true),
            refresh: RefCell::new(None),
            scene: RefCell::new(None),
            updates: Cell::new(0),
        });
        let weak = Rc::downgrade(&state);
        let handle =
            window.attach_refresh(Rc::new(move || weak.upgrade().ok_or(E_ABORT)?.render()))?;
        *state.refresh.borrow_mut() = Some(handle);
        let source: ITfSource = match context.cast() {
            Ok(source) => source,
            Err(error) => {
                state.refresh.borrow_mut().take().unwrap().detach();
                return Err(error);
            }
        };
        let sink: ITfTextEditSink = Observer {
            state: Rc::downgrade(&state),
        }
        .into();
        // SAFETY: same-thread source retains an observer with a Weak state link.
        let cookie = match unsafe { source.AdviseSink(&ITfTextEditSink::IID, &sink) } {
            Ok(cookie) => cookie,
            Err(error) => {
                state.refresh.borrow_mut().take().unwrap().detach();
                return Err(error);
            }
        };
        let binding = Self {
            state,
            source,
            cookie: Some(cookie),
        };
        binding.state.schedule()?;
        Ok(binding)
    }
    pub(crate) fn scene(&self) -> Result<Borrow<'_, Scene>> {
        Borrow::filter_map(self.state.scene.borrow(), |scene| scene.as_ref())
            .map_err(|_| E_ABORT.into())
    }
    pub(crate) fn updates(&self) -> usize {
        self.state.updates.get()
    }
    pub(crate) fn request(&self) -> Result<()> {
        self.state.schedule()
    }
    pub(crate) fn close(&mut self) -> Result<()> {
        if self.state.owner != thread::current().id() {
            return Err(RPC_E_WRONG_THREAD.into());
        }
        self.state.active.set(false);
        if let Some(handle) = self.state.refresh.borrow_mut().take() {
            handle.detach();
        }
        self.state.scene.borrow_mut().take();
        if let Some(cookie) = self.cookie {
            // SAFETY: revoke the exact registration; retain cookie for retries.
            unsafe {
                self.source.UnadviseSink(cookie)?;
            }
            self.cookie = None;
        }
        Ok(())
    }
}
impl Drop for Binding {
    fn drop(&mut self) {
        if let Err(error) = self.close() {
            eprintln!("render observer cleanup: {error}");
        }
    }
}

//! Diagnostic ABI for caller-owned ACP stores. The DLL retains every COM
//! target; an explicitly bound EXE transport owns workers and OS/process waits.
#![allow(non_snake_case)]
// Flat COM arguments avoid exposing Rust struct/String layouts across the ABI.
#![allow(clippy::too_many_arguments)]
use crate::com::{boundary, ObjectLease};
use crate::edit::{manual::ManualEdit, EditPolicy, MAX_SELECTION_UNITS};
use crate::spike::Controller;
use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::rc::{Rc, Weak};
use std::thread::{self, ThreadId};
use std::time::{Duration, Instant};
use sumibi_core::{RequestId, Revision};
use windows::core::{
    implement, interface, IUnknown, IUnknown_Vtbl, Interface, Result, GUID, HRESULT,
};
use windows::Win32::Foundation::{E_ABORT, E_INVALIDARG, RPC_E_WRONG_THREAD};
use windows::Win32::UI::TextServices::{ITfContext, ITfThreadMgr};

pub const FUNCTION_TEST_BROKER: GUID = GUID::from_u128(0x80ab9112_0dfd_48a0_9671_88b2c7d441d0);

/// Development-only opt-in, not a production transport/provider ABI. Attach
/// asserts the caller owns this ACP context and uses synthetic/local test text.
#[interface("976f2aa0-84b9-4636-a63d-a0bf0bb063b9")]
pub unsafe trait ISpikeBroker: IUnknown {
    pub fn Attach(&self, context: *mut c_void, transport: *mut c_void) -> HRESULT;
    pub fn Complete(
        &self,
        request: u64,
        input: u64,
        settings: u64,
        authorization: u64,
        text: *const u16,
        length: u32,
    ) -> HRESULT;
    pub fn Cancel(&self, request: u64) -> HRESULT;
    pub fn Pending(&self, request: *mut u64) -> HRESULT;
}

/// Calls are owner-thread, nonblocking and never wait for a broker. Reserve
/// obtains finite transport capacity and a never-reused ID BEFORE text capture.
/// Start borrows bounded UTF-16 for this call only. No Rust allocations or COM
/// targets cross the ABI. Cancel releases a reservation or signals an EXE job.
#[interface("8950b6a6-e17e-4f87-8898-26c49a0f4879")]
pub unsafe trait ISpikeBrokerTransport: IUnknown {
    pub fn Reserve(&self, request: *mut u64) -> HRESULT;
    pub fn Start(
        &self,
        request: u64,
        input: u64,
        settings: u64,
        authorization: u64,
        timeout_ms: u32,
        text: *const u16,
        length: u32,
    ) -> HRESULT;
    pub fn Cancel(&self, request: u64) -> HRESULT;
}

pub(crate) struct Broker {
    context: ITfContext,
    transport: ISpikeBrokerTransport,
    policy: Rc<Cell<EditPolicy>>,
    ticket: RefCell<Option<ManualEdit>>,
    last_request: Rc<Cell<u64>>,
}
impl Broker {
    fn new(
        context: ITfContext,
        transport: ISpikeBrokerTransport,
        last_request: Rc<Cell<u64>>,
    ) -> Self {
        Self {
            context,
            transport,
            policy: Rc::new(Cell::new(EditPolicy {
                // Fixture policy only; this does not represent product consent or
                // settings. Every Host boot/client lifetime has a fresh OS-bound ID.
                revision: Revision {
                    input: 1,
                    settings: 1,
                    authorization: 1,
                },
                allowed: true,
            })),
            ticket: RefCell::new(None),
            last_request,
        }
    }
    pub(crate) fn pending(&self) -> bool {
        self.ticket.borrow().is_some()
    }
    pub(crate) fn begin(&self, manager: &ITfThreadMgr, client: u32) -> Result<()> {
        if self.pending() || !self.policy.get().allowed {
            return Err(E_ABORT.into());
        }
        let mut id = 0;
        // SAFETY: explicitly bound owner-thread transport and writable scalar.
        unsafe {
            self.transport.Reserve(&mut id).ok()?;
        }
        if id == 0 || id <= self.last_request.get() {
            unsafe {
                let _ = self.transport.Cancel(id);
            }
            return Err(E_ABORT.into());
        }
        self.last_request.set(id);
        let result = (|| {
            let deadline = Instant::now() + Duration::from_secs(5);
            let ticket = ManualEdit::capture(
                manager,
                &self.context,
                client,
                self.policy.clone(),
                RequestId(id.into()),
                id.into(),
                deadline,
            )?;
            let text: Vec<u16> = ticket.source()?.encode_utf16().collect();
            let revision = ticket.revision();
            *self.ticket.borrow_mut() = Some(ticket);
            let timeout_ms = deadline
                .saturating_duration_since(Instant::now())
                .as_millis() as u32;
            if timeout_ms == 0 {
                return Err(E_ABORT.into());
            }
            // SAFETY: bounded owned buffer borrowed for this synchronous call;
            // EXE must copy it before returning. No refcell borrow crosses COM.
            unsafe {
                self.transport
                    .Start(
                        id,
                        revision.input,
                        revision.settings,
                        revision.authorization,
                        timeout_ms,
                        text.as_ptr(),
                        text.len() as u32,
                    )
                    .ok()?;
            }
            if !self.policy.get().allowed || !self.pending() {
                return Err(E_ABORT.into());
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = self.cancel(Some(id));
        }
        result
    }
    pub(crate) fn cancel(&self, request: Option<u64>) -> Result<()> {
        let id = self
            .ticket
            .borrow()
            .as_ref()
            .map(|ticket| ticket.request().0 as u64);
        if let Some(id) = id {
            if request.is_some_and(|expected| expected != id) {
                return Err(E_ABORT.into());
            }
            let ticket = self.ticket.borrow_mut().take();
            // Retire the local target BEFORE transport cancellation; cancellation
            // is independent of success delivering anything to the child.
            drop(ticket);
            unsafe {
                self.transport.Cancel(id).ok()?;
            }
        } else if let Some(request) = request {
            // Also releases a reservation when capture failed before a ticket.
            unsafe {
                self.transport.Cancel(request).ok()?;
            }
        }
        Ok(())
    }
    pub(crate) fn close(&self) {
        let mut policy = self.policy.get();
        policy.allowed = false;
        self.policy.set(policy);
        let _ = self.cancel(None);
    }
    fn request(&self) -> u64 {
        self.ticket
            .borrow()
            .as_ref()
            .map_or(0, |ticket| ticket.request().0 as u64)
    }
    pub(crate) fn complete(
        &self,
        manager: &ITfThreadMgr,
        client: u32,
        id: u64,
        revision: Revision,
        text: String,
    ) -> Result<()> {
        if id == 0 || self.request() != id {
            return Err(E_ABORT.into());
        }
        let ticket = self.ticket.borrow_mut().take().ok_or(E_ABORT)?;
        ticket.finish(manager, client, RequestId(id.into()), revision, text)
    }
}
impl Drop for Broker {
    fn drop(&mut self) {
        self.close();
    }
}

#[implement(ISpikeBroker)]
struct Control {
    state: Weak<Controller>,
    owner: ThreadId,
    _lease: ObjectLease,
}
impl Control_Impl {
    fn state(&self) -> Result<Rc<Controller>> {
        if self.owner != thread::current().id() {
            return Err(RPC_E_WRONG_THREAD.into());
        }
        self.state.upgrade().ok_or_else(|| E_ABORT.into())
    }
}
impl ISpikeBroker_Impl for Control_Impl {
    unsafe fn Attach(&self, context: *mut c_void, transport: *mut c_void) -> HRESULT {
        boundary(|| {
            let state = self.state()?;
            // SAFETY: explicit test caller supplies borrowed interface pointers.
            let context = unsafe { ITfContext::from_raw_borrowed(&context) }.ok_or(E_INVALIDARG)?;
            let transport = unsafe { ISpikeBrokerTransport::from_raw_borrowed(&transport) }
                .ok_or(E_INVALIDARG)?;
            state.grant(context)?;
            *state.broker.borrow_mut() = Some(Rc::new(Broker::new(
                context.clone(),
                transport.clone(),
                state.broker_request_floor.clone(),
            )));
            Ok(())
        })
        .into()
    }
    unsafe fn Complete(
        &self,
        request: u64,
        input: u64,
        settings: u64,
        authorization: u64,
        text: *const u16,
        length: u32,
    ) -> HRESULT {
        boundary(|| {
            let state = self.state()?;
            let matches = state
                .broker
                .borrow()
                .as_ref()
                .is_some_and(|broker| broker.request() == request && request != 0);
            if !matches {
                return Err(E_ABORT.into());
            }
            let result = (|| {
                if text.is_null() || length == 0 || length as usize > MAX_SELECTION_UNITS {
                    return Err(E_INVALIDARG.into());
                }
                // SAFETY: caller owns length readable UTF-16 units for this call.
                let text = String::from_utf16(unsafe {
                    std::slice::from_raw_parts(text, length as usize)
                })
                .map_err(|_| windows::core::Error::from(E_INVALIDARG))?;
                state.complete_broker(
                    request,
                    Revision {
                        input,
                        settings,
                        authorization,
                    },
                    text,
                )
            })();
            if result.is_err() {
                let broker = state.broker.borrow().clone();
                if let Some(broker) = broker {
                    let _ = broker.cancel(Some(request));
                }
            }
            result
        })
        .into()
    }
    unsafe fn Cancel(&self, request: u64) -> HRESULT {
        boundary(|| {
            let state = self.state()?;
            let broker = state.broker.borrow().clone();
            if let Some(broker) = broker {
                broker.cancel((request != 0).then_some(request))?;
            }
            Ok(())
        })
        .into()
    }
    unsafe fn Pending(&self, request: *mut u64) -> HRESULT {
        boundary(|| {
            let state = self.state()?;
            if request.is_null() {
                return Err(E_INVALIDARG.into());
            }
            let id = state
                .broker
                .borrow()
                .as_ref()
                .map_or(0, |broker| broker.request());
            unsafe {
                request.write(id);
            }
            Ok(())
        })
        .into()
    }
}
pub(crate) fn control(state: &Rc<Controller>) -> IUnknown {
    let control: ISpikeBroker = Control {
        state: Rc::downgrade(state),
        owner: thread::current().id(),
        _lease: ObjectLease::new(),
    }
    .into();
    unsafe { IUnknown::from_raw(control.into_raw()) }
}

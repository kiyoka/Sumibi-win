//! EXE-owned worker bridge for the DLL's diagnostic ABI. No DLL reference,
//! COM target, callback pointer or Rust allocation is retained by a worker.
#![allow(non_snake_case)]
use std::cell::{Cell, RefCell};
use std::os::windows::io::AsRawHandle;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::{self, ThreadId};
use std::time::{Duration, Instant};
use sumibi_ipc::bootstrap::Mode;
use sumibi_ipc::job::{Request, Reservation};
use sumibi_ipc::worker::{Permit, Worker};
use sumibi_protocol::message::Revision;
use sumibi_tip::spike::{ISpikeBroker, ISpikeBrokerTransport, ISpikeBrokerTransport_Impl};
use windows::core::{implement, ComObject, Result, HRESULT};
use windows::Win32::Foundation::{E_ABORT, E_INVALIDARG, E_UNEXPECTED, HANDLE, RPC_E_WRONG_THREAD};

#[implement(ISpikeBrokerTransport)]
pub(crate) struct Bridge {
    owner: ThreadId,
    executable: PathBuf,
    mode: Mode,
    reserved: RefCell<Option<(u64, Permit)>>,
    worker: RefCell<Option<Worker>>,
    pending: Cell<Option<u64>>,
    exchange: Cell<Option<(u32, u64)>>,
    input_check: RefCell<Option<InputCheck>>,
    checking_input: Cell<bool>,
    delivering: Cell<bool>,
    // Scalar diagnostics for the owned test host; never store request text.
    last_worker_error: Cell<Option<sumibi_ipc::Error>>,
    last_dll_error: Cell<Option<HRESULT>>,
}
pub(crate) type InputCheck = Box<dyn FnOnce(&[u16]) -> Result<()>>;
static NEXT_REQUEST: AtomicU64 = AtomicU64::new(0);
struct Delivering<'a>(&'a Cell<bool>);
impl Drop for Delivering<'_> {
    fn drop(&mut self) {
        self.0.set(false);
    }
}
impl Bridge {
    pub(crate) fn new(
        mode: Mode,
    ) -> std::result::Result<ComObject<Self>, Box<dyn std::error::Error>> {
        // This diagnostic bridge uses authenticated desktop IPC. Reject an
        // unsupported caller before a TSF request can be accepted or a worker
        // can be started, so AppContainer execution is not mistaken for a
        // conversion or waiting-order failure.
        let _ = sumibi_ipc::PinnedProcess::current()?;
        let executable = std::env::current_exe()?
            .parent()
            .ok_or("missing host directory")?
            .join("sumibi-host.exe");
        if !executable.is_file() {
            return Err("build sumibi-host with --features spike first".into());
        }
        Ok(ComObject::new(Self {
            owner: thread::current().id(),
            executable,
            mode,
            reserved: RefCell::new(None),
            worker: RefCell::new(None),
            pending: Cell::new(None),
            exchange: Cell::new(None),
            input_check: RefCell::new(None),
            checking_input: Cell::new(false),
            delivering: Cell::new(false),
            last_worker_error: Cell::new(None),
            last_dll_error: Cell::new(None),
        }))
    }
    fn check(&self) -> Result<()> {
        if self.owner != thread::current().id() {
            return Err(RPC_E_WRONG_THREAD.into());
        }
        Ok(())
    }
    /// Handle remains owned by this bridge until drain/shutdown on this thread.
    /// A Cancel callback only signals it; it does not remove the pending work.
    pub(crate) fn wait_handle(&self) -> Option<HANDLE> {
        self.pending.get()?;
        self.worker
            .borrow()
            .as_ref()
            .map(|worker| HANDLE(worker.completion_handle().as_raw_handle()))
    }
    pub(crate) fn active(&self) -> bool {
        self.delivering.get() || self.pending.get().is_some() || self.reserved.borrow().is_some()
    }
    pub(crate) fn last_exchange(&self) -> Option<(u32, u64)> {
        self.exchange.get()
    }
    pub(crate) fn pending_id(&self) -> Option<u64> {
        self.pending.get()
    }
    pub(crate) fn last_failure(&self) -> (Option<sumibi_ipc::Error>, Option<HRESULT>) {
        (self.last_worker_error.get(), self.last_dll_error.get())
    }
    /// Owner-only, one synchronous attempt. Drop all captured targets before a
    /// broker I/O can start; no checker or COM reference enters Request/Worker.
    pub(crate) fn check_next_input(&self, check: InputCheck) -> Result<()> {
        self.check()?;
        if self.active() || self.checking_input.get() || self.input_check.borrow().is_some() {
            return Err(E_ABORT.into());
        }
        *self.input_check.borrow_mut() = Some(check);
        Ok(())
    }
    pub(crate) fn clear_input_check(&self) {
        self.input_check.borrow_mut().take();
    }
    /// Owner-loop only, outside WndProc. The worker retains an idle connection;
    /// the completion's permit spans every reentrant DLL call below.
    pub(crate) fn drain(&self, control: &ISpikeBroker) -> Result<Option<Delivery>> {
        self.check()?;
        if self.delivering.get() {
            return Err(E_ABORT.into());
        }
        let Some(pending) = self.pending.get() else {
            return Ok(None);
        };
        self.delivering.set(true);
        let _delivery = Delivering(&self.delivering);
        let received = self.worker.borrow().as_ref().ok_or(E_ABORT)?.try_result();
        let completion = match received {
            Ok(Some(completion)) => completion,
            Ok(None) => return Ok(None),
            Err(error) => {
                self.last_worker_error.set(Some(error));
                self.pending.set(None);
                unsafe {
                    let _ = control.Cancel(pending);
                }
                return Ok(Some(Delivery::Discarded));
            }
        };
        if completion.id != pending {
            // Never retire a newer target on an unrelated completion.
            return Ok(None);
        }
        self.pending.set(None);
        self.exchange.set(completion.exchange());
        let result = match completion.outcome() {
            Ok(text) => {
                let text: Vec<u16> = text.encode_utf16().collect();
                // SAFETY: owner STA, valid independently counted DLL control,
                // borrowed bounded result; no EXE RefCell borrow across COM.
                let result = unsafe {
                    control
                        .Complete(
                            completion.id,
                            completion.revision.input,
                            completion.revision.settings,
                            completion.revision.authorization,
                            text.as_ptr(),
                            text.len() as u32,
                        )
                        .ok()
                };
                if let Err(error) = &result {
                    self.last_dll_error.set(Some(error.code()));
                }
                result
            }
            Err(error) => {
                self.last_worker_error.set(Some(error));
                Err(E_ABORT.into())
            }
        };
        if result.is_err() {
            unsafe {
                let _ = control.Cancel(completion.id);
            }
        }
        Ok(Some(if result.is_ok() {
            Delivery::Applied
        } else {
            Delivery::Discarded
        }))
    }
    /// Explicit teardown outside COM/window callbacks. Worker code belongs to
    /// the EXE, so even a failing teardown cannot execute unloaded DLL code.
    pub(crate) fn shutdown(&self) -> Result<()> {
        self.check()?;
        if self.delivering.get() {
            return Err(E_ABORT.into());
        }
        self.clear_input_check();
        self.reserved.borrow_mut().take();
        self.pending.set(None);
        let worker = self.worker.borrow_mut().take();
        if let Some(mut worker) = worker {
            worker
                .shutdown(Duration::from_secs(3))
                .map_err(|_| windows::core::Error::from(E_ABORT))?;
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Delivery {
    Applied,
    Discarded,
}

fn boundary(action: impl FnOnce() -> Result<()>) -> HRESULT {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(action))
        .unwrap_or_else(|_| Err(E_UNEXPECTED.into()))
        .into()
}
impl ISpikeBrokerTransport_Impl for Bridge_Impl {
    unsafe fn Reserve(&self, output: *mut u64) -> HRESULT {
        boundary(|| {
            self.check()?;
            if output.is_null() {
                return Err(E_INVALIDARG.into());
            }
            unsafe {
                output.write(0);
            }
            if self.active() || self.checking_input.get() {
                return Err(E_ABORT.into());
            }
            let needs_worker = self
                .worker
                .borrow()
                .as_ref()
                .is_none_or(|worker| worker.is_closed());
            if needs_worker {
                // Dropping an old facade only signals; its EXE thread keeps
                // the global slot until child/pipe cleanup actually finishes.
                self.worker.borrow_mut().take();
                let reservation =
                    Reservation::acquire().map_err(|_| windows::core::Error::from(E_ABORT))?;
                let worker = Worker::new(reservation, self.executable.clone(), self.mode)
                    .map_err(|_| windows::core::Error::from(E_ABORT))?;
                *self.worker.borrow_mut() = Some(worker);
            }
            let id = NEXT_REQUEST
                .try_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                    value.checked_add(1)
                })
                .map_err(|_| windows::core::Error::from(E_ABORT))?
                + 1;
            let permit = self
                .worker
                .borrow()
                .as_ref()
                .ok_or(E_ABORT)?
                .reserve(id)
                .map_err(|_| windows::core::Error::from(E_ABORT))?;
            *self.reserved.borrow_mut() = Some((id, permit));
            unsafe {
                output.write(id);
            }
            Ok(())
        })
    }
    unsafe fn Start(
        &self,
        request: u64,
        input: u64,
        settings: u64,
        authorization: u64,
        timeout_ms: u32,
        text: *const u16,
        length: u32,
    ) -> HRESULT {
        boundary(|| {
            self.check()?;
            if request == 0
                || self.checking_input.get()
                || self.reserved.borrow().as_ref().map(|(id, _)| *id) != Some(request)
                || self.pending.get().is_some()
            {
                return Err(E_ABORT.into());
            }
            if text.is_null() || length == 0 || length > 1024 || !(1..=5000).contains(&timeout_ms) {
                return Err(E_INVALIDARG.into());
            }
            // Copy/checker time belongs to the same budget as dispatch and I/O.
            let deadline = Instant::now() + Duration::from_millis(timeout_ms.into());
            // SAFETY: diagnostic caller borrows readable UTF-16 for this call.
            let units = unsafe { std::slice::from_raw_parts(text, length as usize) };
            let text =
                String::from_utf16(units).map_err(|_| windows::core::Error::from(E_INVALIDARG))?;
            let check = self.input_check.borrow_mut().take();
            if let Some(check) = check {
                // Metadata inspection calls COM and can reenter the bridge.
                // A consumed checker must not expose an unchecked Start.
                self.checking_input.set(true);
                let checked = boundary(|| check(units)).ok();
                self.checking_input.set(false);
                if let Err(error) = checked {
                    // A failed one-shot check also retires its reservation;
                    // retrying Start cannot bypass a consumed check.
                    self.reserved.borrow_mut().take();
                    return Err(error);
                }
            }
            let (_, permit) = self.reserved.borrow_mut().take().ok_or(E_ABORT)?;
            self.worker
                .borrow()
                .as_ref()
                .ok_or(E_ABORT)?
                .submit(
                    permit,
                    Request {
                        id: request,
                        revision: Revision {
                            input,
                            settings,
                            authorization,
                        },
                        text,
                        deadline,
                        mode: self.mode,
                    },
                )
                .map_err(|_| windows::core::Error::from(E_ABORT))?;
            self.last_worker_error.set(None);
            self.last_dll_error.set(None);
            self.pending.set(Some(request));
            Ok(())
        })
    }
    unsafe fn Cancel(&self, request: u64) -> HRESULT {
        boundary(|| {
            self.check()?;
            // Match while the pre-capture permit still exists; removing it
            // first would lose the worker's request identity on cancellation.
            if let Some(worker) = self.worker.borrow().as_ref() {
                worker
                    .cancel(request)
                    .map_err(|_| windows::core::Error::from(E_ABORT))?;
            }
            if self
                .reserved
                .borrow()
                .as_ref()
                .is_some_and(|(id, _)| *id == request)
            {
                self.reserved.borrow_mut().take();
                self.clear_input_check();
            }
            Ok(())
        })
    }
}

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
use sumibi_ipc::job::{BrokerJob, Request, Reservation};
use sumibi_protocol::message::Revision;
use sumibi_tip::spike::{ISpikeBroker, ISpikeBrokerTransport, ISpikeBrokerTransport_Impl};
use windows::core::{implement, ComObject, Result, HRESULT};
use windows::Win32::Foundation::{E_ABORT, E_INVALIDARG, E_UNEXPECTED, HANDLE, RPC_E_WRONG_THREAD};

#[implement(ISpikeBrokerTransport)]
pub(crate) struct Bridge {
    owner: ThreadId,
    executable: PathBuf,
    mode: Mode,
    reserved: RefCell<Option<(u64, Reservation)>>,
    job: RefCell<Option<(u64, BrokerJob)>>,
    input_check: RefCell<Option<InputCheck>>,
    checking_input: Cell<bool>,
}
pub(crate) type InputCheck = Box<dyn FnOnce(&[u16]) -> Result<()>>;
static NEXT_REQUEST: AtomicU64 = AtomicU64::new(0);
impl Bridge {
    pub(crate) fn new(
        mode: Mode,
    ) -> std::result::Result<ComObject<Self>, Box<dyn std::error::Error>> {
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
            job: RefCell::new(None),
            input_check: RefCell::new(None),
            checking_input: Cell::new(false),
        }))
    }
    fn check(&self) -> Result<()> {
        if self.owner != thread::current().id() {
            return Err(RPC_E_WRONG_THREAD.into());
        }
        Ok(())
    }
    /// Handle remains owned by this bridge until drain/shutdown on this thread.
    /// A Cancel callback only signals it; it does not remove the active job.
    pub(crate) fn wait_handle(&self) -> Option<HANDLE> {
        self.job
            .borrow()
            .as_ref()
            .map(|(_, job)| HANDLE(job.completion_handle().as_raw_handle()))
    }
    pub(crate) fn active(&self) -> bool {
        self.job.borrow().is_some() || self.reserved.borrow().is_some()
    }
    /// Owner-only, one synchronous attempt. Drop all captured targets before a
    /// worker can start; no checker or COM reference enters Request/BrokerJob.
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
    /// Owner-loop only, outside WndProc. A completed worker has already reaped
    /// its child; drop never waits or schedules a DLL callback.
    pub(crate) fn drain(&self, control: &ISpikeBroker) -> Result<Option<Delivery>> {
        self.check()?;
        let completion = self
            .job
            .borrow()
            .as_ref()
            .and_then(|(_, job)| job.try_result());
        let Some(completion) = completion else {
            return Ok(None);
        };
        let job = self.job.borrow_mut().take();
        drop(job);
        let result = match completion.outcome() {
            Ok(text) => {
                let text: Vec<u16> = text.encode_utf16().collect();
                // SAFETY: owner STA, valid independently counted DLL control,
                // borrowed bounded result; no EXE RefCell borrow across COM.
                unsafe {
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
                }
            }
            Err(_) => Err(E_ABORT.into()),
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
        self.clear_input_check();
        self.reserved.borrow_mut().take();
        let job = self.job.borrow_mut().take();
        if let Some((_, mut job)) = job {
            job.shutdown(Duration::from_secs(3))
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
            let reservation =
                Reservation::acquire().map_err(|_| windows::core::Error::from(E_ABORT))?;
            let id = NEXT_REQUEST
                .try_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                    value.checked_add(1)
                })
                .map_err(|_| windows::core::Error::from(E_ABORT))?
                + 1;
            *self.reserved.borrow_mut() = Some((id, reservation));
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
                || self.job.borrow().is_some()
            {
                return Err(E_ABORT.into());
            }
            if text.is_null() || length == 0 || length > 1024 || !(1..=5000).contains(&timeout_ms) {
                return Err(E_INVALIDARG.into());
            }
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
            let (_, reservation) = self.reserved.borrow_mut().take().ok_or(E_ABORT)?;
            let job = reservation
                .start(
                    self.executable.clone(),
                    Request {
                        id: request,
                        revision: Revision {
                            input,
                            settings,
                            authorization,
                        },
                        text,
                        deadline: Instant::now() + Duration::from_millis(timeout_ms.into()),
                        mode: self.mode,
                    },
                )
                .map_err(|_| windows::core::Error::from(E_ABORT))?;
            *self.job.borrow_mut() = Some((request, job));
            Ok(())
        })
    }
    unsafe fn Cancel(&self, request: u64) -> HRESULT {
        boundary(|| {
            self.check()?;
            if self
                .reserved
                .borrow()
                .as_ref()
                .is_some_and(|(id, _)| *id == request)
            {
                self.reserved.borrow_mut().take();
                self.clear_input_check();
            }
            if let Some((id, job)) = self.job.borrow().as_ref() {
                if *id == request {
                    job.cancel()
                        .map_err(|_| windows::core::Error::from(E_ABORT))?;
                }
            }
            Ok(())
        })
    }
}

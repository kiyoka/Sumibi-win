//! Fixed display attribute definition. No persistent category registration yet.
#![allow(non_snake_case)]

use crate::com::{boundary, ObjectLease};
use std::cell::Cell;
use std::thread::{self, ThreadId};
use windows::core::{implement, Error, Result, BSTR, GUID};
use windows::Win32::Foundation::{E_INVALIDARG, E_NOTIMPL, E_POINTER, RPC_E_WRONG_THREAD, S_FALSE};
use windows::Win32::UI::TextServices::*;

pub const GUID_INPUT_ATTRIBUTE: GUID = GUID::from_u128(0x0a33192e_c875_4b26_aaee_5a5a4b020724);

fn attribute() -> TF_DISPLAYATTRIBUTE {
    TF_DISPLAYATTRIBUTE {
        lsStyle: TF_LS_DOT,
        bAttr: TF_ATTR_INPUT,
        ..Default::default()
    }
}

#[implement(ITfDisplayAttributeInfo)]
struct Info {
    owner: ThreadId,
    _lease: ObjectLease,
}

fn info() -> ITfDisplayAttributeInfo {
    Info {
        owner: thread::current().id(),
        _lease: ObjectLease::new(),
    }
    .into()
}

impl Info {
    fn check(&self) -> Result<()> {
        if self.owner != thread::current().id() {
            return Err(RPC_E_WRONG_THREAD.into());
        }
        Ok(())
    }
}

impl ITfDisplayAttributeInfo_Impl for Info_Impl {
    fn GetGUID(&self) -> Result<GUID> {
        self.check()?;
        Ok(GUID_INPUT_ATTRIBUTE)
    }
    fn GetDescription(&self) -> Result<BSTR> {
        self.check()?;
        Ok(BSTR::from("Sumibi input underline"))
    }
    fn GetAttributeInfo(&self, output: *mut TF_DISPLAYATTRIBUTE) -> Result<()> {
        boundary(|| {
            self.check()?;
            if output.is_null() {
                return Err(E_POINTER.into());
            }
            // SAFETY: COM caller supplies a writable attribute output.
            unsafe { output.write(attribute()) };
            Ok(())
        })
    }
    fn SetAttributeInfo(&self, _: *const TF_DISPLAYATTRIBUTE) -> Result<()> {
        self.check()?;
        // This first definition is read-only; never pretend to persist changes.
        Err(E_NOTIMPL.into())
    }
    fn Reset(&self) -> Result<()> {
        self.check()
    }
}

#[implement(IEnumTfDisplayAttributeInfo)]
struct Enumerator {
    exhausted: Cell<bool>,
    owner: ThreadId,
    _lease: ObjectLease,
}

pub(super) fn enumerate() -> IEnumTfDisplayAttributeInfo {
    enumerator(false)
}
fn enumerator(exhausted: bool) -> IEnumTfDisplayAttributeInfo {
    Enumerator {
        exhausted: Cell::new(exhausted),
        owner: thread::current().id(),
        _lease: ObjectLease::new(),
    }
    .into()
}

impl Enumerator {
    fn check(&self) -> Result<()> {
        if self.owner != thread::current().id() {
            return Err(RPC_E_WRONG_THREAD.into());
        }
        Ok(())
    }
}

impl IEnumTfDisplayAttributeInfo_Impl for Enumerator_Impl {
    fn Clone(&self) -> Result<IEnumTfDisplayAttributeInfo> {
        self.check()?;
        Ok(enumerator(self.exhausted.get()))
    }
    fn Next(
        &self,
        count: u32,
        output: *mut Option<ITfDisplayAttributeInfo>,
        fetched: *mut u32,
    ) -> Result<()> {
        boundary(|| {
            self.check()?;
            if !fetched.is_null() {
                // SAFETY: optional COM output slot.
                unsafe { fetched.write(0) };
            }
            if (count != 0 && output.is_null()) || (count != 1 && fetched.is_null()) {
                return Err(E_POINTER.into());
            }
            if count == 0 {
                return Ok(());
            }
            let available = !self.exhausted.get();
            if available {
                let value = info();
                // SAFETY: caller provides count output slots; only one is filled.
                unsafe {
                    output.write(Some(value));
                    if !fetched.is_null() {
                        fetched.write(1);
                    }
                }
                self.exhausted.set(true);
            }
            if count == 1 && available {
                Ok(())
            } else {
                // The generated COM adapter converts this back to S_FALSE;
                // enumeration exhaustion is a successful HRESULT, not E_FAIL.
                Err(Error::from_hresult(S_FALSE))
            }
        })
    }
    fn Reset(&self) -> Result<()> {
        self.check()?;
        self.exhausted.set(false);
        Ok(())
    }
    fn Skip(&self, count: u32) -> Result<()> {
        self.check()?;
        if count == 0 {
            return Ok(());
        }
        let available = !self.exhausted.replace(true);
        if count == 1 && available {
            Ok(())
        } else {
            Err(Error::from_hresult(S_FALSE))
        }
    }
}

pub(super) fn get(guid: *const GUID) -> Result<ITfDisplayAttributeInfo> {
    if guid.is_null() {
        return Err(E_POINTER.into());
    }
    // SAFETY: COM caller supplies a readable GUID.
    if unsafe { *guid } != GUID_INPUT_ATTRIBUTE {
        return Err(E_INVALIDARG.into());
    }
    Ok(info())
}

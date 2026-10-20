//! Windows TSF activation spike. Keys pass through; no editing or registration yet.
#![cfg(windows)]
#![deny(unsafe_op_in_unsafe_fn)]

mod com;
pub mod display;
pub mod edit;
mod keys;
#[cfg(test)]
mod mock;
#[cfg(feature = "spike")]
mod notifications;
#[cfg(feature = "spike")]
pub mod spike;

pub use com::{DllCanUnloadNow, DllGetClassObject, CLSID_SUMIBI_TIP};

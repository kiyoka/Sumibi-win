//! Windows TSF activation boundary and development-only guarded editing spike.
#![cfg(windows)]
#![deny(unsafe_op_in_unsafe_fn)]

#[cfg(feature = "spike")]
mod broker_spike;
mod com;
mod composition;
pub mod display;
pub mod edit;
mod keys;
#[cfg(test)]
mod mock;
#[cfg(feature = "spike")]
mod notifications;
#[cfg(feature = "spike")]
mod owned_composition;
mod protection;
#[cfg(feature = "spike")]
pub mod spike;

pub use com::{DllCanUnloadNow, DllGetClassObject, CLSID_SUMIBI_TIP};

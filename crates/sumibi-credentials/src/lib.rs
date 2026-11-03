//! Windows per-user generic credential store for the Host only.
//! Loading a key does not grant consent or authorize network transmission.
#[cfg(windows)]
mod windows_store;

#[cfg(windows)]
pub use windows_store::{delete, load, save, StoreError};

//! Registration transaction policy. This library does not write COM or TSF settings.
#[cfg(any(windows, test))]
pub mod install_journal;
#[cfg(windows)]
pub mod install_store;
pub mod install_transaction;

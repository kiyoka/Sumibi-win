//! Registration transaction policy. This library does not write COM or TSF settings.
#[cfg(any(windows, test))]
pub mod install_journal;
pub mod install_transaction;

//! Host-owned manual conversion boundary. No COM, network implementation,
//! persistent settings or credential-store access is contained here.
#![forbid(unsafe_code)]

#[cfg(windows)]
pub mod broker_policy;
#[cfg(windows)]
pub mod broker_runtime;
#[cfg(all(test, windows))]
pub(crate) static HOST_PIPE_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
pub mod diagnostic_http;
pub mod manual;
pub mod policy;

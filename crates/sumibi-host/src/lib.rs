//! Host-owned policy, settings and bounded manual conversion workers.
//! Unsafe Windows I/O remains in dedicated boundary crates.
#![forbid(unsafe_code)]

#[cfg(windows)]
pub mod broker_boot;
#[cfg(windows)]
pub mod broker_policy;
#[cfg(windows)]
pub mod broker_runtime;
#[cfg(windows)]
pub mod broker_workers;
#[cfg(all(test, windows))]
pub(crate) static HOST_PIPE_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
#[cfg(windows)]
pub mod consent_task;
#[cfg(windows)]
pub mod credential_store;
pub mod decisions;
pub mod diagnostic_http;
pub mod first_conversion;
#[cfg(windows)]
pub mod host_control;
#[cfg(windows)]
pub mod host_service;
pub mod local_dictionary;
pub mod manual;
#[cfg(windows)]
pub mod native_https;
pub mod policy;
pub mod provider;
pub mod settings;
#[cfg(windows)]
pub mod settings_task;
#[cfg(windows)]
pub mod startup;

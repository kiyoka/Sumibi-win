//! Desktop IPC building blocks. Run transport waits on a worker, never TSF.
//! Peer process handles must come from a trusted launch/bootstrap path. Names,
//! protocol IDs, PID arguments and matching user SIDs are not proof of a peer.
#![deny(unsafe_op_in_unsafe_fn)]

#[cfg(windows)]
pub mod authenticode;
#[cfg(windows)]
pub mod bootstrap;
#[cfg(windows)]
pub mod broker;
#[cfg(windows)]
pub mod channel;
#[cfg(windows)]
pub mod development_placement;
#[cfg(windows)]
pub mod discovery;
#[cfg(windows)]
pub mod dll_exports;
#[cfg(windows)]
pub mod export_resolver;
#[cfg(windows)]
pub mod framed;
#[cfg(windows)]
pub mod handoff;
#[cfg(windows)]
pub mod host_image;
#[cfg(windows)]
pub mod host_instance;
#[cfg(windows)]
mod identity;
#[cfg(windows)]
pub mod job;
#[cfg(windows)]
pub mod launch_probe;
#[cfg(windows)]
mod pipe;
#[cfg(windows)]
pub mod protected_acl;
#[cfg(windows)]
mod support;
#[cfg(windows)]
pub mod system_module;
#[cfg(windows)]
pub mod worker;

#[cfg(windows)]
pub use identity::PinnedProcess;
#[cfg(windows)]
pub use pipe::{
    bootstrap_pipe_name_for_sid, current_bootstrap_pipe_name, wait_any, AcceptTask,
    BootstrapServer, Completion, Operation, Pipe, ReadBatch, ReadRequest, Server, SharedAnchor,
    Stop, WriteBatch, WriteRequest, MAX_ENDPOINTS,
};
#[cfg(windows)]
pub use support::{Error, Nonce, Result};

#[cfg(windows)]
#[inline]
pub(crate) fn debug_log_ipc(msg: &str) {
    let wide: Vec<u16> = format!("[Sumibi-IPC] {msg}\0").encode_utf16().collect();
    unsafe {
        windows::Win32::System::Diagnostics::Debug::OutputDebugStringW(windows::core::PCWSTR(
            wide.as_ptr(),
        ));
    }
}

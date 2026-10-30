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
pub mod framed;
#[cfg(windows)]
pub mod handoff;
#[cfg(windows)]
mod identity;
#[cfg(windows)]
pub mod job;
#[cfg(windows)]
mod pipe;
#[cfg(windows)]
pub mod protected_acl;
#[cfg(windows)]
mod support;
#[cfg(windows)]
pub mod worker;

#[cfg(windows)]
pub use identity::PinnedProcess;
#[cfg(windows)]
pub use pipe::{
    wait_any, AcceptTask, Completion, Operation, Pipe, ReadBatch, ReadRequest, Server,
    SharedAnchor, Stop, WriteBatch, WriteRequest, MAX_ENDPOINTS,
};
#[cfg(windows)]
pub use support::{Error, Nonce, Result};

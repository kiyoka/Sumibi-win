//! Desktop IPC building blocks. Run transport waits on a worker, never TSF.
//! Peer process handles must come from a trusted launch/bootstrap path. Names,
//! protocol IDs, PID arguments and matching user SIDs are not proof of a peer.
#![deny(unsafe_op_in_unsafe_fn)]

#[cfg(windows)]
mod identity;
#[cfg(windows)]
mod pipe;
#[cfg(windows)]
mod support;

#[cfg(windows)]
pub use identity::PinnedProcess;
#[cfg(windows)]
pub use pipe::{Completion, Operation, Pipe, Server, Stop, MAX_ENDPOINTS};
#[cfg(windows)]
pub use support::{Error, Nonce, Result};

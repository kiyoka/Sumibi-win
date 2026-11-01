//! Logon-scoped mutual discovery between Host and TIP (ADR 0085).
//!
//! Uses a logon-bound bootstrap pipe exclusively for one-shot handoff,
//! never for text input or payload transfer.

use crate::handoff::{self, Offer};
use crate::identity::PinnedProcess;
use crate::pipe::{BootstrapServer, Pipe, Stop};
use crate::support::{Nonce, Result};
use std::time::Instant;

pub use crate::pipe::BootstrapServer as Server;
pub use crate::pipe::{bootstrap_pipe_name_for_sid, current_bootstrap_pipe_name};

/// Connects to the logon-scoped bootstrap pipe, authenticates the pinned Host peer,
/// and receives a one-shot private peer offer.
pub fn discover_offer(stop: &Stop, deadline: Instant) -> Result<(Offer, PinnedProcess)> {
    let pipe_name = current_bootstrap_pipe_name()?;
    let (pipe, host_peer) = Pipe::connect_bootstrap(&pipe_name, stop, deadline)?;
    let offer = handoff::receive(pipe, &host_peer, stop, deadline)?;
    Ok((offer, host_peer))
}

/// Accept one bootstrap connection from the same desktop/logon session, authenticate
/// its pinned client peer, and transfer the one-shot private peer offer.
pub fn serve_offer(
    server: BootstrapServer,
    name: &Nonce,
    broker: &Nonce,
    stop: &Stop,
    deadline: Instant,
) -> Result<PinnedProcess> {
    let (pipe, client_peer) = server.accept(stop, deadline)?;
    handoff::send(pipe, &client_peer, name, broker, stop, deadline)?;
    Ok(client_peer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn discovery_client_and_server_transfer_offer() {
        let stop = Stop::new().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let test_nonce = Nonce::random().unwrap();
        let test_pipe_name = format!(r"\\.\pipe\Sumibi.Bootstrap.Test.{}", test_nonce.bootstrap());
        let server = BootstrapServer::create(&test_pipe_name, 1).unwrap();

        let offer_name = Nonce::random().unwrap();
        let offer_broker = Nonce::random().unwrap();

        let s_name = offer_name.clone();
        let s_broker = offer_broker.clone();
        let s_stop = stop.clone();
        let server_thread =
            thread::spawn(move || serve_offer(server, &s_name, &s_broker, &s_stop, deadline));

        let (pipe, host_peer) = Pipe::connect_bootstrap(&test_pipe_name, &stop, deadline).unwrap();
        let received = handoff::receive(pipe, &host_peer, &stop, deadline).unwrap();

        let client_peer = server_thread.join().unwrap().unwrap();
        let current = PinnedProcess::current().unwrap();
        assert!(client_peer.identity() == current.identity());
        assert!(host_peer.identity() == current.identity());

        assert_eq!(received.name.bootstrap(), offer_name.bootstrap());
        assert_eq!(received.broker.bootstrap(), offer_broker.bootstrap());
    }
}

//! One-shot endpoint transfer over an already authenticated bootstrap pipe.
//! This does not discover a Host or establish trust in the process pins.
use crate::{Error, Nonce, PinnedProcess, Pipe, Result, Stop};
use std::time::Instant;

const MAGIC: &[u8; 4] = b"SBH1";
const RECORD_LEN: usize = 4 + 32 + 32;

pub struct Offer {
    pub name: Nonce,
    pub broker: Nonce,
}

/// Worker-only. Consume the dedicated bootstrap connection after one offer.
/// No bytes are written unless its authenticated peer matches `recipient`.
pub fn send(
    mut pipe: Pipe,
    recipient: &PinnedProcess,
    name: &Nonce,
    broker: &Nonce,
    stop: &Stop,
    deadline: Instant,
) -> Result<()> {
    pipe.require_peer(recipient)?;
    let mut record = Vec::with_capacity(RECORD_LEN);
    record.extend_from_slice(MAGIC);
    record.extend_from_slice(name.bootstrap().as_bytes());
    record.extend_from_slice(broker.bootstrap().as_bytes());
    pipe.write_all(record, stop, deadline)
}

/// Worker-only. The independently pinned Host must match this pipe's peer.
/// This accepts one bounded record, never a path, PID, or arbitrary pipe name.
pub fn receive(
    mut pipe: Pipe,
    host: &PinnedProcess,
    stop: &Stop,
    deadline: Instant,
) -> Result<Offer> {
    pipe.require_peer(host)?;
    let mut record = Vec::with_capacity(RECORD_LEN);
    while record.len() < RECORD_LEN {
        let chunk = pipe.start_read(stop, deadline)?.wait()?.bytes;
        if chunk.is_empty() || chunk.len() > RECORD_LEN - record.len() {
            return Err(Error::InvalidMessage);
        }
        record.extend_from_slice(&chunk);
    }
    if &record[..4] != MAGIC {
        return Err(Error::InvalidMessage);
    }
    let name = std::str::from_utf8(&record[4..36]).map_err(|_| Error::InvalidMessage)?;
    let broker = std::str::from_utf8(&record[36..]).map_err(|_| Error::InvalidMessage)?;
    Ok(Offer {
        name: Nonce::from_bootstrap(name).map_err(|_| Error::InvalidMessage)?,
        broker: Nonce::from_bootstrap(broker).map_err(|_| Error::InvalidMessage)?,
    })
}

//! One authenticated pipe's bounded frame reader. A completed read may hold
//! more than one frame; only the first is decoded until its caller is ready.
use crate::{Error, Pipe, Result, Stop};
use std::time::{Duration, Instant};
use sumibi_protocol::{message, Decoder};

const MAX_PARTIAL_AGE: Duration = Duration::from_secs(10);

#[derive(Default)]
pub struct FrameInbox {
    decoder: Decoder,
    unread: Vec<u8>,
    closed: bool,
    partial_since: Option<Instant>,
}

impl FrameInbox {
    /// Supply exactly one completed pipe read. Decode or reject its unread
    /// tail before loading another chunk; this bounds retained bytes to one
    /// read plus the protocol decoder's single partial frame.
    pub fn push(&mut self, bytes: Vec<u8>) -> Result<()> {
        if self.closed {
            return Err(Error::Closed);
        }
        if !self.unread.is_empty() || bytes.is_empty() || bytes.len() > 4096 {
            return Err(Error::InvalidArgument);
        }
        self.unread = bytes;
        Ok(())
    }

    pub fn has_unread(&self) -> bool {
        !self.unread.is_empty()
    }

    /// Return at most one complete frame. None means another read is needed.
    /// An invalid body makes this connection terminal to its caller.
    pub fn next_frame(&mut self) -> Result<Option<message::Envelope>> {
        if self.closed {
            return Err(Error::Closed);
        }
        if self.unread.is_empty() {
            return Ok(None);
        }
        let decoded = match self.decoder.decode(&self.unread) {
            Ok(decoded) => decoded,
            Err(_) => {
                self.closed = true;
                self.unread.clear();
                return Err(Error::InvalidMessage);
            }
        };
        if decoded.consumed == 0 {
            self.closed = true;
            self.unread.clear();
            return Err(Error::InvalidMessage);
        }
        let envelope = decoded.body.map(message::decode).transpose().map_err(|_| {
            self.closed = true;
            self.unread.clear();
            Error::InvalidMessage
        })?;
        self.unread.drain(..decoded.consumed);
        if envelope.is_some() {
            self.partial_since = None;
        } else {
            self.partial_since.get_or_insert_with(Instant::now);
        }
        Ok(envelope)
    }

    /// Blocking worker-only convenience method. The caller retains this inbox
    /// across calls; a coalesced tail is never lost between messages.
    pub fn read(
        &mut self,
        pipe: &mut Pipe,
        stop: &Stop,
        deadline: Instant,
    ) -> Result<message::Envelope> {
        loop {
            if let Some(envelope) = self.next_frame()? {
                return Ok(envelope);
            }
            self.push(pipe.start_read(stop, deadline)?.wait()?.bytes)?;
        }
    }

    /// Poll bytes already queued on this authenticated pipe. A partial frame
    /// stays in the decoder while another connection can make progress. The
    /// partial frame is retired after ten seconds; idle connections are not.
    pub fn read_queued(
        &mut self,
        pipe: &mut Pipe,
        stop: &Stop,
        deadline: Instant,
    ) -> Result<Option<message::Envelope>> {
        self.check_partial_age(Instant::now())?;
        if let Some(envelope) = self.next_frame()? {
            return Ok(Some(envelope));
        }
        if !pipe.has_data()? {
            return Ok(None);
        }
        self.push(pipe.start_read(stop, deadline)?.wait()?.bytes)?;
        self.next_frame()
    }

    fn check_partial_age(&mut self, now: Instant) -> Result<()> {
        if self
            .partial_since
            .is_some_and(|started| now.saturating_duration_since(started) >= MAX_PARTIAL_AGE)
        {
            self.closed = true;
            self.unread.clear();
            return Err(Error::Timeout);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sumibi_protocol::message::{Envelope, Hello, Id, Message, VERSION};

    fn hello(session: u128) -> Envelope {
        Envelope {
            version: VERSION,
            session: Id::new(session).unwrap(),
            generation: 1,
            message: Message::Hello(Hello {
                broker: Id::new(90).unwrap(),
            }),
        }
    }

    #[test]
    fn fragmented_first_frame_keeps_coalesced_successor() {
        let first = message::encode(&hello(1)).unwrap();
        let second = message::encode(&hello(2)).unwrap();
        let mut inbox = FrameInbox::default();
        inbox.push(first[..3].to_vec()).unwrap();
        assert!(inbox.next_frame().unwrap().is_none());
        let mut rest = first[3..].to_vec();
        rest.extend(second);
        inbox.push(rest).unwrap();
        assert!(inbox.next_frame().unwrap().unwrap().session == Id::new(1).unwrap());
        assert!(inbox.has_unread());
        assert!(inbox.next_frame().unwrap().unwrap().session == Id::new(2).unwrap());
        assert!(!inbox.has_unread());
    }

    #[test]
    fn cannot_replace_unread_tail_with_next_pipe_read() {
        let mut bytes = message::encode(&hello(1)).unwrap();
        bytes.extend(message::encode(&hello(2)).unwrap());
        let mut inbox = FrameInbox::default();
        inbox.push(bytes).unwrap();
        inbox.next_frame().unwrap();
        assert_eq!(inbox.push(vec![1]), Err(Error::InvalidArgument));
        assert!(inbox.next_frame().unwrap().unwrap().session == Id::new(2).unwrap());
    }

    #[test]
    fn malformed_body_retires_the_inbox_and_discards_its_tail() {
        let mut bytes = sumibi_protocol::encode("{").unwrap();
        bytes.extend(message::encode(&hello(2)).unwrap());
        let mut inbox = FrameInbox::default();
        inbox.push(bytes).unwrap();
        assert!(matches!(inbox.next_frame(), Err(Error::InvalidMessage)));
        assert!(!inbox.has_unread());
        assert!(matches!(inbox.next_frame(), Err(Error::Closed)));
        assert!(matches!(inbox.push(vec![1]), Err(Error::Closed)));
    }

    #[test]
    fn partial_frame_expires_without_timing_out_an_idle_connection() {
        let mut idle = FrameInbox::default();
        assert!(idle
            .check_partial_age(Instant::now() + MAX_PARTIAL_AGE)
            .is_ok());
        let mut partial = FrameInbox::default();
        partial.push(vec![1]).unwrap();
        assert!(partial.next_frame().unwrap().is_none());
        let started = partial.partial_since.unwrap();
        assert!(partial
            .check_partial_age(started + MAX_PARTIAL_AGE - Duration::from_millis(1))
            .is_ok());
        assert!(matches!(
            partial.check_partial_age(started + MAX_PARTIAL_AGE),
            Err(Error::Timeout)
        ));
        assert!(matches!(partial.next_frame(), Err(Error::Closed)));
    }
}

//! Bounded framing, typed messages and client ordering for TIP/Broker IPC.
//!
//! The framing decoder treats bodies as opaque UTF-8. `message` validates JSON;
//! `session` enforces ordering and request lifetimes. Neither verifies an OS
//! peer: a transport must authenticate both processes before exchanging frames.
#![forbid(unsafe_code)]

pub mod broker_session;
pub mod message;
pub mod policy_fanout;
pub mod session;

/// Maximum body length in bytes, independent of input and candidate limits.
pub const MAX_FRAME_BYTES: usize = 256 * 1024;

/// Fixed classifications only: errors never contain input or server bodies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameError {
    Empty,
    TooLarge,
    InvalidUtf8,
    Truncated,
    Closed,
}

/// Encode a nonempty UTF-8 body with a four-byte little-endian byte length.
/// The returned buffer contains private text and must not be logged.
pub fn encode(body: &str) -> Result<Vec<u8>, FrameError> {
    check_length(body.len())?;
    let mut frame = Vec::with_capacity(4 + body.len());
    frame.extend_from_slice(&(body.len() as u32).to_le_bytes());
    frame.extend_from_slice(body.as_bytes());
    Ok(frame)
}

fn check_length(length: usize) -> Result<(), FrameError> {
    if length == 0 {
        Err(FrameError::Empty)
    } else if length > MAX_FRAME_BYTES {
        Err(FrameError::TooLarge)
    } else {
        Ok(())
    }
}

/// One frame at a time; never queues an unbounded collection of completed bodies.
/// Deliberately omits Debug because a partial body can contain private text.
#[derive(Default)]
pub struct Decoder {
    header: [u8; 4],
    header_len: usize,
    expected: Option<usize>,
    body: Vec<u8>,
    closed: bool,
}

/// Borrowed result. The caller owns any unconsumed input and applies backpressure
/// by deciding when to decode the next frame. The body borrow ends before the
/// decoder can be used again. Neither this type nor its body is debug logged.
pub struct Decoded<'a> {
    pub consumed: usize,
    pub body: Option<&'a str>,
}

impl Decoder {
    /// Consume at most one frame from this chunk, possibly completing a frame
    /// begun in a previous chunk. A complete frame may leave bytes unconsumed.
    /// On failure the connection becomes terminal: do not try to resynchronize.
    pub fn decode<'a>(&'a mut self, input: &[u8]) -> Result<Decoded<'a>, FrameError> {
        if self.closed {
            return Err(FrameError::Closed);
        }
        if self.expected.is_none() && self.header_len == 0 {
            self.body.clear();
        }
        let mut consumed = 0;
        if self.expected.is_none() {
            let count = (4 - self.header_len).min(input.len());
            self.header[self.header_len..self.header_len + count].copy_from_slice(&input[..count]);
            self.header_len += count;
            consumed += count;
            if self.header_len != 4 {
                return Ok(Decoded {
                    consumed,
                    body: None,
                });
            }
            let length = u32::from_le_bytes(self.header) as usize;
            if let Err(error) = check_length(length) {
                self.close();
                return Err(error);
            }
            // Allocate only after validating the header. Reserving the complete
            // frame avoids geometric growth beyond the frame cap when a large
            // body arrives in uneven chunks.
            self.body.reserve_exact(length);
            self.expected = Some(length);
        }
        let expected = self.expected.expect("validated frame length");
        let count = (expected - self.body.len()).min(input.len() - consumed);
        self.body
            .extend_from_slice(&input[consumed..consumed + count]);
        consumed += count;
        if self.body.len() != expected {
            return Ok(Decoded {
                consumed,
                body: None,
            });
        }
        if std::str::from_utf8(&self.body).is_err() {
            self.close();
            return Err(FrameError::InvalidUtf8);
        }
        self.header_len = 0;
        self.expected = None;
        Ok(Decoded {
            consumed,
            body: Some(std::str::from_utf8(&self.body).expect("validated UTF-8")),
        })
    }

    /// Call when the transport reaches EOF. Even a clean EOF closes this decoder;
    /// a new connection requires a fresh decoder and a fresh handshake.
    pub fn finish(&mut self) -> Result<(), FrameError> {
        if self.closed {
            return Err(FrameError::Closed);
        }
        let partial = self.header_len != 0 || self.expected.is_some();
        self.close();
        if partial {
            Err(FrameError::Truncated)
        } else {
            Ok(())
        }
    }

    fn close(&mut self) {
        self.closed = true;
        self.header = [0; 4];
        self.header_len = 0;
        self.expected = None;
        // Drop the allocation rather than retain a failed frame for diagnostics.
        // Allocator/OS memory erasure is not guaranteed by this operation.
        self.body = Vec::new();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_fragment_boundary_preserves_utf8_and_byte_length() {
        let body = "{\"text\":\"日本語😀e\u{301}\"}";
        let frame = encode(body).unwrap();
        assert_eq!(&frame[..4], &(body.len() as u32).to_le_bytes());
        for split in 0..frame.len() {
            let mut decoder = Decoder::default();
            let part = decoder.decode(&frame[..split]).unwrap();
            assert_eq!(part.consumed, split);
            assert!(part.body.is_none());
            let part = decoder.decode(&frame[split..]).unwrap();
            assert_eq!(part.consumed, frame.len() - split);
            assert_eq!(part.body, Some(body));
            assert_eq!(decoder.finish(), Ok(()));
        }
        let mut decoder = Decoder::default();
        for (index, byte) in frame.iter().enumerate() {
            let part = decoder.decode(std::slice::from_ref(byte)).unwrap();
            assert_eq!(part.consumed, 1);
            assert_eq!(part.body, (index + 1 == frame.len()).then_some(body));
        }
    }

    #[test]
    fn coalesced_frames_are_returned_one_at_a_time() {
        let mut input = encode("first").unwrap();
        input.extend(encode("日本語").unwrap());
        input.extend(encode("last").unwrap());
        let mut decoder = Decoder::default();
        let mut offset = 0;
        for expected in ["first", "日本語", "last"] {
            let part = decoder.decode(&input[offset..]).unwrap();
            assert_eq!(part.body, Some(expected));
            offset += part.consumed;
        }
        assert_eq!(offset, input.len());
        assert!(decoder.decode(&[]).unwrap().body.is_none());
        assert_eq!(decoder.finish(), Ok(()));
    }

    #[test]
    fn lengths_are_checked_before_body_allocation() {
        for (length, expected) in [
            (0, FrameError::Empty),
            (MAX_FRAME_BYTES as u32 + 1, FrameError::TooLarge),
            (u32::MAX, FrameError::TooLarge),
        ] {
            let mut decoder = Decoder::default();
            assert!(
                matches!(decoder.decode(&length.to_le_bytes()), Err(error) if error == expected)
            );
            assert_eq!(decoder.body.capacity(), 0);
            assert!(matches!(
                decoder.decode(&encode("valid").unwrap()),
                Err(FrameError::Closed)
            ));
        }
        assert_eq!(encode(""), Err(FrameError::Empty));
        assert_eq!(
            encode(&"x".repeat(MAX_FRAME_BYTES + 1)),
            Err(FrameError::TooLarge)
        );
        let maximum = "x".repeat(MAX_FRAME_BYTES);
        let mut decoder = Decoder::default();
        assert_eq!(
            decoder.decode(&encode(&maximum).unwrap()).unwrap().body,
            Some(maximum.as_str())
        );
        assert!(decoder.body.capacity() <= MAX_FRAME_BYTES);
    }

    #[test]
    fn invalid_utf8_closes_without_accepting_a_following_frame() {
        for bytes in [
            &[0xff][..],
            &[0xc0, 0x80],
            &[0xed, 0xa0, 0x80],
            &[0xf0, 0x9f],
        ] {
            let mut frame = (bytes.len() as u32).to_le_bytes().to_vec();
            frame.extend_from_slice(bytes);
            frame.extend(encode("valid").unwrap());
            let mut decoder = Decoder::default();
            assert!(matches!(
                decoder.decode(&frame),
                Err(FrameError::InvalidUtf8)
            ));
            assert!(decoder.body.is_empty());
            assert_eq!(decoder.finish(), Err(FrameError::Closed));
        }
    }

    #[test]
    fn uneven_large_chunks_do_not_grow_beyond_one_frame() {
        let body = "x".repeat(MAX_FRAME_BYTES);
        let frame = encode(&body).unwrap();
        let mut decoder = Decoder::default();
        let first = 200_000;
        assert!(decoder.decode(&frame[..first]).unwrap().body.is_none());
        assert!(decoder.body.capacity() <= MAX_FRAME_BYTES);
        assert_eq!(
            decoder.decode(&frame[first..]).unwrap().body,
            Some(body.as_str())
        );
        assert!(decoder.body.capacity() <= MAX_FRAME_BYTES);
        for _ in 0..100 {
            assert_eq!(
                decoder.decode(&encode("small").unwrap()).unwrap().body,
                Some("small")
            );
            assert!(decoder.body.capacity() <= MAX_FRAME_BYTES);
        }
    }

    #[test]
    fn eof_in_any_partial_frame_is_terminal() {
        let frame = encode("日本語").unwrap();
        for end in 1..frame.len() {
            let mut decoder = Decoder::default();
            assert!(decoder.decode(&frame[..end]).unwrap().body.is_none());
            assert_eq!(decoder.finish(), Err(FrameError::Truncated));
            assert!(matches!(
                decoder.decode(&frame[end..]),
                Err(FrameError::Closed)
            ));
        }
        let mut decoder = Decoder::default();
        assert_eq!(decoder.finish(), Ok(()));
        assert!(matches!(decoder.decode(&frame), Err(FrameError::Closed)));
    }
}

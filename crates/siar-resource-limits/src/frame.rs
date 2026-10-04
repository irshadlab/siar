//! §123 inbound limit enforcement: "always enforce before allocation".
//!
//! A length-prefixed frame decoder (4-byte big-endian length). The
//! declared length is compared with the local maximum **as soon as the
//! header is complete**, and a frame declaring 100 MB against a 1 MB cap
//! is rejected having allocated nothing. The body buffer then grows only
//! as bytes actually arrive, never pre-sized from the declared length —
//! so a peer that declares the maximum and trickles data pins memory
//! proportional to what it really sent, not to what it claimed.
//!
//! After any error the decoder is *poisoned*: the stream position is
//! untrustworthy, so the only correct response is to close the
//! connection; further input is refused rather than resynchronised.

use crate::safety::{len_to_u32, SafetyError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum FrameError {
    #[error("declared frame length {declared} exceeds the limit {max}")]
    TooLarge { declared: u64, max: u64 },
    #[error("decoder is poisoned by an earlier error; close the connection")]
    Poisoned,
    #[error("could not allocate {0} bytes for the frame body")]
    OutOfMemory(usize),
}

#[derive(Debug)]
enum State {
    Header { buf: [u8; 4], have: usize },
    Body { expected: usize, buf: Vec<u8> },
    Poisoned,
}

#[derive(Debug)]
pub struct FrameDecoder {
    max_frame: usize,
    state: State,
}

impl FrameDecoder {
    pub fn new(max_frame_bytes: usize) -> Self {
        Self {
            max_frame: max_frame_bytes,
            state: State::Header {
                buf: [0; 4],
                have: 0,
            },
        }
    }

    /// Feed received bytes; returns every frame completed by them.
    pub fn feed(&mut self, mut input: &[u8]) -> Result<Vec<Vec<u8>>, FrameError> {
        let mut out = Vec::new();
        loop {
            match &mut self.state {
                State::Poisoned => return Err(FrameError::Poisoned),
                State::Header { buf, have } => {
                    let take = (4 - *have).min(input.len());
                    buf[*have..*have + take].copy_from_slice(&input[..take]);
                    *have += take;
                    input = &input[take..];
                    if *have < 4 {
                        return Ok(out);
                    }
                    let declared = u64::from(u32::from_be_bytes(*buf));
                    // The check that matters: before any body allocation.
                    if declared > self.max_frame as u64 {
                        self.state = State::Poisoned;
                        return Err(FrameError::TooLarge {
                            declared,
                            max: self.max_frame as u64,
                        });
                    }
                    let expected = declared as usize;
                    if expected == 0 {
                        out.push(Vec::new());
                        self.state = State::Header {
                            buf: [0; 4],
                            have: 0,
                        };
                    } else {
                        self.state = State::Body {
                            expected,
                            buf: Vec::new(),
                        };
                    }
                }
                State::Body { expected, buf } => {
                    let need = *expected - buf.len();
                    let take = need.min(input.len());
                    if take > 0 && buf.try_reserve(take).is_err() {
                        let n = buf.len() + take;
                        self.state = State::Poisoned;
                        return Err(FrameError::OutOfMemory(n));
                    }
                    buf.extend_from_slice(&input[..take]);
                    input = &input[take..];
                    if buf.len() < *expected {
                        return Ok(out);
                    }
                    out.push(std::mem::take(buf));
                    self.state = State::Header {
                        buf: [0; 4],
                        have: 0,
                    };
                }
            }
            if input.is_empty() && matches!(self.state, State::Header { have: 0, .. }) {
                return Ok(out);
            }
        }
    }

    /// Bytes currently held for a partial frame (diagnostics / tests).
    pub fn buffered_bytes(&self) -> usize {
        match &self.state {
            State::Body { buf, .. } => buf.capacity(),
            _ => 0,
        }
    }

    pub fn is_poisoned(&self) -> bool {
        matches!(self.state, State::Poisoned)
    }
}

/// Encode with the same limit applied on the way out, so we never emit a
/// frame our own peer's decoder would reject.
pub fn encode_frame(payload: &[u8], max_frame_bytes: usize) -> Result<Vec<u8>, EncodeError> {
    if payload.len() > max_frame_bytes {
        return Err(EncodeError::TooLarge {
            len: payload.len(),
            max: max_frame_bytes,
        });
    }
    let len = len_to_u32(payload.len()).map_err(EncodeError::Length)?;
    let mut v = Vec::with_capacity(4 + payload.len());
    v.extend_from_slice(&len.to_be_bytes());
    v.extend_from_slice(payload);
    Ok(v)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum EncodeError {
    #[error("payload of {len} bytes exceeds the frame limit {max}")]
    TooLarge { len: usize, max: usize },
    #[error(transparent)]
    Length(SafetyError),
}

#[cfg(test)]
mod tests {
    use super::*;

    const MB: usize = 1 << 20;

    #[test]
    fn spec_example_100mb_declared_against_a_1mb_cap_is_rejected_with_no_allocation() {
        let mut d = FrameDecoder::new(MB);
        let hdr = (100u32 * MB as u32).to_be_bytes();
        assert_eq!(
            d.feed(&hdr).unwrap_err(),
            FrameError::TooLarge {
                declared: 100 * MB as u64,
                max: MB as u64
            }
        );
        assert_eq!(d.buffered_bytes(), 0, "nothing was allocated");
        assert!(d.is_poisoned());
    }

    #[test]
    fn rejection_happens_on_the_header_even_if_the_body_never_arrives() {
        let mut d = FrameDecoder::new(10);
        assert!(
            d.feed(&[0, 0, 0]).unwrap().is_empty(),
            "partial header buffered"
        );
        assert!(
            d.feed(&[11]).is_err(),
            "rejected the instant the 4th header byte lands"
        );
    }

    #[test]
    fn frames_split_across_arbitrary_reads_reassemble() {
        let wire = [
            encode_frame(b"hello", 100).unwrap(),
            encode_frame(b"", 100).unwrap(),
            encode_frame(b"world!", 100).unwrap(),
        ]
        .concat();
        for chunk in 1..=wire.len() {
            let mut d = FrameDecoder::new(100);
            let mut got = Vec::new();
            for piece in wire.chunks(chunk) {
                got.extend(d.feed(piece).unwrap());
            }
            assert_eq!(
                got,
                vec![b"hello".to_vec(), Vec::new(), b"world!".to_vec()],
                "chunk size {chunk}"
            );
        }
    }

    #[test]
    fn several_frames_in_one_read() {
        let wire = [
            encode_frame(b"a", 9).unwrap(),
            encode_frame(b"bc", 9).unwrap(),
        ]
        .concat();
        let mut d = FrameDecoder::new(9);
        assert_eq!(d.feed(&wire).unwrap(), vec![b"a".to_vec(), b"bc".to_vec()]);
        assert_eq!(d.buffered_bytes(), 0);
    }

    #[test]
    fn a_trickling_peer_pins_memory_proportional_to_bytes_sent_not_declared() {
        let mut d = FrameDecoder::new(MB);
        let mut input = (MB as u32).to_be_bytes().to_vec();
        input.extend_from_slice(&[7u8; 100]);
        assert!(d.feed(&input).unwrap().is_empty());
        assert!(
            d.buffered_bytes() < 4_096,
            "held {} bytes for a frame that declared 1 MiB",
            d.buffered_bytes()
        );
    }

    #[test]
    fn a_poisoned_decoder_stays_poisoned() {
        let mut d = FrameDecoder::new(4);
        assert!(d.feed(&99u32.to_be_bytes()).is_err());
        assert_eq!(d.feed(&[1, 2, 3]).unwrap_err(), FrameError::Poisoned);
        assert_eq!(d.feed(&[]).unwrap_err(), FrameError::Poisoned);
    }

    #[test]
    fn exactly_the_limit_is_accepted_one_over_is_not() {
        let mut ok = FrameDecoder::new(8);
        let f = encode_frame(&[1u8; 8], 8).unwrap();
        assert_eq!(ok.feed(&f).unwrap(), vec![vec![1u8; 8]]);
        let mut bad = FrameDecoder::new(8);
        assert!(bad.feed(&9u32.to_be_bytes()).is_err());
    }

    #[test]
    fn encoding_enforces_the_same_limit() {
        assert_eq!(
            encode_frame(&[0; 9], 8),
            Err(EncodeError::TooLarge { len: 9, max: 8 })
        );
        assert_eq!(encode_frame(&[], 8).unwrap(), vec![0, 0, 0, 0]);
    }

    #[test]
    fn garbage_streams_never_panic() {
        // pseudo-random bytes in odd chunk sizes: must only ever Ok or Err.
        let mut x = 0x2545_F491_4F6C_DD1Du64;
        let mut bytes = Vec::new();
        for _ in 0..4_000 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            bytes.push((x >> 24) as u8);
        }
        for chunk in [1, 3, 7, 64, 1_000] {
            let mut d = FrameDecoder::new(256);
            for piece in bytes.chunks(chunk) {
                if d.feed(piece).is_err() {
                    break;
                }
            }
        }
    }
}

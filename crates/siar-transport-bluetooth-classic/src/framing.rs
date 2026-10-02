//! Wire framing over an RFCOMM byte stream — next.md §21.
//!
//! Bluetooth Classic RFCOMM is a reliable, ordered **byte stream**
//! (`android.bluetooth.BluetoothSocket`'s `InputStream`/`OutputStream`),
//! fundamentally unlike BLE's fixed-size, per-write GATT characteristic
//! model that `siar-transport-ble::fragment` exists to chop payloads
//! into. There is no MTU to fragment around here — RFCOMM already
//! delivers arbitrary-length writes in order without loss (retransmit
//! is handled below it, in the Bluetooth stack itself). What RFCOMM
//! does *not* give you is message boundaries: `InputStream.read()`
//! hands back whatever bytes happened to arrive, which may be less
//! than one envelope, more than one envelope, or a split across two
//! reads. This module's job is turning that raw byte stream back into
//! discrete envelopes.
//!
//! `checksum` here is the same corruption-detection-not-authentication
//! role as `siar-transport-ble::fragment`'s — see that module's doc
//! comment for the full reasoning, which applies unchanged: the framed
//! payload is already E2EE ciphertext with its own AEAD tag by the
//! time it reaches this module.

use thiserror::Error;

/// Fixed-size wire header: `length`(4, big-endian) + `checksum`(2, BE)
/// = 6 bytes, followed by exactly `length` bytes of payload.
const HEADER_LEN: usize = 6;

/// next.md §53 classifies Bluetooth Classic for "small/medium"
/// attachments (32 KB – 10 MB), distinctly larger than BLE's
/// tiny-fragment-only role but still bounded — per §61's decode-limits
/// discipline and §73's blob-safety rule ("do not first allocate
/// Vec(size) from an untrusted network field"), [`FrameDecoder`] checks
/// a claimed length against this cap *before* growing its buffer to
/// receive it, so a hostile or corrupted length prefix can't force
/// unbounded allocation. 16 MiB comfortably covers §53's "medium" tier
/// with headroom, without matching video/call traffic (which this
/// transport was never meant to carry — next.md §22, §56).
pub const MAX_FRAME_LEN: u32 = 16 * 1024 * 1024;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum FramingError {
    #[error("frame length {claimed} exceeds the {max}-byte cap")]
    FrameTooLarge { claimed: u32, max: u32 },
    #[error("frame checksum mismatch: claimed {claimed:#06x}, actual {actual:#06x}")]
    ChecksumMismatch { claimed: u16, actual: u16 },
}

/// Serializes one envelope for a single `OutputStream.write()` call.
/// (RFCOMM doesn't require the write to land in one physical packet —
/// the framing survives being split across several reads on the other
/// end, since [`FrameDecoder`] doesn't assume read/write boundaries
/// line up with frame boundaries.)
///
/// Prefer [`try_encode_frame`]: this variant cannot report a payload the
/// 4-byte length prefix can't represent, and `payload.len() as u32`
/// would silently wrap it.
pub fn encode_frame(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_LEN + payload.len());
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(&checksum16(payload).to_be_bytes());
    out.extend_from_slice(payload);
    out
}

/// Like [`encode_frame`] but refuses payloads over the same cap the
/// decoder enforces, so nothing is emitted that the peer would reject.
pub fn try_encode_frame(payload: &[u8]) -> Result<Vec<u8>, FramingError> {
    match u32::try_from(payload.len()) {
        Ok(len) if len <= MAX_FRAME_LEN => Ok(encode_frame(payload)),
        _ => Err(FramingError::FrameTooLarge {
            claimed: u32::try_from(payload.len()).unwrap_or(u32::MAX),
            max: MAX_FRAME_LEN,
        }),
    }
}

/// Incrementally reassembles frames from an RFCOMM `InputStream`. Feed
/// it whatever bytes each `read()` call returns, in order; it hands
/// back every envelope that becomes complete as a result, in framing
/// order. Holds at most one in-flight frame's worth of bytes at a time
/// (bounded by [`MAX_FRAME_LEN`]), so a slow or malicious peer can't
/// grow this decoder's memory without limit.
#[derive(Debug, Default)]
pub struct FrameDecoder {
    buffer: Vec<u8>,
}

impl FrameDecoder {
    pub fn new() -> Self {
        Self { buffer: Vec::new() }
    }

    /// Appends newly read bytes and returns every envelope that's now
    /// complete, in order. On [`FramingError`], the decoder's internal
    /// state is left as-is (nothing consumed) — the caller should treat
    /// this as "the peer/link is misbehaving" and tear down the
    /// connection, the same way `siar-transport-ble`'s reassembly
    /// buffer treats a checksum failure as "ask for retransmit," not
    /// something to silently paper over and keep parsing past.
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<Vec<u8>>, FramingError> {
        self.buffer.extend_from_slice(bytes);
        let mut complete = Vec::new();

        loop {
            if self.buffer.len() < 4 {
                break; // not even a length prefix yet
            }
            let claimed_len = u32::from_be_bytes(
                self.buffer[0..4]
                    .try_into()
                    .expect("slice is exactly 4 bytes"),
            );
            if claimed_len > MAX_FRAME_LEN {
                return Err(FramingError::FrameTooLarge {
                    claimed: claimed_len,
                    max: MAX_FRAME_LEN,
                });
            }
            let total_len = HEADER_LEN + claimed_len as usize;
            if self.buffer.len() < total_len {
                break; // header known, payload not fully arrived yet
            }

            let claimed_checksum = u16::from_be_bytes(
                self.buffer[4..6]
                    .try_into()
                    .expect("slice is exactly 2 bytes"),
            );
            let payload = self.buffer[HEADER_LEN..total_len].to_vec();
            let actual_checksum = checksum16(&payload);
            if actual_checksum != claimed_checksum {
                return Err(FramingError::ChecksumMismatch {
                    claimed: claimed_checksum,
                    actual: actual_checksum,
                });
            }

            complete.push(payload);
            self.buffer.drain(0..total_len);
        }

        Ok(complete)
    }
}

/// Same simple additive checksum as `siar-transport-ble::fragment` —
/// deliberately not a security mechanism, see this module's top doc
/// comment.
fn checksum16(payload: &[u8]) -> u16 {
    payload
        .iter()
        .fold(0u16, |acc, &b| acc.wrapping_add(b as u16))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_a_single_frame() {
        let payload = b"hello over rfcomm".to_vec();
        let wire = encode_frame(&payload);
        let mut decoder = FrameDecoder::new();
        let frames = decoder.push(&wire).unwrap();
        assert_eq!(frames, vec![payload]);
    }

    #[test]
    fn reassembles_a_frame_split_across_many_reads() {
        let payload = vec![7u8; 500];
        let wire = encode_frame(&payload);
        let mut decoder = FrameDecoder::new();
        let mut got = Vec::new();
        for chunk in wire.chunks(3) {
            got.extend(decoder.push(chunk).unwrap());
        }
        assert_eq!(got, vec![payload]);
    }

    #[test]
    fn handles_two_frames_arriving_in_one_read() {
        let a = b"first".to_vec();
        let b = b"second envelope".to_vec();
        let mut wire = encode_frame(&a);
        wire.extend_from_slice(&encode_frame(&b));

        let mut decoder = FrameDecoder::new();
        let frames = decoder.push(&wire).unwrap();
        assert_eq!(frames, vec![a, b]);
    }

    #[test]
    fn waits_for_more_bytes_when_payload_is_incomplete() {
        let payload = vec![1u8; 100];
        let wire = encode_frame(&payload);
        let mut decoder = FrameDecoder::new();

        let frames = decoder.push(&wire[..wire.len() - 10]).unwrap();
        assert!(frames.is_empty());

        let frames = decoder.push(&wire[wire.len() - 10..]).unwrap();
        assert_eq!(frames, vec![payload]);
    }

    #[test]
    fn rejects_a_length_prefix_over_the_cap() {
        let mut wire = (MAX_FRAME_LEN + 1).to_be_bytes().to_vec();
        wire.extend_from_slice(&[0u8; 2]); // checksum, irrelevant — rejected first
        let mut decoder = FrameDecoder::new();
        let err = decoder.push(&wire).unwrap_err();
        assert_eq!(
            err,
            FramingError::FrameTooLarge {
                claimed: MAX_FRAME_LEN + 1,
                max: MAX_FRAME_LEN
            }
        );
    }

    #[test]
    fn rejects_a_corrupted_payload() {
        let payload = b"integrity matters".to_vec();
        let mut wire = encode_frame(&payload);
        let last = wire.len() - 1;
        wire[last] ^= 0xFF; // flip a payload bit without updating the checksum
        let mut decoder = FrameDecoder::new();
        assert!(matches!(
            decoder.push(&wire),
            Err(FramingError::ChecksumMismatch { .. })
        ));
    }

    #[test]
    fn empty_payload_frame_round_trips() {
        let wire = encode_frame(&[]);
        let mut decoder = FrameDecoder::new();
        assert_eq!(decoder.push(&wire).unwrap(), vec![Vec::<u8>::new()]);
    }

    #[test]
    fn try_encode_frame_enforces_the_decoder_cap() {
        let ok = vec![0u8; 16];
        assert_eq!(try_encode_frame(&ok).unwrap(), encode_frame(&ok));
        let too_big = vec![0u8; MAX_FRAME_LEN as usize + 1];
        assert!(matches!(
            try_encode_frame(&too_big),
            Err(FramingError::FrameTooLarge { .. })
        ));
    }
}

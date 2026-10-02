//! Length-prefixed postcard framing (plan.md §59, §61, §73).
//!
//! Generic over the frame type and its size limit so `siar-transport`'s
//! blob protocol (large transfers, its own cap) and the control-message
//! protocol (`WireMessage`, `MAX_CONTROL_FRAME_BYTES`) share one
//! implementation instead of two copies of the same allocation-safety
//! logic. `encode_frame`/`decode_frame` are the `WireMessage`-specific
//! convenience wrappers everything before Phase 4 already calls.

use serde::{de::DeserializeOwned, Serialize};
use thiserror::Error;

const LENGTH_PREFIX_BYTES: usize = 4;

#[derive(Debug, Error)]
pub enum CodecError {
    #[error("frame declares {declared} bytes, exceeds the {limit}-byte limit")]
    FrameTooLarge { declared: usize, limit: usize },
    #[error("buffer has {available} bytes, frame needs {needed}")]
    Incomplete { available: usize, needed: usize },
    #[error("postcard encode/decode failed: {0}")]
    Postcard(#[from] postcard::Error),
}

/// Serialize `message` into `dst` as `[u32 length][postcard bytes]`,
/// rejecting anything over `limit` before it's ever written.
pub fn encode_frame_generic<T: Serialize>(
    message: &T,
    limit: usize,
    dst: &mut Vec<u8>,
) -> Result<(), CodecError> {
    let body = postcard::to_allocvec(message)?;
    if body.len() > limit {
        return Err(CodecError::FrameTooLarge {
            declared: body.len(),
            limit,
        });
    }
    // The prefix is a u32: a caller-supplied `limit` above `u32::MAX`
    // must not let a larger body wrap its own length header.
    let len = u32::try_from(body.len()).map_err(|_| CodecError::FrameTooLarge {
        declared: body.len(),
        limit: u32::MAX as usize,
    })?;
    dst.extend_from_slice(&len.to_le_bytes());
    dst.extend_from_slice(&body);
    Ok(())
}

/// Decode one frame from the front of `src`, rejecting a declared length
/// over `limit` *before* touching `src[4..4+len]` (plan.md §73) — a
/// malicious peer claiming a multi-gigabyte frame gets `Incomplete`/
/// `FrameTooLarge` back, never an allocation proportional to its claim.
///
/// Returns the decoded message plus how many bytes of `src` it consumed.
pub fn decode_frame_generic<T: DeserializeOwned>(
    src: &[u8],
    limit: usize,
) -> Result<(T, usize), CodecError> {
    if src.len() < LENGTH_PREFIX_BYTES {
        return Err(CodecError::Incomplete {
            available: src.len(),
            needed: LENGTH_PREFIX_BYTES,
        });
    }
    let declared = u32::from_le_bytes(src[0..4].try_into().unwrap()) as usize;
    if declared > limit {
        return Err(CodecError::FrameTooLarge { declared, limit });
    }
    let total_needed = LENGTH_PREFIX_BYTES + declared;
    if src.len() < total_needed {
        return Err(CodecError::Incomplete {
            available: src.len(),
            needed: total_needed,
        });
    }
    let body = &src[LENGTH_PREFIX_BYTES..total_needed];
    let message: T = postcard::from_bytes(body)?;
    Ok((message, total_needed))
}

pub fn encode_frame(message: &crate::WireMessage, dst: &mut Vec<u8>) -> Result<(), CodecError> {
    encode_frame_generic(message, crate::limits::MAX_CONTROL_FRAME_BYTES, dst)
}

pub fn decode_frame(src: &[u8]) -> Result<(crate::WireMessage, usize), CodecError> {
    decode_frame_generic(src, crate::limits::MAX_CONTROL_FRAME_BYTES)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v1::{Envelope, EnvelopeKind, CURRENT_VERSION};
    use crate::WireMessage;
    use siar_domain::{ConversationId, DeviceId, MessageId};

    fn sample() -> WireMessage {
        WireMessage::V1(Envelope {
            version: CURRENT_VERSION,
            message_id: MessageId::new(),
            conversation_id: ConversationId::new(),
            sender: DeviceId::new(),
            timestamp_millis: 0,
            sequence: 1,
            kind: EnvelopeKind::Text,
            payload: vec![1, 2, 3],
        })
    }

    #[test]
    fn round_trips() {
        let mut buf = Vec::new();
        encode_frame(&sample(), &mut buf).unwrap();
        let (decoded, consumed) = decode_frame(&buf).unwrap();
        assert_eq!(consumed, buf.len());
        let WireMessage::V1(env) = decoded else {
            panic!("sample() always encodes a V1 envelope");
        };
        assert_eq!(env.sequence, 1);
    }

    #[test]
    fn reports_incomplete_on_partial_prefix() {
        let err = decode_frame(&[0, 1]).unwrap_err();
        assert!(matches!(err, CodecError::Incomplete { .. }));
    }

    #[test]
    fn reports_incomplete_when_body_is_short() {
        let mut buf = Vec::new();
        encode_frame(&sample(), &mut buf).unwrap();
        let truncated = &buf[..buf.len() - 1];
        let err = decode_frame(truncated).unwrap_err();
        assert!(matches!(err, CodecError::Incomplete { .. }));
    }

    #[test]
    fn rejects_oversized_declared_length_without_allocating() {
        let huge = (crate::limits::MAX_CONTROL_FRAME_BYTES as u32 + 1).to_le_bytes();
        let err = decode_frame(&huge).unwrap_err();
        assert!(matches!(err, CodecError::FrameTooLarge { .. }));
    }

    #[test]
    fn two_frames_back_to_back_decode_independently() {
        let mut buf = Vec::new();
        encode_frame(&sample(), &mut buf).unwrap();
        encode_frame(&sample(), &mut buf).unwrap();
        let (_, n1) = decode_frame(&buf).unwrap();
        let (_, n2) = decode_frame(&buf[n1..]).unwrap();
        assert_eq!(n1 + n2, buf.len());
    }

    #[test]
    fn generic_framing_works_for_a_non_wiremessage_type() {
        #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
        struct Ping(u32);

        let mut buf = Vec::new();
        encode_frame_generic(&Ping(42), 1024, &mut buf).unwrap();
        let (decoded, consumed): (Ping, usize) = decode_frame_generic(&buf, 1024).unwrap();
        assert_eq!(decoded, Ping(42));
        assert_eq!(consumed, buf.len());
    }
}

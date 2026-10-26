//! Phase-1 pairing ticket (see module docs on `lib.rs`).
//!
//! Encodes everything the other side needs to both reach us over iroh
//! *and* establish an application-E2EE session with us: the transport
//! address, our X25519 public key (for ECDH), and our Ed25519 verifying
//! key (for future signature verification — unused by `Session` itself
//! yet, carried now so Phase 2 doesn't need a wire-format break to add
//! signing).

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerTicket {
    pub endpoint_addr: iroh::EndpointAddr,
    pub x25519_public: [u8; 32],
    pub ed25519_verifying: [u8; 32],
}

#[derive(Debug, Error)]
pub enum TicketError {
    #[error("ticket is not valid hex: {0}")]
    Hex(String),
    #[error("ticket could not be decoded: {0}")]
    Postcard(#[from] postcard::Error),
}

impl PeerTicket {
    pub fn encode(&self) -> String {
        let bytes = postcard::to_allocvec(self).expect("PeerTicket always serializes");
        to_hex(&bytes)
    }

    pub fn decode(s: &str) -> Result<Self, TicketError> {
        let bytes = from_hex(s).map_err(TicketError::Hex)?;
        Ok(postcard::from_bytes(&bytes)?)
    }
}

fn to_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

fn from_hex(s: &str) -> Result<Vec<u8>, String> {
    let s = s.trim();
    if !s.len().is_multiple_of(2) {
        return Err("odd-length hex string".to_string());
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|e| e.to_string()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh::EndpointAddr;

    #[test]
    fn round_trips_through_text_encoding() {
        use std::str::FromStr;
        let hex = format!("{:02x}", 7u8).repeat(32);
        let secret = iroh::SecretKey::from_str(&hex).expect("valid 64-char hex test secret key");
        let id = secret.public();
        let ticket = PeerTicket {
            endpoint_addr: EndpointAddr::new(id),
            x25519_public: [1u8; 32],
            ed25519_verifying: [2u8; 32],
        };

        let encoded = ticket.encode();
        let decoded = PeerTicket::decode(&encoded).unwrap();
        assert_eq!(decoded.endpoint_addr.id, id);
        assert_eq!(decoded.x25519_public, [1u8; 32]);
    }
}

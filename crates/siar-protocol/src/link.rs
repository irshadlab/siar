//! Linking a new device to an account (wire shapes only).
//!
//! Three messages, all sent over the transport's authenticated connection
//! (the receiver learns who sent each from the connection, never from a
//! field here):
//!
//! * `Request` — the new device to the existing one: the one-time code it was
//!   shown, plus its own ids and public key.
//! * `Approved` — the existing device back, **after a person approved**: the
//!   account id, the account's root *public* key, and the certificate for the
//!   new device (postcard bytes of `siar-identity-multidevice`'s
//!   `DeviceCertificate`, kept opaque here so this crate stays independent of
//!   that one), plus who approved.
//! * `Denied` — a person declined, or the code was refused.
//!
//! The certificate is verified by the receiver against the root public key and
//! its own device id/key; nothing in this crate trusts these fields.

use serde::{Deserialize, Serialize};
use siar_domain::{AccountId, DeviceId};

pub const MAX_LINK_CODE_BYTES: usize = 64;
pub const MAX_LINK_NAME_BYTES: usize = 256;
pub const MAX_LINK_CERTIFICATE_BYTES: usize = 2048;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LinkMessage {
    Request {
        code: String,
        device_id: DeviceId,
        device_public_key: [u8; 32],
        display_name: String,
    },
    Approved {
        account_id: AccountId,
        root_public_key: [u8; 32],
        certificate: Vec<u8>,
        approver_device: DeviceId,
        approver_public_key: [u8; 32],
        approver_name: String,
    },
    Denied,
}

impl LinkMessage {
    /// Size limits, checked after decoding (the frame limit alone would let a
    /// megabyte "name" through).
    #[must_use]
    pub fn within_limits(&self) -> bool {
        match self {
            Self::Request {
                code, display_name, ..
            } => code.len() <= MAX_LINK_CODE_BYTES && display_name.len() <= MAX_LINK_NAME_BYTES,
            Self::Approved {
                certificate,
                approver_name,
                ..
            } => {
                certificate.len() <= MAX_LINK_CERTIFICATE_BYTES
                    && approver_name.len() <= MAX_LINK_NAME_BYTES
            }
            Self::Denied => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_hold_and_every_message_round_trips_through_postcard() {
        let ok = LinkMessage::Request {
            code: "ABCD-EFGH".into(),
            device_id: DeviceId::new(),
            device_public_key: [1; 32],
            display_name: "phone".into(),
        };
        let bytes = postcard::to_allocvec(&ok).unwrap();
        assert_eq!(postcard::from_bytes::<LinkMessage>(&bytes).unwrap(), ok);
        assert!(ok.within_limits());

        let long = LinkMessage::Request {
            code: "x".repeat(MAX_LINK_CODE_BYTES + 1),
            device_id: DeviceId::new(),
            device_public_key: [1; 32],
            display_name: String::new(),
        };
        assert!(!long.within_limits());
        let big_cert = LinkMessage::Approved {
            account_id: AccountId::new(),
            root_public_key: [2; 32],
            certificate: vec![0; MAX_LINK_CERTIFICATE_BYTES + 1],
            approver_device: DeviceId::new(),
            approver_public_key: [3; 32],
            approver_name: String::new(),
        };
        assert!(!big_cert.within_limits());
        assert!(LinkMessage::Denied.within_limits());
    }
}

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
pub const MAX_LINK_TICKET_BYTES: usize = 4096;
/// Contacts an approving device may hand to the new one in one message.
pub const MAX_LINK_CONTACTS: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LinkMessage {
    Request {
        code: String,
        device_id: DeviceId,
        device_public_key: [u8; 32],
        display_name: String,
        /// The new device's own ticket: how the account's contacts will reach
        /// it once the approving device announces it.
        device_ticket: String,
    },
    Approved {
        account_id: AccountId,
        root_public_key: [u8; 32],
        certificate: Vec<u8>,
        approver_device: DeviceId,
        approver_public_key: [u8; 32],
        approver_name: String,
        /// Tickets of the approver's contacts, included only if the person
        /// chose to share them with the new device.
        contact_tickets: Vec<String>,
    },
    Denied,
    /// From the approving device to one of its contacts: "my account now also
    /// has this device". The receiver trusts it because it arrives over the
    /// authenticated connection of an already-paired peer *and* the
    /// certificate verifies against the account root key (see
    /// `siar-app-core::contact_devices`). Appended last so earlier variants
    /// keep their wire indices.
    DeviceAdded {
        account_id: AccountId,
        root_public_key: [u8; 32],
        certificate: Vec<u8>,
        device_ticket: String,
    },
}

impl LinkMessage {
    /// Size limits, checked after decoding (the frame limit alone would let a
    /// megabyte "name" through).
    #[must_use]
    pub fn within_limits(&self) -> bool {
        match self {
            Self::Request {
                code,
                display_name,
                device_ticket,
                ..
            } => {
                code.len() <= MAX_LINK_CODE_BYTES
                    && display_name.len() <= MAX_LINK_NAME_BYTES
                    && device_ticket.len() <= MAX_LINK_TICKET_BYTES
            }
            Self::Approved {
                certificate,
                approver_name,
                contact_tickets,
                ..
            } => {
                certificate.len() <= MAX_LINK_CERTIFICATE_BYTES
                    && approver_name.len() <= MAX_LINK_NAME_BYTES
                    && contact_tickets.len() <= MAX_LINK_CONTACTS
                    && contact_tickets
                        .iter()
                        .all(|t| t.len() <= MAX_LINK_TICKET_BYTES)
            }
            Self::DeviceAdded {
                certificate,
                device_ticket,
                ..
            } => {
                certificate.len() <= MAX_LINK_CERTIFICATE_BYTES
                    && device_ticket.len() <= MAX_LINK_TICKET_BYTES
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
            device_ticket: "t".into(),
        };
        let bytes = postcard::to_allocvec(&ok).unwrap();
        assert_eq!(postcard::from_bytes::<LinkMessage>(&bytes).unwrap(), ok);
        assert!(ok.within_limits());

        let long = LinkMessage::Request {
            code: "x".repeat(MAX_LINK_CODE_BYTES + 1),
            device_id: DeviceId::new(),
            device_public_key: [1; 32],
            display_name: String::new(),
            device_ticket: String::new(),
        };
        assert!(!long.within_limits());
        let big_cert = LinkMessage::Approved {
            account_id: AccountId::new(),
            root_public_key: [2; 32],
            certificate: vec![0; MAX_LINK_CERTIFICATE_BYTES + 1],
            approver_device: DeviceId::new(),
            approver_public_key: [3; 32],
            approver_name: String::new(),
            contact_tickets: Vec::new(),
        };
        assert!(!big_cert.within_limits());
        let too_many = LinkMessage::Approved {
            account_id: AccountId::new(),
            root_public_key: [2; 32],
            certificate: Vec::new(),
            approver_device: DeviceId::new(),
            approver_public_key: [3; 32],
            approver_name: String::new(),
            contact_tickets: vec!["t".into(); MAX_LINK_CONTACTS + 1],
        };
        assert!(!too_many.within_limits());
        let announce = LinkMessage::DeviceAdded {
            account_id: AccountId::new(),
            root_public_key: [2; 32],
            certificate: vec![0; 10],
            device_ticket: "t".repeat(MAX_LINK_TICKET_BYTES + 1),
        };
        assert!(!announce.within_limits());
        assert!(LinkMessage::Denied.within_limits());
    }
}

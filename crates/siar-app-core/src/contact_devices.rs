//! A contact is an *account* that may have several devices.
//!
//! When someone you are paired with links a second device, their existing
//! device announces it to you (`LinkMessage::DeviceAdded`). This module is the
//! rules and the storage for what you do with that:
//!
//! * [`verify_announcement`] — pure. The announcement is accepted only if the
//!   certificate verifies against the account root key it names, the
//!   certificate is for exactly the device whose ticket it carries, it has not
//!   expired, **and** the root key matches the one first seen for this contact
//!   (trust on first use). A changed root key is refused loudly, never
//!   silently replaced.
//! * [`ContactDevices`] — the sibling devices per contact, persisted in one
//!   small file so there is no storage migration: a conversation id is derived
//!   from the contact's *primary* ticket and never changes. A sibling is an
//!   extra hidden peer that mirrors that conversation.
//!
//! What this does not do: a sibling only receives what *you* send from now on
//! (and its own messages arrive in the same conversation). It does not receive
//! the contact's own sent messages or past history — that is device-to-device
//! history sync, a separate feature — and group messages and attachments are
//! not fanned out.

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use siar_domain::{ConversationId, DeviceId};
use siar_identity_multidevice::certificate::DeviceCertificate;
use siar_identity_multidevice::root_key::RootPublicKey;
use siar_messaging::PeerTicket;

const FILE: &str = "contact_devices.bin";
/// A contact may have at most this many extra devices on record.
pub const MAX_SIBLINGS: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AnnounceError {
    #[error("the certificate is not valid for the account key it names")]
    BadCertificate,
    #[error("the certificate is for a different device than the ticket")]
    TicketMismatch,
    #[error("the certificate has expired")]
    Expired,
    #[error("this contact's account key changed")]
    RootChanged,
    #[error("too many devices are recorded for this contact")]
    TooMany,
}

/// Check an announcement. `known_root` is the root key first recorded for this
/// contact, if any.
pub fn verify_announcement(
    known_root: Option<[u8; 32]>,
    root_public_key: [u8; 32],
    certificate: &DeviceCertificate,
    ticket: &PeerTicket,
    now_millis: u64,
) -> Result<(), AnnounceError> {
    if let Some(known) = known_root {
        if known != root_public_key {
            return Err(AnnounceError::RootChanged);
        }
    }
    if certificate.is_expired(now_millis) {
        return Err(AnnounceError::Expired);
    }
    if certificate.device_public_key != ticket.ed25519_verifying {
        return Err(AnnounceError::TicketMismatch);
    }
    certificate
        .verify_signature(&RootPublicKey(root_public_key))
        .map_err(|_| AnnounceError::BadCertificate)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sibling {
    pub device_id: DeviceId,
    pub ticket_text: String,
    pub added_at_millis: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ContactDevices {
    roots: HashMap<ConversationId, [u8; 32]>,
    siblings: HashMap<ConversationId, Vec<Sibling>>,
}

/// What recording a sibling did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recorded {
    /// A device we had not seen before.
    New,
    /// Already known (a repeat announcement): nothing changed.
    Known,
}

impl ContactDevices {
    #[must_use]
    pub fn load(dir: &Path) -> Self {
        std::fs::read(dir.join(FILE))
            .ok()
            .and_then(|b| postcard::from_bytes(&b).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        let bytes = postcard::to_allocvec(self).expect("plain data always serializes");
        let tmp = dir.join(format!("{FILE}.tmp"));
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(tmp, dir.join(FILE))
    }

    #[must_use]
    pub fn known_root(&self, contact: ConversationId) -> Option<[u8; 32]> {
        self.roots.get(&contact).copied()
    }

    /// Record a verified sibling. The first announcement pins the root key.
    pub fn record(
        &mut self,
        contact: ConversationId,
        root_public_key: [u8; 32],
        sibling: Sibling,
    ) -> Result<Recorded, AnnounceError> {
        if let Some(known) = self.roots.get(&contact) {
            if *known != root_public_key {
                return Err(AnnounceError::RootChanged);
            }
        }
        let list = self.siblings.entry(contact).or_default();
        if list.iter().any(|s| s.device_id == sibling.device_id) {
            return Ok(Recorded::Known);
        }
        if list.len() >= MAX_SIBLINGS {
            return Err(AnnounceError::TooMany);
        }
        list.push(sibling);
        self.roots.insert(contact, root_public_key);
        Ok(Recorded::New)
    }

    /// Forget a device (it was removed from the contact's account). `true` if
    /// it was recorded. The pinned root key stays: the account is unchanged.
    pub fn remove(&mut self, contact: ConversationId, device_id: DeviceId) -> bool {
        let Some(list) = self.siblings.get_mut(&contact) else {
            return false;
        };
        let before = list.len();
        list.retain(|s| s.device_id != device_id);
        list.len() != before
    }

    #[must_use]
    pub fn siblings_of(&self, contact: ConversationId) -> Vec<PeerTicket> {
        self.siblings
            .get(&contact)
            .into_iter()
            .flatten()
            .filter_map(|s| PeerTicket::decode(&s.ticket_text).ok())
            .collect()
    }

    /// Which contact a frame from `endpoint` belongs to, and the ticket to
    /// decrypt it with, if `endpoint` is a recorded sibling device.
    #[must_use]
    pub fn lookup_endpoint(
        &self,
        endpoint: &iroh::EndpointId,
    ) -> Option<(ConversationId, PeerTicket)> {
        self.siblings.iter().find_map(|(contact, list)| {
            list.iter().find_map(|s| {
                PeerTicket::decode(&s.ticket_text)
                    .ok()
                    .filter(|t| t.endpoint_addr.id == *endpoint)
                    .map(|t| (*contact, t))
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use siar_domain::AccountId;
    use siar_identity_multidevice::linking_authority::DeviceRole;
    use siar_identity_multidevice::root_key::RootIdentityKey;

    fn ticket_for(seed: u8) -> PeerTicket {
        let secret = iroh::SecretKey::from_bytes(&[seed; 32]);
        PeerTicket {
            endpoint_addr: iroh::EndpointAddr::new(secret.public()),
            ed25519_verifying: [seed; 32],
            x25519_public: [seed; 32],
        }
    }

    fn cert(root: &RootIdentityKey, t: &PeerTicket, now: u64) -> DeviceCertificate {
        DeviceCertificate::issue(
            root,
            AccountId::new(),
            DeviceId::new(),
            t.ed25519_verifying,
            now,
            None,
            DeviceRole::Standard.default_capabilities(),
            1,
        )
    }

    #[test]
    fn a_good_announcement_verifies() {
        let root = RootIdentityKey::generate();
        let t = ticket_for(5);
        let c = cert(&root, &t, 10);
        assert_eq!(
            verify_announcement(None, root.root_public_key().0, &c, &t, 11),
            Ok(())
        );
    }

    #[test]
    fn every_way_to_lie_is_refused() {
        let root = RootIdentityKey::generate();
        let other = RootIdentityKey::generate();
        let t = ticket_for(5);
        let c = cert(&root, &t, 10);
        let rpk = root.root_public_key().0;
        // A certificate signed by a different root than the one named.
        assert_eq!(
            verify_announcement(None, other.root_public_key().0, &c, &t, 11),
            Err(AnnounceError::BadCertificate)
        );
        // A real certificate attached to someone else's ticket.
        assert_eq!(
            verify_announcement(None, rpk, &c, &ticket_for(6), 11),
            Err(AnnounceError::TicketMismatch)
        );
        // A tampered certificate.
        let mut forged = c.clone();
        forged.generation += 1;
        assert_eq!(
            verify_announcement(None, rpk, &forged, &t, 11),
            Err(AnnounceError::BadCertificate)
        );
        // A root key different from the one pinned for this contact.
        assert_eq!(
            verify_announcement(Some(other.root_public_key().0), rpk, &c, &t, 11),
            Err(AnnounceError::RootChanged)
        );
    }

    fn sibling(seed: u8) -> Sibling {
        Sibling {
            device_id: DeviceId::new(),
            ticket_text: ticket_for(seed).encode(),
            added_at_millis: 1,
        }
    }

    #[test]
    fn the_first_announcement_pins_the_root_and_repeats_change_nothing() {
        let contact = ConversationId::new();
        let mut d = ContactDevices::default();
        let s = sibling(7);
        assert_eq!(d.record(contact, [1; 32], s.clone()), Ok(Recorded::New));
        assert_eq!(d.known_root(contact), Some([1; 32]));
        assert_eq!(d.record(contact, [1; 32], s), Ok(Recorded::Known));
        assert_eq!(d.siblings_of(contact).len(), 1);
        // A different root for the same contact is refused, nothing recorded.
        assert_eq!(
            d.record(contact, [2; 32], sibling(8)),
            Err(AnnounceError::RootChanged)
        );
        assert_eq!(d.siblings_of(contact).len(), 1);
    }

    #[test]
    fn a_removed_device_is_forgotten_but_the_pinned_root_stays() {
        let contact = ConversationId::new();
        let mut d = ContactDevices::default();
        let s = sibling(31);
        let id = s.device_id;
        d.record(contact, [1; 32], s).unwrap();
        assert!(d.remove(contact, id));
        assert!(!d.remove(contact, id), "removing twice reports nothing");
        assert!(d.siblings_of(contact).is_empty());
        assert_eq!(d.known_root(contact), Some([1; 32]));
        assert!(!d.remove(ConversationId::new(), id));
    }

    #[test]
    fn siblings_are_bounded() {
        let contact = ConversationId::new();
        let mut d = ContactDevices::default();
        for i in 0..MAX_SIBLINGS as u8 {
            assert_eq!(
                d.record(contact, [1; 32], sibling(10 + i)),
                Ok(Recorded::New)
            );
        }
        assert_eq!(
            d.record(contact, [1; 32], sibling(99)),
            Err(AnnounceError::TooMany)
        );
    }

    #[test]
    fn a_frame_from_a_sibling_resolves_to_the_contact_and_survives_a_restart() {
        let dir = std::env::temp_dir().join(format!("siar-cd-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let contact = ConversationId::new();
        let mut d = ContactDevices::default();
        let s = sibling(21);
        let ticket = PeerTicket::decode(&s.ticket_text).unwrap();
        d.record(contact, [1; 32], s).unwrap();
        d.save(&dir).unwrap();

        let back = ContactDevices::load(&dir);
        let (c, t) = back.lookup_endpoint(&ticket.endpoint_addr.id).unwrap();
        assert_eq!(c, contact);
        assert_eq!(t.endpoint_addr.id, ticket.endpoint_addr.id);
        // A stranger's endpoint is not a sibling of anyone.
        assert!(back
            .lookup_endpoint(&ticket_for(77).endpoint_addr.id)
            .is_none());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_damaged_file_means_no_siblings_not_a_crash() {
        let dir = std::env::temp_dir().join(format!("siar-cd-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(FILE), b"\xff\xff nonsense").unwrap();
        assert!(ContactDevices::load(&dir)
            .siblings_of(ConversationId::new())
            .is_empty());
        std::fs::remove_dir_all(dir).ok();
    }
}

//! §52 "Device Directory", §53 "Device Directory Entry", §55 "Stale
//! Device Directory". A signed snapshot (§52's own "can be a compact
//! snapshot derived from events" — the alternative to a full signed
//! event log this crate takes, since a snapshot's rollback rule (§56)
//! is simpler to state and enforce correctly than event-log conflict
//! resolution, and §52 explicitly allows it). This workspace used to
//! also have an event-sourced alternative for a *different*,
//! now-retired device-trust model — `siar_domain::device::{DeviceEvent,
//! DeviceRegistry}` — see `MIGRATION.md`'s device-certificate
//! reconciliation section; this directory is now the sole local
//! device-registry model in this workspace.

use serde::{Deserialize, Serialize};

use crate::certificate::DeviceCertificate;
use crate::error::IdentityError;
use crate::root_key::{RootIdentityKey, RootPublicKey};
use siar_domain::{AccountId, DeviceId};

/// §53's status values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeviceStatus {
    Active,
    Revoked,
    Expired,
}

/// §53's `transport_endpoints` field, kept fully opaque — this crate
/// has no transport dependency and shouldn't gain one just to type
/// this field (the same transport-neutrality principle spec 01
/// establishes for `siar-protocol-ext::RoutingRequirements`/
/// `PeerIdentity`, applied here too). A real transport layer
/// (`siar-transport`) is responsible for interpreting these bytes;
/// identity only stores and forwards them as part of the signed
/// directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceEndpoint(pub Vec<u8>);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceDirectoryEntry {
    pub device_id: DeviceId,
    pub certificate: DeviceCertificate,
    pub status: DeviceStatus,
    pub transport_endpoints: Vec<DeviceEndpoint>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceDirectory {
    pub account_id: AccountId,
    pub generation: u64,
    pub devices: Vec<DeviceDirectoryEntry>,
    /// See [`crate::certificate::DeviceCertificate::signature`]'s own
    /// doc comment — identical reasoning, same fix.
    pub signature: Vec<u8>,
}

impl DeviceDirectory {
    fn signing_payload(
        account_id: AccountId,
        generation: u64,
        devices: &[DeviceDirectoryEntry],
    ) -> Vec<u8> {
        #[derive(Serialize)]
        struct Payload<'a> {
            account_id: AccountId,
            generation: u64,
            devices: &'a [DeviceDirectoryEntry],
        }
        postcard::to_allocvec(&Payload {
            account_id,
            generation,
            devices,
        })
        .expect("postcard encoding of a fixed-shape struct cannot fail")
    }

    pub fn sign(
        root_key: &RootIdentityKey,
        account_id: AccountId,
        generation: u64,
        devices: Vec<DeviceDirectoryEntry>,
    ) -> Self {
        let payload = Self::signing_payload(account_id, generation, &devices);
        let signature = root_key.sign(&payload).to_vec();
        Self {
            account_id,
            generation,
            devices,
            signature,
        }
    }

    pub fn verify_signature(&self, root_public_key: &RootPublicKey) -> Result<(), IdentityError> {
        let payload = Self::signing_payload(self.account_id, self.generation, &self.devices);
        let signature: [u8; 64] = self
            .signature
            .as_slice()
            .try_into()
            .map_err(|_| IdentityError::MalformedKey)?;
        root_public_key.verify(&payload, &signature)
    }

    /// §26's fan-out rule made concrete: every device this account
    /// should currently receive new data/messages/keys through.
    pub fn active_devices(&self) -> impl Iterator<Item = &DeviceDirectoryEntry> {
        self.devices
            .iter()
            .filter(|d| d.status == DeviceStatus::Active)
    }

    /// §26 "Revocation Semantics": the single check every one of the
    /// four things revocation must prevent — "future session
    /// establishment, future group key delivery, future device
    /// synchronization, future account-authorized operations" — reduces
    /// to. A caller enforcing any of those four should call this once
    /// per operation rather than re-deriving "is this device still
    /// active" locally each time. A device this directory has never
    /// heard of at all (never certified) is also untrusted — absence
    /// isn't a special case a caller needs to check separately from
    /// revocation.
    ///
    /// §26 also states plainly what this function's `true` result does
    /// NOT mean: it "cannot guarantee deletion of data already copied
    /// to the revoked device." That's a real limitation of what
    /// revocation *is*, not something any function here could close —
    /// it belongs in documentation and UI copy, per §26's own words,
    /// not in code.
    /// §156 "New Device History Bootstrap": "identity only
    /// establishes: this device is authorized to participate" —
    /// conversation history/files/contacts/settings sync is
    /// explicitly named as "a synchronization problem layered above
    /// identity," and this function IS that one narrow fact identity
    /// establishes. There is no conversation/file/contact/settings
    /// type anywhere in this crate for the same reason §109's address
    /// book and §141's file-only reuse test both make: the absence is
    /// correct, not a gap. §157 "History Authorization" draws the same
    /// line one layer up — which of "full history / future messages
    /// only / selected history" a newly linked device gets is named
    /// spec's own example of something that "belongs to product/data
    /// policy," so there is no `HistoryPolicy` type here either; this
    /// function answers "is this device authorized to participate" and
    /// stops there, on purpose.
    pub fn is_device_trusted(&self, device_id: DeviceId) -> bool {
        self.devices
            .iter()
            .any(|d| d.device_id == device_id && d.status == DeviceStatus::Active)
    }

    /// §59 "Account State Chain"'s `prev_hash` field needs something
    /// real to hash — this is it: a blake3 hash of this directory's own
    /// signature bytes (which already commit to every other field via
    /// [`Self::signing_payload`], so hashing the signature alone is
    /// sufficient and avoids re-serializing the whole directory a
    /// second time just to hash it). See [`crate::state_chain`] for
    /// where this is actually used, and its own module doc for why
    /// this crate's snapshot model doesn't switch to a live event
    /// chain just because this hash now exists.
    pub fn state_hash(&self) -> [u8; 32] {
        blake3::hash(&self.signature).into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::DeviceCapabilitySet;

    #[test]
    fn a_signed_directory_verifies_and_filters_to_active_devices() {
        let root = RootIdentityKey::generate();
        let account = AccountId::new();
        let active_device = DeviceId::new();
        let revoked_device = DeviceId::new();

        let active_cert = DeviceCertificate::issue(
            &root,
            account,
            active_device,
            [1u8; 32],
            0,
            None,
            DeviceCapabilitySet::SEND_MESSAGE,
            1,
        );
        let revoked_cert = DeviceCertificate::issue(
            &root,
            account,
            revoked_device,
            [2u8; 32],
            0,
            None,
            DeviceCapabilitySet::SEND_MESSAGE,
            1,
        );

        let dir = DeviceDirectory::sign(
            &root,
            account,
            1,
            vec![
                DeviceDirectoryEntry {
                    device_id: active_device,
                    certificate: active_cert,
                    status: DeviceStatus::Active,
                    transport_endpoints: vec![],
                },
                DeviceDirectoryEntry {
                    device_id: revoked_device,
                    certificate: revoked_cert,
                    status: DeviceStatus::Revoked,
                    transport_endpoints: vec![],
                },
            ],
        );

        assert!(dir.verify_signature(&root.root_public_key()).is_ok());
        let active: Vec<DeviceId> = dir.active_devices().map(|d| d.device_id).collect();
        assert_eq!(active, vec![active_device]);
        assert!(dir.is_device_trusted(active_device));
        assert!(!dir.is_device_trusted(revoked_device));
        assert!(!dir.is_device_trusted(DeviceId::new())); // never certified at all
    }
}

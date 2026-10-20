//! §165-168: who may do what with a blob, kept as separate questions.
//!
//! The central rule (§167): **"relay allowed" is not "recipient may
//! re-share plaintext"**, and neither is "can decrypt". Three different
//! facts, three different predicates, no function that answers one by
//! calling another.
//!
//! * A [`Role::Relay`] stores and forwards ciphertext (§168). It is never
//!   authorized to read or forward as the recipient.
//! * A [`Role::Recipient`] holds decryption authorization; whether they
//!   may pass it on is the policy's decision ([`ForwardPolicy`]).
//! * §165: a private `BlobId` is never a public URL by default.
//!   [`PublicShareLink`] cannot be built from a `BlobId` alone, only from
//!   an explicit [`PublicShareGrant`], which exists only for a blob whose
//!   policy is [`ForwardPolicy::Public`].
//! * §166 [`Capability`]: the file-level vocabulary a later capability
//!   system can take over.

use crate::ids::BlobId;

/// §167's four settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForwardPolicy {
    /// Only the addressed recipient may open it.
    RecipientOnly,
    /// The recipient may re-share (and so may decrypt for others).
    Forwardable,
    /// Third parties may carry the ciphertext, but only the recipient
    /// holds access.
    RelayableCiphertext,
    /// Anyone holding a share link may open it (§165).
    Public,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Recipient,
    Relay,
    Stranger,
}

/// §166.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    Read,
    Write,
    Forward,
}

/// May `role` exercise `cap` on a blob governed by `policy`? Nothing
/// here answers "may this relay carry ciphertext" — that is
/// [`relay_may_carry`], deliberately separate.
pub fn authorize(role: Role, cap: Capability, policy: ForwardPolicy) -> bool {
    match (role, cap) {
        (Role::Recipient, Capability::Read) => true,
        (Role::Recipient, Capability::Forward) => {
            matches!(policy, ForwardPolicy::Forwardable | ForwardPolicy::Public)
        }
        // Writing a blob someone else sent is never a granted capability.
        (Role::Recipient, Capability::Write) => false,
        // A relay holds ciphertext, not authority.
        (Role::Relay, _) => false,
        // Anyone may read a public blob *given the link material*; that
        // is checked by the link, not by role.
        (Role::Stranger, Capability::Read) => matches!(policy, ForwardPolicy::Public),
        (Role::Stranger, _) => false,
    }
}

/// §168: may a node that is not the recipient store and forward this
/// blob's ciphertext? `on_delivery_path` is true for a relay the sender
/// themself routed through (DTN custody): carrying a recipient-only blob
/// *to its recipient* is the point of relaying; carrying it for strangers
/// is not.
pub fn relay_may_carry(policy: ForwardPolicy, on_delivery_path: bool) -> bool {
    match policy {
        ForwardPolicy::RecipientOnly => on_delivery_path,
        ForwardPolicy::Forwardable | ForwardPolicy::RelayableCiphertext | ForwardPolicy::Public => {
            true
        }
    }
}

/// Proof that the owner explicitly made a blob public. Only
/// [`PublicShareGrant::explicit`] makes one, and only for
/// [`ForwardPolicy::Public`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublicShareGrant(());

impl PublicShareGrant {
    pub fn explicit(policy: ForwardPolicy) -> Option<Self> {
        (policy == ForwardPolicy::Public).then_some(Self(()))
    }
}

/// A share link's content. Includes no key: the access material travels
/// separately, and the grant is required to construct this at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublicShareLink {
    pub blob_id: BlobId,
}

impl PublicShareLink {
    pub fn new(blob_id: BlobId, _grant: PublicShareGrant) -> Self {
        Self { blob_id }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Capability::*;
    use ForwardPolicy::*;
    use Role::*;

    const ALL_POLICIES: [ForwardPolicy; 4] =
        [RecipientOnly, Forwardable, RelayableCiphertext, Public];

    #[test]
    fn a_recipient_can_always_read_but_forwards_only_when_the_policy_says() {
        for p in ALL_POLICIES {
            assert!(authorize(Recipient, Read, p));
            assert!(!authorize(Recipient, Write, p));
        }
        assert!(!authorize(Recipient, Forward, RecipientOnly));
        assert!(!authorize(Recipient, Forward, RelayableCiphertext));
        assert!(authorize(Recipient, Forward, Forwardable));
        assert!(authorize(Recipient, Forward, Public));
    }

    #[test]
    fn relay_allowed_is_not_recipient_may_reshare() {
        // The §167 conflation, ruled out as a property: RelayableCiphertext
        // lets a relay carry the blob and still forbids the recipient from
        // re-sharing plaintext.
        assert!(relay_may_carry(RelayableCiphertext, false));
        assert!(!authorize(Recipient, Forward, RelayableCiphertext));
        // And the reverse: a forwardable blob that a stranger relay may
        // carry gives that relay no authority at all.
        for cap in [Read, Write, Forward] {
            for p in ALL_POLICIES {
                assert!(!authorize(Relay, cap, p), "relay got {cap:?} under {p:?}");
            }
        }
    }

    #[test]
    fn a_recipient_only_blob_travels_only_along_its_delivery_path() {
        assert!(relay_may_carry(RecipientOnly, true));
        assert!(!relay_may_carry(RecipientOnly, false));
    }

    #[test]
    fn strangers_read_only_public_blobs_and_never_write_or_forward() {
        for p in ALL_POLICIES {
            assert_eq!(authorize(Stranger, Read, p), p == Public);
            assert!(!authorize(Stranger, Write, p));
            assert!(!authorize(Stranger, Forward, p));
        }
    }

    #[test]
    fn a_public_link_needs_an_explicit_public_grant() {
        for p in [RecipientOnly, Forwardable, RelayableCiphertext] {
            assert!(PublicShareGrant::explicit(p).is_none(), "{p:?}");
        }
        let g = PublicShareGrant::explicit(Public).unwrap();
        let id = BlobId::from_ciphertext(b"x");
        assert_eq!(PublicShareLink::new(id, g).blob_id, id);
    }
}

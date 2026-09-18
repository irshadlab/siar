//! next.md §76–77's mailbox check-in — a device explicitly identifying
//! *itself* to a relay and asking "what do you have for me," distinct
//! from `mesh.rs`'s `MeshEnvelope` routing.
//!
//! ## Why this is a different message, not a `Mesh` variant
//!
//! `apps/emergency-node`'s existing forward-on-contact behavior offers
//! stored bundles to *whichever peer it happens to be talking to*,
//! learning nothing about who that peer is — `MeshEnvelope` deliberately
//! carries a `destination` but no sender identity (see that module's
//! own doc comment), because next.md §73–74's mesh-privacy sections are
//! explicit that an intermediate relay shouldn't learn more than it
//! needs to route. A `MailboxCheckIn` is the opposite kind of
//! disclosure: the device sending one is revealing *its own* identity,
//! by its own choice, specifically so a relay can answer "is anything
//! here addressed to you" — the same distinction next.md §32 draws
//! between "an intermediate carrying ciphertext for someone else" and
//! "the actual recipient identifying themselves to collect it."
//! Folding this into `MeshEnvelope` would have blurred that line rather
//! than just being a smaller diff.
//!
//! ## Authentication (added this pass) vs. what next.md §32 fully asks for
//!
//! A `MailboxCheckIn` now carries a self-asserted Ed25519 verifying key
//! and a signature over `(device, verifying_key, issued_at_millis)`,
//! verified with `siar_crypto::DeviceIdentity::verify` — the same
//! already-adopted primitive every signed message in this workspace
//! uses, not a new cryptographic scheme (this module's own earlier
//! version of this doc comment flagged custom cryptography as
//! something to avoid per architecture.md §106; standard signature
//! verification with an existing primitive isn't that). This closes
//! the plain claim this type used to be: without a valid signature
//! matching the claimed key, `MailboxCheckIn::verify` rejects it
//! outright, so an attacker without a device's private key can no
//! longer produce a check-in for it at all.
//!
//! [`DeviceKeyDirectory`] adds trust-on-first-use pinning on top of
//! that — the same well-understood pattern SSH host keys use, not a
//! bespoke scheme either: the first check-in seen for a `DeviceId`
//! establishes its key, and a later check-in claiming the same
//! `DeviceId` with a *different* key is rejected as a probable
//! impersonation attempt.
//!
//! What this still does NOT do, and next.md §32 explicitly asks for:
//! "Avoid stable public account IDs directly indexing mailbox messages.
//! Use rotating mailbox capabilities/tokens." `MailboxCheckIn::device`
//! is still a bare, stable `DeviceId` — this pass closes the
//! *authentication* half of §32 (is this really the device it claims
//! to be), not the *unlinkability* half. [`AnonymousMailboxCheckIn`]
//! (added a later pass, alongside [`siar_crypto::mailbox_token`]) is
//! that unlinkability half — a genuinely different, opt-in check-in
//! path a device uses instead of this one once it's paired with
//! whoever it expects mailbox contents from, not a replacement for
//! this identified path (see [`AnonymousMailboxCheckIn`]'s own doc
//! comment for why both still need to coexist, and exactly what
//! "closes the gap" means and doesn't mean here).

use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use siar_crypto::{epoch_for, DeviceIdentity, MailboxToken, MailboxTokenSecret};
use siar_domain::DeviceId;
use std::collections::HashMap;
use thiserror::Error;

/// What can go wrong verifying or pinning a [`MailboxCheckIn`] — kept
/// in this module rather than a shared crate-wide error type, same
/// "one error type per concern" convention `siar_messaging::
/// GroupServiceError` etc. already use elsewhere in this workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum MailboxCheckInError {
    #[error("check-in's verifying key is malformed")]
    MalformedKey,
    #[error("check-in signature does not match its claimed verifying key")]
    BadSignature,
    #[error("check-in is outside the accepted freshness window")]
    Expired,
    #[error("check-in's verifying key does not match this device's previously pinned key")]
    KeyMismatch,
}

/// See this module's top doc comment for what sending one now proves
/// (possession of the private key matching `verifying_key`) and what a
/// relay answering one still can't verify on its own (whether
/// `verifying_key` is genuinely *this* device's long-term key, as
/// opposed to a key generated for the occasion — that's
/// [`DeviceKeyDirectory`]'s job, one layer up).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MailboxCheckIn {
    pub device: DeviceId,
    /// Self-asserted Ed25519 verifying key, raw bytes — same
    /// `[u8; 32]`-over-the-wire convention `PeerTicket::
    /// ed25519_verifying` already uses, rather than a differently-typed
    /// field for what's conceptually the same kind of value.
    pub verifying_key: [u8; 32],
    /// Millis since epoch this check-in was signed (`siar_domain`'s own
    /// opaque-tick convention doesn't apply here — a relay checking
    /// freshness needs comparable wall-clock time from a device it has
    /// no other clock-sync relationship with, so this one field is
    /// genuine wall-clock milliseconds, not a caller-supplied tick).
    pub issued_at_millis: u64,
    /// Ed25519 signature over this check-in's own fields (see
    /// `signing_payload`) — 64 raw signature bytes. `Vec<u8>` rather
    /// than `[u8; 64]`, matching `siar_identity_multidevice::
    /// certificate::DeviceCertificate::signature`'s own already-established
    /// precedent for exactly this: serde's derive doesn't support
    /// fixed-size arrays longer than 32 elements without an extra
    /// crate (confirmed by a real compile error against this exact
    /// field, not a guess). `verify` below is what actually enforces
    /// it's a valid 64-byte signature via `try_into`.
    pub signature: Vec<u8>,
}

impl MailboxCheckIn {
    /// Builds and signs a check-in. `now_millis` is genuine wall-clock
    /// time (see `issued_at_millis`'s own doc comment) — a caller
    /// without a reliable clock shouldn't be signing check-ins in the
    /// first place, since a relay will reject one outside its
    /// freshness window regardless.
    pub fn new(identity: &DeviceIdentity, device: DeviceId, now_millis: u64) -> Self {
        let verifying_key = identity.verifying_key().to_bytes();
        let payload = Self::signing_payload(device, &verifying_key, now_millis);
        let signature = identity.sign(&payload).to_bytes().to_vec();
        Self {
            device,
            verifying_key,
            issued_at_millis: now_millis,
            signature,
        }
    }

    /// Canonical bytes the signature covers — `device`'s raw UUID bytes,
    /// then the verifying key, then the timestamp as little-endian —
    /// binding all three together so an attacker can't splice a valid
    /// signature from one check-in onto a different `device` or a
    /// replayed-but-relabeled `issued_at_millis`.
    fn signing_payload(
        device: DeviceId,
        verifying_key: &[u8; 32],
        issued_at_millis: u64,
    ) -> Vec<u8> {
        let mut payload = Vec::with_capacity(16 + 32 + 8);
        payload.extend_from_slice(device.as_uuid().as_bytes());
        payload.extend_from_slice(verifying_key);
        payload.extend_from_slice(&issued_at_millis.to_le_bytes());
        payload
    }

    /// Verifies the signature (proof of key possession) and freshness
    /// only — deliberately does NOT check whether `verifying_key` is
    /// the key this `device` has used before. That's stateful
    /// (`DeviceKeyDirectory`'s job); this method stays the same "pure
    /// logic over caller-supplied data" shape as everything else in
    /// this workspace's protocol/routing layer.
    pub fn verify(&self, now_millis: u64, max_age_millis: u64) -> Result<(), MailboxCheckInError> {
        let verifying_key = VerifyingKey::from_bytes(&self.verifying_key)
            .map_err(|_| MailboxCheckInError::MalformedKey)?;
        let signature_bytes: [u8; 64] = self
            .signature
            .as_slice()
            .try_into()
            .map_err(|_| MailboxCheckInError::MalformedKey)?;
        let signature = Signature::from_bytes(&signature_bytes);
        let payload =
            Self::signing_payload(self.device, &self.verifying_key, self.issued_at_millis);
        DeviceIdentity::verify(&verifying_key, &payload, &signature)
            .map_err(|_| MailboxCheckInError::BadSignature)?;

        // Two-sided window: rejects both a stale, possibly-replayed
        // check-in AND one absurdly far in the future (which could
        // otherwise be replayed indefinitely, since a signature alone
        // doesn't expire). `max_age_millis` doubles as the clock-skew
        // tolerance on the future side rather than a separate tunable —
        // simpler, and this workspace has no other established
        // "acceptable clock skew" constant to borrow instead.
        let is_too_old = now_millis.saturating_sub(self.issued_at_millis) > max_age_millis;
        let is_too_far_future = self.issued_at_millis.saturating_sub(now_millis) > max_age_millis;
        if is_too_old || is_too_far_future {
            return Err(MailboxCheckInError::Expired);
        }
        Ok(())
    }
}

/// Trust-on-first-use pinning of a device's verifying key across
/// check-ins — see this module's top doc comment for why this is a
/// well-understood existing pattern (SSH host keys), not a bespoke
/// cryptographic scheme. The first check-in seen for a `DeviceId`
/// establishes that device's key for this directory's lifetime; a
/// later check-in claiming the same `DeviceId` with a *different* key
/// is rejected outright.
///
/// Deliberately unbounded (no `remove_stale`, unlike `PathTable`/
/// `DeviceRoutes`) — pinning is a security property that should persist
/// for as long as this process considers a device "known," not
/// something that should silently lapse and let a since-rotated,
/// wrongly-signed key back in. A relay that runs long enough to make
/// this directory's memory use a real concern is real, separate
/// capacity-planning work, same "not attempted here" honesty this
/// workspace already gives every other genuinely-deferred concern.
pub struct DeviceKeyDirectory {
    pinned: HashMap<DeviceId, [u8; 32]>,
}

impl DeviceKeyDirectory {
    pub fn new() -> Self {
        Self {
            pinned: HashMap::new(),
        }
    }

    /// Verifies `check_in` (signature + freshness, via
    /// [`MailboxCheckIn::verify`]) and enforces key pinning in one call
    /// — a caller has no correct reason to pin a check-in whose
    /// signature hasn't already been verified, so this doesn't offer
    /// the two as separate steps a caller could accidentally reorder.
    pub fn verify_and_pin(
        &mut self,
        check_in: &MailboxCheckIn,
        now_millis: u64,
        max_age_millis: u64,
    ) -> Result<(), MailboxCheckInError> {
        check_in.verify(now_millis, max_age_millis)?;
        match self.pinned.get(&check_in.device) {
            Some(pinned_key) if *pinned_key != check_in.verifying_key => {
                Err(MailboxCheckInError::KeyMismatch)
            }
            Some(_) => Ok(()),
            None => {
                self.pinned.insert(check_in.device, check_in.verifying_key);
                Ok(())
            }
        }
    }
}

impl Default for DeviceKeyDirectory {
    fn default() -> Self {
        Self::new()
    }
}

/// The unlinkability half of next.md §32 — a device presenting a
/// [`siar_crypto::mailbox_token::MailboxToken`] instead of its own
/// [`DeviceId`], so a relay serving this check-in learns nothing that
/// lets it correlate it with any other check-in outside the current
/// epoch (see `siar_crypto::mailbox_token`'s own doc comment for the
/// derivation and its real, named limits — in particular: within one
/// epoch a token is still a stable, linkable index, and possessing a
/// token *is* the authorization, bearer-capability style, with no
/// separate signature).
///
/// ## Why this doesn't replace [`MailboxCheckIn`] — both stay
///
/// [`MailboxCheckIn`] answers "who is this device" for cases that
/// genuinely need that: `apps/emergency-node`'s `DeviceRoutes`
/// proactive-push feature (§76-77) has to learn *which* `DeviceId` a
/// peer is, or it has nothing to key its routing hint on. A device
/// that wants that convenience is choosing to disclose itself, exactly
/// as this module's top doc comment already describes.
///
/// [`AnonymousMailboxCheckIn`] answers a different question a relay
/// only needs to be able to answer: "does anything exist for the
/// bearer of this specific token" — no device identity, no routing
/// hint, nothing beyond what's filed under this one epoch's token.
/// It's the check-in a privacy-preferring device uses once it already
/// has a [`siar_crypto::mailbox_token::MailboxTokenSecret`] with the
/// specific contact it expects mail from (see that type's own
/// "must already exist" precondition) — not a drop-in replacement for
/// every check-in `MailboxCheckIn` currently handles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnonymousMailboxCheckIn {
    pub token: MailboxToken,
}

impl AnonymousMailboxCheckIn {
    /// Builds a check-in for the epoch `now_millis` falls in — see
    /// `siar_crypto::mailbox_token::epoch_for`'s doc comment for what
    /// "falls in" means and its clock-skew implications. Infallible:
    /// unlike [`MailboxCheckIn::new`], there's no signature step that
    /// could be built over stale input, since the token itself is
    /// simply a deterministic function of the secret and the epoch.
    pub fn new(secret: &MailboxTokenSecret, now_millis: u64) -> Self {
        Self {
            token: secret.token_for_epoch(epoch_for(now_millis)),
        }
    }
}

/// Stores mailbox contents keyed by [`MailboxToken`] instead of
/// [`DeviceId`] — the "relay must adopt a token-keyed store" piece
/// `siar_crypto::mailbox_token`'s own doc comment names as still
/// unwired. Deliberately its own type rather than a second index bolted
/// onto whatever already stores `MailboxCheckIn`-addressed bundles: a
/// caller storing something here is making the same deliberate choice
/// [`AnonymousMailboxCheckIn`] represents on the receiving side —
/// addressing a bundle by token, not by the recipient's `DeviceId` —
/// and keeping the two stores structurally separate makes it
/// impossible to accidentally leak a `DeviceId`-keyed lookup into the
/// token-keyed path or vice versa.
///
/// Pure storage, no bundle contents/encryption opinions of its own —
/// same "just persistence, not policy" shape as
/// `siar_dtn::bundle`'s own storage layer, `V` left generic rather than
/// hardcoded to `siar_dtn::bundle::MeshBundle` so this stays usable in
/// isolated tests without pulling in that crate as a dependency.
///
/// `apps/emergency-node` (a later pass) is this type's first real
/// caller — see [`TokenMailboxEnvelope`]'s own doc comment for the wire
/// message that actually reaches it.
pub struct TokenMailboxStore<V> {
    entries: HashMap<MailboxToken, Vec<V>>,
}

impl<V> TokenMailboxStore<V> {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    /// Files `value` under `token` — appends rather than replaces,
    /// since more than one bundle can legitimately be waiting under
    /// the same epoch's token at once (mirrors how a `DeviceId`-keyed
    /// mailbox can hold more than one pending bundle today).
    pub fn deposit(&mut self, token: MailboxToken, value: V) {
        self.entries.entry(token).or_default().push(value);
    }

    /// Removes and returns everything currently filed under `token` —
    /// a check-in consumes what it collects, same "ask once, get
    /// everything waiting" shape a `DeviceId`-keyed mailbox lookup
    /// already has, not a peek that would let a check-in be repeated
    /// to keep re-reading the same contents.
    pub fn collect(&mut self, token: MailboxToken) -> Vec<V> {
        self.entries.remove(&token).unwrap_or_default()
    }
}

impl<V> Default for TokenMailboxStore<V> {
    fn default() -> Self {
        Self::new()
    }
}

/// The wire message a sender uses to file something under a
/// [`MailboxToken`] instead of addressing [`crate::mesh::MeshEnvelope`]
/// at a [`DeviceId`] — the anonymous-path counterpart to
/// `MeshEnvelope`, deliberately a separate type rather than adding an
/// enum variant to `destination` there: keeping the two wire shapes
/// structurally distinct is what makes it impossible for a relay
/// implementation to accidentally run `DeviceId`-keyed lookup logic
/// against a token-addressed deposit, mirroring why
/// [`TokenMailboxStore`] is a separate type from whatever stores
/// `MeshEnvelope`-addressed bundles.
///
/// Fields otherwise mirror `MeshEnvelope` exactly (same hop-limit/
/// expiry/priority/payload-hash shape, same "ciphertext is opaque to
/// this crate" rule) — this is genuinely the same kind of message, just
/// addressed differently, not a reason to duplicate documentation for
/// every field again here.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenMailboxEnvelope {
    pub id: siar_domain::MessageId,
    pub destination_token: MailboxToken,
    pub created_at: u64,
    pub expires_at: u64,
    pub hop_limit: u8,
    pub priority: siar_domain::MessagePriority,
    pub payload_hash: [u8; 32],
    pub ciphertext: Vec<u8>,
}

impl TokenMailboxEnvelope {
    pub fn is_expired(&self, now: u64) -> bool {
        now >= self.expires_at
    }

    /// Mirrors `MeshEnvelope::forwarded`'s exact contract — see that
    /// method's own doc comment for why this is duplicated rather than
    /// shared (same crate-layering reason).
    pub fn forwarded(mut self) -> Option<Self> {
        if self.hop_limit == 0 {
            return None;
        }
        self.hop_limit -= 1;
        Some(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_freshly_signed_check_in_verifies() {
        let identity = DeviceIdentity::generate();
        let device = DeviceId::new();
        let check_in = MailboxCheckIn::new(&identity, device, 1_000);
        assert!(check_in.verify(1_000, 60_000).is_ok());
    }

    #[test]
    fn a_check_in_signed_by_a_different_key_fails_verification() {
        let identity = DeviceIdentity::generate();
        let attacker_identity = DeviceIdentity::generate();
        let device = DeviceId::new();

        let mut check_in = MailboxCheckIn::new(&identity, device, 1_000);
        // Swap in the attacker's key without re-signing — simulates
        // someone claiming a `DeviceId` without possessing its
        // matching private key.
        check_in.verifying_key = attacker_identity.verifying_key().to_bytes();

        assert!(matches!(
            check_in.verify(1_000, 60_000),
            Err(MailboxCheckInError::BadSignature)
        ));
    }

    #[test]
    fn a_tampered_device_id_fails_verification() {
        let identity = DeviceIdentity::generate();
        let mut check_in = MailboxCheckIn::new(&identity, DeviceId::new(), 1_000);
        check_in.device = DeviceId::new(); // splice attempt: different device, same signature
        assert!(matches!(
            check_in.verify(1_000, 60_000),
            Err(MailboxCheckInError::BadSignature)
        ));
    }

    #[test]
    fn a_stale_check_in_is_rejected() {
        let identity = DeviceIdentity::generate();
        let check_in = MailboxCheckIn::new(&identity, DeviceId::new(), 1_000);
        assert!(matches!(
            check_in.verify(1_000 + 61_000, 60_000),
            Err(MailboxCheckInError::Expired)
        ));
    }

    #[test]
    fn a_check_in_from_too_far_in_the_future_is_rejected() {
        let identity = DeviceIdentity::generate();
        let check_in = MailboxCheckIn::new(&identity, DeviceId::new(), 1_000 + 61_000);
        assert!(matches!(
            check_in.verify(1_000, 60_000),
            Err(MailboxCheckInError::Expired)
        ));
    }

    #[test]
    fn a_check_in_within_the_window_on_either_edge_is_accepted() {
        let identity = DeviceIdentity::generate();
        let check_in = MailboxCheckIn::new(&identity, DeviceId::new(), 1_000);
        assert!(check_in.verify(1_000 + 60_000, 60_000).is_ok());
        assert!(check_in.verify(1_000, 60_000).is_ok());
    }

    #[test]
    fn device_key_directory_pins_the_first_key_it_sees() {
        let identity = DeviceIdentity::generate();
        let device = DeviceId::new();
        let check_in = MailboxCheckIn::new(&identity, device, 1_000);

        let mut directory = DeviceKeyDirectory::new();
        assert!(directory.verify_and_pin(&check_in, 1_000, 60_000).is_ok());
        // Re-checking in with the same, still-valid key succeeds again.
        assert!(directory.verify_and_pin(&check_in, 1_000, 60_000).is_ok());
    }

    #[test]
    fn device_key_directory_rejects_a_different_key_for_an_already_pinned_device() {
        let identity = DeviceIdentity::generate();
        let device = DeviceId::new();
        let first_check_in = MailboxCheckIn::new(&identity, device, 1_000);

        let mut directory = DeviceKeyDirectory::new();
        directory
            .verify_and_pin(&first_check_in, 1_000, 60_000)
            .unwrap();

        // A different, legitimately self-signed identity claiming the
        // *same* `DeviceId` — passes `MailboxCheckIn::verify` on its
        // own (its signature is genuine for its own key), but must
        // still be rejected by pinning.
        let impostor_identity = DeviceIdentity::generate();
        let impostor_check_in = MailboxCheckIn::new(&impostor_identity, device, 2_000);
        assert!(impostor_check_in.verify(2_000, 60_000).is_ok());
        assert!(matches!(
            directory.verify_and_pin(&impostor_check_in, 2_000, 60_000),
            Err(MailboxCheckInError::KeyMismatch)
        ));
    }

    #[test]
    fn anonymous_check_ins_from_both_paired_devices_present_the_same_token() {
        let alice = DeviceIdentity::generate();
        let bob = DeviceIdentity::generate();
        let alice_secret = MailboxTokenSecret::establish(&alice, &bob.x25519_public());
        let bob_secret = MailboxTokenSecret::establish(&bob, &alice.x25519_public());

        let alice_check_in = AnonymousMailboxCheckIn::new(&alice_secret, 5_000);
        let bob_check_in = AnonymousMailboxCheckIn::new(&bob_secret, 5_000);
        assert_eq!(alice_check_in, bob_check_in);
    }

    #[test]
    fn anonymous_check_ins_do_not_carry_a_device_id_anywhere_in_their_fields() {
        // Structural check, not just a behavioral one: this type has
        // exactly one field, and it isn't a `DeviceId` — the whole
        // point being that nothing in this message *could* leak one,
        // not just that nothing currently does.
        let secret = MailboxTokenSecret::establish(
            &DeviceIdentity::generate(),
            &DeviceIdentity::generate().x25519_public(),
        );
        let AnonymousMailboxCheckIn { token: _ } = AnonymousMailboxCheckIn::new(&secret, 0);
    }

    #[test]
    fn token_mailbox_store_returns_everything_deposited_under_one_token() {
        let mut store: TokenMailboxStore<&'static str> = TokenMailboxStore::new();
        let secret = MailboxTokenSecret::establish(
            &DeviceIdentity::generate(),
            &DeviceIdentity::generate().x25519_public(),
        );
        let token = secret.token_for_epoch(1);

        store.deposit(token, "first bundle");
        store.deposit(token, "second bundle");
        assert_eq!(store.collect(token), vec!["first bundle", "second bundle"]);
    }

    #[test]
    fn token_mailbox_store_collect_consumes_what_it_returns() {
        let mut store: TokenMailboxStore<&'static str> = TokenMailboxStore::new();
        let secret = MailboxTokenSecret::establish(
            &DeviceIdentity::generate(),
            &DeviceIdentity::generate().x25519_public(),
        );
        let token = secret.token_for_epoch(1);

        store.deposit(token, "one-time bundle");
        assert_eq!(store.collect(token), vec!["one-time bundle"]);
        // A second check-in against the same token finds nothing left
        // — a check-in collects, it doesn't peek.
        assert!(store.collect(token).is_empty());
    }

    #[test]
    fn token_mailbox_store_never_conflates_two_different_tokens() {
        let mut store: TokenMailboxStore<&'static str> = TokenMailboxStore::new();
        let secret = MailboxTokenSecret::establish(
            &DeviceIdentity::generate(),
            &DeviceIdentity::generate().x25519_public(),
        );
        let token_epoch_1 = secret.token_for_epoch(1);
        let token_epoch_2 = secret.token_for_epoch(2);

        store.deposit(token_epoch_1, "for epoch 1");
        assert!(store.collect(token_epoch_2).is_empty());
        assert_eq!(store.collect(token_epoch_1), vec!["for epoch 1"]);
    }

    #[test]
    fn token_mailbox_envelope_is_expired_true_once_now_reaches_expires_at() {
        let envelope = TokenMailboxEnvelope {
            id: siar_domain::MessageId::new(),
            destination_token: MailboxToken([0u8; 32]),
            created_at: 0,
            expires_at: 100,
            hop_limit: 4,
            priority: siar_domain::MessagePriority::Normal,
            payload_hash: [0u8; 32],
            ciphertext: vec![1, 2, 3],
        };
        assert!(!envelope.is_expired(99));
        assert!(envelope.is_expired(100));
    }

    #[test]
    fn token_mailbox_envelope_forwarded_decrements_until_zero_then_stops() {
        let envelope = TokenMailboxEnvelope {
            id: siar_domain::MessageId::new(),
            destination_token: MailboxToken([0u8; 32]),
            created_at: 0,
            expires_at: 100,
            hop_limit: 1,
            priority: siar_domain::MessagePriority::Normal,
            payload_hash: [0u8; 32],
            ciphertext: vec![1, 2, 3],
        };
        let envelope = envelope
            .forwarded()
            .expect("hop_limit 1 -> 0 should still forward");
        assert_eq!(envelope.hop_limit, 0);
        assert!(envelope.forwarded().is_none());
    }
}

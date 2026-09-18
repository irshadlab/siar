#![forbid(unsafe_code)]

//! siar-identity-multidevice: a first slice of "Part 02 — Multi-Device
//! Identity Architecture" (one of a 24-part architecture series the
//! person supplied; Part 01, "Protocol Extension System Architecture",
//! already has its own slice in this workspace —
//! `siar-protocol-ext` — and Part 03, "Transport Routing Policy
//! Engine Architecture", does not yet have a dedicated crate built
//! against *that* specific document, though a related, independently
//! evolved system, `siar-routing`, already covers similar ground
//! against a different, earlier design doc — see that crate's own top
//! doc comment).
//!
//! ## What's real here (implemented against the spec text, not guessed)
//!
//! - [`root_key`] — §5 "Account Identity", §6 "Root Key Strategy":
//!   [`root_key::RootIdentityKey`] signs rarely (device certificates
//!   and directory snapshots only), never per-session — the spec's own
//!   explicit alternative to "root key used for every message/session."
//! - [`capability`] — §14 "Device Capability Set":
//!   [`capability::DeviceCapabilitySet`], a bitset rather than an open
//!   string set (Part 01 §9's reasoning against arbitrary strings for
//!   hot capability checks, applied here to per-device authorization).
//! - [`certificate`] — §8 "Device Certificate", §9 "Device Certificate
//!   Semantics", §30 "Device Expiry":
//!   [`certificate::DeviceCertificate::issue`]/`verify_signature`
//!   enforce §9's rule that a valid signature proves account
//!   membership only, not current trust — [`certificate::DeviceCertificate::is_expired`]
//!   is a deliberately separate check (§30: "expiration is not a
//!   replacement for revocation").
//! - [`directory`] — §52 "Device Directory", §53 "Device Directory
//!   Entry": [`directory::DeviceDirectory`] as a signed snapshot (§52's
//!   own permitted alternative to a full event log) plus
//!   [`directory::DeviceDirectory::active_devices`], the §26 fan-out
//!   rule made concrete.
//! - [`trust_store`] — §55 "Stale Device Directory", §56 "Rollback
//!   Protection", §29 "Revocation Conflict Rules":
//!   [`trust_store::TrustedAccountStore::accept`] tracks the highest
//!   trusted generation per account and rejects anything at or below
//!   it — including, per a dedicated test, the spec's own named attack
//!   scenario: a revoked device regaining authority by replaying a
//!   stale, pre-revocation directory.
//! - [`link_key`] — §16/§17's `ephemeral_link_key`:
//!   [`link_key::EphemeralLinkKeyPair`], a real X25519 keypair
//!   generated fresh per linking attempt, with real Diffie-Hellman
//!   agreement (mirrors `siar_crypto::DeviceIdentity`'s own
//!   `x25519_dalek` usage, duplicated rather than imported — this
//!   crate stays independent of `siar-crypto`, see below).
//! - [`invite`] — §16 "Device Linking Invitation":
//!   [`invite::DeviceLinkInvite`], root-key-signed, with a real,
//!   internally-generated nonce (a caller can't accidentally reuse
//!   one — §16's "one-time" requirement) and
//!   [`invite::DeviceLinkInvite::contains_no_secret_material`] making
//!   §17's "the QR should not contain private/session keys" rule a
//!   checkable property, not just a followed convention.
//! - [`verification_code`] — §19 "Numeric Verification":
//!   [`verification_code::derive_verification_code`] is a real
//!   transcript-derived (not random) 6-digit code — the invite's own
//!   signed content plus both ephemeral public keys plus the derived
//!   Diffie-Hellman shared secret, so an attacker observing only the
//!   public handshake traffic can't precompute it.
//! - [`approval`] — §20 "Linking Trust Decision":
//!   [`approval::LinkingApprovalPrompt`] carries every field §20 says
//!   must be shown before approval, with a real, if narrow, guardrail
//!   against "silent device addition" — the ordinary constructor can
//!   only produce a `NumericCodeConfirmed` prompt; an unverified one
//!   requires calling a differently-named, harder-to-reach-by-accident
//!   function instead.
//! - [`revocation`] — §25 "Device Revocation", §26 "Revocation
//!   Semantics" (the check half —
//!   [`directory::DeviceDirectory::is_device_trusted`]), §27
//!   "Immediate Local Revocation": [`revocation::revoke_device`] is the
//!   piece that was missing before this round — `DeviceStatus::Revoked`
//!   was a value a directory could hold, and
//!   [`trust_store::TrustedAccountStore`] already rejected a stale
//!   directory trying to un-revoke a device (§29, tested since the
//!   original session), but nothing actually *produced* a revocation
//!   until now. [`revocation::verify_revocation`] independently
//!   double-checks the result (generation advanced, target really
//!   revoked, every other device's status untouched) rather than
//!   trusting `revoke_device`'s own return value blindly.
//! - [`rotation`] — §31 "Device Rotation"
//!   ([`rotation::rotate_device_key`], mirroring `revoke_device`'s
//!   exact shape: same generation-advances-by-one discipline, same
//!   real-error-not-silent-noop handling for an unknown or already-
//!   revoked device), §32 "Rotation Reasons"
//!   ([`rotation::RotationReason`], verbatim five reasons). See that
//!   module's own doc comment for why "device key generation N -> N+1"
//!   is modeled via the directory's existing generation counter rather
//!   than a second, invented per-device counter.
//! - [`root_rotation`] — §33 "Root Key Rotation", §34 "Root Rotation
//!   Event" ([`root_rotation::rotate_root_key`]/[`root_rotation::verify_root_rotation`],
//!   a dual-signed continuity attestation — old root authorizes,
//!   new root accepts, both signatures cover the same payload), §35
//!   "Compromised Root Scenario"
//!   ([`root_rotation::CompromisedRootRecoveryStrategy`], spec's own
//!   five named future candidates — only two get real implementations,
//!   see [`recovery`] — plus a test making "rotation structurally
//!   requires the old private key" visible, not just true by
//!   inspection).
//! - [`recovery`] — §36 "Recovery Architecture", §37 "Recovery Policy
//!   Type" ([`recovery::RecoveryPolicy`], verbatim four variants), §38
//!   "Recovery Secret" ([`recovery::RecoverySecret`], deliberately not
//!   `Serialize`/`Deserialize` so nothing can put it on the wire by
//!   accident; [`recovery::RecoveryKeyDerivation`] is the boundary
//!   trait to a real Argon2id implementation this dependency-minimal
//!   crate doesn't provide itself), §39 "Recovery Device Addition"
//!   ([`recovery::add_device_via_recovery`], provably the same
//!   certificate-issuance path [`rotation::rotate_device_key`]/
//!   [`revocation::revoke_device`] already use, gated by recovery
//!   evidence instead of an existing device's approval — tested for
//!   both the `RecoverySecret` and `TrustedDeviceQuorum` policies,
//!   including a revoked device's signature correctly not counting
//!   toward quorum).
//! - [`fanout`] — §40 "Multi-Device Messaging Fan-Out"
//!   ([`fanout::fan_out_targets`], recipient's active devices plus the
//!   sender's OTHER active devices, never the originating one), §41
//!   "Sender Attribution" ([`fanout::SenderIdentity`], verbatim), §42
//!   "Account-Level Presentation" ([`fanout::account_level_display`],
//!   spec's own four device-revealing contexts vs. the one that
//!   doesn't), §43 "Device-Level Receipts"
//!   ([`fanout::aggregate_delivered_to_account`], only ever derived
//!   from real [`fanout::DeviceReceipt`]s, never stored on its own —
//!   "the core should retain device-level truth" enforced
//!   structurally), §44 "Sync Between User's Own Devices"
//!   ([`fanout::OwnDeviceSyncPolicy`], per-[`fanout::SyncDataClass`],
//!   defaults to NOT synced for anything unconfigured).
//! - [`device_classes`] — §45 "Device Trust Classes"
//!   ([`device_classes::DeviceTrustClass`], verbatim four, plus spec's
//!   own four worked examples reproduced as a test), §46 "Headless
//!   Devices" ([`device_classes::HeadlessDeviceOwner`], verbatim
//!   three), §47 "Service Identities"
//!   ([`device_classes::ServiceIdentityKind`], a label only — tested
//!   proving a "service identity" uses the exact same
//!   `RootIdentityKey`/`DeviceDirectory` a normal account does, no
//!   separate authentication model), §48 "Organization Identity"
//!   ([`device_classes::OrganizationDeviceRole`], verbatim four —
//!   "identity proves membership, authorization decides what it may
//!   do" is already true across this workspace: this crate has zero
//!   dependency on `siar-protocol-ext::ExtensionAuthorization`, spec
//!   01 §33's real authorization-decision trait).
//! - [`namespace`] — §49 "Multiple Accounts on One Device"
//!   ([`namespace::device_membership_is_isolated`], checking no two
//!   [`namespace::LocalAccountSession`]s share a `DeviceId` — a real,
//!   checkable form of "isolated device membership"), §50
//!   "Application Namespace" ([`namespace::ApplicationNamespace`],
//!   open string newtype; [`namespace::is_shared_across_applications_by_default`]
//!   always `false` for all four of spec's named resources), §51
//!   "Cross-Application Identity Reuse"
//!   ([`namespace::CrossApplicationIdentityMode`], whose literal
//!   `Default` impl is `IsolatedPerApp` — "the default should favor
//!   isolation" made structural, not just documented).
//! - §52 "Device Directory", §53 "Device Directory Entry": already
//!   built (see [`directory`]'s own top-of-file doc) — this round adds
//!   the one field that pass was missing,
//!   [`directory::DeviceDirectoryEntry::transport_endpoints`] (spec's
//!   own conceptual struct always had it). Kept fully opaque
//!   ([`directory::DeviceEndpoint`], a `Vec<u8>` newtype) rather than
//!   typed against any real transport's address format — this crate
//!   has no transport dependency and shouldn't gain one just to type
//!   this field.
//! - §54 "Device Directory Synchronization": no new code — "the
//!   directory is signed, so transport is not trusted for
//!   authenticity" is already true of every code path in [`directory`]
//!   ([`directory::DeviceDirectory::verify_signature`] is the only way
//!   a directory is ever accepted, regardless of which of spec's six
//!   listed sync paths carried it here).
//! - §55 "Stale Device Directory", §56 "Rollback Protection": already
//!   built (see [`trust_store`]'s own top-of-file doc, written in an
//!   earlier round).
//! - §57 "Fork Detection": **a real bug found and fixed this round**,
//!   not just new coverage — [`trust_store::TrustedAccountStore::accept`]
//!   used to treat ANY same-generation resend as a harmless no-op,
//!   regardless of whether the content actually matched. That's
//!   exactly the "silently choose one" behavior spec §57 explicitly
//!   forbids for two genuinely different signed directories at the
//!   same generation. Fixed: same generation + same signature bytes
//!   (Ed25519 signing is deterministic, so identical content always
//!   produces identical signatures) is still a harmless resend;
//!   same generation + different signature now returns
//!   [`error::IdentityError::IdentityForkDetected`], per spec §57's
//!   own words, "do not silently choose one... require
//!   reconciliation/security handling."
//! - §58 "Concurrent Device Changes": no new code — this crate's
//!   existing design (one root key signs the entire directory snapshot
//!   per generation) already IS spec §58's own first listed option,
//!   "single account authority," which is exactly why forks are a
//!   `bug`/`compromise` signal here rather than a routine occurrence
//!   needing its own resolution protocol.
//! - [`state_chain`] — §59 "Account State Chain"
//!   ([`state_chain::AccountStateEvent`]/[`state_chain::StateHash`]/[`state_chain::DeviceEvent`],
//!   spec's own struct shape, bridgeable to a real
//!   [`directory::DeviceDirectory::state_hash`] — honestly scoped: this
//!   crate's live data path is still the signed-snapshot model, and
//!   `siar-event-log` (Part 04) doesn't yet implement hash-chaining
//!   itself either, so this type is a real, usable primitive toward
//!   §59, not a claim the chain exists end-to-end anywhere in this
//!   workspace yet).
//! - [`linking_authority`] — §60 "Device Linking Authority"
//!   ([`linking_authority::LinkingAuthorityPolicy`], verbatim four),
//!   §61 "Default Consumer Policy"
//!   ([`linking_authority::default_consumer_policy`]/[`linking_authority::default_enterprise_policy`]),
//!   §62 "Link Approval Certificate"
//!   ([`linking_authority::device_can_approve_links`], checked against
//!   the real `LINK_NEW_DEVICE` capability bit, not just "is Active" —
//!   tested including a revoked device with the bit still set), §63
//!   "Device Roles" ([`linking_authority::DeviceRole`], verbatim six,
//!   each mapped to a real [`capability::DeviceCapabilitySet`] via
//!   [`linking_authority::DeviceRole::default_capabilities`] — "not UI
//!   labels only" made real), §64 "Security Capabilities" (extended
//!   [`capability::DeviceCapabilitySet`] with its three still-missing
//!   named bits — `ROTATE_ACCOUNT_STATE`/`SYNC_HISTORY`/`RELAY`,
//!   alongside the five already there from an earlier round), §65
//!   "Principle of Least Authority"
//!   ([`linking_authority::headless_relay_minimum_capabilities`],
//!   spec's own worked example — a relay gets `RELAY` and nothing that
//!   could link, revoke, rotate account state, or send messages).
//! - [`destination`] — §66 "Multi-Device File Transfer", §67 "Account
//!   Address vs Device Address" ([`destination::Destination`],
//!   verbatim three-variant enum), §68 "Device Resolution"
//!   ([`destination::resolve_destination`], the real flow — directory
//!   lookup, active+capability-authorized filtering, transport
//!   endpoints attached — so "the application must manually maintain
//!   endpoint lists" never has to be true for a caller of this
//!   function), §69 "Fan-Out Policy"
//!   ([`destination::FanOutPolicy`], verbatim five, plus spec's own
//!   two named defaults for messaging vs. large files), §70
//!   "Own-Device Synchronization Policy"
//!   ([`destination::SyncTarget`]/[`destination::spec_70_example_target`]
//!   — a distinct axis from [`fanout::OwnDeviceSyncPolicy`] §44: that
//!   type gates whether a data class is trusted/synced at all, this
//!   one picks which devices once trust says yes; see this module's
//!   own doc comment for why they're kept separate types).
//! - [`device_state`] — §71 "Device Presence"
//!   ([`device_state::AccountPresence`], per-device map preserved,
//!   never collapsed to derive "account reachable"), §72
//!   "Reachability vs Trust", §73 "Device State Type"
//!   ([`device_state::DeviceState`], verbatim three-field struct —
//!   trust and reachability stay independent fields, never merged
//!   into one boolean), §74 "Device Lifecycle"
//!   ([`device_state::DeviceLifecycle`], verbatim five states — **a
//!   real, documented gap**: this is richer than
//!   [`directory::DeviceStatus`] (3 variants), which is what's
//!   actually embedded in the signed directory every function in this
//!   crate operates on; see that type's own doc comment for why
//!   retrofitting `DeviceStatus` itself wasn't done this round), §75
//!   "Suspension vs Revocation"
//!   ([`device_state::suspend_device`]/[`device_state::reinstate_suspended_device`],
//!   genuinely separate functions from
//!   [`revocation::revoke_device`] — reversible, tested doing so).
//! - [`device_flows`] — §76 "Lost Device Flow"
//!   ([`device_flows::handle_lost_device`], revokes now, names the
//!   three steps this crate can't do itself so a caller can't assume
//!   revocation alone was enough), §77 "Compromised Device Flow"
//!   ([`device_flows::handle_compromised_device`], one step longer
//!   than §76's own list — never claims key erasure, matching §77's
//!   own explicit warning that revocation alone doesn't erase
//!   anything already obtained), §78 "Device Reinstallation", §79
//!   "Device Migration" ([`device_flows::migrate_device`], structurally
//!   cannot copy a private key — its only key parameter is a public
//!   key — and rejects migrating a device to its own id, tested).
//! - [`device_keys`] — §21 "New Device Key Generation": the piece
//!   sitting between [`link_key`]'s ephemeral handshake key and
//!   [`certificate::DeviceCertificate::issue`]'s signature — before
//!   this round, nothing generated the actual permanent keys a
//!   newly-linked device would use going forward.
//!   [`device_keys::NewDeviceKeys`] bundles every key §21 lists (device
//!   signing key, transport key, local database key); only
//!   [`device_keys::NewDeviceKeys::public_keys`]'s output is meant to
//!   leave the device — there's no function anywhere in that module
//!   that returns or serializes a private key, matching §21's own
//!   "private keys remain local" rule structurally, not just by
//!   convention. A dedicated test runs the real §21 → §8 pipeline
//!   end to end: generate keys locally, certify only the public
//!   signing key, verify the resulting certificate.
//! - [`audit_log`] — a real cross-crate integration with
//!   `siar-event-log` (this same session's Part 04 crate), not itself
//!   named by a single spec section: constructs real
//!   `siar_event_log::NewEvent`s for the three identity operations
//!   this crate can already really perform (device linked, device
//!   revoked, revocation verified), closing the "no revocation event
//!   log" half of the gap this crate's own notes used to carry. See
//!   that module's own doc comment for why it only constructs events
//!   rather than appending them itself.
//! - [`contact_verification`] — §91 "Offline Identity Verification"
//!   ([`contact_verification::OfflineVerification::bind_root_identity`],
//!   the one constructor, binding a [`root_key::RootPublicKey`] rather
//!   than any transport/session value — §91's own "prevents
//!   re-verifying every transport change" only holds if that's true
//!   structurally), §92 "Contact Verification"
//!   ([`contact_verification::VerifiedContact`], `verified_root` vs
//!   `current_root` kept as two separate fields so root-rotation
//!   awareness — [`contact_verification::VerifiedContact::is_continuous`] —
//!   is a real comparison, not a claim; trust inheritance
//!   ([`contact_verification::VerifiedContact::new_device_inherits_trust`])
//!   requires both continuity AND `TrustAccountRoot`, never one alone),
//!   §93/§94 "New Device"/"Identity Change Notification"
//!   ([`contact_verification::IdentityNotification`], two variants, not
//!   one generic event with a severity field — §94's own "the
//!   distinction must be explicit"; an identity change is
//!   security-significant under every [`contact_verification::VerificationPolicy`],
//!   a new device only under the two non-default ones), §95
//!   "Verification Modes" ([`contact_verification::VerificationPolicy`],
//!   verbatim three variants, `TrustAccountRoot` default per spec's own
//!   words), §96 "Device Transparency Log"
//!   ([`contact_verification::DeviceTransparencyLog`], a boundary trait
//!   only — spec calls this "a future enhancement" itself, so nothing
//!   here calls `append`, matching [`recovery::RecoveryKeyDerivation`]'s
//!   existing precedent for a real external capability this crate
//!   doesn't implement), §97 "Self-Hosted Transparency"
//!   ([`contact_verification::TransparencyDeploymentMode`], a pure
//!   label — hosting one changes nothing about verification, per §97's
//!   own words), §99 "Directory Service Role"
//!   ([`contact_verification::DirectoryServiceResponse::verify_and_accept`],
//!   the only way to accept one — routes through the exact same
//!   [`directory::DeviceDirectory::verify_signature`] every other path
//!   in this crate already requires, so a directory service gets no
//!   weaker acceptance path than any other untrusted source). §98 "No
//!   Mandatory Central Directory" gets no new code — see that module's
//!   own top-of-file note for why it's already true structurally.
//! - [`state_transport`] — §100 "Device State Through DTN"
//!   ([`state_transport::SignedDeviceStateUpdate`], reusing
//!   [`audit_log::IdentityAuditPayload`] directly as its wire payload
//!   rather than a parallel enum, so a relay that only stores and
//!   forwards opaque bytes can neither read nor forge a device-state
//!   change — verification needs only the account's root public key,
//!   never anything from the transport that carried it).
//! - [`principal_claims`] — §101 "Emergency Identity"
//!   ([`principal_claims::PrincipalType`], a plain label with no
//!   emergency business rules attached to any variant, per §101's own
//!   explicit instruction), §102 "Authority Identity" and §103/§104
//!   "Identity Claims"/"Claim Type"
//!   ([`principal_claims::IdentityClaim`], spec's own struct shape
//!   verbatim; [`principal_claims::IdentityClaim::is_valid`] is §102's
//!   "UI may display Verified Authority only when cryptographic policy
//!   validates it" made into one function — signature, expiry, AND a
//!   caller-supplied trusted-issuer list must all agree, so this crate
//!   never decides issuer trust on an application's behalf).
//! - [`discovery_privacy`] — §105/§106 "Privacy"/"Public vs Private
//!   Device Metadata" ([`discovery_privacy::DeviceMetadata`] vs
//!   [`discovery_privacy::PrivateDeviceMetadata`], two genuinely
//!   different types rather than one struct with fields a caller is
//!   trusted not to read pre-authentication), §107 "Rotating Discovery
//!   Tokens" ([`discovery_privacy::RotatingDiscoveryToken`], opaque
//!   BLAKE3-keyed-hash bytes with no embedded `AccountId`/`DeviceId`
//!   field to accidentally leak), §108 "Device Tracking Resistance"
//!   ([`discovery_privacy::TransportDiscoveryIdentity`], a marker
//!   trait seam for a real transport crate to implement, deliberately
//!   defining zero implementors here). §109 "Address Book Mapping"
//!   gets no type at all — see that module's own top-of-file note for
//!   why the absence, not a boundary type, is the correct fix. §110
//!   "Device Audit Log"/§111 "Audit Event Type" needed no new code
//!   either: [`audit_log`]'s five existing event constructors already
//!   cover §110's exact five named items, and its
//!   [`audit_log::IdentityAuditPayload`] already covers §111.
//! - [`reconciliation`] — §113 "Cross-Device Consistency"
//!   ([`reconciliation::ConvergenceStatus`], a real three-way
//!   comparison rather than a boolean, so "conservative during
//!   divergence" survives as distinct information), §114
//!   "Reconciliation" ([`reconciliation::ReconciliationPlan`], a
//!   decision only — never itself requests, transmits, or applies
//!   anything), §119 "Idempotency"
//!   ([`reconciliation::EventDeduplicator`], keyed on an event's own
//!   BLAKE3 hash), §120 "Replay Protection" (no new mechanism — a
//!   pointer to the four existing checks that already cover it, plus
//!   [`reconciliation::ReplayProtectionIndex`] for tracking
//!   highest-seen-generation before a live directory exists). §112
//!   "Notifications" and §115 "Merkle / Hash Chain Support" needed no
//!   new code — see that module's own top note for why both are
//!   already true.
//! - [`storage`] — §116 "Identity Storage" / §117 "Storage Interface"
//!   ([`storage::IdentityStore`], a trait boundary with no
//!   implementation, matching [`secure_storage::SecureStore`]'s own
//!   precedent exactly, including its `-> impl Future<...> + Send`
//!   shape — a real store is backend-specific and out of scope for
//!   this dependency-minimal crate).
//! - [`transaction`] — §118 "Transaction Boundaries"
//!   ([`transaction::CertificateVerified`] →
//!   [`transaction::EventAppended`] → [`transaction::SnapshotUpdated`]
//!   → [`transaction::Committed`], a type-state machine where
//!   `Committed::into_audit_event` is the only way to get a
//!   `DeviceLinked` audit payload out of this module — "never emit
//!   before durable persistence" is a compile-time property of this
//!   path, not a comment).
//! - [`device_authorization`] — §121 "Device-Specific Authorization"
//!   ([`device_authorization::DeviceAuthorizationDecision::combine`],
//!   a pure combinator where the device's own capability set is a
//!   hard ceiling no user/network policy can override, and the
//!   tightest of any caller-supplied size limits wins).
//! - [`enterprise_policy`] — §122 "Enterprise Device Policy"
//!   ([`enterprise_policy::EnterpriseDevicePolicy`], `Default` requires
//!   nothing at all — "do not make attestation mandatory for the core
//!   protocol" made structural), §123 "Platform Attestation"
//!   ([`enterprise_policy::PlatformAttestation`], no field or method
//!   resembling identity anywhere on it — "must not replace
//!   cryptographic identity" is enforced by absence, not a comment),
//!   §124 "Device Health Claims"
//!   ([`enterprise_policy::DeviceHealthClaims`], every field
//!   self-reported and unverified, matching spec's "claims... not
//!   identity itself").
//! - [`wire_limits`] — §127/§128 "Input Limits"/"Device Count Policy"
//!   ([`wire_limits::InputLimits`], one configurable value rather than
//!   scattered constants, checked against real
//!   [`directory::DeviceDirectory`]/[`principal_claims::IdentityClaim`]
//!   data before any of it is retained). §126 "Serialization" needed
//!   no new code — a real audit (not an assumption) found zero
//!   `usize`/`SystemTime` fields on this crate's wire types. §125
//!   "Version Compatibility" is HONESTLY NOT fully closed: this
//!   crate's two most load-bearing wire types
//!   ([`certificate::DeviceCertificate`], [`directory::DeviceDirectory`])
//!   still carry no explicit schema-version field, and adding one now
//!   would break their already-shipped, already-tested signed-payload
//!   bytes — the same category of gap this crate already names openly
//!   for Part 28's `DeviceLinkInvite`. [`wire_limits::SchemaVersion`]
//!   exists so new wire types follow §125's convention going forward;
//!   the two pre-existing types' gap is named, not fixed, here.
//! - [`session_cache`] — §129 "Session Cache"
//!   ([`session_cache::SessionCacheEntry::is_invalidated_by`], checked
//!   against a live [`directory::DeviceDirectory`] rather than trusting
//!   the session's own fields, since a session invalidated by exactly
//!   one of §129's three named triggers still looks internally
//!   consistent on its own), §130 "Revocation Cache"
//!   ([`session_cache::RevocationCache::from_directory`], the only
//!   constructor — "must be derived from durable authenticated state"
//!   means there is no path to insert a device id into this cache that
//!   didn't come from a real signed directory).
//! - [`client_api`] — §131 "Device Identity API"
//!   ([`client_api::IdentityClient`], [`client_api::DeviceClient`],
//!   [`client_api::TrustClient`], [`client_api::RecoveryClient`],
//!   grounded in this crate's real existing functions rather than
//!   inventing parallel ones — §131's own `LinkPolicy` becomes
//!   [`linking_authority::LinkingAuthorityPolicy`], the type that
//!   already fills that role). §132 "Example API" needed no new type
//!   — its rule ("application should not directly construct signed
//!   device certificates") is exactly why these traits exist. §133
//!   "Revoke API" adds [`client_api::RevocationReason`], with an
//!   honest scope note that it is NOT yet threaded into
//!   [`audit_log::IdentityAuditPayload::DeviceRevoked`] — that's real
//!   future work, not silently assumed done. §134 "Recovery API"
//!   ([`client_api::RecoveryClient::begin_recovery`] returns the
//!   INITIAL [`recovery_state_machine::RecoveryState`], not a finished
//!   result — "guided state machine, not one monolithic call" made
//!   structural).
//! - [`linking_state_machine`] — §135 "State Machine for Linking"
//!   ([`linking_state_machine::LinkingState::advance`], guarded enum
//!   transitions matching [`device_state::DeviceLifecycle::advance`]'s
//!   own precedent exactly; all 8 success states plus all 4 named
//!   failure states, each failure reachable only from the states
//!   where it plausibly occurs, and none reachable once a certificate
//!   is actually issued).
//! - [`recovery_state_machine`] — §136 "State Machine for Recovery"
//!   ([`recovery_state_machine::RecoveryState::advance`], a strictly
//!   linear six-state chain — spec names no failure states here,
//!   unlike §135, so none are invented; rejection already happens one
//!   layer up, before this state machine is ever entered).
//! - [`platform_boundary`] — §137 "UI Boundary"
//!   ([`platform_boundary::DeviceListVm`],
//!   [`platform_boundary::DeviceLinkVm`],
//!   [`platform_boundary::SecurityIdentityVm`],
//!   [`platform_boundary::RecoveryVm`] — none of these types can name
//!   [`root_key::RootIdentityKey`] or any other secret-holding type,
//!   because none of those types are imported into that file at all;
//!   "never receives a private key" is enforced by absence, not a
//!   runtime check). §138 "Android Kotlin Boundary"/§139 "iOS
//!   Boundary" needed no new code — see that module's own top note for
//!   why both are already true of this crate's existing scope.
//! - [`reuse_patterns`] — §140 "Headless Linking" through §144
//!   "Service-to-Service Reuse", proved by real composition tests
//!   using only this crate's public API
//!   (`spec_140_`/`spec_141_`/`spec_143_`/`spec_144_`-prefixed, same
//!   convention as [`destination::spec_70_example_target`]) rather
//!   than new runtime types — these five sections are claims about
//!   existing primitives composing, not requests for new mechanism.
//!   §142's one genuine addition: [`reuse_patterns::MapsToAccount`], a
//!   trait an application's OWN id type implements (never a type this
//!   crate defines) — "the communication SDK should not know what an
//!   employee is" stays true because there is still no `EmployeeId`
//!   type anywhere in this crate.
//! - [`device_privacy_presentation`] — §145 "Privacy-Preserving Device
//!   Names" ([`device_privacy_presentation::remote_device_label`], the
//!   ONLY function that can produce a value carrying a real friendly
//!   name, and only when called with an explicit `true` — the default
//!   path computes [`device_privacy_presentation::GenericDeviceDescriptor`]
//!   from capabilities alone, never from a friendly name at all).
//! - [`client_api`] gained §146 "Device Removal UX Semantics"
//!   ([`client_api::RevocationReason::presentation`] — the identical
//!   [`revocation::revoke_device`] call underneath every reason, with
//!   exactly two distinct presentations on top, not one screen per
//!   reason).
//! - [`local_records`] — §147 "Device History Retention"
//!   ([`local_records::DeviceHistoryRecord`], spec's own four fields
//!   verbatim and no others; [`local_records::DeviceHistoryLog::retain_since`]
//!   makes "without retaining... forever" a real operation, not a
//!   policy statement), §149 "Root Trust Cache"
//!   ([`local_records::RootTrustCacheEntry`], deliberately a SEPARATE
//!   type from [`contact_verification::VerifiedContact`] rather than a
//!   field bolted onto that already-shipped type — the overlap is
//!   named, not hidden; "root changes require explicit policy" reuses
//!   `VerifiedContact::re_anchor`'s exact same explicit-reconstruction
//!   shape).
//! - [`session_cache`] gained §148 "Key Compromise Warnings"
//!   ([`session_cache::RevocationCache::authentication_attempt`],
//!   returning a real [`session_cache::AuthenticationOutcome`] rather
//!   than a bare `bool` so "revoked" can never collapse into the same
//!   value as "unknown device").
//! - [`contact_verification::OfflineVerificationMethod`] gained §150's
//!   remaining two named methods (`OrganizationCertificate`,
//!   `TrustedDirectory`) — "record method for audit" needed no new
//!   code, already true of every type that carries this field.
//! - [`linking_channel`] — §151 "Device Linking Over Existing Secure
//!   Session" ([`linking_channel::LinkingChannel`], recorded alongside
//!   but orthogonal to the bootstrap-proof method). §152/§153
//!   ("Without"/"With Internet") needed no new code — both are already
//!   true of [`certificate::DeviceCertificate::issue`]/
//!   [`directory::DeviceDirectory::sign`] having no network dependency
//!   at all, proved by this module's own tests rather than asserted.
//! - [`recovery`] gained a §154 "Recovery Without Internet"
//!   reconciliation note — every recovery function was already a pure
//!   local computation before §154 was ever read.
//! - [`identity_backup`] — §155 "Backup Relationship"
//!   ([`identity_backup::IdentityBackup`], spec's own three named
//!   contents and nothing else; `recovery_material` is
//!   [`recovery::DerivedRecoveryKey`] — the one thing
//!   [`recovery::RecoverySecret`]'s own doc comment says is allowed to
//!   leave the device — never the secret itself; no session-key or
//!   root-private-key type is even imported into that file).
//! - [`directory::DeviceDirectory::is_device_trusted`] gained §156/§157
//!   reconciliation notes: "this device is authorized to participate"
//!   IS this function's return value, and history/sync policy stops
//!   exactly there, on purpose, with no `HistoryPolicy` type anywhere
//!   in this crate.
//! - [`client_api::RemovalPresentation`] gained a required
//!   `backup_caveat` field for §158 "Device Removal and Backups" — "the
//!   system must not claim otherwise" (that removal erases backups)
//!   enforced by there being no way to construct a presentation without
//!   this field.
//! - [`threat_model`] — §159 "Threat Model", all eleven named threats
//!   mapped to the module/function that actually mitigates each one,
//!   with two of the eleven (`StolenDevice`, `IdentityFork`) backed by
//!   tests that exercise the real mitigating code end-to-end rather
//!   than only asserting a pointer string is non-empty.
//! - [`trust_boundary`] — §160 "Trust Assumptions", spec's own three
//!   trusted foundations and six untrusted inputs, each pointed at the
//!   real code that draws that exact line — same traceability approach
//!   as [`threat_model`].
//! - [`security_invariants`] — §161's ten numbered invariants and
//!   §163's five example properties, each tested directly against real
//!   crate functions; invariants 2 and 3 get genuinely new multi-case
//!   coverage (several generation values in one test, in the spirit of
//!   a property test without adding a property-testing dependency this
//!   crate has never had), while invariants already thoroughly covered
//!   elsewhere (9, 10) are cited rather than duplicated.
//! - [`integration_tests`] — §162's integration-test category and
//!   §165's own named Alice/Bob topology and eight-step scenario, run
//!   for real end-to-end (link, sync, revoke, reconnect-rejected)
//!   rather than only described. §162's "process death during linking"
//!   fault-test case is named as a real, NOT-covered gap rather than
//!   assumed fine — this crate's linking functions are all pure with
//!   no partial-completion state, so the property probably already
//!   holds, but nothing here actually tests a crash-and-resume.
//! - [`wire_limits`] gained a §164 "Fuzz Targets" honest gap note: the
//!   bounding half is real ([`wire_limits::InputLimits`]), but no
//!   actual `cargo-fuzz` harness exists in this workspace yet.
//! - [`state_transport`] gained a §166 "Disaster Test"
//!   (`spec_166_disaster_propagation_survives_three_untrusted_hops`,
//!   round-tripping a signed update through three simulated
//!   store-and-forward hops with none of them touching the signature).
//! - [`directory_cache`] — §167 "Performance Goals" needed no new
//!   type (already true of [`directory_cache::DirectoryCache`] and
//!   [`session_cache::RevocationCache`] alike: validate once, consult
//!   cheaply, never re-walk). §168 "Cache Strategy"
//!   ([`directory_cache::DirectoryCache::from_directory`], the only
//!   constructor, aggregating all five named cache categories from one
//!   directory — "invalidate on signed state update" enforced by there
//!   being no incremental mutator at all). §169 "Device Directory
//!   Size" ([`directory_cache::HandshakeSummary`], spec's exact four
//!   fields, deliberately not the whole directory — paired with
//!   [`reconciliation::ConvergenceStatus::compare`] for the "request
//!   missing state only if needed" half).
//! - [`handshake_integration`] — §170 "Protocol Extension Integration"
//!   ([`handshake_integration::HandshakePhase`], a guarded five-phase
//!   state machine matching spec's own diagram exactly — extension
//!   negotiation is structurally unreachable before identity
//!   verification, not merely documented as coming first), §171
//!   "Capability Negotiation Integration"
//!   ([`handshake_integration::AuthenticatedCapabilityAdvertisement`],
//!   the only constructor pulls capabilities from an actual signed
//!   certificate — there is no path to build one from a bare,
//!   wire-claimed capability set).
//! - [`routing_integration`] — §172 "Routing Integration"
//!   ([`routing_integration::resolve_account_endpoints`], filtered to
//!   `Active` devices only, so a router can never be handed a revoked
//!   device's stale endpoint by accident), §173 "DTN Integration"
//!   ([`routing_integration::dtn_opaque_identifier`], a one-way
//!   BLAKE3 hash matching
//!   [`discovery_privacy::RotatingDiscoveryToken`]'s own derivation
//!   style — a relay holding only this value learns nothing about the
//!   account or device it names).
//! - [`destination`] gained §174/§175 reconciliation notes: "account /
//!   device / selected devices" and "fan out to recipient + sender's
//!   own devices" are exactly its existing `Destination`/`FanOutPolicy`
//!   variants — no new code needed.
//! - [`call_integration`] — §176/§177 "Call Integration"/"Call Ring
//!   Arbitration" ([`call_integration::CallRingState`], spec's four
//!   named states; only `Ringing` can transition anywhere, so a second
//!   `AcceptedBy` after the first is a compile-checked-shape runtime
//!   rejection, not a race a caller has to lock against itself).
//! - [`notification_integration`] — §178 "Notification Integration"
//!   ([`notification_integration::PushToken`]/`PushEndpointRegistry`,
//!   an opaque, unsigned, locally-held mapping with no function
//!   anywhere that lets a push token authenticate or authorize
//!   anything — "push token is not identity" enforced by absence of
//!   capability, not a comment), §179 "Device Endpoint Privacy"
//!   ([`notification_integration::ScopedEndpoint`], `Ephemeral` as the
//!   no-action-needed default, `is_visible_in_public_profile` false
//!   for every scope except `PublicProfile` and only while unexpired).
//! - [`link_rate_limits`] — §180 "Device Link Rate Limits"
//!   ([`link_rate_limits::LinkRateLimiter`], three independently
//!   tracked counters per spec's own three named categories — a slow
//!   legitimate device doesn't share a budget with one presenting
//!   wrong verification codes; "require user confirmation" is
//!   deliberately NOT modeled, since that's a UI action outside this
//!   crate's scope). §181 "Recovery Rate Limits"
//!   ([`link_rate_limits::RecoveryRateLimiter`] for the
//!   infrastructure-side half; the "offline recovery uses
//!   cryptographic proof, not rate limits" half needed no new code —
//!   already [`recovery::add_device_via_recovery`]'s existing design).
//! - [`platform_boundary`] gained §182 "UX States"
//!   ([`platform_boundary::DeviceManagementUiState`], four separate
//!   fields rather than one tagged list, since the categories come
//!   from genuinely different data sources) and §183 "New Device UX"
//!   ([`platform_boundary::NewDeviceUxStep`], its three bootstrap steps
//!   mapped onto real [`linking_state_machine::LinkingState`]
//!   variants via `corresponding_linking_state`, the purely
//!   navigational steps and the final sync step left unmapped since
//!   nothing in `LinkingState` models them).
//! - [`pairing_vs_linking`] — §184 "Contact Pairing vs Device Linking",
//!   a proof module (no new type) confirming
//!   [`contact_verification`]'s and
//!   [`linking_state_machine`]'s/[`platform_boundary::NewDeviceUxStep`]'s
//!   type families share nothing at all, using spec's own Alice/Bob
//!   vs. Alice-Phone/Alice-Laptop example directly.
//! - [`wire_limits`] gained §185 "Device Name Validation"
//!   (`sanitize_device_name`, strips control characters and truncates
//!   to the byte limit on a real char boundary; "non-authoritative"
//!   needed no code — `is_device_trusted` never looks at a name at
//!   all).
//! - [`audit_export`] — §186 "Audit Export"
//!   ([`audit_export::AuditExport`], spec's exact four named contents
//!   and nothing else — no secret-holding type is even imported into
//!   that file).
//! - §187 "API Surface" and §188 "Crate Split" needed no new code:
//!   this crate's real module set already covers spec's suggested
//!   `identity/{account,device,certificate,directory,event,trust,
//!   linking,recovery,claims,audit,error}.rs` sketch several times
//!   over (this file's own module list is the actual answer), and
//!   §188's own recommendation — "keep split only if complexity
//!   justifies it... initially one crate with internal modules may be
//!   sufficient" — is exactly the single-crate, many-internal-modules
//!   structure this crate has used for all ten rounds so far.
//! - [`error_taxonomy`] — §189 "Error Types": this crate's real
//!   [`error::IdentityError`] has a different, smaller shape than
//!   spec's suggested twelve-variant enum because error handling here
//!   is deliberately split across several already-shipped types
//!   ([`error::IdentityError`], [`recovery::RecoveryError`],
//!   [`secure_storage::SecureStoreError`]) plus state-machine variants
//!   and plain booleans for outcomes that aren't really failures
//!   (`DeviceCertificate::is_expired`, `DeviceLinkInvite::is_expired`).
//!   [`error_taxonomy::SuggestedErrorCategory::where_this_lives`] maps
//!   every one of spec's twelve categories to where it actually lives
//!   rather than silently ignoring the mismatch — renaming this
//!   crate's real, already-tested `IdentityError` to match spec's
//!   literal suggestion now would break every existing call site, the
//!   same caution already applied to §125's schema-versioning gap.
//! - §190 "No `anyhow` in Public Domain API" needed no new code — a
//!   real audit (`grep -rn anyhow Cargo.toml src/`) confirms zero
//!   usage; this crate has used `thiserror`-based typed errors from
//!   round 1, and [`error`]'s own top doc comment already cited §190
//!   by name before this final round ever read it.
//! - [`algorithm_agility`] — §192 "Algorithm Agility"
//!   ([`algorithm_agility::AlgorithmId`], deliberately three labels
//!   only, per spec's own "avoid needless generic abstraction" —
//!   nothing in this crate's real signing/verification code uses it;
//!   this crate still calls `ed25519_dalek`/`x25519_dalek`/`blake3`
//!   directly everywhere, unchanged), §193 "Algorithm Downgrade
//!   Protection" ([`algorithm_agility::negotiate_algorithm`], trusting
//!   the caller's own preference ordering rather than inventing a
//!   cross-category strength ranking that wouldn't mean anything
//!   between a signature scheme and a hash function).
//! - [`root_key_backup`] — §194 "Root Key Backup"
//!   ([`root_key_backup::RootKeyBackupEnvelope`], versioned/salted
//!   fields required to exist, no
//!   [`root_key::RootIdentityKey`] type even importable into that
//!   file so plaintext export has no path to occur; the actual
//!   AEAD cipher is the caller's job, dependency-minimal by design),
//!   §195 "Backup Import"
//!   (`root_key_backup::validate_before_import`, spec's own four
//!   checks run in order before any local state is touched).
//! - [`identity_lifecycle`] — §196 "Identity Reset"
//!   (`identity_lifecycle::is_a_real_identity_reset`,
//!   [`identity_lifecycle::IdentityResetPresentation`]'s two required
//!   disclosure fields), §197 "Account Deletion"
//!   ([`identity_lifecycle::AccountDeletionPresentation`], the
//!   remote-deletion caveat required to exist, matching
//!   [`client_api::RemovalPresentation`]'s own established pattern),
//!   §198 "Organization Offboarding"
//!   ([`identity_lifecycle::OffboardingStep`], a guarded four-step
//!   machine operating only on an organization-scoped `DeviceId`,
//!   never an `AccountId` — "personal identity may remain unaffected"
//!   is why that type distinction exists).
//! - [`namespace`] gained §199 "Multi-Tenant Safety"
//!   ([`namespace::TenantScopedAccountId`], "never key solely by
//!   AccountId" made structural — a bare `AccountId` key and a
//!   tenant-scoped one are different Rust types, so using the wrong
//!   one is a type error, not a runtime bug waiting to happen). Also
//!   fixed a genuinely broken intra-doc link in this same file left
//!   over from an earlier round (`[`crate::identifier`]`, a type that
//!   was never defined) — found and corrected as part of this round's
//!   own `cargo doc` verification, dropping this crate's long-standing
//!   doc-warning count from 4 to 3.
//! - [`migration_fixtures`] — §191 "Migration Strategy," honestly
//!   incomplete: no version field exists yet to migrate FROM (see
//!   §125's own gap note), so real cross-version migration tests can't
//!   exist before that's closed. What exists now is the seed —
//!   fixture-based round-trip tests that would fail the moment today's
//!   wire format broke compatibility with itself.
//! - [`definition_of_done`] — §200/§201 needed no new code (every item
//!   in spec's recommended-initial-implementation list and all seven
//!   implementation phases are already built, just not in that exact
//!   literal order). §202 "Definition of Done": an itemized,
//!   twenty-one-item self-audit against spec's own checklist, same
//!   pattern `siar-protocol-ext`'s own §106 self-audit established —
//!   **19 of 21 fully done**, 2 honestly `PartiallyDone`
//!   (`UserConfirmationRequired` — this crate models the prompt but
//!   ships no UI, by design; `FuzzPropertyIntegrationTestsExist` —
//!   property and integration tests exist, real fuzzing doesn't, per
//!   §164's own gap). §203 "Relationship to Other Architecture Parts"
//!   needed no code — a plain list this file's own doc comment already
//!   is the answer to (this crate feeds `siar-routing-policy`,
//!   `siar-event-log`, `siar-blob-manifest`, `siar-dtn-bundle`,
//!   `siar-capability`, and more, exactly as spec's own dependency
//!   list names). §204 "Final Principle"
//!   (`spec_204_account_device_transport_session_identity_are_four_separate_types`,
//!   proving by construction that `AccountId`/`DeviceId`/
//!   `DeviceEndpoint`/`SessionId` have no conversion or shared
//!   supertype between any pair of them — the separation spec's final
//!   principle asks for is a compiler-enforced property here, not a
//!   convention).
//!
//! **Spec 02 (`sys-multi-device-identity-architecture`) is now
//! 204/204 sections reconciled against this crate**, across eleven
//! rounds. "Reconciled" does not mean "nothing left to improve" — the
//! honest gaps named throughout this file (§125's schema versioning,
//! §164's missing fuzz harness, §189's error-taxonomy divergence from
//! spec's literal suggestion, §191's deferred cross-version migration
//! tests, the two `PartiallyDone` items above, and several call sites
//! this crate defines but nothing yet calls — `storage::IdentityStore`,
//! `transaction`'s type-state machine, all four `client_api` traits)
//! are real, current, and worth returning to. What 204/204 means is
//! that every section of the spec has been read, and for each one
//! either real code or a real, specific reason no code was needed
//! exists in this crate today.
//!
//! Every one of the above is covered by tests that exercise the actual
//! cryptographic round trip (real Ed25519/X25519 keys, real signatures,
//! real Diffie-Hellman agreement, real rejection of tampered/forged/
//! stale/mismatched input) — not just type shapes. One test runs the
//! full realistic flow end to end: revoke a real device, accept the
//! result into a real `TrustedAccountStore`, then confirm a stale
//! pre-revocation directory is rejected — §29's own scenario, now
//! exercised against this round's real output instead of a hand-built
//! fixture standing in for one.
//!
//! ## Device-certificate reconciliation (resolved)
//!
//! This workspace previously contained an older device-linking system —
//! `siar_domain::device::{DeviceEvent, DeviceRegistry}` plus
//! `siar_crypto::device_cert::{DeviceCertificate, issue_device_certificate,
//! verify_device_certificate}` — built against an earlier design document
//! ("plan.md §38–42"). That system was device-vouches-for-device without
//! an account root key.
//!
//! That older system has been fully retired in favor of Part 02's
//! root-key-signed model implemented in this crate ([`certificate::DeviceCertificate`]),
//! which is now the sole device-certificate model in the workspace (see
//! `MIGRATION.md`). The capability from the older model that Part 02's
//! literal certificate struct omitted — binding a device's transport key
//! alongside its signing key — is closed additively by
//! [`transport_key_binding::TransportKeyBinding`].
//!
//! ## What's explicitly NOT here
//!
//! - **No wire integration.** Nothing here touches `siar-protocol`,
//!   `siar-messaging`, or any JNI/app call site. This is a standalone
//!   policy layer, same posture `siar-protocol-ext` took toward
//!   `siar-messaging`'s existing traffic.
//! - **No persistent storage.** [`trust_store::TrustedAccountStore`] is
//!   in-memory (a `HashMap`) — real durability (§56: "or stronger state
//!   continuity") would mean a `siar-storage` repository, not attempted
//!   here (and, separately, `siar-storage` itself needs rustc 1.87 —
//!   past what this pass's own sandbox environment could verify a
//!   build against, an additional real reason this wasn't attempted
//!   blind this round).
//! - **Linking flow covers §16-17/§19-20 only.** §18 NFC linking has no
//!   real proximity-transport code (NFC needs platform bindings this
//!   crate doesn't have — the same "no wire integration" posture every
//!   crate in this series takes); [`approval::LinkMethod::Nfc`] exists
//!   as a value an NFC-based flow would report, nothing constructs one.
//!   §21's key *generation* is real now (see [`device_keys`] above) —
//!   what's still not attempted is binding the generated transport
//!   public key into anything a root key signs (see
//!   [`device_keys::NewDeviceKeys::transport_public_key_bytes`]'s own
//!   doc comment for the real reason: `DeviceCertificate` only
//!   certifies one key today). §16's own "replay-resistant"
//!   requirement is only half-real: the nonce contributes real entropy
//!   to the signed payload, but nothing tracks *used* nonces to reject
//!   an actual replay — that needs a persistent store, the same gap
//!   [`trust_store::TrustedAccountStore`] already has.
//! - **Revocation event log now real; offline propagation still not.**
//!   §25-27's revocation *operation* is real (see [`revocation`]
//!   above), and now so is turning it into an audit trail: [`audit_log`]
//!   builds real `siar_event_log::NewEvent`s for device-linked/
//!   device-revoked/revocation-verified via that same workspace's
//!   `siar_event_log::EventStore` rather than a redundant event system
//!   inside this crate (§52's own "signed snapshot over event log"
//!   choice this crate already made — see `directory.rs`'s own doc
//!   comment). [`audit_log`] only *constructs* events, though — no
//!   caller here actually appends them to a live `EventStore`, since
//!   that's the caller's own I/O to own (see that module's own doc
//!   comment for why). §28's actual propagation transport (direct
//!   sync/relay/DTN/linked-device sync) is still fully unattempted —
//!   this crate has no wire integration for any of it, same posture
//!   every crate in this series takes. §40 multi-device fan-out beyond the
//!   directory-filtering already in
//!   [`directory::DeviceDirectory::active_devices`], §33–39
//!   root key rotation/recovery, §41–51 (sender attribution,
//!   organizations, application namespaces), §57 fork detection, §60
//!   onward (linking authority, device roles, presence, secure storage,
//!   recovery, migration, and everything past — the spec runs to 204
//!   sections; this crate stops at a deliberately small, real Phase
//!   1/2 slice per its own §201 "Implementation Phases", now extended
//!   with real slices of both the linking flow and revocation, neither
//!   of which that Phase list separately numbers).
//! - **Parts 01's remaining ~90 sections past what `siar-protocol-ext`
//!   covers, and all of Part 03**, are unstarted by this crate (Part 01
//!   has its own crate and doc comment; Part 03 has no dedicated crate
//!   against its specific spec text at all — see this comment's own
//!   opening paragraph).

pub mod algorithm_agility;
pub mod approval;
pub mod audit_export;
pub mod audit_log;
pub mod call_integration;
pub mod capability;
pub mod certificate;
pub mod client_api;
pub mod contact_verification;
pub mod definition_of_done;
pub mod destination;
pub mod device_authorization;
pub mod device_classes;
pub mod device_flows;
pub mod device_keys;
pub mod device_privacy_presentation;
pub mod device_state;
pub mod directory;
pub mod directory_cache;
pub mod discovery_privacy;
pub mod enterprise_policy;
pub mod error;
pub mod error_taxonomy;
pub mod fanout;
pub mod handshake_integration;
pub mod identity_backup;
pub mod identity_lifecycle;
pub mod integration_tests;
pub mod invite;
pub mod link_key;
pub mod link_rate_limits;
pub mod linking_authority;
pub mod linking_channel;
pub mod linking_state_machine;
pub mod local_records;
pub mod migration_fixtures;
pub mod namespace;
pub mod notification_integration;
pub mod pairing_vs_linking;
pub mod platform_boundary;
pub mod principal_claims;
pub mod reconciliation;
pub mod recovery;
pub mod recovery_state_machine;
pub mod reuse_patterns;
pub mod revocation;
pub mod root_key;
pub mod root_key_backup;
pub mod root_rotation;
pub mod rotation;
pub mod routing_integration;
pub mod safety_fingerprint;
pub mod secure_storage;
pub mod security_invariants;
pub mod session_cache;
pub mod state_chain;
pub mod state_transport;
pub mod storage;
pub mod threat_model;
pub mod transaction;
pub mod transport_key_binding;
pub mod trust_boundary;
pub mod trust_store;
pub mod verification_code;
pub mod wire_limits;

pub use algorithm_agility::{negotiate_algorithm, AlgorithmId};
pub use approval::{LinkMethod, LinkingApprovalPrompt, VerificationStatus};
pub use audit_export::{AuditDeviceRow, AuditExport};
pub use audit_log::{
    decode_audit_payload, device_linked_event, device_revoked_event, device_rotated_event,
    device_suspended_event, fork_detected_event, identity_stream_id, is_audited_status,
    recovery_used_event, revocation_verified_event, root_rotated_event, IdentityAuditPayload,
    EVENT_TYPE_DEVICE_LINKED, EVENT_TYPE_DEVICE_REVOKED, EVENT_TYPE_DEVICE_ROTATED,
    EVENT_TYPE_DEVICE_SUSPENDED, EVENT_TYPE_FORK_DETECTED, EVENT_TYPE_RECOVERY_USED,
    EVENT_TYPE_REVOCATION_VERIFIED, EVENT_TYPE_ROOT_ROTATED,
};
pub use call_integration::{CallRingState, InvalidCallRingTransition};
pub use capability::DeviceCapabilitySet;
pub use certificate::DeviceCertificate;
pub use client_api::{
    DeviceClient, IdentityClient, RecoveryClient, RemovalPresentation, RevocationReason,
    TrustClient,
};
pub use contact_verification::{
    DeviceTransparencyChange, DeviceTransparencyEntry, DeviceTransparencyLog,
    DirectoryServiceResponse, IdentityNotification, OfflineVerification, OfflineVerificationMethod,
    TransparencyDeploymentMode, VerificationPolicy, VerifiedContact,
};
pub use definition_of_done::{DefinitionOfDoneItem, DoneStatus};
pub use destination::{
    large_file_default_fan_out_policy, messaging_default_fan_out_policy, resolve_destination,
    spec_70_example_target, Destination, FanOutPolicy, ResolvedDevice, SyncTarget,
};
pub use device_authorization::DeviceAuthorizationDecision;
pub use device_classes::{
    headless_device_trust_class, spec_45_example_classification, DeviceTrustClass,
    HeadlessDeviceOwner, OrganizationDeviceRole, ServiceIdentityKind,
};
pub use device_flows::{
    handle_compromised_device, handle_lost_device, migrate_device, CompromisedDeviceOutcome,
    CompromisedDeviceStep, LostDeviceOutcome, LostDeviceStep,
};
pub use device_keys::{generate_new_device_keys, NewDeviceKeys, NewDevicePublicKeys};
pub use device_privacy_presentation::{
    remote_device_label, GenericDeviceDescriptor, RemoteDeviceLabel,
};
pub use device_state::DeviceLifecycle;
pub use device_state::{
    reinstate_suspended_device, suspend_device, AccountPresence, DevicePresence,
    DeviceReachability, DeviceState, DeviceTrustState, InvalidLifecycleTransition,
};
pub use directory::{DeviceDirectory, DeviceDirectoryEntry, DeviceEndpoint, DeviceStatus};
pub use directory_cache::{DirectoryCache, HandshakeSummary};
pub use discovery_privacy::{
    DeviceMetadata, DiscoveryTokenEpoch, PrivateDeviceMetadata, RotatingDiscoveryToken,
    TransportDiscoveryIdentity,
};
pub use enterprise_policy::{
    DeviceHealthClaims, EnterpriseDevicePolicy, EnterprisePolicyViolation, Platform,
    PlatformAttestation,
};
pub use error::IdentityError;
pub use error_taxonomy::SuggestedErrorCategory;
pub use fanout::{
    account_level_display, aggregate_delivered_to_account, fan_out_targets, DeviceReceipt,
    DeviceReceiptStatus, OwnDeviceSyncPolicy, PresentationContext, SenderIdentity, SyncDataClass,
};
pub use handshake_integration::{
    AuthenticatedCapabilityAdvertisement, HandshakePhase, InvalidHandshakeTransition,
};
pub use identity_backup::IdentityBackup;
pub use identity_lifecycle::{
    is_a_real_identity_reset, AccountDeletionAction, AccountDeletionPresentation,
    IdentityResetPresentation, InvalidOffboardingTransition, OffboardingStep, OffboardingTarget,
};
pub use invite::DeviceLinkInvite;
pub use link_key::{EphemeralLinkKeyPair, EphemeralLinkPublicKey};
pub use link_rate_limits::{
    LinkRateLimitViolation, LinkRateLimiter, LinkRateLimits, RecoveryRateLimiter,
};
pub use linking_authority::{
    default_consumer_policy, default_enterprise_policy, device_can_approve_links,
    headless_relay_minimum_capabilities, DeviceRole, LinkingAuthorityPolicy,
};
pub use linking_channel::LinkingChannel;
pub use linking_state_machine::{InvalidLinkingTransition, LinkingState};
pub use local_records::{
    certificate_fingerprint, DeviceHistoryLog, DeviceHistoryRecord, RootTrustCacheEntry,
};
pub use namespace::{
    device_membership_is_isolated, is_shared_across_applications_by_default,
    AccountIsolationDomain, ApplicationNamespace, ApplicationScopedResource,
    CrossApplicationIdentityMode, LocalAccountSession,
};
pub use namespace::{TenantId, TenantScopedAccountId};
pub use notification_integration::{
    EndpointScope, PushEndpointRegistry, PushToken, ScopedEndpoint,
};
pub use platform_boundary::{
    DeviceLinkVm, DeviceListVm, DeviceManagementUiState, DeviceRowVm, DeviceSummaryVm,
    NewDeviceUxStep, RecoveryVm, SecurityIdentityVm,
};
pub use principal_claims::{ClaimType, ClaimValue, IdentityClaim, IssuerId, PrincipalType};
pub use reconciliation::{
    state_hash_of, ConvergenceStatus, EventDeduplicator, ReconciliationPlan, ReplayProtectionIndex,
};
pub use recovery::{
    add_device_via_recovery, DerivedRecoveryKey, RecoveryError, RecoveryEvidence,
    RecoveryKeyDerivation, RecoveryPolicy, RecoverySecret,
};
pub use recovery_state_machine::{InvalidRecoveryTransition, RecoveryState};
pub use reuse_patterns::MapsToAccount;
pub use revocation::{revoke_device, verify_revocation, RevocationError};
pub use root_key::{RootIdentityKey, RootPublicKey};
pub use root_key_backup::{validate_before_import, BackupImportRejection, RootKeyBackupEnvelope};
pub use root_rotation::{
    rotate_root_key, verify_root_rotation, CompromisedRootRecoveryStrategy, RootRotation,
    RootRotationError,
};
pub use rotation::{rotate_device_key, RotationError, RotationReason};
pub use routing_integration::{dtn_opaque_identifier, resolve_account_endpoints};
pub use safety_fingerprint::SafetyFingerprint;
pub use secure_storage::{
    verify_device_session_presentation, DevicePrekeyBundle, DeviceSessionPresentation,
    KeyDerivationDomain, LocalDatabaseKey, OneTimePrekey, PrekeyPool, SecretBytes, SecretKeyId,
    SecureStore, SecureStoreError, SessionAuthenticationError, SignedPrekey, StalePeerPolicy,
};
pub use session_cache::{AuthenticationOutcome, RevocationCache, SessionCacheEntry, SessionId};
pub use state_chain::{AccountStateEvent, DeviceEvent, StateHash};
pub use state_transport::SignedDeviceStateUpdate;
pub use storage::IdentityStore;
pub use threat_model::ThreatCategory;
pub use transaction::{CertificateVerified, Committed, EventAppended, SnapshotUpdated};
pub use transport_key_binding::TransportKeyBinding;
pub use trust_boundary::{TrustedInput, UntrustedInput};
pub use trust_store::TrustedAccountStore;
pub use verification_code::derive_verification_code;
pub use wire_limits::{InputLimitViolation, InputLimits, SchemaVersion};

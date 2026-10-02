//! Part 07 — Capability Negotiation Architecture.
//!
//! This crate is being built the same way as the workspace's other
//! Part-01–06 crates (see [[resilient-mesh]] in project memory): one
//! real, deliberately-scoped, tested slice per pass, tracked honestly
//! against the spec's own section numbers rather than a shallow first
//! pass across all 164 sections.
//!
//! ## Round N+2 — §13-15, §40-44, §89-93, §108-111, §136-138
//!
//! - [`advertisement`] — authenticated, session-bound, replay-resistant
//!   envelope (signature check delegated to a verifier trait so this
//!   crate doesn't depend on Part 02's cert types).
//! - [`health`] + [`media_extension`] — half-open breaker for runtime
//!   capability failure; deterministic codec choice from a shared catalog.
//! - [`privacy`] — staged disclosure, profiles, round-down coarsening.
//! - [`device_view`] — multi-device selection/fan-out, deterministic.
//!
//! Defect fix: `BoundedBytes` derived `Deserialize`, so its 256-byte
//! bound did not apply to peer-supplied bytes; it now deserializes via
//! `try_from`.
//!
//! ## Round N+1 — §28-32, §45-55, §112-114
//!
//! - [`security`] — downgrade resistance: shared-catalog algorithm
//!   selection (never peer order), hard minimum floor, ratcheting
//!   remembered baseline with policy-decided `DowngradeSuspected`.
//! - [`epoch`] — epoch-numbered delta updates; stale = ignored, skipped
//!   epoch = request snapshot, atomic apply, bounded size.
//! - [`cache`] — bounded cache; stale absence is `Unknown`, never `No`.
//! - [`guard`] — per-operation requirements with version minimums,
//!   fallback suggestions, fail-closed on unknown operations.
//!
//! Defect fixes: `CapabilityBits::with_bit(64)` silently aliased bit 0
//! (`& 63`), so a capability could be read as a different one — now
//! panics/`None`/`false`; `CapabilitySet` lookups are binary-search.
//!
//! ## Earlier — a second real extension: dtn/1 (§37, §11)
//!
//! - [`dtn_extension`] — §37 "DTN Capabilities" (plus §11's own
//!   `dtn.max_bundle_size = 1 MiB` worked example):
//!   [`dtn_extension::DtnExtensionNegotiator`], covering all seven
//!   `dtn/1` capabilities the same way `files_extension` covers
//!   `files/1`. Deliberately registers no dependency edge between any
//!   of the seven — the spec gives one worked dependency example
//!   total (§69, for `files/1`) and none for `dtn/1`, so none is
//!   invented here (see that module's own doc comment)
//!
//! ## Earlier this pass — extension negotiator + a real files/1 implementation (§33-36)
//!
//! - [`extension`] — §33 "Extension Capability Negotiation", §34
//!   "Extension Negotiator Trait": [`extension::ExtensionNegotiator`],
//!   using [`set::CapabilitySet`] directly for both the advertised and
//!   negotiated shape (see that module's doc comment for why the
//!   spec's `ExtensionCapabilitySet`/`NegotiatedExtensionCapabilities`
//!   aren't separate types here), with a provided `negotiate` method
//!   built on [`mod@negotiate`] so implementors only supply
//!   `advertise()` and a per-extension [`registry::CapabilityRegistry`]
//! - [`files_extension`] — §35 "File Capabilities", §36 "File Limit
//!   Negotiation": [`files_extension::FilesExtensionNegotiator`], a
//!   real (not sketch) implementation covering all seven listed
//!   `files/1` capabilities, wiring in §69's own worked dependency
//!   example (`parallel_chunks` requires `fixed_chunking`) and tested
//!   against §36's own worked limit example (4 MiB vs 1 MiB → 1 MiB
//!   effective) — proof the whole stack built across this crate's
//!   passes (registry, negotiate, policy, extension trait) actually
//!   composes into one real negotiator, not just type-checks in
//!   isolation
//!
//! ## Earlier this pass — two-phase confirmation (§25-26)
//!
//! - [`transcript`] — §25 "Two-Phase Confirmation", §26 "Negotiation
//!   Transcript Hash": [`transcript::HandshakeNonce`] (real
//!   `OsRng`-sourced generation, matching this workspace's existing
//!   nonce convention), [`transcript::NegotiationHash`] (a real
//!   blake3 transcript commitment over both peers' offered sets, the
//!   negotiated selection, and the nonce — computed so it doesn't
//!   matter which peer calls itself "local", the same symmetry
//!   [`mod@negotiate`] already guarantees), and [`transcript::confirm`],
//!   the equality check §25's "both peers confirm the same negotiated
//!   capability set" actually is. Not the full §13
//!   `CapabilityAdvertisement` — see that module's own doc comment
//!   for why.
//!
//! ## Earlier this pass — policy + negotiate() (§18-24, §72-73)
//!
//! Building directly on Phase 1's types:
//!
//! - [`policy`] — §20-23 "Policy Filter" / "Hard Policy" / "User
//!   Policy" / "Application Policy": [`policy::CapabilityPolicy`],
//!   modeling the three per-id disable layers as the shape §21-23's
//!   own examples actually show
//! - [`mod@negotiate`] — §19 "Intersection Rule", §24's Validate →
//!   Intersect → PolicyFilter → RequiredCheck pipeline, §72
//!   "Negotiation Determinism", §73 "Selection Function":
//!   [`negotiate::negotiate`], a pure function (no I/O) combining two
//!   [`set::CapabilitySet`]s via registry validation, per-parameter
//!   intersection (boolean/bitset AND, range overlap, min-of-limits,
//!   equality for opaque bytes), policy filtering that runs *after*
//!   intersection so hard policy genuinely can't be overridden by
//!   mutual peer support (§21), and required-capability enforcement
//!   in both directions — tested for the swapped-input symmetry §72
//!   actually requires, not just one direction
//!
//! ## Earlier pass — Phase 1, Core Types (§161)
//!
//! - [`id`] — §5 "Capability Identifier", §6 "Capability Namespace":
//!   [`id::CapabilityId`], [`id::CapabilityNamespace`], [`id::NamespaceId`]
//! - [`version`] — §9-10 "Capability Versioning": [`version::CapabilityVersion`],
//!   with real major/minor negotiation logic (§19's "effective max =
//!   min(...)" rule specialized to versions)
//! - [`descriptor`] — §7-8 "Capability Descriptor" / "Required vs
//!   Optional", §11-12 "Parameterized Capabilities", §17 "Durable vs
//!   Ephemeral", §76 "Direction": [`descriptor::CapabilityDescriptor`],
//!   [`descriptor::CapabilityRequirement`], [`descriptor::CapabilityParameters`]
//!   (a closed typed enum, never `HashMap<String, String>`, per §12's
//!   explicit instruction), [`descriptor::CapabilityLifetime`],
//!   [`descriptor::CapabilityDirection`], [`descriptor::BoundedBytes`],
//!   [`descriptor::CapabilityBits`] (with a real bitwise-AND
//!   intersection, §19)
//! - [`set`] — §27 "Canonical Encoding", §61 "Capability Advertisement
//!   Size": [`set::CapabilitySet`], a bounded collection that
//!   maintains canonical `(namespace, code, version)` order on every
//!   insert rather than sorting lazily before use
//! - [`hash`] — §26, §27, §94: [`hash::CapabilitySetHash`], a real
//!   blake3-over-canonical-postcard hash, tested for insertion-order
//!   independence (the property §26's "both peers verify equality"
//!   actually depends on)
//! - [`registry`] — §65-69 "Capability Registry" / "Definition" /
//!   "Security Class" / "Mandatory Unknown Critical Capability" /
//!   "Capability Dependency": [`registry::CapabilityRegistry`] with
//!   real validation — §68's unknown-required rejection and §69's
//!   dependency-consistency check, both tested against both the
//!   pass and fail case
//! - [`error`] — §159 "Error Model": [`error::CapabilityNegotiationError`],
//!   reconciled against §103's overlapping sketch (see that module's
//!   doc comment)
//!
//! ## Deliberately not attempted yet
//!
//! `CapabilityAdvertisement` and the authenticated-session binding it
//! needs (§13-15, needs Part 02 wiring), transport/platform/media
//! extension negotiators (§38-41 — only files/1 and dtn/1 are built,
//! the two the spec gives enough worked detail to implement for real
//! rather than guess at), dynamic capability updates and epochs (§45-53),
//! the capability cache (§49-52), and the wire format (§100-102). The
//! full §18 `NegotiatedCapabilities` struct is also not built —
//! [`negotiate::negotiate`]'s module doc explains why and what a
//! caller does instead. None of these are guessed at here.

pub mod advertisement;
pub mod cache;
pub mod descriptor;
pub mod device_view;
pub mod dtn_extension;
pub mod epoch;
pub mod error;
pub mod extension;
pub mod files_extension;
pub mod guard;
pub mod hash;
pub mod health;
pub mod id;
pub mod media_extension;
pub mod negotiate;
pub mod policy;
pub mod privacy;
pub mod registry;
pub mod security;
pub mod set;
pub mod transcript;
pub mod version;

pub use advertisement::{
    signing_digest, verify_advertisement, AdvertisementEnvelope, AdvertisementError,
    AdvertisementVerifier, ReplayGuard,
};
pub use cache::{CapabilityCache, Freshness, PeerKey, StalenessPolicy, Support};
pub use descriptor::{
    BoundedBytes, BoundedBytesError, CapabilityBits, CapabilityDescriptor, CapabilityDirection,
    CapabilityLifetime, CapabilityParameters, CapabilityRequirement, MAX_PARAMETER_BYTES,
};
pub use device_view::{AccountCapabilityView, DeviceKey, Need};
pub use dtn_extension::DtnExtensionNegotiator;
pub use epoch::{
    ApplyOutcome, CapabilityDelta, CapabilityEpoch, CapabilityUpdate, EpochError,
    LocalCapabilityState, PeerCapabilityView, MAX_DELTAS_PER_UPDATE,
};
pub use error::CapabilityNegotiationError;
pub use extension::ExtensionNegotiator;
pub use files_extension::FilesExtensionNegotiator;
pub use guard::{
    FallbackSuggestion, GuardVerdict, OperationGuard, OperationId, OperationRequirement,
};
pub use hash::CapabilitySetHash;
pub use health::{Health, HealthPolicy, HealthTracker};
pub use id::{CapabilityId, CapabilityNamespace, NamespaceId};
pub use media_extension::{select_codec, SelectedCodec};
pub use negotiate::negotiate;
pub use policy::CapabilityPolicy;
pub use privacy::{disclose, DisclosureStage, PrivacyProfile, ResourceClass};
pub use registry::{
    CapabilityDefinition, CapabilityDependency, CapabilityRegistry, ParameterSchema, SecurityClass,
};
pub use security::{
    assess_session, select_algorithm, AlgorithmCatalog, AlgorithmId, BaselinePolicy,
    MinimumSecurityProfile, SecurityBaseline, SecurityGeneration, SecurityVerdict,
};
pub use set::{CapabilitySet, CapabilitySetError, MAX_CAPABILITIES_PER_SET};
pub use transcript::{confirm, HandshakeNonce, NegotiationHash};
pub use version::CapabilityVersion;

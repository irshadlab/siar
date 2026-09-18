#![forbid(unsafe_code)]

//! siar-dtn-bundle: a first slice of "Part 06 — DTN Store-Carry-Forward
//! Architecture" (the sixth of the architecture documents supplied so
//! far). This workspace used to also have a differently-modeled,
//! "next.md"-era DTN crate, `siar-dtn` (`MeshBundle`, sync
//! `BundleStore`) — retired into this crate; see `MIGRATION.md`'s
//! routing/DTN reconciliation section for the disposition (this
//! crate's own [`dedup`] module is the one real gap that was ported
//! over; everything else was superseded by this crate's richer,
//! async, privacy-improved model).
//!
//! ## Scope: §189 "Implementation Phases" 1, plus real slices of 2-4
//!
//! - [`types`] — §5-6, §10-12, §17, §36: `BundleId`, `RouteToken`
//!   (§11's opaque, non-correlatable destination/source identifiers —
//!   a real privacy improvement over `siar-dtn::MeshBundle`'s plain
//!   `DeviceId` destination field), `DtnDestination`, `StorageClass`,
//!   `DtnPriority` with §22's own worked replication-budget numbers.
//! - [`payload`] — §7 `PayloadReference`, genuinely integrated with
//!   this same session's Part 04/05 crates (`siar_event_log::EventId`,
//!   `siar_blob_manifest::BlobId`) rather than parallel placeholder
//!   types, plus real inline-payload size validation.
//! - [`bundle`] — §6 `DtnBundle`, §8 `BundleIntegrity`, §13's
//!   immutability principle (enforced by only exposing hop-local
//!   mutation via `forwarded`/`consume_replication`, mirroring
//!   `siar-dtn::MeshBundle`'s own existing shape for the same two
//!   operations), §20-22 expiry/hop-limit/replication-budget mechanics.
//! - [`state`] — §18 `BundleState` as a real state machine (illegal
//!   transitions rejected, not just an enum), §19's forwarded-vs-
//!   delivered distinction made a real, hard-to-misuse check
//!   (`is_delivered`).
//! - [`store`] — §15-16 `BundleStore` trait (`async-trait`, matching
//!   `siar_event_log::EventStore`'s own reasoning) plus
//!   [`store::InMemoryBundleStore`], a real tested implementation. Adds
//!   `mark_eligible`, a method the spec's own §16 snippet doesn't show
//!   (elided with `...`) but which turned out to be load-bearing —
//!   without it, `Eligible` is unreachable and `list_candidates` can
//!   never return anything, a gap this crate's own test suite
//!   surfaced while exercising the trait for real, not a guess.
//! - [`dedup`] — bounded seen-bundle deduplication (next.md §31's
//!   original "mesh forwarding creates storms" reasoning, ported from
//!   the retired `siar-dtn::dedup::SeenBundles` per `MIGRATION.md`
//!   step 1 — this crate had no equivalent of its own before this
//!   pass).
//! - [`spray`] — §23 Spray-and-Wait, a real (binary-spray) allocation
//!   function — the spec names the strategy but not a concrete
//!   algorithm, so this is this crate's own reasonable choice, flagged
//!   as such.
//! - [`forwarding`] — §23/§25/§26, the piece that was previously
//!   listed below under "not attempted": `ForwardingClass` was a value
//!   a bundle carried; nothing read it. [`forwarding::decide_forwarding`]
//!   does now — §25 direct delivery checked first and always winning
//!   regardless of `forwarding_class` (even preempting `SprayAndWait`),
//!   §26 gateway preference with a documented, spec-silent fallback
//!   choice when no gateway is present, and §23 spray allocation
//!   actually calling [`spray::spray_allocation`] instead of that
//!   function sitting unused. Deliberately no `Epidemic` variant — §24
//!   rejects epidemic routing as the default outright; see
//!   [`forwarding::ForwardingDecision`]'s own doc comment for why that
//!   restriction isn't silently loosened here.
//!
//! Every module is covered by tests exercising real values — actual
//! expiry/hop/replication arithmetic, actual state-machine rejection of
//! illegal transitions, actual store behavior including the
//! eligible-but-expired edge case.
//!
//! ## What's explicitly NOT here
//!
//! - **No encounter protocol.** §189 Phase 3 (HELLO, inventory,
//!   request, transfer, relay ACK) and §27-31 (encounter identity,
//!   inventory summary, Bloom filters, reconciliation) — no peer
//!   connection abstraction exists yet for this to talk through (same
//!   gap `siar-dtn`'s own `lib.rs` doc comment already names for its
//!   own forwarding/custody modules).
//! - **No routing integration for gateway/route *selection*.** §189
//!   Phase 5 — [`routing_bridge::select_dtn_bundle_policy`] now bridges
//!   `siar-routing-policy`'s `DeliveryRequirements` into a concrete
//!   [`types::DtnPriority`]/[`types::ForwardingClass`] pair for
//!   constructing a bundle (the "select DTN bundle policy" step §23
//!   names), but nothing here calls `siar-routing-policy` for an actual
//!   *route plan* or gateway *path* — that still needs the encounter
//!   transport this crate doesn't have (see the point above).
//! - **No file/blob chunk carriage beyond the type-level reference**
//!   in [`payload::PayloadReference::Chunk`] — §189 Phase 6's actual
//!   thumbnail-first/chunk-carriage logic (§40-45) isn't implemented.
//! - **No emergency priority enforcement.** §189 Phase 7 — critical
//!   reserve, priority authorization, broadcast — `DtnPriority::Sos`
//!   exists as a value; nothing enforces a reserved capacity for it.
//! - **No wire/local bundle split.** §14 asks for a `WireBundle`
//!   distinct from a `LocalBundleRecord` (storage path, retry count,
//!   peer history, custody state kept off the wire); [`bundle::DtnBundle`]
//!   currently plays both roles in one struct — a real gap, caught and
//!   corrected in this crate's own doc comments rather than left
//!   implied as done.
//! - **No real durability** — [`store::InMemoryBundleStore`] is a
//!   `HashMap`, same honestly-scoped limitation as every other
//!   in-memory store this session's crates use.
//! - **§32-35 peer utility/destination likelihood/encounter history
//!   (deferred in the spec's own §188 "Initial Production Scope" list:
//!   "social routing, predictive routing"), and everything from
//!   roughly §46 onward** — a genuinely small slice of a 190-section
//!   document.

pub mod bundle;
pub mod dedup;
pub mod forwarding;
pub mod payload;
pub mod routing_bridge;
pub mod spray;
pub mod state;
pub mod store;
pub mod types;

pub use bundle::{BundleIntegrity, DtnBundle};
pub use dedup::SeenBundles;
pub use forwarding::{decide_forwarding, EncounteredPeer, ForwardingDecision};
pub use payload::{InlinePayloadTooLarge, PayloadReference, MAX_INLINE_PAYLOAD_BYTES};
pub use routing_bridge::{select_dtn_bundle_policy, DtnBundlePolicy, DEFAULT_BUNDLE_TTL_MILLIS};
pub use spray::spray_allocation;
pub use state::{BundleEvent, BundleState, InvalidBundleTransition};
pub use store::{BundleStore, DtnStoreError, ForwardQuery, InMemoryBundleStore, StoredBundle};
pub use types::{
    BroadcastScope, BundleId, DtnDestination, DtnPriority, DtnSource, ForwardingClass,
    PayloadTypeId, RouteToken, StorageClass,
};

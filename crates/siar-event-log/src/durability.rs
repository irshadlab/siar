//! `04-offline-event-log-architecture.md` §56 "Durability Classes,"
//! the spec's own three variants verbatim, plus its two worked
//! examples (`DeviceRevoked` → `Critical`, `MessageQueued` →
//! `Durable`) and its one conceptual one (`typing` → not journaled /
//! `BestEffort`).
//!
//! ## What this is, and what it deliberately isn't
//!
//! This is a pure classification vocabulary — `DurabilityClass` alone
//! — living here because it's genuinely domain-agnostic, the same
//! reason `EventOrigin`/`EventTypeId` live in this crate rather than
//! in any one domain's own event module. It is NOT a new field on
//! [`crate::store::NewEvent`], and `EventStore::append`'s actual
//! behavior — fsync policy, write batching, anything the spec's own
//! "must balance safety with mobile battery/performance" line is
//! really about — is completely unchanged by this module. Doing that
//! for real would mean widening `NewEvent`, which means touching every
//! existing constructor across every domain crate that builds one
//! (`siar_messaging::events`, `siar_blob_manifest::events`,
//! `siar_identity_multidevice::audit_log`, `siar_dtn_bundle::events`,
//! `siar_emergency::events` — five call sites this crate can't see
//! from here) — real, but a bigger and riskier change than this round
//! attempts. What this DOES give a real caller: each domain's own
//! event catalog gets a `durability_class(&self) -> DurabilityClass`
//! method (see each domain crate's own `events.rs`/`audit_log.rs` for
//! its classification and the reasoning behind each variant) — a real
//! classification of real, already-shipped events, just not yet wired
//! into how any backend actually persists them.
//!
//! ## The rule this round's per-variant classifications follow
//!
//! The spec gives exactly two concrete examples and one conceptual
//! one — not enough to mechanically derive the other ~40 real event
//! variants across five domains, so this needed an actual rule,
//! stated here once rather than re-litigated per variant:
//!
//! - **`Critical`**: losing this specific event (it never made it to
//!   durable storage, or a backend silently drops it) could let
//!   something UNSAFE happen — a revoked device still trusted, a
//!   forked identity still accepted, a real SOS report never seen.
//!   Direction matters: erring toward MORE caution on loss doesn't
//!   qualify, only erring toward LESS.
//! - **`Durable`**: a normal durable business fact. Losing it leaves
//!   state stale or forces redoing recoverable work (re-verifying a
//!   blob already on disk, re-requesting a redelivery), but nothing
//!   unsafe follows from the direction the staleness points.
//! - **`BestEffort`**: an ephemeral, presence-like signal — the
//!   spec's own `typing` example. Losing it is invisible to anyone;
//!   arguably it shouldn't be journaled at all, which is the spec's
//!   own parenthetical ("not journaled / `BestEffort`").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DurabilityClass {
    Critical,
    Durable,
    BestEffort,
}

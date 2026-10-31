//! # siar-abuse — resource abuse resistance (Part 28 §60-§67, §121)
//!
//! Security is also about *resources*: CPU, RAM, storage, battery, radio,
//! notifications and the pairing UI. This crate is the policy that stops
//! unauthenticated input from spending them.
//!
//! | piece | spec |
//! |---|---|
//! | [`gate`] — cheap-validation-first admission, per-kind quotas, blocklist, pairing cooldown | §60-§62, §65, §66 |
//! | [`inbox`] — bounded message-request inbox | §63 |
//! | [`offers`] — unknown file-offer policy | §64 |
//! | [`authority`] — un-forgeable emergency authority | §67 |
//!
//! Pure and deterministic: no clock, no I/O, no randomness; the caller passes
//! time in. Every table is bounded.
//!
//! ## Honest scope
//!
//! * **Sybil (§66):** identity creation is free and this does not pretend
//!   otherwise. It bounds what *all* strangers together can consume.
//! * **Not yet wired** into `siar-messaging`/`siar-transport` ingress; that is
//!   the next step and needs those call sites (see ROADMAP).
//! * Defaults are reasoned starting points, not tuned on real traffic.
//! * Storage and CPU quotas (§121) are expressed as admission limits here; the
//!   process-wide ceilings remain `siar-resource-limits`'.

#![forbid(unsafe_code)]

pub mod authority;
pub mod bucket;
pub mod gate;
pub mod inbox;
pub mod offers;

pub use authority::{effective, Authority, CriticalGrant, EmergencyAuthority};
pub use gate::{
    Admit, Gate, GateConfig, GateStats, Incoming, PeerKey, QuotaKind, Reject, Standing,
};
pub use inbox::{InboxConfig, InboxError, Request, RequestId, RequestInbox};
pub use offers::{OfferDecision, OfferPolicy, OfferReject};

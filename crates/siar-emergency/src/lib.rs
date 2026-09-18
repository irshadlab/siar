//! Emergency Mode data shapes — next.md §44–52, §64–67, §97–98. Phase 6
//! of `next.md`'s roadmap.
//!
//! - [`kind`]: [`kind::EmergencyMessageKind`] (§45).
//! - [`report`]: [`report::EmergencyReport`], [`report::LocationSharing`]
//!   (§46, §51–52).
//! - [`trust`]: [`trust::AlertTrust`] — the classification a UI shows
//!   (§49–50, §97–98), not the crypto that produces it.
//! - [`mode`]: [`mode::DiscoveryMode`]/[`mode::settings_for`] (§64–66),
//!   [`mode::RelayCapacity`]/[`mode::relay_capacity_for_battery_percent`]
//!   (§67).
//!
//! next.md §44's own list of what Emergency Mode should *do* ("increase
//! discovery frequency... reduce media auto-download... extend
//! critical-message retention") is split across what's actually
//! implemented so far: discovery frequency is [`mode::settings_for`]
//! here; "reduce media auto-download" and "extend critical-message
//! retention" are policy decisions for whatever owns attachment
//! fetching and `siar_dtn_bundle::store::BundleStore`'s retention respectively
//! — this crate defines the mode a caller is in, not every downstream
//! behavior that mode should trigger elsewhere in the workspace.
//!
//! §47's "public disaster channels" (§48) and the actual cryptographic
//! separation between private-SOS and public-alert message classes
//! aren't here either — that's `siar-crypto`/`siar-messaging` work this
//! infra-free crate deliberately doesn't reach into, same boundary
//! `siar_domain::attachment`'s own doc comment already draws for itself
//! ("the actual hashing/encryption lives in `siar-crypto`").

pub mod kind;
pub mod mode;
pub mod report;
pub mod trust;

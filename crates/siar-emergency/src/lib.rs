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
//!
//! `events.rs`/`ids.rs` are a separate matter — not this crate's own
//! next.md numbering above, but
//! `04-offline-event-log-architecture.md`'s §37 "Emergency Events"/§80
//! "Emergency Integration" (that spec's own Phase 3, the fifth and
//! last domain event catalog after identity/messaging/files/DTN — see
//! that module's own doc comment for the full picture, including the
//! new [`ids::ReportId`] it needed and [`report::EmergencyReport`]
//! didn't have).
//!
//! [`report_status`] is newer still than either of those: a real gap
//! found while wiring a real `EventStore::append` caller for this
//! crate's events in the sibling `siar-emergency-service` crate — see
//! that module's own doc comment for why `EmergencyEvent`'s six
//! variants had no decide layer of their own before this.

pub mod events;
pub mod ids;
pub mod kind;
pub mod mode;
pub mod report;
pub mod report_status;
pub mod trust;

pub use events::{
    decode_emergency_event, report_stream_id, EmergencyEvent, EmergencyEventDecodeError,
    CURRENT_EMERGENCY_EVENT_SCHEMA_VERSION, EVENT_TYPE_REPORT_ACKNOWLEDGED,
    EVENT_TYPE_REPORT_CANCELLED, EVENT_TYPE_REPORT_CREATED, EVENT_TYPE_REPORT_EXPIRED,
    EVENT_TYPE_REPORT_RESOLVED, EVENT_TYPE_TRUST_RECLASSIFIED,
};
pub use ids::ReportId;
pub use report_status::{InvalidReportTransition, ReportEvent, ReportStatus};

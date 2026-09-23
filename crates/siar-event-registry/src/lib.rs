//! `04-offline-event-log-architecture.md` §63 "Namespaced Custom
//! Events" — made real, as real as this workspace's dependency
//! direction allows.
//!
//! Every domain event catalog built so far
//! (`siar_identity_multidevice::audit_log`, `siar_messaging::events`,
//! `siar_blob_manifest::events`, `siar_dtn_bundle::events`,
//! `siar_emergency::events`) picks its own block of `EventTypeId`
//! values by an INFORMAL convention repeated in every one of those
//! modules' own doc comments (identity 1-8, messaging 100-108, files
//! 200-208, DTN 300-308, emergency 400-405) — and every one of those
//! doc comments also says the same thing: nothing actually enforces
//! that convention. This crate is that enforcement, finally built,
//! rather than named as a gap a sixth time.
//!
//! ## Why a whole new crate, not a test added to an existing one
//!
//! `siar-event-log` itself cannot do this — it has no dependency on
//! any domain crate, by design (§1's "independent of any one domain,"
//! already the reasoning every domain module's own doc comment gives
//! for why ITS catalog doesn't live in `siar-event-log` either). No
//! existing domain crate can do it either: `siar-messaging` depending
//! on `siar-identity-multidevice` just to compare `EventTypeId`
//! constants would be a real, new coupling between two domains that
//! have no other reason to know about each other, and doesn't even
//! generalize (whoever's checked last still can't see whoever gets
//! added sixth). The only shape that actually works without creating
//! a dependency cycle is a NEW crate that depends on every domain
//! catalog and is depended on by none of them — this one.
//!
//! ## What this ACTUALLY proves, each time it's run
//!
//! [`ALL_REGISTERED_EVENT_TYPES`] lists every `EventTypeId` constant
//! that exists across all five domains AS OF THIS FILE'S OWN LAST
//! EDIT, and [`find_collisions`] proves none of them collide — a real
//! check, not a paper one; it would fail loudly if two domains ever
//! picked the same number. Try tampering with the roster in this
//! crate's own tests to see it fail as intended before trusting it.
//!
//! ## What this does NOT solve — and can't, in this architecture
//!
//! This list is **manually maintained**. Nothing automatically adds a
//! new entry here when a domain crate defines a new `EventTypeId`
//! constant — the only thing stopping a sixth domain, or a new event
//! added to an existing one, from silently colliding is whoever adds
//! it also remembering to add a line here. That is a real, meaningful
//! reduction in risk compared to the status quo before this crate
//! existed (a collision that DOES make it into this list is now
//! caught for certain, by a real running test, not by someone
//! eyeballing five different files' doc comments), but it is not the
//! same guarantee as a registry that can't drift out of sync by
//! construction. Closing that last gap for real would need something
//! like the `inventory` or `linkme` crates (compile-time, cross-crate
//! static registration collected without any one crate needing to
//! list every other one by hand) — genuinely new dependencies and a
//! bigger design commitment than this round's own scope, not attempted
//! here, and worth naming rather than quietly leaving implicit.

use siar_event_log::EventTypeId;

/// One entry: a human-readable label (`"crate::CONSTANT_NAME"`) and
/// the [`EventTypeId`] it resolves to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegisteredEventType {
    pub label: &'static str,
    pub event_type: EventTypeId,
}

/// Every `EventTypeId` constant across all five domain event catalogs
/// this workspace has built — see this module's own doc comment for
/// what "every" actually means (as of this file's own last edit, not
/// automatically).
pub const ALL_REGISTERED_EVENT_TYPES: &[RegisteredEventType] = &[
    // siar-identity-multidevice::audit_log — §35, range 1-8.
    RegisteredEventType {
        label: "siar-identity-multidevice::EVENT_TYPE_DEVICE_LINKED",
        event_type: siar_identity_multidevice::EVENT_TYPE_DEVICE_LINKED,
    },
    RegisteredEventType {
        label: "siar-identity-multidevice::EVENT_TYPE_DEVICE_REVOKED",
        event_type: siar_identity_multidevice::EVENT_TYPE_DEVICE_REVOKED,
    },
    RegisteredEventType {
        label: "siar-identity-multidevice::EVENT_TYPE_REVOCATION_VERIFIED",
        event_type: siar_identity_multidevice::EVENT_TYPE_REVOCATION_VERIFIED,
    },
    RegisteredEventType {
        label: "siar-identity-multidevice::EVENT_TYPE_DEVICE_SUSPENDED",
        event_type: siar_identity_multidevice::EVENT_TYPE_DEVICE_SUSPENDED,
    },
    RegisteredEventType {
        label: "siar-identity-multidevice::EVENT_TYPE_DEVICE_ROTATED",
        event_type: siar_identity_multidevice::EVENT_TYPE_DEVICE_ROTATED,
    },
    RegisteredEventType {
        label: "siar-identity-multidevice::EVENT_TYPE_ROOT_ROTATED",
        event_type: siar_identity_multidevice::EVENT_TYPE_ROOT_ROTATED,
    },
    RegisteredEventType {
        label: "siar-identity-multidevice::EVENT_TYPE_RECOVERY_USED",
        event_type: siar_identity_multidevice::EVENT_TYPE_RECOVERY_USED,
    },
    RegisteredEventType {
        label: "siar-identity-multidevice::EVENT_TYPE_FORK_DETECTED",
        event_type: siar_identity_multidevice::EVENT_TYPE_FORK_DETECTED,
    },
    // siar-messaging::events — §33, range 100-108.
    RegisteredEventType {
        label: "siar-messaging::EVENT_TYPE_MESSAGE_CREATED",
        event_type: siar_messaging::EVENT_TYPE_MESSAGE_CREATED,
    },
    RegisteredEventType {
        label: "siar-messaging::EVENT_TYPE_MESSAGE_QUEUED",
        event_type: siar_messaging::EVENT_TYPE_MESSAGE_QUEUED,
    },
    RegisteredEventType {
        label: "siar-messaging::EVENT_TYPE_MESSAGE_RECEIVED",
        event_type: siar_messaging::EVENT_TYPE_MESSAGE_RECEIVED,
    },
    RegisteredEventType {
        label: "siar-messaging::EVENT_TYPE_MESSAGE_DELIVERED",
        event_type: siar_messaging::EVENT_TYPE_MESSAGE_DELIVERED,
    },
    RegisteredEventType {
        label: "siar-messaging::EVENT_TYPE_MESSAGE_READ",
        event_type: siar_messaging::EVENT_TYPE_MESSAGE_READ,
    },
    RegisteredEventType {
        label: "siar-messaging::EVENT_TYPE_MESSAGE_EDITED",
        event_type: siar_messaging::EVENT_TYPE_MESSAGE_EDITED,
    },
    RegisteredEventType {
        label: "siar-messaging::EVENT_TYPE_MESSAGE_DELETED",
        event_type: siar_messaging::EVENT_TYPE_MESSAGE_DELETED,
    },
    RegisteredEventType {
        label: "siar-messaging::EVENT_TYPE_REACTION_ADDED",
        event_type: siar_messaging::EVENT_TYPE_REACTION_ADDED,
    },
    RegisteredEventType {
        label: "siar-messaging::EVENT_TYPE_REACTION_REMOVED",
        event_type: siar_messaging::EVENT_TYPE_REACTION_REMOVED,
    },
    // siar-blob-manifest::events — §34, range 200-208.
    RegisteredEventType {
        label: "siar-blob-manifest::EVENT_TYPE_TRANSFER_CREATED",
        event_type: siar_blob_manifest::EVENT_TYPE_TRANSFER_CREATED,
    },
    RegisteredEventType {
        label: "siar-blob-manifest::EVENT_TYPE_TRANSFER_ACCEPTED",
        event_type: siar_blob_manifest::EVENT_TYPE_TRANSFER_ACCEPTED,
    },
    RegisteredEventType {
        label: "siar-blob-manifest::EVENT_TYPE_TRANSFER_STARTED",
        event_type: siar_blob_manifest::EVENT_TYPE_TRANSFER_STARTED,
    },
    RegisteredEventType {
        label: "siar-blob-manifest::EVENT_TYPE_TRANSFER_PAUSED",
        event_type: siar_blob_manifest::EVENT_TYPE_TRANSFER_PAUSED,
    },
    RegisteredEventType {
        label: "siar-blob-manifest::EVENT_TYPE_TRANSFER_RESUMED",
        event_type: siar_blob_manifest::EVENT_TYPE_TRANSFER_RESUMED,
    },
    RegisteredEventType {
        label: "siar-blob-manifest::EVENT_TYPE_TRANSFER_COMPLETED",
        event_type: siar_blob_manifest::EVENT_TYPE_TRANSFER_COMPLETED,
    },
    RegisteredEventType {
        label: "siar-blob-manifest::EVENT_TYPE_TRANSFER_CANCELLED",
        event_type: siar_blob_manifest::EVENT_TYPE_TRANSFER_CANCELLED,
    },
    RegisteredEventType {
        label: "siar-blob-manifest::EVENT_TYPE_TRANSFER_FAILED",
        event_type: siar_blob_manifest::EVENT_TYPE_TRANSFER_FAILED,
    },
    RegisteredEventType {
        label: "siar-blob-manifest::EVENT_TYPE_BLOB_VERIFIED",
        event_type: siar_blob_manifest::EVENT_TYPE_BLOB_VERIFIED,
    },
    // siar-dtn-bundle::events — §36, range 300-308.
    RegisteredEventType {
        label: "siar-dtn-bundle::EVENT_TYPE_BUNDLE_CREATED",
        event_type: siar_dtn_bundle::EVENT_TYPE_BUNDLE_CREATED,
    },
    RegisteredEventType {
        label: "siar-dtn-bundle::EVENT_TYPE_BUNDLE_STORED",
        event_type: siar_dtn_bundle::EVENT_TYPE_BUNDLE_STORED,
    },
    RegisteredEventType {
        label: "siar-dtn-bundle::EVENT_TYPE_BUNDLE_FORWARDED",
        event_type: siar_dtn_bundle::EVENT_TYPE_BUNDLE_FORWARDED,
    },
    RegisteredEventType {
        label: "siar-dtn-bundle::EVENT_TYPE_BUNDLE_DESTINATION_REACHED",
        event_type: siar_dtn_bundle::EVENT_TYPE_BUNDLE_DESTINATION_REACHED,
    },
    RegisteredEventType {
        label: "siar-dtn-bundle::EVENT_TYPE_BUNDLE_ACKNOWLEDGED",
        event_type: siar_dtn_bundle::EVENT_TYPE_BUNDLE_ACKNOWLEDGED,
    },
    RegisteredEventType {
        label: "siar-dtn-bundle::EVENT_TYPE_BUNDLE_COMPLETED",
        event_type: siar_dtn_bundle::EVENT_TYPE_BUNDLE_COMPLETED,
    },
    RegisteredEventType {
        label: "siar-dtn-bundle::EVENT_TYPE_BUNDLE_EXPIRED",
        event_type: siar_dtn_bundle::EVENT_TYPE_BUNDLE_EXPIRED,
    },
    RegisteredEventType {
        label: "siar-dtn-bundle::EVENT_TYPE_BUNDLE_EVICTED",
        event_type: siar_dtn_bundle::EVENT_TYPE_BUNDLE_EVICTED,
    },
    RegisteredEventType {
        label: "siar-dtn-bundle::EVENT_TYPE_BUNDLE_CANCELLED",
        event_type: siar_dtn_bundle::EVENT_TYPE_BUNDLE_CANCELLED,
    },
    // siar-emergency::events — §37, range 400-405.
    RegisteredEventType {
        label: "siar-emergency::EVENT_TYPE_REPORT_CREATED",
        event_type: siar_emergency::EVENT_TYPE_REPORT_CREATED,
    },
    RegisteredEventType {
        label: "siar-emergency::EVENT_TYPE_TRUST_RECLASSIFIED",
        event_type: siar_emergency::EVENT_TYPE_TRUST_RECLASSIFIED,
    },
    RegisteredEventType {
        label: "siar-emergency::EVENT_TYPE_REPORT_ACKNOWLEDGED",
        event_type: siar_emergency::EVENT_TYPE_REPORT_ACKNOWLEDGED,
    },
    RegisteredEventType {
        label: "siar-emergency::EVENT_TYPE_REPORT_RESOLVED",
        event_type: siar_emergency::EVENT_TYPE_REPORT_RESOLVED,
    },
    RegisteredEventType {
        label: "siar-emergency::EVENT_TYPE_REPORT_CANCELLED",
        event_type: siar_emergency::EVENT_TYPE_REPORT_CANCELLED,
    },
    RegisteredEventType {
        label: "siar-emergency::EVENT_TYPE_REPORT_EXPIRED",
        event_type: siar_emergency::EVENT_TYPE_REPORT_EXPIRED,
    },
];

/// Two entries whose [`EventTypeId`] values are equal — a real
/// collision, since the whole point of the value is to tell two kinds
/// of event apart.
#[derive(Debug, Clone, Copy)]
pub struct Collision {
    pub a: RegisteredEventType,
    pub b: RegisteredEventType,
}

/// Pure O(n²) pairwise comparison — `entries` is a few dozen items,
/// called from a handful of tests, not a hot path; clarity over
/// asymptotic cleverness.
pub fn find_collisions(entries: &[RegisteredEventType]) -> Vec<Collision> {
    let mut collisions = Vec::new();
    for i in 0..entries.len() {
        for j in (i + 1)..entries.len() {
            if entries[i].event_type == entries[j].event_type {
                collisions.push(Collision {
                    a: entries[i],
                    b: entries[j],
                });
            }
        }
    }
    collisions
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real point of this whole crate — see this module's own top
    /// doc comment. A failure here means two different domains really
    /// did pick the same `EventTypeId`, for real, right now.
    #[test]
    fn no_two_domains_share_an_event_type_id() {
        let collisions = find_collisions(ALL_REGISTERED_EVENT_TYPES);
        assert!(
            collisions.is_empty(),
            "EventTypeId collisions found: {collisions:#?}"
        );
    }

    /// A sanity check on the roster itself, not the domains: if this
    /// count drifts from what's actually in the five `events.rs`/
    /// `audit_log.rs` files, this list has gone stale — see this
    /// module's own top doc comment on why nothing catches that
    /// automatically except a human noticing this assertion fail (or,
    /// as likely, simply not updating it — this test can prove the
    /// list is INTERNALLY consistent, it cannot prove the list is
    /// COMPLETE).
    #[test]
    fn the_roster_has_exactly_the_entries_documented_as_of_this_writing() {
        assert_eq!(ALL_REGISTERED_EVENT_TYPES.len(), 41);
    }

    /// Every label really is unique too — catches an accidental
    /// copy-paste of one entry over another (which would otherwise
    /// silently hide a real missing entry behind a harmless-looking
    /// duplicate rather than a collision `find_collisions` would
    /// flag).
    #[test]
    fn every_label_in_the_roster_is_unique() {
        let mut labels: Vec<&str> = ALL_REGISTERED_EVENT_TYPES.iter().map(|e| e.label).collect();
        let original_len = labels.len();
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(
            labels.len(),
            original_len,
            "a label appears more than once in ALL_REGISTERED_EVENT_TYPES"
        );
    }

    /// `find_collisions` itself, proven on data it did NOT get from
    /// the real roster — without this, "the roster has no collisions"
    /// could just as easily mean "the function is broken and reports
    /// no collisions no matter what."
    #[test]
    fn find_collisions_actually_detects_a_real_collision() {
        let a = RegisteredEventType {
            label: "fake::A",
            event_type: EventTypeId(9999),
        };
        let b = RegisteredEventType {
            label: "fake::B",
            event_type: EventTypeId(9999),
        };
        let c = RegisteredEventType {
            label: "fake::C",
            event_type: EventTypeId(10000),
        };
        let collisions = find_collisions(&[a, b, c]);
        assert_eq!(collisions.len(), 1);
        assert_eq!(collisions[0].a.event_type, EventTypeId(9999));
        assert_eq!(collisions[0].b.event_type, EventTypeId(9999));
    }

    /// Same per-domain block boundaries every `events.rs`/
    /// `audit_log.rs` doc comment already states in prose — checked
    /// here as code instead, so a value assigned outside its own
    /// domain's stated block (even one that happens not to collide
    /// with anything TODAY) gets caught rather than silently drifting.
    #[test]
    fn every_entry_falls_inside_its_own_domain_s_documented_block() {
        for entry in ALL_REGISTERED_EVENT_TYPES {
            let value = entry.event_type.0;
            let expected_block = if entry.label.starts_with("siar-identity-multidevice::") {
                1..=8
            } else if entry.label.starts_with("siar-messaging::") {
                100..=108
            } else if entry.label.starts_with("siar-blob-manifest::") {
                200..=208
            } else if entry.label.starts_with("siar-dtn-bundle::") {
                300..=308
            } else if entry.label.starts_with("siar-emergency::") {
                400..=405
            } else {
                panic!("unrecognized domain prefix in label {:?}", entry.label);
            };
            assert!(
                expected_block.contains(&value),
                "{} = {value} falls outside its own domain's documented block {expected_block:?}",
                entry.label
            );
        }
    }
}

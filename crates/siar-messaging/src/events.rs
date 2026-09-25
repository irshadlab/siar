//! `04-offline-event-log-architecture.md` §33 "Messaging Events",
//! §92 Phase 3 ("messaging, files, identity integration").
//!
//! Identity's own equivalent (`siar_identity_multidevice::audit_log`)
//! already did this for §35 "Identity Events" — this module is the
//! same shape, deliberately: `EventTypeId` constants this crate
//! assigns and owns, a typed payload enum
//! (postcard-serialized — same convention `siar_dtn_bundle::payload`
//! and `audit_log::IdentityAuditPayload` both already use), a
//! `into_new_event` constructor per event, a `decode` counterpart, and
//! — same discipline `audit_log`'s own doc comment states outright —
//! this module only *constructs* [`NewEvent`]s. It never calls
//! `EventStore::append` itself: `MessageService` already owns
//! `siar-storage`'s `Arc<Database>`/repositories, so it's the natural
//! (future) caller of both this module's constructors and `append`,
//! not this module.
//!
//! ## Streams
//!
//! One stream per conversation ([`conversation_stream_id`]) — the
//! natural aggregate boundary for messaging (matches
//! `siar_storage::StoredMessage`'s own `conversation_id` +
//! `sequence`), same reasoning `audit_log::identity_stream_id` already
//! gives for "one stream per account": every caller who knows the
//! conversation id arrives at the same stream without a lookup table.
//! Every payload below also carries `conversation_id` even though it's
//! implied by which stream the event lives in — denormalized on
//! purpose, so a reader looking at one event in isolation (e.g. from
//! [`siar_event_log::store::EventStore::read_log`], which spans every
//! stream) doesn't have to separately resolve which conversation it
//! belongs to.
//!
//! ## §9: explicit schemas, not domain structs
//!
//! None of the payload types below are `siar_storage::StoredMessage`
//! — that type is this workspace's storage *row* shape, not a durable
//! wire schema, and §9 is explicit that a domain struct's shape can
//! change for reasons that have nothing to do with the durable
//! meaning of "a message was created." Each variant here is its own
//! `V1` schema; a `V2` becomes a new variant + a new `EVENT_TYPE_*`
//! constant + an `event_type`/`schema_version` bump, exactly the
//! extension path §9 itself describes — not implemented here since
//! nothing yet needs it.
//!
//! ## Numeric range and an honest open gap
//!
//! `audit_log`'s own constants use tags 1-8. This module deliberately
//! starts at 100, not 1 — §9's own text that messaging/file/identity/
//! DTN/emergency events "share this one numeric namespace" is a real
//! constraint an isolated glance at either module's source won't
//! surface (each was written against a different crate's own local
//! `git diff`, with no shared registry to check against). Picking a
//! block per domain is a stopgap, not a fix: nothing anywhere in this
//! workspace actually enforces non-collision across crates yet — a
//! real gap for whoever adds the next domain's event catalog (files,
//! DTN, emergency) to either respect this informal convention or,
//! better, replace it with an actual shared registry crate/module.
use serde::{Deserialize, Serialize};
use siar_domain::{ConversationId, DeviceId, MessageId};
use siar_event_log::envelope::EventOrigin;
use siar_event_log::ids::{CorrelationId, EventId, EventTypeId, StreamId, Timestamp};
use siar_event_log::store::NewEvent;
use thiserror::Error;

/// One stream per conversation — see this module's own doc comment.
pub fn conversation_stream_id(conversation_id: ConversationId) -> StreamId {
    StreamId::from_name(&format!("conversation:{conversation_id}"))
}

pub const EVENT_TYPE_MESSAGE_CREATED: EventTypeId = EventTypeId(100);
pub const EVENT_TYPE_MESSAGE_QUEUED: EventTypeId = EventTypeId(101);
pub const EVENT_TYPE_MESSAGE_RECEIVED: EventTypeId = EventTypeId(102);
pub const EVENT_TYPE_MESSAGE_DELIVERED: EventTypeId = EventTypeId(103);
pub const EVENT_TYPE_MESSAGE_READ: EventTypeId = EventTypeId(104);
pub const EVENT_TYPE_MESSAGE_EDITED: EventTypeId = EventTypeId(105);
pub const EVENT_TYPE_MESSAGE_DELETED: EventTypeId = EventTypeId(106);
pub const EVENT_TYPE_REACTION_ADDED: EventTypeId = EventTypeId(107);
pub const EVENT_TYPE_REACTION_REMOVED: EventTypeId = EventTypeId(108);

/// §33's nine event names, each its own `V1` payload (see this
/// module's own doc comment on why these are not `StoredMessage`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessagingEvent {
    MessageCreated {
        conversation_id: ConversationId,
        message_id: MessageId,
        sender_device: DeviceId,
        sequence: u64,
        /// Application-E2EE ciphertext — same "storage never sees
        /// plaintext" boundary `siar_storage::StoredMessage::payload`
        /// already documents; this crate's own `siar-crypto`/
        /// `siar-crypto-mls` dependency is what produces it, not this
        /// module.
        ciphertext: Vec<u8>,
    },
    MessageQueued {
        conversation_id: ConversationId,
        message_id: MessageId,
    },
    MessageReceived {
        conversation_id: ConversationId,
        message_id: MessageId,
        sender_device: DeviceId,
    },
    MessageDelivered {
        conversation_id: ConversationId,
        message_id: MessageId,
    },
    MessageRead {
        conversation_id: ConversationId,
        message_id: MessageId,
        reader_device: DeviceId,
    },
    MessageEdited {
        conversation_id: ConversationId,
        message_id: MessageId,
        new_ciphertext: Vec<u8>,
    },
    MessageDeleted {
        conversation_id: ConversationId,
        message_id: MessageId,
    },
    ReactionAdded {
        conversation_id: ConversationId,
        message_id: MessageId,
        reactor_device: DeviceId,
        emoji: String,
    },
    ReactionRemoved {
        conversation_id: ConversationId,
        message_id: MessageId,
        reactor_device: DeviceId,
        emoji: String,
    },
}

impl MessagingEvent {
    /// §56 "Durability Classes" — see `siar_event_log::durability`'s
    /// own doc comment for the rule this follows. `MessageQueued` is
    /// the spec's own worked example (`Durable`); the rest follow the
    /// same rule applied to this crate's own nine variants:
    /// `MessageCreated`/`MessageEdited`/`MessageDeleted` are `Critical`
    /// — this event log is the real sync mechanism between this
    /// device and every other device on the account (see this
    /// module's own top doc comment on streams), so losing one of
    /// these three isn't just local staleness, it's another device
    /// never learning a message existed/changed/was deleted at all.
    /// `MessageRead`/`ReactionAdded`/`ReactionRemoved` are
    /// `BestEffort` — the same "presence-like social signal" class as
    /// the spec's own `typing` example; losing a read receipt or a
    /// reaction is invisible in practice and arguably these shouldn't
    /// be journaled at all, same as `typing` itself isn't.
    pub fn durability_class(&self) -> siar_event_log::DurabilityClass {
        use siar_event_log::DurabilityClass as D;
        match self {
            Self::MessageCreated { .. } => D::Critical,
            Self::MessageQueued { .. } => D::Durable,
            Self::MessageReceived { .. } => D::Durable,
            Self::MessageDelivered { .. } => D::Durable,
            Self::MessageRead { .. } => D::BestEffort,
            Self::MessageEdited { .. } => D::Critical,
            Self::MessageDeleted { .. } => D::Critical,
            Self::ReactionAdded { .. } => D::BestEffort,
            Self::ReactionRemoved { .. } => D::BestEffort,
        }
    }

    pub fn event_type(&self) -> EventTypeId {
        match self {
            Self::MessageCreated { .. } => EVENT_TYPE_MESSAGE_CREATED,
            Self::MessageQueued { .. } => EVENT_TYPE_MESSAGE_QUEUED,
            Self::MessageReceived { .. } => EVENT_TYPE_MESSAGE_RECEIVED,
            Self::MessageDelivered { .. } => EVENT_TYPE_MESSAGE_DELIVERED,
            Self::MessageRead { .. } => EVENT_TYPE_MESSAGE_READ,
            Self::MessageEdited { .. } => EVENT_TYPE_MESSAGE_EDITED,
            Self::MessageDeleted { .. } => EVENT_TYPE_MESSAGE_DELETED,
            Self::ReactionAdded { .. } => EVENT_TYPE_REACTION_ADDED,
            Self::ReactionRemoved { .. } => EVENT_TYPE_REACTION_REMOVED,
        }
    }

    pub fn conversation_id(&self) -> ConversationId {
        match self {
            Self::MessageCreated {
                conversation_id, ..
            }
            | Self::MessageQueued {
                conversation_id, ..
            }
            | Self::MessageReceived {
                conversation_id, ..
            }
            | Self::MessageDelivered {
                conversation_id, ..
            }
            | Self::MessageRead {
                conversation_id, ..
            }
            | Self::MessageEdited {
                conversation_id, ..
            }
            | Self::MessageDeleted {
                conversation_id, ..
            }
            | Self::ReactionAdded {
                conversation_id, ..
            }
            | Self::ReactionRemoved {
                conversation_id, ..
            } => *conversation_id,
        }
    }

    /// The stream this event belongs to — a thin wrapper over
    /// [`conversation_stream_id`] so a caller building an
    /// [`siar_event_log::store::AppendRequest`] doesn't have to
    /// destructure the payload itself just to find the conversation.
    pub fn stream_id(&self) -> StreamId {
        conversation_stream_id(self.conversation_id())
    }

    /// Unlike `audit_log::IdentityAuditPayload::origin` (which can
    /// safely derive `LocalDevice` vs `System` from the payload alone,
    /// since every identity operation it covers is either performed by
    /// exactly one local device or by none), messaging genuinely needs
    /// the caller to say whether this particular occurrence was
    /// produced locally or is being recorded on behalf of a remote
    /// peer (§23) — `MessageReceived` in particular is *definitionally*
    /// about a remote party's message arriving here, but "here" is
    /// itself a local device, so even that isn't safe to hardcode
    /// either way. `origin` is therefore a real parameter on
    /// [`Self::into_new_event`], not inferred.
    ///
    /// §8 "Correlation and Causation": `event_id`, `correlation_id`,
    /// and `causation_id` are all caller-supplied rather than generated
    /// in here, unlike `audit_log::IdentityAuditPayload::into_new_event`
    /// (which generates its own `EventId` and never sets either id,
    /// since a single identity operation has no lifecycle to
    /// correlate). A message's own lifecycle
    /// (`MessageCreated`→`MessageQueued`→...→`MessageDelivered`) spans
    /// several calls to this method over time, so only the caller —
    /// `service.rs`'s own `MessageCorrelation` registry — can know
    /// which `EventId` to mint next and which earlier one caused it;
    /// see that type's own doc comment for the real mechanism.
    pub fn into_new_event(
        self,
        event_id: EventId,
        origin: EventOrigin,
        correlation_id: Option<CorrelationId>,
        causation_id: Option<EventId>,
    ) -> NewEvent {
        let event_type = self.event_type();
        let payload =
            postcard::to_allocvec(&self).expect("MessagingEvent always postcard-serializes");
        NewEvent {
            event_id,
            event_type,
            schema_version: CURRENT_MESSAGING_EVENT_SCHEMA_VERSION,
            created_at: Timestamp::now(),
            origin,
            correlation_id,
            causation_id,
            payload,
        }
    }
}

/// §9 "Versioned Event Schemas": the schema version [`NewEvent`]
/// carries alongside the payload bytes, not a magic number repeated at
/// every call site. Bump this, and add a real `V2` decode branch to
/// [`decode_messaging_event`] below, the day any of this module's nine
/// variants' fields actually change shape — not attempted here since
/// nothing has yet.
pub const CURRENT_MESSAGING_EVENT_SCHEMA_VERSION: u16 = 1;

/// §9's own read side of "never permanently serialize current domain
/// structs" / "do not silently reinterpret old bytes using changed
/// Rust structs": [`decode_messaging_event`] takes the STORED
/// `schema_version` as a real parameter and checks it before trusting
/// the bytes at all, rather than blindly running `postcard::from_bytes`
/// against whatever `MessagingEvent`'s CURRENT Rust shape happens to
/// be — which is exactly the anti-pattern §9 names, and is what this
/// function used to do before this fix (`postcard`'s own enum
/// encoding is positional/discriminant-based, so adding, removing, or
/// reordering a variant silently reinterprets old bytes as the wrong
/// variant instead of failing loudly).
#[derive(Debug, Error)]
pub enum MessagingEventDecodeError {
    #[error("unsupported messaging event schema version {0} (highest known: {CURRENT_MESSAGING_EVENT_SCHEMA_VERSION})")]
    UnsupportedVersion(u16),
    #[error("payload did not decode as a valid messaging event: {0}")]
    Malformed(#[from] postcard::Error),
}

/// The read-side counterpart to [`MessagingEvent::into_new_event`] —
/// same role `audit_log::decode_audit_payload` plays for identity
/// events. `schema_version` should come from the same
/// [`siar_event_log::envelope::EventEnvelope::schema_version`] the
/// payload itself was read alongside — see
/// [`MessagingEventDecodeError`]'s own doc comment for why this isn't
/// just `decode_messaging_event(payload)` anymore.
pub fn decode_messaging_event(
    schema_version: u16,
    payload: &[u8],
) -> Result<MessagingEvent, MessagingEventDecodeError> {
    match schema_version {
        1 => Ok(postcard::from_bytes(payload)?),
        other => Err(MessagingEventDecodeError::UnsupportedVersion(other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids() -> (ConversationId, MessageId, DeviceId) {
        (ConversationId::new(), MessageId::new(), DeviceId::new())
    }

    #[test]
    fn message_queued_is_durable_the_spec_own_worked_example() {
        use siar_event_log::DurabilityClass;
        let (conversation_id, message_id, _) = ids();
        assert_eq!(
            MessagingEvent::MessageQueued {
                conversation_id,
                message_id,
            }
            .durability_class(),
            DurabilityClass::Durable
        );
    }

    #[test]
    fn content_changing_events_are_critical_read_and_reaction_signals_are_not() {
        use siar_event_log::DurabilityClass;
        let (conversation_id, message_id, device) = ids();
        assert_eq!(
            MessagingEvent::MessageCreated {
                conversation_id,
                message_id,
                sender_device: device,
                sequence: 0,
                ciphertext: vec![],
            }
            .durability_class(),
            DurabilityClass::Critical
        );
        assert_eq!(
            MessagingEvent::MessageDeleted {
                conversation_id,
                message_id,
            }
            .durability_class(),
            DurabilityClass::Critical
        );
        assert_eq!(
            MessagingEvent::MessageRead {
                conversation_id,
                message_id,
                reader_device: device,
            }
            .durability_class(),
            DurabilityClass::BestEffort
        );
        assert_eq!(
            MessagingEvent::ReactionAdded {
                conversation_id,
                message_id,
                reactor_device: device,
                emoji: "👍".to_string(),
            }
            .durability_class(),
            DurabilityClass::BestEffort
        );
    }

    #[test]
    fn same_conversation_always_derives_the_same_stream_id() {
        let conversation = ConversationId::new();
        assert_eq!(
            conversation_stream_id(conversation),
            conversation_stream_id(conversation)
        );
    }

    #[test]
    fn different_conversations_derive_different_stream_ids() {
        assert_ne!(
            conversation_stream_id(ConversationId::new()),
            conversation_stream_id(ConversationId::new())
        );
    }

    #[test]
    fn message_created_round_trips_through_postcard_and_carries_its_own_stream() {
        let (conversation_id, message_id, sender_device) = ids();
        let event = MessagingEvent::MessageCreated {
            conversation_id,
            message_id,
            sender_device,
            sequence: 1,
            ciphertext: vec![9, 9, 9],
        };
        assert_eq!(event.stream_id(), conversation_stream_id(conversation_id));

        let new_event = event.clone().into_new_event(
            EventId::new(),
            EventOrigin::LocalDevice(sender_device),
            None,
            None,
        );
        assert_eq!(new_event.event_type, EVENT_TYPE_MESSAGE_CREATED);
        assert_eq!(new_event.origin, EventOrigin::LocalDevice(sender_device));

        let decoded = decode_messaging_event(new_event.schema_version, &new_event.payload).unwrap();
        assert_eq!(decoded, event);
    }

    #[test]
    fn message_received_uses_remote_device_origin_for_the_sender() {
        let (conversation_id, message_id, sender_device) = ids();
        let event = MessagingEvent::MessageReceived {
            conversation_id,
            message_id,
            sender_device,
        };
        let new_event = event.into_new_event(
            EventId::new(),
            EventOrigin::RemoteDevice(sender_device),
            None,
            None,
        );
        assert_eq!(new_event.origin, EventOrigin::RemoteDevice(sender_device));
        assert_eq!(new_event.event_type, EVENT_TYPE_MESSAGE_RECEIVED);
    }

    #[test]
    fn each_event_type_gets_a_distinct_tag_and_does_not_collide_with_identity() {
        let tags = [
            EVENT_TYPE_MESSAGE_CREATED,
            EVENT_TYPE_MESSAGE_QUEUED,
            EVENT_TYPE_MESSAGE_RECEIVED,
            EVENT_TYPE_MESSAGE_DELIVERED,
            EVENT_TYPE_MESSAGE_READ,
            EVENT_TYPE_MESSAGE_EDITED,
            EVENT_TYPE_MESSAGE_DELETED,
            EVENT_TYPE_REACTION_ADDED,
            EVENT_TYPE_REACTION_REMOVED,
        ];
        for (i, a) in tags.iter().enumerate() {
            for b in &tags[i + 1..] {
                assert_ne!(a, b);
            }
            // `siar_identity_multidevice::audit_log`'s own tags are
            // 1-8 — this module's range starts at 100 specifically to
            // stay clear of them (see this module's own doc comment).
            assert!(a.0 >= 100);
        }
    }

    #[test]
    fn every_variant_round_trips_and_reports_its_own_conversation_id() {
        let (conversation_id, message_id, device) = ids();
        let events = vec![
            MessagingEvent::MessageQueued {
                conversation_id,
                message_id,
            },
            MessagingEvent::MessageDelivered {
                conversation_id,
                message_id,
            },
            MessagingEvent::MessageRead {
                conversation_id,
                message_id,
                reader_device: device,
            },
            MessagingEvent::MessageEdited {
                conversation_id,
                message_id,
                new_ciphertext: vec![1, 2],
            },
            MessagingEvent::MessageDeleted {
                conversation_id,
                message_id,
            },
            MessagingEvent::ReactionAdded {
                conversation_id,
                message_id,
                reactor_device: device,
                emoji: "👍".to_string(),
            },
            MessagingEvent::ReactionRemoved {
                conversation_id,
                message_id,
                reactor_device: device,
                emoji: "👍".to_string(),
            },
        ];
        for event in events {
            assert_eq!(event.conversation_id(), conversation_id);
            let new_event = event.clone().into_new_event(
                EventId::new(),
                EventOrigin::LocalDevice(device),
                None,
                None,
            );
            let decoded =
                decode_messaging_event(new_event.schema_version, &new_event.payload).unwrap();
            assert_eq!(decoded, event);
        }
    }

    #[test]
    fn decoding_rejects_an_unsupported_schema_version() {
        let event = MessagingEvent::MessageQueued {
            conversation_id: ConversationId::new(),
            message_id: MessageId::new(),
        };
        let new_event = event.into_new_event(
            EventId::new(),
            EventOrigin::LocalDevice(DeviceId::new()),
            None,
            None,
        );

        // §9's own worked example, made real: the CURRENT
        // `MessagingEvent` shape can decode this payload just fine —
        // this test asserts the version GATE rejects it anyway when
        // told the payload came from a schema version this module
        // doesn't recognize, rather than trusting that the bytes
        // happen to still parse.
        let result = decode_messaging_event(99, &new_event.payload);
        assert!(matches!(
            result,
            Err(MessagingEventDecodeError::UnsupportedVersion(99))
        ));
    }

    #[test]
    fn correlation_and_causation_round_trip_through_a_new_event() {
        let (conversation_id, message_id, device) = ids();
        let created_id = EventId::new();
        let correlation_id = CorrelationId::new();
        let created = MessagingEvent::MessageCreated {
            conversation_id,
            message_id,
            sender_device: device,
            sequence: 1,
            ciphertext: vec![],
        }
        .into_new_event(
            created_id,
            EventOrigin::LocalDevice(device),
            Some(correlation_id),
            None,
        );
        assert_eq!(created.event_id, created_id);
        assert_eq!(created.correlation_id, Some(correlation_id));
        assert_eq!(created.causation_id, None);

        let queued = MessagingEvent::MessageQueued {
            conversation_id,
            message_id,
        }
        .into_new_event(
            EventId::new(),
            EventOrigin::LocalDevice(device),
            Some(correlation_id),
            Some(created_id),
        );
        assert_eq!(queued.correlation_id, Some(correlation_id));
        assert_eq!(queued.causation_id, Some(created_id));
    }
}

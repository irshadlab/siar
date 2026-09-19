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
use siar_event_log::ids::{EventId, EventTypeId, StreamId, Timestamp};
use siar_event_log::store::NewEvent;

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
    pub fn into_new_event(self, origin: EventOrigin) -> NewEvent {
        let event_type = self.event_type();
        let payload =
            postcard::to_allocvec(&self).expect("MessagingEvent always postcard-serializes");
        NewEvent {
            event_id: EventId::new(),
            event_type,
            schema_version: 1,
            created_at: Timestamp::now(),
            origin,
            correlation_id: None,
            causation_id: None,
            payload,
        }
    }
}

/// The read-side counterpart to [`MessagingEvent::into_new_event`] —
/// same role `audit_log::decode_audit_payload` plays for identity
/// events.
pub fn decode_messaging_event(payload: &[u8]) -> Result<MessagingEvent, postcard::Error> {
    postcard::from_bytes(payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids() -> (ConversationId, MessageId, DeviceId) {
        (ConversationId::new(), MessageId::new(), DeviceId::new())
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

        let new_event = event
            .clone()
            .into_new_event(EventOrigin::LocalDevice(sender_device));
        assert_eq!(new_event.event_type, EVENT_TYPE_MESSAGE_CREATED);
        assert_eq!(new_event.origin, EventOrigin::LocalDevice(sender_device));

        let decoded = decode_messaging_event(&new_event.payload).unwrap();
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
        let new_event = event.into_new_event(EventOrigin::RemoteDevice(sender_device));
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
            let new_event = event
                .clone()
                .into_new_event(EventOrigin::LocalDevice(device));
            let decoded = decode_messaging_event(&new_event.payload).unwrap();
            assert_eq!(decoded, event);
        }
    }
}

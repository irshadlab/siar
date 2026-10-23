//! `AppCommand`/`AppEvent` (plan.md §54): the boundary between the UI and
//! the messaging core. UI components only ever construct `AppCommand`s
//! and react to `AppEvent`s — never call `MessageService` methods
//! directly, which is what keeps `siar-messaging`/`siar-transport` out of
//! `apps/desktop`'s component code (plan.md §86).
//!
//! Scoped to what's actually implemented through Phase 3 — plan.md §54's
//! fuller list (`AddContact`, `DownloadAttachment`, `StartCall`, ...)
//! grows this enum as those features land, not before; an unused variant
//! that nothing can act on yet is just dead weight in the match arms.

use crate::group_list::AddMemberInput;
use siar_domain::{
    AccountId, AttachmentReference, ConversationId, MessageContent, MessageId, MessageText,
};

#[derive(Debug, Clone)]
pub enum AppCommand {
    SendMessage {
        conversation: ConversationId,
        text: MessageText,
    },
    OpenConversation(ConversationId),
    MarkRead {
        conversation: ConversationId,
        through_sequence: u64,
    },
    /// plan.md §27 group creation, MLS path (`GroupService::
    /// create_group_mls`) — the only path this UI wires up; the
    /// original static-key path stays CLI/test-only for now, matching
    /// `group_service.rs`'s own note that a caller picks one path per
    /// conversation.
    CreateGroup {
        founder: AccountId,
    },
    /// Mirrors `apps/cli`'s `group-add-member` exactly — see
    /// `AddMemberInput`'s doc comment for why the four raw fields are
    /// pre-parsed before this command is ever constructed.
    AddGroupMember {
        conversation: ConversationId,
        new_member: AccountId,
        input: AddMemberInput,
    },
    SendGroupMessage {
        conversation: ConversationId,
        text: MessageText,
    },
    /// Accepts a `PendingInvite` — `state_input` is read from
    /// `PendingInviteState` by the component dispatching this, not
    /// carried on the command itself, since `GroupService::
    /// join_group_mls` needs the *decoded* `GroupState`, and decoding
    /// happens in `apps/desktop`'s command loop (the one place that's
    /// allowed to call `siar-messaging` directly — see `app.rs`'s
    /// module doc).
    AcceptGroupInvite {
        conversation: ConversationId,
    },
    DeclineGroupInvite {
        conversation: ConversationId,
    },
    /// Persists a contact (see `siar-storage`'s `ContactRepository`) —
    /// dispatched either from the "save as contact" step next to
    /// pairing, or from the add-member form's own save action.
    SaveContact {
        device_id: siar_domain::DeviceId,
        account_id: AccountId,
        display_name: String,
        ticket_text: String,
        key_package_b64: Option<String>,
    },
    RemoveContact {
        device_id: siar_domain::DeviceId,
    },
    /// Dispatched by `MessageTimeline` the first time it renders an
    /// image attachment it hasn't requested yet (see
    /// `AttachmentPreviewState`'s doc comment for why this is on-demand
    /// rather than automatic). Only meaningful for `MessageContent::
    /// Attachment` entries whose `MediaType` is one of the still-image
    /// kinds `siar-media-image` can actually decode — the component
    /// dispatching this is what checks that, so this command doesn't
    /// need its own `MediaType` match to stay a thin data carrier.
    LoadAttachmentPreview {
        message_id: MessageId,
        reference: AttachmentReference,
    },
    /// Pair with a peer from a pasted ticket: opens (or reuses) the
    /// direct conversation with that peer and selects it. The UI has
    /// already run the host's ticket validator, so a failure here is a
    /// genuine runtime failure, reported back as a notice.
    PairWithTicket {
        ticket_text: String,
    },
    /// Record (or clear) that the user compared this contact's safety
    /// number out of band. The core remembers the contact's key at that
    /// moment, so a later key change is detected as "identity changed".
    SetContactVerified {
        device_id: siar_domain::DeviceId,
        verified: bool,
    },
    /// Load the next page of older messages for a 1:1 conversation.
    LoadEarlierMessages {
        conversation: siar_domain::ConversationId,
    },
    /// Start/open the direct conversation with a saved contact.
    UseContact {
        device_id: siar_domain::DeviceId,
    },
}

#[derive(Debug, Clone)]
pub enum AppEvent {
    MessageAdded {
        conversation: ConversationId,
        message_id: MessageId,
        content: MessageContent,
    },
    MessageSent {
        message_id: MessageId,
    },
    MessageDelivered {
        message_id: MessageId,
    },
    NetworkChanged(crate::NetworkState),
    /// Part 28 §42: a background task translates a real security event
    /// (a verified `DeviceRevocation`, a `SecurityEpoch` advance from a
    /// root-key rotation, a new `DeviceCertificate`) into
    /// `crate::SecurityEventKind` before ever constructing this
    /// variant — see `security_event.rs`'s own top doc comment for why
    /// that translation, not a `siar-crypto` dependency here, is what
    /// keeps this crate's boundary rule intact.
    SecurityEventRaised(crate::SecurityEventKind),
}

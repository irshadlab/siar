//! All UI state in one place, behind one lock (see `hub.rs`).
//!
//! Dioxus gave every slice its own `Signal` so unrelated components did
//! not re-render. Slint projects only what changed (see `projection.rs`'s
//! keyed model diff), so a single `Mutex<Slices>` is simpler and cheaper
//! than a lock per slice, with no lock-ordering hazards.
//!
//! The `siar-ui-state` types stay the single source of truth for display
//! logic; this struct only adds the few UI-owned fields (selection,
//! filters, notices) and the conversation bookkeeping the old
//! single-active-peer shell was missing.

use std::collections::{HashMap, HashSet};

use siar_domain::{AccountId, ConversationId, DeliveryState, DeviceId, MessageContent, MessageId};
use siar_ui_state::{
    AttachmentPreviewState, ComposerState, ContactListState, ConversationKind,
    ConversationListState, ConversationSummary, DeviceListState, GroupListState, NetworkStatus,
    OwnIdentityView, PendingInviteState, SecurityDiagnosticDetail, SecurityEventState,
    SecurityStatusBanner, TimelineState, TimelineWindow,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeLevel {
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub level: NoticeLevel,
    pub text: String,
}

/// Which security events the list shows (`EventRow` filter tabs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EventView {
    #[default]
    All,
    Unresolved,
    Critical,
}

/// Where a contact stands on out-of-band verification (ui-ux-15 §58-59).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VerificationState {
    #[default]
    Unverified,
    Verified,
    /// Was verified, but the contact's key is not the one that was
    /// verified. Never silently downgraded to "Unverified": the user must
    /// see that something changed.
    KeyChanged,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContactSafety {
    /// 60-digit safety number, space-separated 5-digit groups.
    pub number: String,
    pub state: VerificationState,
}

/// Where a 1:1 chat's stored history has got to.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HistoryPaging {
    /// The newest page has been loaded from storage (once per session).
    pub loaded: bool,
    /// `before` cursor for the next-older page.
    pub next_before: Option<u64>,
    /// Nothing older exists.
    pub exhausted: bool,
}

/// This device's own identity, filled in once bootstrap finishes.
#[derive(Debug, Default, Clone)]
pub struct LocalIdentity {
    pub account: Option<AccountId>,
    pub device: Option<DeviceId>,
    pub ticket: String,
    pub key_package: String,
}

#[derive(Debug, Default)]
pub struct Slices {
    pub conversations: ConversationListState,
    /// One bounded window per conversation (plan.md §64). The Dioxus shell
    /// shared a single timeline across every conversation.
    pub timelines: HashMap<ConversationId, TimelineState>,
    pub selected: Option<ConversationId>,
    pub history: HashMap<ConversationId, HistoryPaging>,
    pub conversation_filter: String,
    /// Direct conversations that have a reachable peer. Groups need no
    /// entry — membership itself makes them sendable.
    pub sendable: HashSet<ConversationId>,

    pub composer: ComposerState,
    pub contacts: ContactListState,
    pub selected_contact: Option<DeviceId>,
    pub groups: GroupListState,
    pub selected_group: Option<ConversationId>,
    pub pending_invites: PendingInviteState,
    pub attachment_previews: AttachmentPreviewState,
    pub security_events: SecurityEventState,
    /// Local "seen" set for the top banner. Dismissing there can NEVER
    /// resolve an event (ui-ux-15 §98) — it has no access to resolution.
    pub banner: SecurityStatusBanner,
    pub own_identity: Option<OwnIdentityView>,
    pub diagnostics: Option<SecurityDiagnosticDetail>,
    pub contact_safety: HashMap<DeviceId, ContactSafety>,
    pub event_view: EventView,
    pub devices: DeviceListState,
    pub network: NetworkStatus,

    pub identity: LocalIdentity,
    pub ready: bool,
    pub startup_error: Option<String>,
    /// Latest transient notice; the hub copies it to the window once and
    /// clears it, so a repeated identical message still re-shows.
    pub notice: Option<Notice>,
    /// Content-pane request (compact layouts): set by selection changes.
    pub open_content: bool,
}

impl Slices {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn timeline_mut(&mut self, conversation: ConversationId) -> &mut TimelineState {
        self.timelines.entry(conversation).or_default()
    }

    pub fn has_conversation(&self, id: ConversationId) -> bool {
        self.conversations.ordered().iter().any(|c| c.id == id)
    }

    pub fn is_group(&self, id: ConversationId) -> bool {
        self.conversations
            .ordered()
            .iter()
            .any(|c| c.id == id && c.kind == ConversationKind::Group)
    }

    pub fn can_send(&self, id: ConversationId) -> bool {
        self.is_group(id) || self.sendable.contains(&id)
    }

    /// Adds a conversation row only if it is not already there — a plain
    /// `upsert` would reset the unread count of an existing row.
    pub fn ensure_conversation(
        &mut self,
        id: ConversationId,
        display_name: impl Into<String>,
        kind: ConversationKind,
    ) {
        if !self.has_conversation(id) {
            self.conversations.upsert(ConversationSummary {
                id,
                display_name: display_name.into(),
                last_message_preview: None,
                unread_count: 0,
                kind,
            });
        }
    }

    pub fn select(&mut self, id: ConversationId) {
        self.selected = Some(id);
        self.conversations.mark_read(id);
        self.open_content = true;
    }

    /// Merges a page of stored history (oldest → newest) into a chat.
    pub fn apply_history_page(
        &mut self,
        conversation: ConversationId,
        entries: Vec<TimelineWindow>,
        next_before: Option<u64>,
        exhausted: bool,
    ) {
        let first_load = !self.history.get(&conversation).is_some_and(|h| h.loaded);
        self.timeline_mut(conversation).prepend_page(entries);
        self.history.insert(
            conversation,
            HistoryPaging {
                loaded: true,
                next_before,
                exhausted,
            },
        );
        if first_load {
            // A restart must not leave the chat list showing "No messages yet".
            let latest = self
                .timelines
                .get(&conversation)
                .and_then(|t| t.visible().last())
                .and_then(|e| match &e.content {
                    MessageContent::Text(t) => Some(if e.from_me {
                        format!("You: {}", t.as_str())
                    } else {
                        t.as_str().to_string()
                    }),
                    _ => None,
                });
            if let Some(preview) = latest {
                self.conversations.set_preview(conversation, preview);
            }
        }
    }

    pub fn notify(&mut self, level: NoticeLevel, text: impl Into<String>) {
        self.notice = Some(Notice {
            level,
            text: text.into(),
        });
    }

    /// A message this device just sent.
    pub fn record_outgoing(
        &mut self,
        conversation: ConversationId,
        message_id: MessageId,
        content: MessageContent,
    ) {
        if let MessageContent::Text(text) = &content {
            self.conversations
                .set_preview(conversation, format!("You: {}", text.as_str()));
        }
        // Storage files this message under (roughly) the same clock reading;
        // it is only used as a paging cursor, never for ordering display.
        let sequence = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        self.timeline_mut(conversation).push_latest(TimelineWindow {
            message_id,
            sequence,
            content,
            delivery_state: DeliveryState::Local,
            from_me: true,
        });
        self.conversations.mark_read(conversation);
    }

    /// A message from a peer. Unread only counts when the conversation is
    /// not the one on screen.
    pub fn record_incoming(
        &mut self,
        conversation: ConversationId,
        message_id: MessageId,
        sequence: u64,
        content: MessageContent,
    ) {
        if let MessageContent::Text(text) = &content {
            self.conversations
                .set_preview(conversation, text.as_str().to_string());
        }
        if self.selected == Some(conversation) {
            self.conversations.mark_read(conversation);
        } else {
            self.conversations.increment_unread(conversation);
        }
        self.timeline_mut(conversation).push_latest(TimelineWindow {
            message_id,
            sequence,
            content,
            delivery_state: DeliveryState::Delivered,
            from_me: false,
        });
    }

    /// Finds the attachment reference of a message in any loaded timeline.
    pub fn find_attachment(
        &self,
        message_id: MessageId,
    ) -> Option<siar_domain::AttachmentReference> {
        self.timelines.values().find_map(|t| {
            t.visible().find_map(|e| match &e.content {
                MessageContent::Attachment(r) if e.message_id == message_id => Some(r.clone()),
                _ => None,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use siar_domain::MessageText;

    fn text(s: &str) -> MessageContent {
        MessageContent::Text(MessageText::parse(s.to_string()).unwrap())
    }

    fn unread(s: &Slices, id: ConversationId) -> u32 {
        s.conversations
            .ordered()
            .iter()
            .find(|c| c.id == id)
            .unwrap()
            .unread_count
    }

    #[test]
    fn incoming_in_a_background_conversation_counts_unread_and_selected_does_not() {
        let mut s = Slices::new();
        let (a, b) = (ConversationId::new(), ConversationId::new());
        s.ensure_conversation(a, "A", ConversationKind::Direct);
        s.ensure_conversation(b, "B", ConversationKind::Direct);
        s.select(a);

        s.record_incoming(a, MessageId::new(), 1, text("on screen"));
        s.record_incoming(b, MessageId::new(), 1, text("elsewhere"));
        s.record_incoming(b, MessageId::new(), 2, text("again"));

        assert_eq!(unread(&s, a), 0);
        assert_eq!(unread(&s, b), 2);

        s.select(b);
        assert_eq!(unread(&s, b), 0);
    }

    #[test]
    fn ensure_conversation_never_resets_unread() {
        let mut s = Slices::new();
        let a = ConversationId::new();
        s.ensure_conversation(a, "A", ConversationKind::Direct);
        s.record_incoming(a, MessageId::new(), 1, text("hi"));
        s.ensure_conversation(a, "A renamed", ConversationKind::Direct);
        assert_eq!(unread(&s, a), 1);
    }

    #[test]
    fn timelines_are_per_conversation() {
        let mut s = Slices::new();
        let (a, b) = (ConversationId::new(), ConversationId::new());
        s.record_outgoing(a, MessageId::new(), text("to a"));
        assert_eq!(s.timelines[&a].visible().count(), 1);
        assert!(!s.timelines.contains_key(&b));
    }

    #[test]
    fn groups_are_sendable_but_direct_chats_need_a_peer() {
        let mut s = Slices::new();
        let (g, d) = (ConversationId::new(), ConversationId::new());
        s.ensure_conversation(g, "G", ConversationKind::Group);
        s.ensure_conversation(d, "D", ConversationKind::Direct);
        assert!(s.can_send(g));
        assert!(!s.can_send(d));
        s.sendable.insert(d);
        assert!(s.can_send(d));
    }

    #[test]
    fn history_pages_merge_without_duplicating_live_messages_and_set_the_preview() {
        let mut s = Slices::new();
        let a = ConversationId::new();
        s.ensure_conversation(a, "A", ConversationKind::Direct);
        let live = MessageId::new();
        s.record_incoming(a, live, 30, text("live"));
        let stored = |id: MessageId, seq: u64, body: &str| TimelineWindow {
            message_id: id,
            sequence: seq,
            content: text(body),
            delivery_state: DeliveryState::Delivered,
            from_me: false,
        };
        s.apply_history_page(
            a,
            vec![
                stored(MessageId::new(), 10, "old"),
                stored(live, 30, "live"),
            ],
            Some(10),
            false,
        );
        let bodies: Vec<_> = s.timelines[&a]
            .visible()
            .map(|e| match &e.content {
                MessageContent::Text(t) => t.as_str().to_string(),
                _ => String::new(),
            })
            .collect();
        assert_eq!(bodies, ["old", "live"]);
        assert!(s.history[&a].loaded && !s.history[&a].exhausted);
        assert_eq!(s.history[&a].next_before, Some(10));
    }

    #[test]
    fn outgoing_updates_preview_and_clears_unread() {
        let mut s = Slices::new();
        let a = ConversationId::new();
        s.ensure_conversation(a, "A", ConversationKind::Direct);
        s.record_incoming(a, MessageId::new(), 1, text("hi"));
        s.record_outgoing(a, MessageId::new(), text("hello"));
        let row = s
            .conversations
            .ordered()
            .iter()
            .find(|c| c.id == a)
            .unwrap();
        assert_eq!(row.unread_count, 0);
        assert_eq!(row.last_message_preview.as_deref(), Some("You: hello"));
    }
}

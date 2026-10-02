//! `ConversationListState` (plan.md §53): kept as its own state slice
//! rather than folded into one giant `AppState`, so a Dioxus component
//! that only renders the sidebar doesn't re-render on every keystroke in
//! the composer (plan.md §94's granular-component rule starts here, at
//! the state layer, not just in the component tree).

use siar_domain::ConversationId;

/// Real gap this closes (flagged in the group-UI work item, not
/// invented speculatively): before this, every `ConversationSummary`
/// the desktop app produced was implicitly 1:1 — there was no field a
/// component could read to tell a group conversation apart from a
/// direct one in the sidebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConversationKind {
    Direct,
    Group,
}

#[derive(Debug, Clone)]
pub struct ConversationSummary {
    pub id: ConversationId,
    pub display_name: String,
    pub last_message_preview: Option<String>,
    pub unread_count: u32,
    pub kind: ConversationKind,
}

#[derive(Debug, Default)]
pub struct ConversationListState {
    conversations: Vec<ConversationSummary>,
}

impl ConversationListState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn upsert(&mut self, summary: ConversationSummary) {
        if let Some(existing) = self.conversations.iter_mut().find(|c| c.id == summary.id) {
            *existing = summary;
        } else {
            self.conversations.push(summary);
        }
    }

    /// plan.md §45: a read receipt clears unread count for that
    /// conversation, nothing more granular than that at the list level.
    pub fn mark_read(&mut self, conversation: ConversationId) {
        if let Some(c) = self.conversations.iter_mut().find(|c| c.id == conversation) {
            c.unread_count = 0;
        }
    }

    pub fn increment_unread(&mut self, conversation: ConversationId) {
        if let Some(c) = self.conversations.iter_mut().find(|c| c.id == conversation) {
            c.unread_count = c.unread_count.saturating_add(1);
        }
    }

    /// Updates just the preview text for an existing entry — used when
    /// a new message arrives for a conversation whose `ConversationSummary`
    /// already exists (e.g. a group message, where the group itself was
    /// already upserted at create/join time). A no-op if the
    /// conversation isn't known yet, same "nothing to update" shape
    /// `mark_read`/`increment_unread` already have.
    pub fn set_preview(&mut self, conversation: ConversationId, preview: String) {
        if let Some(c) = self.conversations.iter_mut().find(|c| c.id == conversation) {
            c.last_message_preview = Some(preview);
        }
    }

    /// Newest-activity-first — plan.md §53 doesn't mandate an order, but
    /// this is what every chat client's sidebar does, and there's no
    /// reason to diverge from that expectation.
    pub fn ordered(&self) -> &[ConversationSummary] {
        &self.conversations
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(id: ConversationId, name: &str) -> ConversationSummary {
        ConversationSummary {
            id,
            display_name: name.to_string(),
            last_message_preview: None,
            unread_count: 0,
            kind: ConversationKind::Direct,
        }
    }

    #[test]
    fn kind_distinguishes_direct_from_group_entries() {
        let mut state = ConversationListState::new();
        let direct_id = ConversationId::new();
        let group_id = ConversationId::new();
        state.upsert(summary(direct_id, "Bob"));
        state.upsert(ConversationSummary {
            kind: ConversationKind::Group,
            ..summary(group_id, "Team")
        });

        assert_eq!(
            state
                .ordered()
                .iter()
                .find(|c| c.id == direct_id)
                .unwrap()
                .kind,
            ConversationKind::Direct
        );
        assert_eq!(
            state
                .ordered()
                .iter()
                .find(|c| c.id == group_id)
                .unwrap()
                .kind,
            ConversationKind::Group
        );
    }

    #[test]
    fn upsert_inserts_then_updates_in_place() {
        let mut state = ConversationListState::new();
        let id = ConversationId::new();
        state.upsert(summary(id, "Bob"));
        assert_eq!(state.ordered().len(), 1);

        state.upsert(ConversationSummary {
            last_message_preview: Some("hey".to_string()),
            ..summary(id, "Bob")
        });
        assert_eq!(state.ordered().len(), 1);
        assert_eq!(
            state.ordered()[0].last_message_preview.as_deref(),
            Some("hey")
        );
    }

    #[test]
    fn unread_count_increments_and_clears() {
        let mut state = ConversationListState::new();
        let id = ConversationId::new();
        state.upsert(summary(id, "Bob"));

        state.increment_unread(id);
        state.increment_unread(id);
        assert_eq!(state.ordered()[0].unread_count, 2);

        state.mark_read(id);
        assert_eq!(state.ordered()[0].unread_count, 0);
    }

    #[test]
    fn operations_on_unknown_conversation_are_no_ops() {
        let mut state = ConversationListState::new();
        let unknown = ConversationId::new();
        state.mark_read(unknown); // must not panic
        state.increment_unread(unknown);
        assert!(state.ordered().is_empty());
    }

    #[test]
    fn set_preview_updates_only_the_matching_conversation() {
        let mut state = ConversationListState::new();
        let a = ConversationId::new();
        let b = ConversationId::new();
        state.upsert(summary(a, "A"));
        state.upsert(summary(b, "B"));

        state.set_preview(a, "hello".to_string());
        assert_eq!(
            state
                .ordered()
                .iter()
                .find(|c| c.id == a)
                .unwrap()
                .last_message_preview,
            Some("hello".to_string())
        );
        assert_eq!(
            state
                .ordered()
                .iter()
                .find(|c| c.id == b)
                .unwrap()
                .last_message_preview,
            None
        );
    }

    #[test]
    fn set_preview_on_unknown_conversation_is_a_no_op() {
        let mut state = ConversationListState::new();
        state.set_preview(ConversationId::new(), "hello".to_string());
        assert!(state.ordered().is_empty());
    }
}

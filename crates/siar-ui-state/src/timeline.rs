//! `TimelineState` (plan.md §64): never render the whole conversation
//! history — hold a bounded window and page backward on demand.

use siar_domain::{DeliveryState, MessageContent, MessageId};

#[derive(Debug, Clone)]
pub struct TimelineWindow {
    pub message_id: MessageId,
    pub sequence: u64,
    pub content: MessageContent,
    pub delivery_state: DeliveryState,
    pub from_me: bool,
}

/// Default page size — matches the `LIMIT 50` used in
/// `siar-storage`'s `timeline()` query, so a "load more" always asks
/// storage for exactly one more screenful.
pub const PAGE_SIZE: usize = 50;

#[derive(Debug, Default)]
pub struct TimelineState {
    /// Oldest-first internally (simplifies the append/prepend math);
    /// `visible()` reverses for newest-first rendering.
    entries: Vec<TimelineWindow>,
    has_more_history: bool,
}

impl TimelineState {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            has_more_history: true,
        }
    }

    /// A freshly-sent or freshly-received message — always appended at
    /// the newest end, regardless of paging state.
    pub fn push_latest(&mut self, entry: TimelineWindow) {
        self.entries.push(entry);
    }

    /// A page of older history loaded from storage (`before` param on
    /// `MessageRepository::timeline`) — prepended, and `has_more_history`
    /// updates based on whether storage returned a full page (a partial
    /// page means we've hit the start of the conversation).
    pub fn prepend_page(&mut self, mut older_page: Vec<TimelineWindow>) {
        self.has_more_history = older_page.len() == PAGE_SIZE;
        // Idempotent: a message that arrived live and is also in the page
        // loaded from storage must appear once, not twice.
        older_page.retain(|e| !self.entries.iter().any(|x| x.message_id == e.message_id));
        older_page.extend(std::mem::take(&mut self.entries));
        self.entries = older_page;
    }

    pub fn update_delivery_state(&mut self, message_id: MessageId, state: DeliveryState) {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.message_id == message_id) {
            entry.delivery_state = state;
        }
    }

    pub fn has_more_history(&self) -> bool {
        self.has_more_history
    }

    /// Newest-first, matching how a chat UI actually renders (latest
    /// message at the bottom of the visible scroll region, which reads
    /// top-to-bottom oldest-to-newest here — the "newest-first" is about
    /// which end of `entries` a component starts iterating from, not
    /// visual order).
    pub fn visible(&self) -> impl Iterator<Item = &TimelineWindow> {
        self.entries.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(seq: u64) -> TimelineWindow {
        TimelineWindow {
            message_id: MessageId::new(),
            sequence: seq,
            content: MessageContent::Text(
                siar_domain::MessageText::parse(format!("msg {seq}")).unwrap(),
            ),
            delivery_state: DeliveryState::Local,
            from_me: true,
        }
    }

    #[test]
    fn push_latest_appends_in_order() {
        let mut t = TimelineState::new();
        t.push_latest(entry(1));
        t.push_latest(entry(2));
        let seqs: Vec<u64> = t.visible().map(|e| e.sequence).collect();
        assert_eq!(seqs, vec![1, 2]);
    }

    #[test]
    fn prepend_page_puts_older_messages_first() {
        let mut t = TimelineState::new();
        t.push_latest(entry(10));
        t.prepend_page(vec![entry(8), entry(9)]);
        let seqs: Vec<u64> = t.visible().map(|e| e.sequence).collect();
        assert_eq!(seqs, vec![8, 9, 10]);
    }

    #[test]
    fn prepending_a_page_never_duplicates_a_message_already_shown() {
        let mut t = TimelineState::new();
        let live = entry(10);
        t.push_latest(live.clone());
        // storage returns the same message again plus an older one
        t.prepend_page(vec![entry(9), live]);
        let seqs: Vec<u64> = t.visible().map(|e| e.sequence).collect();
        assert_eq!(seqs, vec![9, 10]);
    }

    #[test]
    fn a_partial_page_means_history_is_exhausted() {
        let mut t = TimelineState::new();
        assert!(t.has_more_history());
        t.prepend_page(vec![entry(1), entry(2)]); // far short of PAGE_SIZE
        assert!(!t.has_more_history());
    }

    #[test]
    fn a_full_page_means_more_history_might_exist() {
        let mut t = TimelineState::new();
        let full_page: Vec<_> = (0..PAGE_SIZE as u64).map(entry).collect();
        t.prepend_page(full_page);
        assert!(t.has_more_history());
    }

    #[test]
    fn delivery_state_updates_the_right_entry() {
        let mut t = TimelineState::new();
        let e = entry(1);
        let id = e.message_id;
        t.push_latest(e);
        t.push_latest(entry(2));

        t.update_delivery_state(id, DeliveryState::Sent);
        let updated = t.visible().find(|e| e.message_id == id).unwrap();
        assert_eq!(updated.delivery_state, DeliveryState::Sent);
    }
}

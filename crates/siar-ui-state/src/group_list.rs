//! `GroupListState`/`PendingInviteState` — the group-UI counterpart to
//! `ConversationListState`. Kept as their own state slices for the same
//! reason `ConversationListState` is (plan.md §94): a component
//! rendering the invite banner shouldn't re-render on every keystroke
//! in the add-member form, and vice versa.
//!
//! Deliberately mirrors what `GroupService`'s actual public API
//! (`crates/siar-messaging/src/group_service.rs`) can do today rather
//! than a wished-for fuller feature set:
//! - `join_group_mls` requires the founder's `GroupState` bookkeeping
//!   to be supplied out-of-band (see that method's own doc comment on
//!   why it can't derive membership from the MLS session alone) —
//!   `PendingInvite` carries a `state_input` field the accept flow
//!   fills in from a paste box, matching `apps/cli`'s `join-group
//!   <conversation-id> <welcome> <state>` shape exactly, not inventing
//!   a friendlier flow the backend doesn't support yet.
//! - There's no `GroupService::list_groups` — nothing in this crate
//!   queries the backend for "what groups exist"; `GroupListState` is
//!   populated by the desktop app's own command/incoming-event loop
//!   whenever it locally creates or joins one, same pattern
//!   `ConversationListState` already uses for 1:1 conversations.

use siar_domain::{AccountId, ConversationId, DeviceId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupSummary {
    pub conversation_id: ConversationId,
    /// No group-naming feature exists yet (`DurableGroupEvent::
    /// GroupRenamed` is defined in siar-domain but nothing calls it) —
    /// this is a display label the UI derives locally (e.g. the
    /// conversation id's `fmt_short()`), not a real group name from
    /// the backend.
    pub display_label: String,
    pub member_count: usize,
    pub is_admin: bool,
    pub epoch: u64,
}

#[derive(Debug, Default)]
pub struct GroupListState {
    groups: Vec<GroupSummary>,
}

impl GroupListState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn upsert(&mut self, summary: GroupSummary) {
        if let Some(existing) = self
            .groups
            .iter_mut()
            .find(|g| g.conversation_id == summary.conversation_id)
        {
            *existing = summary;
        } else {
            self.groups.push(summary);
        }
    }

    pub fn ordered(&self) -> &[GroupSummary] {
        &self.groups
    }

    pub fn get(&self, conversation_id: ConversationId) -> Option<&GroupSummary> {
        self.groups
            .iter()
            .find(|g| g.conversation_id == conversation_id)
    }
}

/// One `GroupMlsWelcome` that arrived and hasn't been accepted or
/// declined yet. `from_device`/`from_account` are the *sender* of the
/// welcome frame (the admin who added this device) — not necessarily
/// meaningful beyond display, since `handle_incoming_mls`'s own doc
/// comment already notes a welcome is deliberately not auto-joined.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingInvite {
    pub conversation_id: ConversationId,
    pub from_device: DeviceId,
    pub welcome_bytes: Vec<u8>,
    /// Filled in by the accept form's paste box before `AcceptGroupInvite`
    /// is dispatched — see this module's top doc comment on why this
    /// can't be auto-populated from the welcome alone.
    pub state_input: String,
    /// The welcome payload already carries the group state (the admin's
    /// client sent it), so the user has nothing to paste. `false` for an
    /// invite from an older client that sent a bare welcome.
    pub carries_state: bool,
}

#[derive(Debug, Default)]
pub struct PendingInviteState {
    invites: Vec<PendingInvite>,
}

impl PendingInviteState {
    pub fn new() -> Self {
        Self::default()
    }

    /// A second welcome for a conversation we already have a pending
    /// invite for replaces it rather than stacking a duplicate entry —
    /// mirrors `GroupService::handle_incoming_mls`'s own "ignore a
    /// welcome for a session we already have" idempotency, one layer
    /// up at the not-yet-accepted stage.
    pub fn add(&mut self, invite: PendingInvite) {
        self.invites
            .retain(|i| i.conversation_id != invite.conversation_id);
        self.invites.push(invite);
    }

    pub fn remove(&mut self, conversation_id: ConversationId) -> Option<PendingInvite> {
        let index = self
            .invites
            .iter()
            .position(|i| i.conversation_id == conversation_id)?;
        Some(self.invites.remove(index))
    }

    pub fn pending(&self) -> &[PendingInvite] {
        &self.invites
    }

    /// Lets the accept-form component update just the paste-box text
    /// for one invite without touching the others — `Signal::write()`
    /// on `PendingInviteState` needs a mutator, not a full replace, the
    /// same reason `ConversationListState::mark_read` exists instead of
    /// components reaching into the `Vec` directly.
    pub fn set_state_input(&mut self, conversation_id: ConversationId, text: String) {
        if let Some(invite) = self
            .invites
            .iter_mut()
            .find(|i| i.conversation_id == conversation_id)
        {
            invite.state_input = text;
        }
    }
}

/// Parsed, validated form input for adding a member to an MLS group —
/// kept as its own type so `AppCommand::AddGroupMember` doesn't carry
/// four raw `String`s with no indication which is which at the call
/// site, and so `components.rs`'s add-member form can validate/report
/// per-field errors before ever constructing an `AppCommand`.
#[derive(Debug, Clone)]
pub struct AddMemberInput {
    pub peer_account: AccountId,
    pub peer_device: DeviceId,
    pub peer_ticket_text: String,
    pub key_package_bytes: Vec<u8>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(id: ConversationId) -> GroupSummary {
        GroupSummary {
            conversation_id: id,
            display_label: "test".to_string(),
            member_count: 1,
            is_admin: true,
            epoch: 0,
        }
    }

    #[test]
    fn upsert_inserts_then_updates_in_place() {
        let mut state = GroupListState::new();
        let id = ConversationId::new();
        state.upsert(summary(id));
        assert_eq!(state.ordered().len(), 1);

        state.upsert(GroupSummary {
            member_count: 3,
            ..summary(id)
        });
        assert_eq!(state.ordered().len(), 1);
        assert_eq!(state.ordered()[0].member_count, 3);
    }

    #[test]
    fn get_finds_by_conversation_id() {
        let mut state = GroupListState::new();
        let id = ConversationId::new();
        state.upsert(summary(id));
        assert!(state.get(id).is_some());
        assert!(state.get(ConversationId::new()).is_none());
    }

    #[test]
    fn a_second_invite_for_the_same_conversation_replaces_the_first() {
        let mut state = PendingInviteState::new();
        let conversation_id = ConversationId::new();
        state.add(PendingInvite {
            conversation_id,
            from_device: DeviceId::new(),
            welcome_bytes: vec![1],
            state_input: String::new(),
            carries_state: false,
        });
        let second_device = DeviceId::new();
        state.add(PendingInvite {
            conversation_id,
            from_device: second_device,
            welcome_bytes: vec![2],
            state_input: String::new(),
            carries_state: false,
        });
        assert_eq!(state.pending().len(), 1);
        assert_eq!(state.pending()[0].from_device, second_device);
    }

    #[test]
    fn remove_takes_the_invite_out_and_returns_it() {
        let mut state = PendingInviteState::new();
        let conversation_id = ConversationId::new();
        state.add(PendingInvite {
            conversation_id,
            from_device: DeviceId::new(),
            welcome_bytes: vec![1],
            state_input: String::new(),
            carries_state: false,
        });
        let removed = state.remove(conversation_id);
        assert!(removed.is_some());
        assert!(state.pending().is_empty());
        assert!(state.remove(conversation_id).is_none());
    }

    #[test]
    fn set_state_input_only_touches_the_matching_invite() {
        let mut state = PendingInviteState::new();
        let a = ConversationId::new();
        let b = ConversationId::new();
        state.add(PendingInvite {
            conversation_id: a,
            from_device: DeviceId::new(),
            welcome_bytes: vec![],
            state_input: String::new(),
            carries_state: false,
        });
        state.add(PendingInvite {
            conversation_id: b,
            from_device: DeviceId::new(),
            welcome_bytes: vec![],
            state_input: String::new(),
            carries_state: false,
        });

        state.set_state_input(a, "pasted-text".to_string());
        assert_eq!(
            state
                .pending()
                .iter()
                .find(|i| i.conversation_id == a)
                .unwrap()
                .state_input,
            "pasted-text"
        );
        assert_eq!(
            state
                .pending()
                .iter()
                .find(|i| i.conversation_id == b)
                .unwrap()
                .state_input,
            ""
        );
    }
}

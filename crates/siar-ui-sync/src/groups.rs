//! Groups and pending invites on the wire.
//!
//! The daemon owns MLS group state; a window only needs to *see* the group
//! list and the invites waiting for a decision, and to send decisions back.
//!
//! Deliberately **not** sent: an invite's Welcome bytes (the daemon keeps them
//! and acts on `AcceptGroupInvite` itself — a window never handles key
//! material) and the legacy "paste the group state" box, which is window-local
//! text. Consequence, stated plainly: an invite from an *older* client that
//! does not carry the group record (`carries_state == false`) can only be
//! finished from a window that embeds the core; windows attached to a daemon
//! can accept every invite that carries its own state, which is every invite
//! this version sends.

use serde::{Deserialize, Serialize};
use siar_domain::{ConversationId, DeviceId};
use siar_ui_state::{GroupSummary, PendingInvite};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupDto {
    pub conversation_id: ConversationId,
    pub label: String,
    pub member_count: u32,
    pub is_admin: bool,
    pub epoch: u64,
}

impl From<&GroupSummary> for GroupDto {
    fn from(g: &GroupSummary) -> Self {
        Self {
            conversation_id: g.conversation_id,
            label: g.display_label.clone(),
            member_count: u32::try_from(g.member_count).unwrap_or(u32::MAX),
            is_admin: g.is_admin,
            epoch: g.epoch,
        }
    }
}

impl From<&GroupDto> for GroupSummary {
    fn from(g: &GroupDto) -> Self {
        Self {
            conversation_id: g.conversation_id,
            display_label: g.label.clone(),
            member_count: g.member_count as usize,
            is_admin: g.is_admin,
            epoch: g.epoch,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InviteDto {
    pub conversation_id: ConversationId,
    pub from_device: DeviceId,
    pub carries_state: bool,
}

impl From<&PendingInvite> for InviteDto {
    fn from(i: &PendingInvite) -> Self {
        Self {
            conversation_id: i.conversation_id,
            from_device: i.from_device,
            carries_state: i.carries_state,
        }
    }
}

impl From<&InviteDto> for PendingInvite {
    fn from(i: &InviteDto) -> Self {
        // No Welcome bytes in a window: the daemon holds them.
        Self {
            conversation_id: i.conversation_id,
            from_device: i.from_device,
            welcome_bytes: Vec::new(),
            state_input: String::new(),
            carries_state: i.carries_state,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_group_round_trips() {
        let g = GroupSummary {
            conversation_id: ConversationId::new(),
            display_label: "team".into(),
            member_count: 3,
            is_admin: true,
            epoch: 7,
        };
        assert_eq!(GroupSummary::from(&GroupDto::from(&g)), g);
    }

    #[test]
    fn an_invite_never_carries_key_material_to_a_window() {
        let i = PendingInvite {
            conversation_id: ConversationId::new(),
            from_device: DeviceId::new(),
            welcome_bytes: vec![9; 4096],
            state_input: "secret paste".into(),
            carries_state: true,
        };
        let dto = InviteDto::from(&i);
        let bytes = postcard::to_allocvec(&dto).unwrap();
        assert!(bytes.len() < 64, "{} bytes", bytes.len());
        let back = PendingInvite::from(&dto);
        assert!(back.welcome_bytes.is_empty());
        assert!(back.state_input.is_empty());
        assert!(back.carries_state);
    }
}

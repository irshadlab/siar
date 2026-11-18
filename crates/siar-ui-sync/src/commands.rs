//! The window → core direction. Commands cross the process boundary as an
//! explicit, **re-validated** wire type: `MessageText` derives `Deserialize`
//! without checking its invariants, so nothing decoded off a socket is
//! trusted until [`WireCommand::validate`] has re-parsed it.

use serde::{Deserialize, Serialize};
use siar_domain::{
    AccountId, AttachmentReference, ConversationId, DeviceId, MessageId, MessageText,
};
use siar_ui_state::AppCommand;

const MAX_TICKET: usize = 4 * 1024;
const MAX_NAME: usize = 256;
const MAX_PATH: usize = 4 * 1024;
const MAX_KEY_PACKAGE: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WireCommand {
    SendMessage {
        conversation: ConversationId,
        text: MessageText,
    },
    SendGroupMessage {
        conversation: ConversationId,
        text: MessageText,
    },
    OpenConversation(ConversationId),
    SendAttachment {
        conversation: ConversationId,
        path: String,
    },
    LinkOpenOffer,
    LinkApprove {
        device: DeviceId,
    },
    LinkReject {
        device: DeviceId,
    },
    LinkJoin {
        ticket_text: String,
        code: String,
    },
    SendViaRelay {
        conversation: ConversationId,
        relay_ticket_text: String,
        text: MessageText,
    },
    CheckRelayMailbox {
        conversation: ConversationId,
        relay_ticket_text: String,
    },
    MarkRead {
        conversation: ConversationId,
        through_sequence: u64,
    },
    LoadEarlierMessages {
        conversation: ConversationId,
    },
    PairWithTicket {
        ticket_text: String,
    },
    SaveContact {
        device_id: DeviceId,
        account_id: AccountId,
        display_name: String,
        ticket_text: String,
        key_package_b64: Option<String>,
    },
    RemoveContact {
        device_id: DeviceId,
    },
    SetContactVerified {
        device_id: DeviceId,
        verified: bool,
    },
    UseContact {
        device_id: DeviceId,
    },
    LoadAttachmentPreview {
        message_id: MessageId,
        reference: AttachmentReference,
    },
    /// Mark a synced security event resolved. The id is the one the window
    /// received in the `SecurityEvents` section; an unknown id is a no-op.
    ResolveSecurityEvent {
        event_id: u64,
    },
    CreateGroup {
        founder: AccountId,
    },
    AddGroupMember {
        conversation: ConversationId,
        new_member: AccountId,
        peer_account: AccountId,
        peer_device: DeviceId,
        peer_ticket_text: String,
        key_package_bytes: Vec<u8>,
    },
    /// Acts on the Welcome the daemon holds; the window never sees it.
    AcceptGroupInvite {
        conversation: ConversationId,
    },
    DeclineGroupInvite {
        conversation: ConversationId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WireCommandError {
    /// Valid, but not yet available when the core is in another process.
    #[error("that action isn’t available through the background service yet")]
    Unsupported,
    #[error("the command was malformed")]
    Invalid,
}

impl TryFrom<&AppCommand> for WireCommand {
    type Error = WireCommandError;

    fn try_from(c: &AppCommand) -> Result<Self, Self::Error> {
        use AppCommand as A;
        Ok(match c {
            A::SendMessage { conversation, text } => Self::SendMessage {
                conversation: *conversation,
                text: text.clone(),
            },
            A::SendGroupMessage { conversation, text } => Self::SendGroupMessage {
                conversation: *conversation,
                text: text.clone(),
            },
            A::OpenConversation(id) => Self::OpenConversation(*id),
            A::LinkOpenOffer => Self::LinkOpenOffer,
            A::LinkApprove { device } => Self::LinkApprove { device: *device },
            A::LinkReject { device } => Self::LinkReject { device: *device },
            A::LinkJoin { ticket_text, code } => Self::LinkJoin {
                ticket_text: ticket_text.clone(),
                code: code.clone(),
            },
            A::SendViaRelay {
                conversation,
                relay_ticket_text,
                text,
            } => Self::SendViaRelay {
                conversation: *conversation,
                relay_ticket_text: relay_ticket_text.clone(),
                text: text.clone(),
            },
            A::CheckRelayMailbox {
                conversation,
                relay_ticket_text,
            } => Self::CheckRelayMailbox {
                conversation: *conversation,
                relay_ticket_text: relay_ticket_text.clone(),
            },
            A::SendAttachment { conversation, path } => Self::SendAttachment {
                conversation: *conversation,
                path: path.clone(),
            },
            A::MarkRead {
                conversation,
                through_sequence,
            } => Self::MarkRead {
                conversation: *conversation,
                through_sequence: *through_sequence,
            },
            A::LoadEarlierMessages { conversation } => Self::LoadEarlierMessages {
                conversation: *conversation,
            },
            A::PairWithTicket { ticket_text } => Self::PairWithTicket {
                ticket_text: ticket_text.clone(),
            },
            A::SaveContact {
                device_id,
                account_id,
                display_name,
                ticket_text,
                key_package_b64,
            } => Self::SaveContact {
                device_id: *device_id,
                account_id: *account_id,
                display_name: display_name.clone(),
                ticket_text: ticket_text.clone(),
                key_package_b64: key_package_b64.clone(),
            },
            A::RemoveContact { device_id } => Self::RemoveContact {
                device_id: *device_id,
            },
            A::SetContactVerified {
                device_id,
                verified,
            } => Self::SetContactVerified {
                device_id: *device_id,
                verified: *verified,
            },
            A::UseContact { device_id } => Self::UseContact {
                device_id: *device_id,
            },
            A::LoadAttachmentPreview {
                message_id,
                reference,
            } => Self::LoadAttachmentPreview {
                message_id: *message_id,
                reference: reference.clone(),
            },
            A::ResolveSecurityEvent { event_id } => Self::ResolveSecurityEvent {
                event_id: *event_id,
            },
            A::CreateGroup { founder } => Self::CreateGroup { founder: *founder },
            A::AddGroupMember {
                conversation,
                new_member,
                input,
            } => Self::AddGroupMember {
                conversation: *conversation,
                new_member: *new_member,
                peer_account: input.peer_account,
                peer_device: input.peer_device,
                peer_ticket_text: input.peer_ticket_text.clone(),
                key_package_bytes: input.key_package_bytes.clone(),
            },
            A::AcceptGroupInvite { conversation } => Self::AcceptGroupInvite {
                conversation: *conversation,
            },
            A::DeclineGroupInvite { conversation } => Self::DeclineGroupInvite {
                conversation: *conversation,
            },
            // No catch-all: a new `AppCommand` must get a wire form (or an
            // explicit `Unsupported` arm) before this compiles.
        })
    }
}

impl WireCommand {
    /// Re-check every invariant a deserializer skipped. Call on anything
    /// that came off a socket.
    pub fn validate(self) -> Result<Self, WireCommandError> {
        let ok = match &self {
            Self::SendMessage { text, .. } | Self::SendGroupMessage { text, .. } => {
                MessageText::parse(text.as_str().to_owned()).is_ok()
            }
            Self::PairWithTicket { ticket_text } => {
                !ticket_text.is_empty() && ticket_text.len() <= MAX_TICKET
            }
            Self::LinkJoin { ticket_text, code } => {
                !ticket_text.is_empty()
                    && ticket_text.len() <= MAX_TICKET
                    && !code.is_empty()
                    && code.len() <= 64
            }
            Self::SendViaRelay {
                relay_ticket_text,
                text,
                ..
            } => {
                !relay_ticket_text.is_empty()
                    && relay_ticket_text.len() <= MAX_TICKET
                    && MessageText::parse(text.as_str().to_owned()).is_ok()
            }
            Self::CheckRelayMailbox {
                relay_ticket_text, ..
            } => !relay_ticket_text.is_empty() && relay_ticket_text.len() <= MAX_TICKET,
            // An absolute path only: a relative one would be resolved against
            // the daemon's working directory, which the sender cannot know.
            Self::SendAttachment { path, .. } => {
                !path.is_empty()
                    && path.len() <= MAX_PATH
                    && !path.contains('\0')
                    && std::path::Path::new(path).is_absolute()
            }
            Self::SaveContact {
                display_name,
                ticket_text,
                key_package_b64,
                ..
            } => {
                display_name.len() <= MAX_NAME
                    && !ticket_text.is_empty()
                    && ticket_text.len() <= MAX_TICKET
                    && key_package_b64
                        .as_ref()
                        .is_none_or(|k| k.len() <= MAX_KEY_PACKAGE)
            }
            Self::AddGroupMember {
                new_member,
                peer_account,
                peer_ticket_text,
                key_package_bytes,
                ..
            } => {
                // One member, one account: the two fields must agree.
                new_member == peer_account
                    && !peer_ticket_text.is_empty()
                    && peer_ticket_text.len() <= MAX_TICKET
                    && !key_package_bytes.is_empty()
                    && key_package_bytes.len() <= MAX_KEY_PACKAGE
            }
            _ => true,
        };
        if ok {
            Ok(self)
        } else {
            Err(WireCommandError::Invalid)
        }
    }

    #[must_use]
    pub fn into_app(self) -> AppCommand {
        match self {
            Self::SendMessage { conversation, text } => {
                AppCommand::SendMessage { conversation, text }
            }
            Self::SendGroupMessage { conversation, text } => {
                AppCommand::SendGroupMessage { conversation, text }
            }
            Self::OpenConversation(id) => AppCommand::OpenConversation(id),
            Self::SendAttachment { conversation, path } => {
                AppCommand::SendAttachment { conversation, path }
            }
            Self::LinkOpenOffer => AppCommand::LinkOpenOffer,
            Self::LinkApprove { device } => AppCommand::LinkApprove { device },
            Self::LinkReject { device } => AppCommand::LinkReject { device },
            Self::LinkJoin { ticket_text, code } => AppCommand::LinkJoin { ticket_text, code },
            Self::SendViaRelay {
                conversation,
                relay_ticket_text,
                text,
            } => AppCommand::SendViaRelay {
                conversation,
                relay_ticket_text,
                text,
            },
            Self::CheckRelayMailbox {
                conversation,
                relay_ticket_text,
            } => AppCommand::CheckRelayMailbox {
                conversation,
                relay_ticket_text,
            },
            Self::MarkRead {
                conversation,
                through_sequence,
            } => AppCommand::MarkRead {
                conversation,
                through_sequence,
            },
            Self::LoadEarlierMessages { conversation } => {
                AppCommand::LoadEarlierMessages { conversation }
            }
            Self::PairWithTicket { ticket_text } => AppCommand::PairWithTicket { ticket_text },
            Self::SaveContact {
                device_id,
                account_id,
                display_name,
                ticket_text,
                key_package_b64,
            } => AppCommand::SaveContact {
                device_id,
                account_id,
                display_name,
                ticket_text,
                key_package_b64,
            },
            Self::RemoveContact { device_id } => AppCommand::RemoveContact { device_id },
            Self::SetContactVerified {
                device_id,
                verified,
            } => AppCommand::SetContactVerified {
                device_id,
                verified,
            },
            Self::UseContact { device_id } => AppCommand::UseContact { device_id },
            Self::ResolveSecurityEvent { event_id } => {
                AppCommand::ResolveSecurityEvent { event_id }
            }
            Self::LoadAttachmentPreview {
                message_id,
                reference,
            } => AppCommand::LoadAttachmentPreview {
                message_id,
                reference,
            },
            Self::CreateGroup { founder } => AppCommand::CreateGroup { founder },
            Self::AddGroupMember {
                conversation,
                new_member,
                peer_account,
                peer_device,
                peer_ticket_text,
                key_package_bytes,
            } => AppCommand::AddGroupMember {
                conversation,
                new_member,
                input: siar_ui_state::AddMemberInput {
                    peer_account,
                    peer_device,
                    peer_ticket_text,
                    key_package_bytes,
                },
            },
            Self::AcceptGroupInvite { conversation } => {
                AppCommand::AcceptGroupInvite { conversation }
            }
            Self::DeclineGroupInvite { conversation } => {
                AppCommand::DeclineGroupInvite { conversation }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str) -> MessageText {
        MessageText::parse(s.to_owned()).unwrap()
    }

    #[test]
    fn supported_commands_round_trip_through_the_wire_form() {
        let c = ConversationId::new();
        let dev = DeviceId::new();
        for app in [
            AppCommand::SendMessage {
                conversation: c,
                text: text("hi"),
            },
            AppCommand::OpenConversation(c),
            AppCommand::LinkOpenOffer,
            AppCommand::LinkApprove { device: dev },
            AppCommand::LinkReject { device: dev },
            AppCommand::LinkJoin {
                ticket_text: "t".into(),
                code: "c".into(),
            },
            AppCommand::SendViaRelay {
                conversation: c,
                relay_ticket_text: "relay".into(),
                text: text("later"),
            },
            AppCommand::CheckRelayMailbox {
                conversation: c,
                relay_ticket_text: "relay".into(),
            },
            AppCommand::MarkRead {
                conversation: c,
                through_sequence: 9,
            },
            AppCommand::PairWithTicket {
                ticket_text: "ticket".into(),
            },
            AppCommand::RemoveContact {
                device_id: DeviceId::new(),
            },
            AppCommand::SetContactVerified {
                device_id: DeviceId::new(),
                verified: true,
            },
        ] {
            let w = WireCommand::try_from(&app).unwrap();
            let bytes = postcard::to_allocvec(&w).unwrap();
            let back: WireCommand = postcard::from_bytes(&bytes).unwrap();
            assert_eq!(back, w);
            assert_eq!(
                format!("{:?}", back.validate().unwrap().into_app()),
                format!("{app:?}")
            );
        }
    }

    #[test]
    fn group_commands_round_trip_and_are_revalidated() {
        let c = ConversationId::new();
        let (acct, dev) = (AccountId::new(), DeviceId::new());
        for app in [
            AppCommand::CreateGroup { founder: acct },
            AppCommand::AcceptGroupInvite { conversation: c },
            AppCommand::DeclineGroupInvite { conversation: c },
            AppCommand::AddGroupMember {
                conversation: c,
                new_member: acct,
                input: siar_ui_state::AddMemberInput {
                    peer_account: acct,
                    peer_device: dev,
                    peer_ticket_text: "ticket".into(),
                    key_package_bytes: vec![1, 2, 3],
                },
            },
        ] {
            let w = WireCommand::try_from(&app).unwrap();
            let bytes = postcard::to_allocvec(&w).unwrap();
            let back: WireCommand = postcard::from_bytes(&bytes).unwrap();
            assert_eq!(
                format!("{:?}", back.validate().unwrap().into_app()),
                format!("{app:?}")
            );
        }
        // Oversize or empty member input is refused after decoding.
        let bad = |ticket: &str, kp: Vec<u8>| WireCommand::AddGroupMember {
            conversation: c,
            new_member: acct,
            peer_account: acct,
            peer_device: dev,
            peer_ticket_text: ticket.into(),
            key_package_bytes: kp,
        };
        assert!(bad("", vec![1]).validate().is_err());
        assert!(bad("t", vec![]).validate().is_err());
        assert!(bad("t", vec![0; 70_000]).validate().is_err());
        assert!(bad("t", vec![0; 100]).validate().is_ok());
        let send = |p: &str| WireCommand::SendAttachment {
            conversation: c,
            path: p.into(),
        };
        assert!(send("relative/file.png").validate().is_err());
        assert!(send("").validate().is_err());
        assert!(send("/tmp/a\0b").validate().is_err());
        assert!(send(&format!("/{}", "a".repeat(5_000))).validate().is_err());
        #[cfg(unix)]
        assert!(send("/tmp/photo.png").validate().is_ok());
        // new_member and peer_account naming different accounts is refused.
        let mismatched = WireCommand::AddGroupMember {
            conversation: c,
            new_member: AccountId::new(),
            peer_account: AccountId::new(),
            peer_device: dev,
            peer_ticket_text: "t".into(),
            key_package_bytes: vec![1],
        };
        assert!(mismatched.validate().is_err());
    }

    #[test]
    fn a_hostile_oversize_message_that_deserializes_is_still_rejected() {
        // Hand-build the bytes a malicious client could send: derive(Deserialize)
        // accepts any String for MessageText, so validate() is the real gate.
        let huge = "y".repeat(100_000);
        #[derive(Serialize)]
        struct RawSend {
            conversation: ConversationId,
            text: String,
        }
        #[derive(Serialize)]
        enum RawCmd {
            SendMessage(RawSend),
        }
        let bytes = postcard::to_allocvec(&RawCmd::SendMessage(RawSend {
            conversation: ConversationId::new(),
            text: huge,
        }))
        .unwrap();
        let decoded: WireCommand = postcard::from_bytes(&bytes).expect("serde alone accepts it");
        assert_eq!(decoded.validate(), Err(WireCommandError::Invalid));
    }

    #[test]
    fn empty_and_oversize_fields_are_rejected() {
        assert!(WireCommand::PairWithTicket {
            ticket_text: String::new()
        }
        .validate()
        .is_err());
        assert!(WireCommand::PairWithTicket {
            ticket_text: "a".repeat(5_000)
        }
        .validate()
        .is_err());
        assert!(WireCommand::SaveContact {
            device_id: DeviceId::new(),
            account_id: AccountId::new(),
            display_name: "n".repeat(300),
            ticket_text: "t".into(),
            key_package_b64: None
        }
        .validate()
        .is_err());
        assert!(WireCommand::PairWithTicket {
            ticket_text: "ok".into()
        }
        .validate()
        .is_ok());
    }
}

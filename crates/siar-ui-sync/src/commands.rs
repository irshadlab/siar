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
            // Group management, invites and security-event resolution need
            // their own synced state first (see ROADMAP).
            _ => return Err(WireCommandError::Unsupported),
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
            Self::LoadAttachmentPreview {
                message_id,
                reference,
            } => AppCommand::LoadAttachmentPreview {
                message_id,
                reference,
            },
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
        for app in [
            AppCommand::SendMessage {
                conversation: c,
                text: text("hi"),
            },
            AppCommand::OpenConversation(c),
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
    fn group_management_is_reported_unsupported_not_silently_dropped() {
        let c = AppCommand::DeclineGroupInvite {
            conversation: ConversationId::new(),
        };
        assert_eq!(
            WireCommand::try_from(&c),
            Err(WireCommandError::Unsupported)
        );
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

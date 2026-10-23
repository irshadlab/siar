//! UI → core. Every Slint callback is validated here (so the user gets an
//! inline, plain-language answer next to the field) and then becomes an
//! `AppCommand` or a local slice mutation. Nothing in this file touches
//! networking or storage.

use std::str::FromStr;

use base64::Engine;
use siar_domain::{AccountId, ConversationId, DeviceId, MediaType, MessageId};
use siar_ui_state::{AddMemberInput, AppCommand, AttachmentPreview, SecurityEventId};
use slint::ComponentHandle;

use crate::hub::UiHub;
use crate::settings::{DensityChoice, ThemeChoice, UiSettings};
use crate::slices::{EventView, NoticeLevel};
use crate::ui::{AppState, Density, MainWindow, Page, Theme, ThemeMode};

fn parse_id<T: FromStr>(s: &str) -> Option<T> {
    s.trim().parse().ok()
}

pub(crate) fn settings_from_window(window: &MainWindow) -> UiSettings {
    let theme = window.global::<Theme>();
    UiSettings {
        theme: match theme.get_mode() {
            ThemeMode::System => ThemeChoice::System,
            ThemeMode::Light => ThemeChoice::Light,
            ThemeMode::Dark => ThemeChoice::Dark,
            ThemeMode::HighContrast => ThemeChoice::HighContrast,
        },
        density: match theme.get_density() {
            Density::Comfortable => DensityChoice::Comfortable,
            Density::Compact => DensityChoice::Compact,
        },
        scale: UiSettings::clamp_scale(theme.get_ui_scale()),
    }
}

pub(crate) fn apply_settings(window: &MainWindow, s: UiSettings) {
    let theme = window.global::<Theme>();
    theme.set_mode(match s.theme {
        ThemeChoice::System => ThemeMode::System,
        ThemeChoice::Light => ThemeMode::Light,
        ThemeChoice::Dark => ThemeMode::Dark,
        ThemeChoice::HighContrast => ThemeMode::HighContrast,
    });
    theme.set_density(match s.density {
        DensityChoice::Comfortable => Density::Comfortable,
        DensityChoice::Compact => Density::Compact,
    });
    theme.set_ui_scale(UiSettings::clamp_scale(s.scale));
}

fn clean_base64(s: &str) -> Result<Vec<u8>, base64::DecodeError> {
    // Pasted key packages routinely carry line breaks and spaces.
    let compact: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    base64::engine::general_purpose::STANDARD.decode(compact)
}

pub(crate) fn install(window: &MainWindow, hub: &UiHub) {
    let app = window.global::<AppState>();
    let weak = window.as_weak();

    // ---- chat ------------------------------------------------------------------
    {
        let hub = hub.clone();
        let weak = weak.clone();
        app.on_send_message(move |draft| {
            if draft.trim().is_empty() {
                return;
            }
            let Some((conversation, is_group, can_send)) =
                hub.read(|s| s.selected.map(|c| (c, s.is_group(c), s.can_send(c))))
            else {
                return;
            };
            if !can_send {
                hub.notify(
                    NoticeLevel::Warning,
                    "Pair with this person first — there’s nobody to send to yet.",
                );
                return;
            }
            // ComposerState validates (length cap) and clears on success;
            // on failure the draft stays so nothing the user typed is lost.
            let parsed = hub.update_quiet(|s| {
                s.composer.set_draft(draft.to_string());
                s.composer.try_submit()
            });
            match parsed {
                Ok(text) => {
                    if let Some(w) = weak.upgrade() {
                        w.global::<AppState>().set_composer_draft("".into());
                    }
                    let command = if is_group {
                        AppCommand::SendGroupMessage { conversation, text }
                    } else {
                        AppCommand::SendMessage { conversation, text }
                    };
                    hub.dispatch(command);
                }
                Err(_) => hub.notify(
                    NoticeLevel::Warning,
                    "That message is too long to send (limit 64 KiB).",
                ),
            }
        });
    }
    {
        let hub = hub.clone();
        app.on_draft_edited(move |text| {
            hub.update_quiet(|s| s.composer.set_draft(text.to_string()));
        });
    }
    {
        let hub = hub.clone();
        let weak = weak.clone();
        app.on_open_conversation(move |id| {
            let Some(conversation) = parse_id::<ConversationId>(&id) else {
                return;
            };
            hub.update(|s| s.select(conversation));
            if let Some(w) = weak.upgrade() {
                // "Open chat" buttons live on other pages too.
                w.global::<AppState>().set_page(Page::Chats);
            }
            hub.dispatch(AppCommand::OpenConversation(conversation));
        });
    }
    {
        let hub = hub.clone();
        app.on_load_earlier(move || {
            if let Some(conversation) = hub.read(|s| s.selected) {
                hub.dispatch(AppCommand::LoadEarlierMessages { conversation });
            }
        });
    }
    {
        let hub = hub.clone();
        app.on_filter_conversations(move |text| {
            hub.update(|s| s.conversation_filter = text.to_string());
        });
    }
    {
        let hub = hub.clone();
        app.on_load_attachment(move |id| {
            let Some(message_id) = parse_id::<MessageId>(&id) else {
                return;
            };
            let request = hub.update(|s| {
                if s.attachment_previews.get(message_id) != AttachmentPreview::NotRequested {
                    return None; // already loading / loaded / failed
                }
                let reference = s.find_attachment(message_id)?;
                if !matches!(
                    reference.media_type,
                    MediaType::ImagePng | MediaType::ImageJpeg | MediaType::ImageWebp
                ) {
                    return None;
                }
                s.attachment_previews.set_loading(message_id);
                Some(reference)
            });
            if let Some(reference) = request {
                hub.dispatch(AppCommand::LoadAttachmentPreview {
                    message_id,
                    reference,
                });
            }
        });
    }

    // ---- pairing & contacts ------------------------------------------------------
    {
        let hub = hub.clone();
        app.on_pair_with_ticket(move |ticket| {
            let ticket = ticket.trim().to_string();
            if ticket.is_empty() {
                hub.notify(NoticeLevel::Warning, "Paste their ticket first.");
                return false;
            }
            if let Err(why) = (hub.0.validate_ticket)(&ticket) {
                tracing::debug!(%why, "ticket rejected");
                hub.notify(
                    NoticeLevel::Error,
                    "That doesn’t look like a valid ticket. Ask them to copy it again.",
                );
                return false;
            }
            hub.dispatch(AppCommand::PairWithTicket {
                ticket_text: ticket,
            })
        });
    }
    {
        let hub = hub.clone();
        app.on_use_contact(move |id| {
            let Some(device_id) = parse_id::<DeviceId>(&id) else {
                return;
            };
            hub.dispatch(AppCommand::UseContact { device_id });
        });
    }
    {
        let hub = hub.clone();
        app.on_select_contact(move |id| {
            let selected = parse_id::<DeviceId>(&id);
            hub.update(|s| {
                s.selected_contact = selected;
                if selected.is_some() {
                    s.open_content = true;
                }
            });
        });
    }
    {
        let hub = hub.clone();
        app.on_save_contact(move |name, account, device, ticket| {
            let name = name.trim().to_string();
            if name.is_empty() {
                hub.notify(NoticeLevel::Warning, "Give the contact a name.");
                return false;
            }
            let Some(account_id) = parse_id::<AccountId>(&account) else {
                hub.notify(
                    NoticeLevel::Error,
                    "That account id isn’t valid. Copy it again from their Settings.",
                );
                return false;
            };
            let Some(device_id) = parse_id::<DeviceId>(&device) else {
                hub.notify(
                    NoticeLevel::Error,
                    "That device id isn’t valid. Copy it again from their Settings.",
                );
                return false;
            };
            let ticket = ticket.trim().to_string();
            if (hub.0.validate_ticket)(&ticket).is_err() {
                hub.notify(
                    NoticeLevel::Error,
                    "That doesn’t look like a valid ticket. Ask them to copy it again.",
                );
                return false;
            }
            hub.dispatch(AppCommand::SaveContact {
                device_id,
                account_id,
                display_name: name,
                ticket_text: ticket,
                key_package_b64: None,
            })
        });
    }
    {
        let hub = hub.clone();
        app.on_remove_contact(move |id| {
            let Some(device_id) = parse_id::<DeviceId>(&id) else {
                return;
            };
            hub.update(|s| {
                if s.selected_contact == Some(device_id) {
                    s.selected_contact = None;
                }
            });
            hub.dispatch(AppCommand::RemoveContact { device_id });
        });
    }

    // ---- groups ---------------------------------------------------------------------
    {
        let hub = hub.clone();
        app.on_create_group(move || match hub.read(|s| s.identity.account) {
            Some(founder) => {
                hub.dispatch(AppCommand::CreateGroup { founder });
            }
            None => hub.notify(
                NoticeLevel::Info,
                "Siar is still starting up — try again in a moment.",
            ),
        });
    }
    {
        let hub = hub.clone();
        app.on_select_group(move |id| {
            let selected = parse_id::<ConversationId>(&id);
            hub.update(|s| {
                s.selected_group = selected;
                if selected.is_some() {
                    s.open_content = true;
                }
            });
        });
    }
    {
        let hub = hub.clone();
        app.on_add_member(move |account, device, ticket, key_package| {
            let Some((conversation, is_admin)) = hub.read(|s| {
                s.selected_group
                    .map(|g| (g, s.groups.get(g).map(|x| x.is_admin).unwrap_or(false)))
            }) else {
                hub.notify(NoticeLevel::Warning, "Select a group first.");
                return false;
            };
            if !is_admin {
                hub.notify(NoticeLevel::Warning, "Only group admins can add members.");
                return false;
            }
            let (Some(peer_account), Some(peer_device)) = (
                parse_id::<AccountId>(&account),
                parse_id::<DeviceId>(&device),
            ) else {
                hub.notify(
                    NoticeLevel::Error,
                    "Check the account id and device id — one of them isn’t valid.",
                );
                return false;
            };
            let ticket = ticket.trim().to_string();
            if (hub.0.validate_ticket)(&ticket).is_err() {
                hub.notify(
                    NoticeLevel::Error,
                    "That doesn’t look like a valid ticket. Ask them to copy it again.",
                );
                return false;
            }
            let Ok(key_package_bytes) = clean_base64(&key_package) else {
                hub.notify(
                    NoticeLevel::Error,
                    "That key package isn’t valid base64. Copy it again in full.",
                );
                return false;
            };
            if key_package_bytes.is_empty() {
                hub.notify(NoticeLevel::Warning, "Paste their key package first.");
                return false;
            }
            hub.dispatch(AppCommand::AddGroupMember {
                conversation,
                new_member: peer_account,
                input: AddMemberInput {
                    peer_account,
                    peer_device,
                    peer_ticket_text: ticket,
                    key_package_bytes,
                },
            })
        });
    }
    {
        let hub = hub.clone();
        app.on_accept_invite(move |conversation, state| {
            let Some(conversation) = parse_id::<ConversationId>(&conversation) else {
                return false;
            };
            let state = state.trim().to_string();
            if state.is_empty() {
                hub.notify(
                    NoticeLevel::Warning,
                    "Paste the group state the admin sent you first.",
                );
                return false;
            }
            hub.update_quiet(|s| s.pending_invites.set_state_input(conversation, state));
            hub.dispatch(AppCommand::AcceptGroupInvite { conversation })
        });
    }
    {
        let hub = hub.clone();
        app.on_decline_invite(move |conversation| {
            if let Some(conversation) = parse_id::<ConversationId>(&conversation) {
                hub.dispatch(AppCommand::DeclineGroupInvite { conversation });
            }
        });
    }

    // ---- security ---------------------------------------------------------------------
    {
        let hub = hub.clone();
        app.on_resolve_event(move |id| {
            if let Ok(raw) = id.trim().parse::<u64>() {
                hub.update(|s| s.security_events.resolve(SecurityEventId::from_raw(raw)));
            }
        });
    }
    {
        let hub = hub.clone();
        app.on_set_event_filter(move |i| {
            hub.update(|s| {
                s.event_view = match i {
                    1 => EventView::Unresolved,
                    2 => EventView::Critical,
                    _ => EventView::All,
                }
            });
        });
    }

    {
        let hub = hub.clone();
        app.on_dismiss_banner(move |id| {
            if let Ok(raw) = id.trim().parse::<u64>() {
                // Local "seen" only: resolution state is untouched (§98).
                hub.update(|s| s.banner.dismiss(SecurityEventId::from_raw(raw)));
            }
        });
    }
    {
        let weak = weak.clone();
        app.on_review_security(move || {
            if let Some(w) = weak.upgrade() {
                let app = w.global::<AppState>();
                app.set_page(Page::Security);
                app.set_security_tab(crate::ui::SecurityTab::Events);
                app.set_content_open(true);
            }
        });
    }
    {
        let hub = hub.clone();
        app.on_set_contact_verified(move |id, verified| {
            let Some(device_id) = parse_id::<DeviceId>(&id) else {
                return;
            };
            hub.dispatch(AppCommand::SetContactVerified {
                device_id,
                verified,
            });
        });
    }

    // ---- settings -----------------------------------------------------------------------
    {
        let hub = hub.clone();
        let weak = weak.clone();
        app.on_settings_changed(move || {
            if let Some(w) = weak.upgrade() {
                (hub.0.on_settings_changed)(settings_from_window(&w));
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_tolerates_pasted_whitespace() {
        let encoded = base64::engine::general_purpose::STANDARD.encode(b"hello key package");
        let wrapped = format!("  {}\n{}  \r\n", &encoded[..8], &encoded[8..]);
        assert_eq!(clean_base64(&wrapped).unwrap(), b"hello key package");
        assert!(clean_base64("***not base64***").is_err());
    }

    #[test]
    fn id_parsing_trims_and_rejects_garbage() {
        let id = ConversationId::new();
        assert_eq!(parse_id::<ConversationId>(&format!(" {id}\n")), Some(id));
        assert_eq!(parse_id::<ConversationId>("nope"), None);
        assert_eq!(parse_id::<ConversationId>(""), None);
    }
}

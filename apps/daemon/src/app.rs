//! The daemon's application surface: what `siar-daemon ctl` can ask and do.
//!
//! These are *headless equivalents* of what the window does through
//! `AppCommand`s: read the pairing ticket, list conversations, read recent
//! messages, pair, send. Payloads are plain UTF-8 text, so the protocol stays
//! trivially scriptable and no UI types need to be serializable.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use siar_app_core::core::Core;
use siar_domain::{ConversationId, MessageContent};
use siar_runtime::{AppService, BoxFuture, RuntimeError};
use siar_ui_state::AppCommand;
use tokio::sync::mpsc;

/// Filled once the core has started (after recovery, inside the barrier).
pub struct Live {
    pub core: Arc<Core>,
    pub commands: mpsc::Sender<AppCommand>,
    pub sync: Arc<siar_ui_sync::UiSyncService>,
}

#[derive(Clone, Default)]
pub struct DaemonApp {
    pub live: Arc<OnceLock<Live>>,
}

fn utf8(data: Vec<u8>) -> Result<String, RuntimeError> {
    String::from_utf8(data).map_err(|_| RuntimeError::InvalidCommand("payload is not UTF-8".into()))
}

fn conversation(text: &str) -> Result<ConversationId, RuntimeError> {
    uuid::Uuid::parse_str(text.trim())
        .map(ConversationId::from_uuid)
        .map_err(|_| RuntimeError::InvalidCommand("not a conversation id".into()))
}

impl DaemonApp {
    fn live(&self) -> Result<&Live, RuntimeError> {
        self.live.get().ok_or(RuntimeError::NotReady)
    }

    /// `<conversation-id> <peer-ticket> <peer-device-id> <base64-key-package>`
    async fn pairwise_open(&self, data: Vec<u8>) -> Result<Vec<u8>, RuntimeError> {
        let live = self.live()?;
        let text = utf8(data)?;
        let mut parts = text.split_whitespace();
        let (Some(conv), Some(ticket), Some(device), Some(kp), None) = (
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
        ) else {
            return Err(RuntimeError::InvalidCommand(
                "expected: <conversation-id> <peer-ticket> <peer-device-id> <key-package>".into(),
            ));
        };
        let device = uuid::Uuid::parse_str(device)
            .map(siar_domain::DeviceId::from_uuid)
            .map_err(|_| RuntimeError::InvalidCommand("not a device id".into()))?;
        let opened = live
            .core
            .pairwise_open(
                conversation(conv)?,
                ticket,
                device,
                kp,
                siar_crypto_mls::PairwiseProfile::default(),
            )
            .await
            .map_err(RuntimeError::InvalidCommand)?;
        Ok(if opened {
            b"opened".to_vec()
        } else {
            b"no usable key package; conversation keeps the static-key path".to_vec()
        })
    }

    async fn run_query(&self, name: &str, data: Vec<u8>) -> Result<Vec<u8>, RuntimeError> {
        let live = self.live()?;
        if name == "ui-snapshot" {
            return live.sync.snapshot_page(&data);
        }
        let hub = live.core.state();
        let out = match name {
            "ticket" => hub.read(|s| s.identity.ticket.clone()),
            // `<device-id> <base64 key package>` — hand both to the peer.
            "pairwise-publish" => {
                let (device, kp) = live
                    .core
                    .pairwise_publish(siar_crypto_mls::PairwiseProfile::default())
                    .map_err(RuntimeError::InvalidCommand)?;
                format!("{device} {kp}")
            }
            "summary" => {
                let (ready, convs, unread) = hub.read(|s| {
                    let c = s.conversations.ordered();
                    (
                        s.ready,
                        c.len(),
                        c.iter().map(|x| u64::from(x.unread_count)).sum::<u64>(),
                    )
                });
                let p = live.core.power_gate().status();
                format!(
                    "ready: {ready}\nconversations: {convs}\nunread: {unread}\npower: {}\nwaiting for better power conditions: {}",
                    p.headline, p.waiting
                )
            }
            "conversations" => hub.read(|s| {
                s.conversations
                    .ordered()
                    .iter()
                    .map(|c| {
                        let kind = match c.kind {
                            siar_ui_state::ConversationKind::Direct => "direct",
                            siar_ui_state::ConversationKind::Group => "group",
                        };
                        format!("{}\t{}\t{}\t{}", c.id, kind, c.display_name, c.unread_count)
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            }),
            // `<device-id> <account-id>`
            "identity" => hub.read(|s| match (s.identity.device, s.identity.account) {
                (Some(d), Some(a)) => Ok(format!("{d} {a}")),
                _ => Err(RuntimeError::NotReady),
            })?,
            // `<device-id> <account-id> <base64 key package>` — what a group
            // admin needs in order to add this device.
            "key-package" => hub.read(|s| match (s.identity.device, s.identity.account) {
                (Some(d), Some(a)) if !s.identity.key_package.is_empty() => {
                    Ok(format!("{d} {a} {}", s.identity.key_package))
                }
                _ => Err(RuntimeError::NotReady),
            })?,
            // `<device-id>\t<name>\t<fingerprint>` per waiting link request.
            "link-pending" => live
                .core
                .link_pending()
                .iter()
                .map(|p| format!("{}\t{}\t{}", p.device_id, p.name, p.fingerprint))
                .collect::<Vec<_>>()
                .join("\n"),
            // `restart-needed` once this device has joined an account.
            "link-state" => {
                if live.core.link_restart_needed() {
                    "restart-needed".to_owned()
                } else {
                    "ok".to_owned()
                }
            }
            // `<device-id>\t<name>\t<this|linked>`
            "devices" => hub.read(|s| {
                s.devices
                    .all()
                    .iter()
                    .map(|d| {
                        format!(
                            "{}\t{}\t{}",
                            d.id,
                            d.display_name,
                            if d.current_device { "this" } else { "linked" }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            }),
            "groups" => hub.read(|s| {
                s.groups
                    .ordered()
                    .iter()
                    .map(|g| {
                        format!(
                            "{}\t{}\t{}\t{}",
                            g.conversation_id,
                            g.member_count,
                            g.epoch,
                            if g.is_admin { "admin" } else { "member" }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            }),
            "messages" => {
                let id = conversation(&utf8(data)?)?;
                // Opening loads the newest page of stored history.
                if live
                    .commands
                    .try_send(AppCommand::OpenConversation(id))
                    .is_err()
                {
                    return Err(RuntimeError::Busy);
                }
                for _ in 0..40 {
                    if hub.read(|s| s.history.get(&id).is_some_and(|h| h.loaded)) {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                hub.read(|s| {
                    let Some(t) = s.timelines.get(&id) else {
                        return String::new();
                    };
                    let all: Vec<_> = t.visible().collect();
                    let skip = all.len().saturating_sub(20);
                    all.iter()
                        .skip(skip)
                        .map(|e| {
                            let who = if e.from_me { "me  " } else { "them" };
                            match &e.content {
                                MessageContent::Text(t) => format!("{who}  {}", t.as_str()),
                                MessageContent::Attachment(r) => format!(
                                    "{who}  [attachment: {:?}, {} bytes]",
                                    r.media_type,
                                    r.encrypted_size.bytes()
                                ),
                            }
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                })
            }
            _ => return Err(RuntimeError::Unsupported),
        };
        Ok(out.into_bytes())
    }

    fn run_command(&self, name: &str, data: Vec<u8>) -> Result<Vec<u8>, RuntimeError> {
        let live = self.live()?;
        if name == "ui-command" {
            // Window commands are validated again inside; see siar-ui-sync.
            return live.sync.command(&data).map(|()| Vec::new());
        }
        let cmd = match name {
            "pair" => {
                let ticket_text = utf8(data)?.trim().to_owned();
                // The conversation id is derived from the ticket, so it can
                // be returned now; the command channel is FIFO, so a `send`
                // issued next runs after the pairing has been applied.
                let ticket = siar_messaging::PeerTicket::decode(&ticket_text)
                    .map_err(|e| RuntimeError::InvalidCommand(e.to_string()))?;
                let id = siar_app_core::conversation_for(&ticket);
                live.commands
                    .try_send(AppCommand::PairWithTicket { ticket_text })
                    .map_err(|_| RuntimeError::Busy)?;
                return Ok(id.to_string().into_bytes());
            }
            "group-create" => {
                let founder = live
                    .core
                    .state()
                    .read(|s| s.identity.account)
                    .ok_or(RuntimeError::NotReady)?;
                AppCommand::CreateGroup { founder }
            }
            // `<conversation-id> <peer-ticket> <peer-device-id> <peer-account-id> <base64-key-package>`
            "group-add" => {
                let text = utf8(data)?;
                let p: Vec<&str> = text.split_whitespace().collect();
                let [conv, ticket, device, account, kp] = p.as_slice() else {
                    return Err(RuntimeError::InvalidCommand(
                        "expected: <conversation-id> <peer-ticket> <peer-device-id> <peer-account-id> <key-package>".into(),
                    ));
                };
                let parse = |t: &str| {
                    uuid::Uuid::parse_str(t)
                        .map_err(|_| RuntimeError::InvalidCommand("not a valid id".into()))
                };
                let key_package_bytes = {
                    use base64::Engine as _;
                    base64::engine::general_purpose::STANDARD
                        .decode(kp)
                        .map_err(|_| {
                            RuntimeError::InvalidCommand("key package is not base64".into())
                        })?
                };
                AppCommand::AddGroupMember {
                    conversation: conversation(conv)?,
                    new_member: siar_domain::AccountId::from_uuid(parse(account)?),
                    input: siar_ui_state::AddMemberInput {
                        peer_account: siar_domain::AccountId::from_uuid(parse(account)?),
                        peer_device: siar_domain::DeviceId::from_uuid(parse(device)?),
                        peer_ticket_text: (*ticket).to_owned(),
                        key_package_bytes,
                    },
                }
            }
            // `<conversation-id> <absolute path>`; the path may contain spaces.
            "send-file" => {
                let text = utf8(data)?;
                let (id, path) = text.split_once(' ').ok_or_else(|| {
                    RuntimeError::InvalidCommand("expected: <conversation-id> <path>".into())
                })?;
                AppCommand::SendAttachment {
                    conversation: conversation(id)?,
                    path: path.to_owned(),
                }
            }
            // `<conversation-id> <relay-ticket> <text>`
            "send-anon" => {
                let text = utf8(data)?;
                let mut it = text.splitn(3, ' ');
                let (Some(id), Some(relay), Some(body)) = (it.next(), it.next(), it.next()) else {
                    return Err(RuntimeError::InvalidCommand(
                        "expected: <conversation-id> <relay-ticket> <text>".into(),
                    ));
                };
                relay_ticket(relay)?;
                AppCommand::SendViaRelay {
                    conversation: conversation(id)?,
                    relay_ticket_text: relay.to_owned(),
                    text: siar_domain::MessageText::parse(body.to_owned())
                        .map_err(|e| RuntimeError::InvalidCommand(e.to_string()))?,
                }
            }
            // `<conversation-id> <relay-ticket>`
            "check-mailbox-anon" => {
                let text = utf8(data)?;
                let Some((id, relay)) = text.split_once(' ') else {
                    return Err(RuntimeError::InvalidCommand(
                        "expected: <conversation-id> <relay-ticket>".into(),
                    ));
                };
                relay_ticket(relay.trim())?;
                AppCommand::CheckRelayMailbox {
                    conversation: conversation(id)?,
                    relay_ticket_text: relay.trim().to_owned(),
                }
            }
            "group-send" => {
                let text = utf8(data)?;
                let (id, body) = text.split_once(' ').ok_or_else(|| {
                    RuntimeError::InvalidCommand("expected: <conversation-id> <text>".into())
                })?;
                AppCommand::SendGroupMessage {
                    conversation: conversation(id)?,
                    text: siar_domain::MessageText::parse(body.to_owned())
                        .map_err(|e| RuntimeError::InvalidCommand(e.to_string()))?,
                }
            }
            "send" => {
                let text = utf8(data)?;
                let (id, body) = text.split_once(' ').ok_or_else(|| {
                    RuntimeError::InvalidCommand("expected: <conversation-id> <text>".into())
                })?;
                AppCommand::SendMessage {
                    conversation: conversation(id)?,
                    text: siar_domain::MessageText::parse(body.to_owned())
                        .map_err(|e| RuntimeError::InvalidCommand(e.to_string()))?,
                }
            }
            _ => return Err(RuntimeError::Unsupported),
        };
        live.commands
            .try_send(cmd)
            .map_err(|_| RuntimeError::Busy)?;
        Ok(Vec::new())
    }
}

impl DaemonApp {
    /// Device linking. `link-offer` returns `<code>\n<ticket>`; the rest
    /// return a short word. Failures come back as readable errors.
    async fn link_command(&self, name: &str, data: Vec<u8>) -> Result<Vec<u8>, RuntimeError> {
        let live = self.live.get().ok_or(RuntimeError::NotReady)?;
        let text = utf8(data)?;
        let bad = RuntimeError::InvalidCommand;
        let device = |t: &str| {
            uuid::Uuid::parse_str(t.trim())
                .map(siar_domain::DeviceId::from_uuid)
                .map_err(|_| RuntimeError::InvalidCommand("not a valid device id".into()))
        };
        match name {
            "link-offer" => {
                let o = live.core.link_open_offer().map_err(bad)?;
                Ok(format!("{}\n{}", o.code, o.ticket).into_bytes())
            }
            // `<device-id> [share]` — `share` also gives the new device this
            // device's contacts.
            "link-approve" => {
                let mut parts = text.split_whitespace();
                let id = parts.next().unwrap_or_default();
                let share = parts.next() == Some("share");
                live.core
                    .link_approve(device(id)?, share)
                    .await
                    .map_err(bad)?;
                Ok(b"approved".to_vec())
            }
            "link-revoke" => {
                live.core.link_revoke(device(&text)?).await.map_err(bad)?;
                Ok(b"removed".to_vec())
            }
            "link-reject" => {
                let found = live.core.link_reject(device(&text)?).await;
                Ok(if found {
                    b"declined".to_vec()
                } else {
                    b"not found".to_vec()
                })
            }
            // `<ticket> <code>`
            "link-join" => {
                let (ticket, code) = text.split_once(' ').ok_or_else(|| {
                    RuntimeError::InvalidCommand("expected: <ticket> <code>".into())
                })?;
                live.core.link_join(ticket, code).await.map_err(bad)?;
                Ok(b"requested".to_vec())
            }
            _ => Err(RuntimeError::InvalidCommand("unknown link command".into())),
        }
    }
}

impl AppService for DaemonApp {
    fn command<'a>(
        &'a self,
        name: &'a str,
        data: Vec<u8>,
    ) -> BoxFuture<'a, Result<Vec<u8>, RuntimeError>> {
        Box::pin(async move {
            if name == "pairwise-open" {
                return self.pairwise_open(data).await;
            }
            if name.starts_with("link-") {
                return self.link_command(name, data).await;
            }
            self.run_command(name, data)
        })
    }

    fn query<'a>(
        &'a self,
        name: &'a str,
        data: Vec<u8>,
    ) -> BoxFuture<'a, Result<Vec<u8>, RuntimeError>> {
        Box::pin(self.run_query(name, data))
    }
}

/// A relay ticket is checked when the command arrives, so a typo is an
/// immediate error for the person who made it, not a silent no-op later.
fn relay_ticket(text: &str) -> Result<(), RuntimeError> {
    siar_messaging::PeerTicket::decode(text)
        .map(drop)
        .map_err(|_| RuntimeError::InvalidCommand("that relay ticket isn't valid".into()))
}

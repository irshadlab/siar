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

    async fn run_query(&self, name: &str, data: Vec<u8>) -> Result<Vec<u8>, RuntimeError> {
        let live = self.live()?;
        let hub = live.core.state();
        let out = match name {
            "ticket" => hub.read(|s| s.identity.ticket.clone()),
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
                    .map(|c| format!("{}\t{}\t{}", c.id, c.display_name, c.unread_count))
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
                                MessageContent::Attachment(_) => format!("{who}  [attachment]"),
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
        let cmd = match name {
            "pair" => AppCommand::PairWithTicket {
                ticket_text: utf8(data)?.trim().to_owned(),
            },
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

impl AppService for DaemonApp {
    fn command<'a>(
        &'a self,
        name: &'a str,
        data: Vec<u8>,
    ) -> BoxFuture<'a, Result<Vec<u8>, RuntimeError>> {
        Box::pin(async move { self.run_command(name, data) })
    }

    fn query<'a>(
        &'a self,
        name: &'a str,
        data: Vec<u8>,
    ) -> BoxFuture<'a, Result<Vec<u8>, RuntimeError>> {
        Box::pin(self.run_query(name, data))
    }
}

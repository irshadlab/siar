//! The background half of the desktop app: owns `MessageService` and
//! `GroupService`, drains `AppCommand`s from the UI and pushes results
//! back through `UiHub`.
//!
//! Same architecture as the Dioxus shell (plan.md §52–55), with the toolkit
//! swapped underneath: `signal.write()` became `hub.update(..)`, and the
//! single global `ActivePeer` became a per-conversation `PeerRegistry`
//! (see `peers.rs`). Failures are now also *shown* (a toast through
//! `hub.notify`), not only logged.

use std::collections::HashSet;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use siar_domain::{AccountId, ConversationId, DeliveryState, MessageContent, MessageId};
use siar_messaging::{
    GroupService, InMemoryDeviceDirectory, IncomingEvent, MemberDevice, MessageService, PeerTicket,
};
use siar_protocol::v1::EnvelopeKind;
use siar_protocol::WireMessage;
use siar_storage::{ContactRepository, MessageRepository};
use siar_ui_slint::{NoticeLevel, UiHub};
use siar_ui_state::{
    AppCommand, ConversationKind, ConversationSummary, GroupSummary, NetworkState, PendingInvite,
    SavedContact,
};
use tokio::sync::mpsc;

use crate::bootstrap::Bootstrapped;
use crate::peers::PeerRegistry;

pub struct Core {
    service: Arc<MessageService>,
    group_service: Arc<GroupService>,
    device_directory: Arc<InMemoryDeviceDirectory>,
    contact_repo: Arc<dyn ContactRepository + Send + Sync>,
    messages: Arc<dyn MessageRepository + Send + Sync>,
    local_account: AccountId,
    hub: UiHub,
    peers: Mutex<PeerRegistry>,
    /// Senders we already told the user about, so a chatty unknown device
    /// produces one notice, not a toast storm.
    warned_unknown: Mutex<HashSet<iroh::EndpointId>>,
}

/// Seeds the UI from the freshly booted core and starts the background
/// tasks. Returns when the command channel closes (app shutdown).
pub async fn run(boot: Bootstrapped, hub: UiHub, commands: mpsc::Receiver<AppCommand>) {
    let Bootstrapped {
        service,
        group_service,
        my_ticket,
        incoming_rx,
        local_account,
        device_id,
        key_package_b64,
        device_directory,
        contact_repo,
        saved_contacts,
        messages,
        ..
    } = boot;

    let core = Arc::new(Core {
        service,
        group_service,
        device_directory,
        contact_repo,
        messages,
        local_account,
        hub: hub.clone(),
        peers: Mutex::new(PeerRegistry::new()),
        warned_unknown: Mutex::new(HashSet::new()),
    });

    // ---- seed the UI -----------------------------------------------------------
    // Saved contacts become conversations right away, so a restart brings
    // back the chat list instead of an empty window.
    let mut seeded = Vec::new();
    for c in &saved_contacts {
        match PeerTicket::decode(&c.ticket_text) {
            Ok(ticket) => {
                let conversation = core.peers().register(ticket);
                seeded.push((conversation, c.display_name.clone()));
            }
            Err(e) => tracing::warn!(error = %e, "saved contact has an unparseable ticket"),
        }
    }
    hub.update(|s| {
        s.identity.account = Some(local_account);
        s.identity.device = Some(device_id);
        s.identity.ticket = my_ticket.encode();
        s.identity.key_package = key_package_b64.unwrap_or_default();
        s.contacts.load(
            saved_contacts
                .iter()
                .map(|c| SavedContact {
                    device_id: c.device_id,
                    account_id: c.account_id,
                    display_name: c.display_name.clone(),
                    ticket_text: c.ticket_text.clone(),
                    key_package_b64: c.key_package_b64.clone(),
                })
                .collect(),
        );
        for (conversation, name) in seeded {
            s.ensure_conversation(conversation, name, ConversationKind::Direct);
            s.sendable.insert(conversation);
        }
        // The transport endpoint is bound. This does not prove internet
        // reachability (relay/hole-punching can still fail), so the label
        // stays the generic "Online"; real reachability probing is future work.
        s.network.set_overall(NetworkState::Online);
        s.ready = true;
    });

    tokio::spawn(retry_scheduler(core.service.clone()));
    tokio::spawn(delivery_poller(core.clone()));
    tokio::spawn(incoming_loop(core.clone(), incoming_rx));
    command_loop(core, commands).await;
}

impl Core {
    fn peers(&self) -> std::sync::MutexGuard<'_, PeerRegistry> {
        self.peers.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn fail(&self, what: &str, e: impl std::fmt::Display) {
        tracing::warn!(error = %e, "{what}");
        self.hub.notify(
            NoticeLevel::Error,
            format!("{what}. Try again, and check the details in the log if it keeps happening."),
        );
    }

    /// Opens (creating if needed) the 1:1 conversation with `ticket`,
    /// makes it sendable and selects it.
    fn open_direct(&self, ticket: PeerTicket, name: Option<String>) {
        let conversation = self.peers().register(ticket.clone());
        let title = name.unwrap_or_else(|| ticket.endpoint_addr.id.fmt_short().to_string());
        self.hub.update(|s| {
            s.ensure_conversation(conversation, title, ConversationKind::Direct);
            s.sendable.insert(conversation);
            s.select(conversation);
        });
    }

    fn rename_conversation(&self, conversation: ConversationId, name: &str) {
        self.hub.update(|s| {
            let existing = s
                .conversations
                .ordered()
                .iter()
                .find(|c| c.id == conversation)
                .cloned();
            if let Some(c) = existing {
                if c.display_name != name {
                    s.conversations.upsert(ConversationSummary {
                        display_name: name.to_string(),
                        ..c
                    });
                }
            }
        });
    }

    fn upsert_group_summary(&self, g: &siar_domain::GroupState) {
        self.hub.update(|s| {
            s.groups.upsert(GroupSummary {
                conversation_id: g.conversation_id,
                display_label: format!("Group {}", g.conversation_id.fmt_short()),
                member_count: g.members().len(),
                is_admin: g.is_admin(self.local_account),
                epoch: g.epoch.number(),
            });
            s.ensure_conversation(
                g.conversation_id,
                format!("Group {}", g.conversation_id.fmt_short()),
                ConversationKind::Group,
            );
        });
    }
}

/// plan.md §33's retry scheduler — identical cadence to `apps/cli`.
async fn retry_scheduler(service: Arc<MessageService>) {
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    loop {
        interval.tick().await;
        if let Err(e) = service.retry_due().await {
            tracing::warn!(error = %e, "retry_due failed");
        }
    }
}

fn is_terminal(state: DeliveryState) -> bool {
    matches!(
        state,
        DeliveryState::Delivered
            | DeliveryState::Read
            | DeliveryState::Failed
            | DeliveryState::Expired
    )
}

/// Mirrors stored delivery state (Sent → Delivered on ACK, Failed…) into
/// the visible timelines. Before this, every sent message stayed on its
/// initial local state forever, because nothing read the repository back.
/// Only still-pending outgoing messages are polled, so the cost stays
/// proportional to what is actually in flight.
async fn delivery_poller(core: Arc<Core>) {
    let mut interval = tokio::time::interval(Duration::from_millis(1500));
    loop {
        interval.tick().await;
        let pending: Vec<(ConversationId, MessageId, DeliveryState)> = core.hub.read(|s| {
            s.timelines
                .iter()
                .flat_map(|(c, t)| {
                    t.visible()
                        .filter(|e| e.from_me && !is_terminal(e.delivery_state))
                        .map(move |e| (*c, e.message_id, e.delivery_state))
                })
                .collect()
        });
        if pending.is_empty() {
            continue;
        }
        let repo = core.messages.clone();
        let changed = tokio::task::spawn_blocking(move || {
            pending
                .into_iter()
                .filter_map(|(c, id, current)| match repo.get(id) {
                    Ok(Some(m)) if m.delivery_state != current => Some((c, id, m.delivery_state)),
                    _ => None,
                })
                .collect::<Vec<_>>()
        })
        .await
        .unwrap_or_default();
        if !changed.is_empty() {
            core.hub.update(|s| {
                for (c, id, state) in changed {
                    s.timeline_mut(c).update_delivery_state(id, state);
                }
            });
        }
    }
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn base64_decode(s: &str) -> Result<Vec<u8>, String> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(s.trim())
        .map_err(|e| e.to_string())
}

async fn command_loop(core: Arc<Core>, mut commands: mpsc::Receiver<AppCommand>) {
    while let Some(command) = commands.recv().await {
        match command {
            AppCommand::PairWithTicket { ticket_text } => match PeerTicket::decode(&ticket_text) {
                Ok(ticket) => {
                    // A pasted ticket of someone already saved keeps their name.
                    let name = core.hub.read(|s| {
                        s.contacts
                            .ordered()
                            .iter()
                            .find(|c| c.ticket_text.trim() == ticket_text.trim())
                            .map(|c| c.display_name.clone())
                    });
                    core.open_direct(ticket, name);
                    core.hub.notify(NoticeLevel::Success, "Paired — say hello.");
                }
                Err(e) => core.fail("That ticket couldn’t be read", e),
            },
            AppCommand::UseContact { device_id } => {
                let contact = core.hub.read(|s| {
                    s.contacts
                        .ordered()
                        .iter()
                        .find(|c| c.device_id == device_id)
                        .cloned()
                });
                match contact {
                    Some(c) => match PeerTicket::decode(&c.ticket_text) {
                        Ok(ticket) => {
                            core.open_direct(ticket, Some(c.display_name.clone()));
                            core.hub.update(|s| s.open_content = true);
                        }
                        Err(e) => core.fail("That contact’s ticket couldn’t be read", e),
                    },
                    None => tracing::warn!(?device_id, "UseContact for an unknown contact"),
                }
            }
            AppCommand::SendMessage { conversation, text } => {
                let Some(peer) = core.peers().ticket(conversation).cloned() else {
                    core.hub.notify(
                        NoticeLevel::Warning,
                        "This conversation has no paired peer yet.",
                    );
                    continue;
                };
                match core
                    .service
                    .send_text(conversation, &peer, text.clone())
                    .await
                {
                    Ok(message_id) => core.hub.update(|s| {
                        s.record_outgoing(conversation, message_id, MessageContent::Text(text));
                    }),
                    Err(e) => core.fail("Your message couldn’t be sent", e),
                }
            }
            AppCommand::OpenConversation(conversation) => {
                core.hub.update(|s| s.conversations.mark_read(conversation));
            }
            AppCommand::MarkRead { conversation, .. } => {
                core.hub.update(|s| s.conversations.mark_read(conversation));
            }
            AppCommand::CreateGroup { founder } => {
                match core
                    .group_service
                    .create_group_mls(ConversationId::new(), founder)
                {
                    Ok(group_state) => {
                        core.upsert_group_summary(&group_state);
                        core.hub.update(|s| {
                            s.selected_group = Some(group_state.conversation_id);
                            s.open_content = true;
                        });
                        core.hub
                            .notify(NoticeLevel::Success, "Group created. Add members below.");
                    }
                    Err(e) => core.fail("The group couldn’t be created", e),
                }
            }
            AppCommand::AddGroupMember {
                conversation,
                new_member,
                input,
            } => {
                let peer_ticket = match PeerTicket::decode(&input.peer_ticket_text) {
                    Ok(t) => t,
                    Err(e) => {
                        core.fail("That ticket couldn’t be read", e);
                        continue;
                    }
                };
                core.device_directory.register(
                    new_member,
                    MemberDevice {
                        device_id: input.peer_device,
                        ticket: peer_ticket,
                    },
                );
                match core
                    .group_service
                    .add_member_mls(
                        conversation,
                        new_member,
                        input.peer_device,
                        &input.key_package_bytes,
                    )
                    .await
                {
                    Ok(()) => {
                        match core.group_service.group_state(conversation) {
                            Ok(Some(g)) => core.upsert_group_summary(&g),
                            Ok(None) => tracing::warn!(
                                "add_member_mls succeeded but group_state is empty — bug"
                            ),
                            Err(e) => tracing::warn!(error = %e, "couldn’t refresh group state"),
                        }
                        core.hub.notify(NoticeLevel::Success, "Member added.");
                    }
                    Err(e) => core.fail("The member couldn’t be added", e),
                }
            }
            AppCommand::SendGroupMessage { conversation, text } => {
                match core
                    .group_service
                    .send_text_mls(conversation, text.clone())
                    .await
                {
                    Ok(message_id) => core.hub.update(|s| {
                        s.record_outgoing(conversation, message_id, MessageContent::Text(text));
                    }),
                    Err(e) => core.fail("Your group message couldn’t be sent", e),
                }
            }
            AppCommand::AcceptGroupInvite { conversation } => {
                let Some(invite) = core.hub.update(|s| s.pending_invites.remove(conversation))
                else {
                    tracing::warn!(?conversation, "AcceptGroupInvite with no pending invite");
                    continue;
                };
                let parsed: Result<siar_domain::GroupState, String> =
                    base64_decode(&invite.state_input)
                        .and_then(|b| postcard::from_bytes(&b).map_err(|e| e.to_string()));
                let group_state = match parsed {
                    Ok(g) => g,
                    Err(e) => {
                        // Put the invite back so the user can fix the paste and retry.
                        core.hub.update(|s| s.pending_invites.add(invite));
                        core.fail(
                            "That group state couldn’t be read — copy it again in full",
                            e,
                        );
                        continue;
                    }
                };
                match core.group_service.join_group_mls(
                    conversation,
                    &invite.welcome_bytes,
                    group_state.clone(),
                ) {
                    Ok(()) => {
                        core.upsert_group_summary(&group_state);
                        core.hub
                            .notify(NoticeLevel::Success, "You joined the group.");
                    }
                    Err(e) => core.fail("Joining the group failed", e),
                }
            }
            AppCommand::DeclineGroupInvite { conversation } => {
                core.hub.update(|s| {
                    s.pending_invites.remove(conversation);
                });
            }
            AppCommand::SaveContact {
                device_id,
                account_id,
                display_name,
                ticket_text,
                key_package_b64,
            } => {
                // Validate before persisting: an undecodable saved ticket would
                // silently fail every future start-up registration.
                let ticket = match PeerTicket::decode(&ticket_text) {
                    Ok(t) => t,
                    Err(e) => {
                        core.fail("That ticket couldn’t be read", e);
                        continue;
                    }
                };
                let stored = siar_storage::StoredContact {
                    device_id,
                    account_id,
                    display_name: display_name.clone(),
                    ticket_text: ticket_text.clone(),
                    key_package_b64: key_package_b64.clone(),
                    added_at_millis: now_millis(),
                };
                match core.contact_repo.upsert(&stored) {
                    Ok(()) => {
                        core.device_directory.register(
                            account_id,
                            MemberDevice {
                                device_id,
                                ticket: ticket.clone(),
                            },
                        );
                        let conversation = core.peers().register(ticket);
                        core.hub.update(|s| {
                            s.contacts.upsert(SavedContact {
                                device_id,
                                account_id,
                                display_name: display_name.clone(),
                                ticket_text,
                                key_package_b64,
                            });
                            s.ensure_conversation(
                                conversation,
                                display_name.clone(),
                                ConversationKind::Direct,
                            );
                            s.sendable.insert(conversation);
                            s.selected_contact = Some(device_id);
                        });
                        core.rename_conversation(conversation, &display_name);
                        core.hub
                            .notify(NoticeLevel::Success, format!("Saved {display_name}."));
                    }
                    Err(e) => core.fail("The contact couldn’t be saved", e),
                }
            }
            AppCommand::RemoveContact { device_id } => match core.contact_repo.remove(device_id) {
                Ok(()) => core.hub.update(|s| s.contacts.remove(device_id)),
                Err(e) => core.fail("The contact couldn’t be removed", e),
            },
            AppCommand::LoadAttachmentPreview {
                message_id,
                reference,
            } => {
                // The attachment belongs to the conversation on screen.
                let peer = core
                    .hub
                    .read(|s| s.selected)
                    .and_then(|c| core.peers().ticket(c).cloned());
                let Some(peer) = peer else {
                    core.hub.update(|s| {
                        s.attachment_previews
                            .set_failed(message_id, "no paired peer to fetch it from".into())
                    });
                    continue;
                };
                // Spawned: fetch may hit the network (plan.md §22) and must
                // not stall sends queued behind it.
                let core = core.clone();
                tokio::spawn(async move {
                    load_attachment_preview(core, peer, message_id, reference).await
                });
            }
        }
    }
}

/// fetch → decrypt → decode → downscale for one image attachment.
async fn load_attachment_preview(
    core: Arc<Core>,
    peer: PeerTicket,
    message_id: MessageId,
    reference: siar_domain::AttachmentReference,
) {
    let fail = |why: String| {
        core.hub
            .update(|s| s.attachment_previews.set_failed(message_id, why));
    };
    let plaintext = match core.service.fetch_attachment(&peer, &reference).await {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!(error = %e, "attachment fetch failed");
            return fail("the sender couldn’t be reached".into());
        }
    };
    // CPU-bound and I/O-free (see siar-media-image): keep it off the
    // async executor threads.
    let result = tokio::task::spawn_blocking(move || {
        let decoded = siar_media_image::decode_image(&plaintext)?;
        siar_media_image::generate_preview(&decoded)
    })
    .await;
    match result {
        Ok(Ok(encoded)) => core.hub.update(|s| {
            s.attachment_previews.set_ready(
                message_id,
                encoded.jpeg_bytes,
                encoded.width,
                encoded.height,
            );
        }),
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "attachment decode failed");
            fail("the image couldn’t be decoded".into());
        }
        Err(e) => {
            tracing::error!(error = %e, "decode task panicked");
            fail("the image couldn’t be decoded".into());
        }
    }
}

/// plan.md §112's receive flow, feeding the UI hub.
async fn incoming_loop(
    core: Arc<Core>,
    mut incoming: mpsc::Receiver<siar_transport::IncomingFrame>,
) {
    while let Some(frame) = incoming.recv().await {
        let envelope = match frame.message {
            WireMessage::V1(envelope) => envelope,
            // Relay-only traffic: not an ordinary chat client's job.
            WireMessage::Mesh(_)
            | WireMessage::MailboxCheckIn(_)
            | WireMessage::TokenMailboxDeposit(_)
            | WireMessage::AnonymousMailboxCheckIn(_)
            | WireMessage::RouteAdvertisement(_) => {
                tracing::warn!(from = ?frame.from, "dropping a relay-only frame — this app isn't a relay");
                continue;
            }
        };

        if matches!(
            envelope.kind,
            EnvelopeKind::GroupEvent
                | EnvelopeKind::GroupMlsCommit
                | EnvelopeKind::GroupMlsWelcome
                | EnvelopeKind::GroupMlsApplication
        ) {
            match envelope.kind {
                EnvelopeKind::GroupMlsWelcome => {
                    core.hub.update(|s| {
                        s.pending_invites.add(PendingInvite {
                            conversation_id: envelope.conversation_id,
                            from_device: envelope.sender,
                            welcome_bytes: envelope.payload,
                            state_input: String::new(),
                        });
                    });
                    core.hub.notify(
                        NoticeLevel::Info,
                        "You were invited to a group — see Groups.",
                    );
                }
                _ => match core
                    .group_service
                    .handle_incoming_mls(envelope.conversation_id, &envelope)
                {
                    Ok(Some(content)) => core.hub.update(|s| {
                        s.record_incoming(
                            envelope.conversation_id,
                            envelope.message_id,
                            envelope.sequence,
                            content,
                        );
                    }),
                    Ok(None) => {} // a commit merged, or nothing to show
                    Err(e) => tracing::warn!(error = %e, "failed to handle incoming group frame"),
                },
            }
            continue;
        }

        // 1:1: route by WHO SENT IT, not by whoever is selected.
        let Some(peer) = core.peers().ticket_of_endpoint(&frame.from).cloned() else {
            let first_time = core
                .warned_unknown
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(frame.from);
            tracing::warn!(from = ?frame.from, "dropping a frame from an unpaired device");
            if first_time {
                core.hub.notify(
                    NoticeLevel::Warning,
                    "A device you haven’t paired with tried to message you. It was ignored.",
                );
            }
            continue;
        };
        let (message_id, sequence) = (envelope.message_id, envelope.sequence);
        match core.service.handle_incoming(&peer, envelope).await {
            Ok(Some(IncomingEvent::Content(content))) => {
                let conversation = crate::peers::conversation_for(&peer);
                let title = frame.from.fmt_short().to_string();
                core.hub.update(|s| {
                    s.ensure_conversation(conversation, title, ConversationKind::Direct);
                    s.sendable.insert(conversation);
                    s.record_incoming(conversation, message_id, sequence, content);
                });
            }
            Ok(_) => {} // ACKs and call signals are not timeline content
            Err(e) => tracing::warn!(error = %e, "failed to handle incoming frame"),
        }
    }
}

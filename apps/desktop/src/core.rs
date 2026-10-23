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

use siar_domain::DeviceId;
use siar_domain::{AccountId, ConversationId, DeliveryState, MessageContent, MessageId};
use siar_messaging::{
    GroupService, InMemoryDeviceDirectory, IncomingEvent, MemberDevice, MessageService, PeerTicket,
};
use siar_protocol::v1::EnvelopeKind;
use siar_protocol::WireMessage;
use siar_storage::{ContactRepository, MessageRepository};
use siar_ui_slint::{ContactSafety, NoticeLevel, UiHub, VerificationState};
use siar_ui_state::{
    AppCommand, ConversationKind, ConversationSummary, GroupSummary, IdentityKeyHealth,
    NetworkState, OwnIdentityView, PendingInvite, SavedContact, SecurityDiagnosticDetail,
    SecurityEventAction, SecurityEventKind, PAGE_SIZE,
};
use tokio::sync::mpsc;

use crate::bootstrap::Bootstrapped;
use crate::peers::PeerRegistry;
use crate::read_marks::{newest_incoming, unread_above, ReadMarks};
use crate::verification::{key_fingerprint, safety_number, VerificationStore};

pub struct Core {
    service: Arc<MessageService>,
    group_service: Arc<GroupService>,
    device_directory: Arc<InMemoryDeviceDirectory>,
    contact_repo: Arc<dyn ContactRepository + Send + Sync>,
    messages: Arc<dyn MessageRepository + Send + Sync>,
    local_account: AccountId,
    hub: UiHub,
    peers: Mutex<PeerRegistry>,
    /// This device's signing public key (half of every safety number).
    local_key: [u8; 32],
    my_device: siar_domain::DeviceId,
    read_marks: Mutex<ReadMarks>,
    /// Where every peer files its messages TO us (see `history.rs`).
    inbound_conversation: ConversationId,
    verification: Mutex<VerificationStore>,
    /// Senders we already told the user about, so a chatty unknown device
    /// produces one notice, not a toast storm.
    warned_unknown: Mutex<HashSet<iroh::EndpointId>>,
}

/// Boots the core into the UI and runs until the command channel closes
/// (app shutdown).
pub async fn run(boot: Bootstrapped, hub: UiHub, commands: mpsc::Receiver<AppCommand>) {
    let core = start(boot, hub).await;
    command_loop(core, commands).await;
}

/// Seeds the UI from the freshly booted core and starts the background
/// tasks. Split from `run` so tests can keep the `Core` and drive its
/// internals (e.g. a simulated restart) directly.
pub(crate) async fn start(boot: Bootstrapped, hub: UiHub) -> Arc<Core> {
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
        data_dir,
        identity_created_at_millis,
        ..
    } = boot;
    let local_key = my_ticket.ed25519_verifying;

    let core = Arc::new(Core {
        service,
        group_service,
        device_directory,
        contact_repo,
        messages,
        local_account,
        hub: hub.clone(),
        peers: Mutex::new(PeerRegistry::new()),
        local_key,
        my_device: device_id,
        read_marks: Mutex::new(ReadMarks::load(&data_dir.join("read-marks.txt"))),
        inbound_conversation: crate::peers::conversation_for(&my_ticket),
        verification: Mutex::new(VerificationStore::load(
            &data_dir.join("verified-contacts.txt"),
        )),
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
    let (short_fp, full_fp) = key_fingerprint(&local_key);
    let seeded_safety: Vec<_> = saved_contacts
        .iter()
        .filter_map(|c| {
            let ticket = PeerTicket::decode(&c.ticket_text).ok()?;
            Some((
                c.device_id,
                c.account_id,
                core.safety_for(c.device_id, &ticket),
            ))
        })
        .collect();
    hub.update(|s| {
        s.own_identity = Some(OwnIdentityView {
            account: local_account,
            short_fingerprint: short_fp.clone(),
            full_fingerprint: full_fp,
            created_at_millis: identity_created_at_millis,
            key_health: IdentityKeyHealth::Healthy,
        });
        s.diagnostics = Some(SecurityDiagnosticDetail {
            key_id: short_fp,
            epoch_id: String::new(), // per-group, derived by the UI from the group list
            signature_status: "Messages are signed with this device’s Ed25519 key.".into(),
            transport_security_details: format!(
                "QUIC with TLS 1.3 (iroh) · endpoint {}",
                my_ticket.endpoint_addr.id.fmt_short()
            ),
        });
        for (device, account, safety) in seeded_safety {
            core.apply_safety(s, device, account, safety);
        }
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

    // Bring back each saved contact's recent messages (and the chat-list
    // previews) after a restart, without delaying the window.
    let preload = core.clone();
    tokio::spawn(async move { preload.preload_history().await });

    tokio::spawn(retry_scheduler(core.service.clone()));
    tokio::spawn(delivery_poller(core.clone()));
    tokio::spawn(incoming_loop(core.clone(), incoming_rx));
    core
}

impl Core {
    fn peers(&self) -> std::sync::MutexGuard<'_, PeerRegistry> {
        self.peers.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn verification(&self) -> std::sync::MutexGuard<'_, VerificationStore> {
        self.verification
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Safety number + current verification state for one contact.
    fn safety_for(&self, device: DeviceId, ticket: &PeerTicket) -> ContactSafety {
        ContactSafety {
            number: safety_number(&self.local_key, &ticket.ed25519_verifying),
            state: self.verification().state(device, &ticket.ed25519_verifying),
        }
    }

    /// Stores `safety` for the contact and, when a previously verified key
    /// has changed, raises a security event ONCE per transition into that
    /// state (not on every refresh).
    fn apply_safety(
        &self,
        s: &mut siar_ui_slint::Slices,
        device: DeviceId,
        account: AccountId,
        safety: ContactSafety,
    ) {
        let was_changed = s
            .contact_safety
            .get(&device)
            .is_some_and(|c| c.state == VerificationState::KeyChanged);
        if safety.state == VerificationState::KeyChanged && !was_changed {
            s.security_events.push(
                SecurityEventKind::IdentityChanged,
                now_millis(),
                None,
                Some(account),
                vec![SecurityEventAction::Review],
            );
        }
        s.contact_safety.insert(device, safety);
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

    /// Loads one page of stored history, newest first from `before`.
    async fn load_history(
        self: &Arc<Self>,
        conversation: ConversationId,
        before: Option<u64>,
    ) -> Option<crate::history::Page> {
        let peer = self.peers().ticket(conversation).cloned()?;
        let (repo, service) = (self.messages.clone(), self.service.clone());
        let (my_device, inbound) = (self.my_device, self.inbound_conversation);
        tokio::task::spawn_blocking(move || {
            crate::history::collect_page(
                &[conversation, inbound],
                before,
                PAGE_SIZE,
                my_device,
                |c, b, l| {
                    repo.timeline(c, b, l).unwrap_or_else(|e| {
                        tracing::warn!(error = %e, "couldn’t read stored messages");
                        Vec::new()
                    })
                },
                |payload| service.open_stored_payload(&peer, payload).ok(),
            )
        })
        .await
        .ok()
    }

    fn read_marks(&self) -> std::sync::MutexGuard<'_, ReadMarks> {
        self.read_marks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// The user has seen everything in this chat up to now: persist it, so
    /// the same messages don't come back as unread after a restart.
    fn advance_read(&self, conversation: ConversationId) {
        let newest = self.hub.read(|s| {
            s.timelines
                .get(&conversation)
                .and_then(|t| newest_incoming(t.visible()))
        });
        if let Some(n) = newest {
            if let Err(e) = self.read_marks().advance(conversation, n) {
                tracing::warn!(error = %e, "couldn’t save read state");
            }
        }
    }

    /// Start-up: bring back each saved chat's recent messages and its
    /// unread badge. A chat with no recorded mark is treated as already
    /// read (tracking starts now) — upgrading must not paint a wall of
    /// phantom unread badges over old history.
    pub(crate) async fn preload_history(self: &Arc<Self>) {
        let ids: Vec<ConversationId> = self.hub.read(|s| s.sendable.iter().copied().collect());
        for conversation in ids {
            self.ensure_history(conversation).await;
            let timeline_view = self.hub.read(|s| {
                s.timelines.get(&conversation).map(|t| {
                    (
                        newest_incoming(t.visible()),
                        t.visible().cloned().collect::<Vec<_>>(),
                    )
                })
            });
            let Some((newest, entries)) = timeline_view else {
                continue;
            };
            let mark = self.read_marks().get(conversation);
            match (mark, newest) {
                (None, Some(n)) => {
                    if let Err(e) = self.read_marks().advance(conversation, n) {
                        tracing::warn!(error = %e, "couldn’t save read state");
                    }
                }
                (Some(mark), _) => {
                    let unread = unread_above(entries.iter(), mark);
                    self.hub.update(|s| {
                        if s.selected != Some(conversation) {
                            s.conversations.set_unread(conversation, unread);
                        }
                    });
                }
                (None, None) => {}
            }
        }
    }

    /// First time a 1:1 chat is needed this session: pull its newest page.
    async fn ensure_history(self: &Arc<Self>, conversation: ConversationId) {
        if self
            .hub
            .read(|s| s.history.get(&conversation).is_some_and(|h| h.loaded))
        {
            return;
        }
        if let Some(page) = self.load_history(conversation, None).await {
            self.hub.update(|s| {
                s.apply_history_page(conversation, page.entries, page.next_before, page.exhausted)
            });
        }
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

pub(crate) async fn command_loop(core: Arc<Core>, mut commands: mpsc::Receiver<AppCommand>) {
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
                    let conversation = crate::peers::conversation_for(&ticket);
                    core.open_direct(ticket, name);
                    core.ensure_history(conversation).await;
                    core.advance_read(conversation);
                    core.hub.notify(NoticeLevel::Success, "Paired — say hello.");
                }
                Err(e) => core.fail("That ticket couldn’t be read", e),
            },
            AppCommand::SetContactVerified {
                device_id,
                verified,
            } => {
                let contact = core.hub.read(|s| {
                    s.contacts
                        .ordered()
                        .iter()
                        .find(|c| c.device_id == device_id)
                        .cloned()
                });
                let ticket = contact
                    .as_ref()
                    .and_then(|c| PeerTicket::decode(&c.ticket_text).ok());
                let (Some(contact), Some(ticket)) = (contact, ticket) else {
                    core.hub.notify(
                        NoticeLevel::Error,
                        "That contact’s details couldn’t be read.",
                    );
                    continue;
                };
                let result = if verified {
                    core.verification()
                        .mark(device_id, &ticket.ed25519_verifying)
                } else {
                    core.verification().unmark(device_id)
                };
                match result {
                    Ok(()) => {
                        let safety = core.safety_for(device_id, &ticket);
                        core.hub.update(|s| {
                            core.apply_safety(s, device_id, contact.account_id, safety)
                        });
                        core.hub.notify(
                            NoticeLevel::Success,
                            if verified {
                                "Marked as verified."
                            } else {
                                "Verification removed."
                            },
                        );
                    }
                    Err(e) => core.fail("Your choice couldn’t be saved", e),
                }
            }
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
                            let conversation = crate::peers::conversation_for(&ticket);
                            core.open_direct(ticket, Some(c.display_name.clone()));
                            core.hub.update(|s| s.open_content = true);
                            core.ensure_history(conversation).await;
                            core.advance_read(conversation);
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
                    Ok(message_id) => {
                        core.hub.update(|s| {
                            s.record_outgoing(conversation, message_id, MessageContent::Text(text));
                        });
                        core.advance_read(conversation); // replying means you read it
                    }
                    Err(e) => core.fail("Your message couldn’t be sent", e),
                }
            }
            AppCommand::OpenConversation(conversation) => {
                core.hub.update(|s| s.conversations.mark_read(conversation));
                core.ensure_history(conversation).await;
                core.advance_read(conversation);
            }
            AppCommand::LoadEarlierMessages { conversation } => {
                let cursor = core.hub.read(|s| s.history.get(&conversation).copied());
                match cursor {
                    Some(h) if h.loaded && !h.exhausted => {
                        if let Some(page) = core.load_history(conversation, h.next_before).await {
                            core.hub.update(|s| {
                                s.apply_history_page(
                                    conversation,
                                    page.entries,
                                    page.next_before,
                                    page.exhausted,
                                )
                            });
                        }
                    }
                    _ => core.ensure_history(conversation).await,
                }
            }
            AppCommand::MarkRead { conversation, .. } => {
                core.hub.update(|s| s.conversations.mark_read(conversation));
                core.advance_read(conversation);
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
                        let safety = core.safety_for(device_id, &ticket);
                        let conversation = core.peers().register(ticket);
                        core.hub.update(|s| {
                            core.apply_safety(s, device_id, account_id, safety);
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
                // Arrived while this chat is on screen: it is read.
                if core.hub.read(|s| s.selected) == Some(conversation) {
                    core.advance_read(conversation);
                }
            }
            Ok(_) => {} // ACKs and call signals are not timeline content
            Err(e) => tracing::warn!(error = %e, "failed to handle incoming frame"),
        }
    }
}

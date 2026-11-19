//! The background half of the desktop app: owns `MessageService` and
//! `GroupService`, drains `AppCommand`s from the UI and pushes results
//! back through `StateHub`.
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
use siar_storage::{ContactRepository, MessageRepository, SecurityEventRepository};
use siar_ui_model::{ContactSafety, NoticeLevel, StateHub, VerificationState};
use siar_ui_state::{
    AppCommand, ConversationKind, ConversationSummary, DeviceSecurityView, DeviceTrustState,
    GroupSummary, IdentityKeyHealth, NetworkState, OwnIdentityView, PendingInvite, SavedContact,
    SecurityDiagnosticDetail, SecurityEventAction, SecurityEventKind, PAGE_SIZE,
};
use tokio::sync::mpsc;

use crate::bootstrap::Bootstrapped;
use crate::peers::PeerRegistry;
use crate::power_gate::{
    mailbox_work, preload_work, preview_work, Admission, Parked, PowerGate, PreviewJob,
};
use crate::read_marks::{newest_incoming, unread_above, ReadMarks};
use crate::verification::{key_fingerprint, safety_number, VerificationStore};

pub struct Core {
    pub(crate) service: Arc<MessageService>,
    group_service: Arc<GroupService>,
    device_directory: Arc<InMemoryDeviceDirectory>,
    contact_repo: Arc<dyn ContactRepository + Send + Sync>,
    security_repo: Arc<dyn SecurityEventRepository + Send + Sync>,
    messages: Arc<dyn MessageRepository + Send + Sync>,
    pub(crate) local_account: AccountId,
    pub(crate) hub: Arc<StateHub>,
    /// Part 13: the one place the core asks "may I do this background work now?".
    /// Messages never go through it.
    power: Arc<PowerGate>,
    pub(crate) peers: Mutex<PeerRegistry>,
    /// This device's signing public key (half of every safety number).
    pub(crate) local_key: [u8; 32],
    pub(crate) my_device: siar_domain::DeviceId,
    read_marks: Mutex<ReadMarks>,
    /// Where every peer files its messages TO us (see `history.rs`).
    inbound_conversation: ConversationId,
    verification: Mutex<VerificationStore>,
    /// Senders we already told the user about, so a chatty unknown device
    /// produces one notice, not a toast storm.
    warned_unknown: Mutex<HashSet<iroh::EndpointId>>,
    /// Part 28 §60-§62: cheap admission checks before any decryption.
    ingress: siar_messaging::IngressGuard,
    /// Senders already told about a rate limit, so a flood is one notice.
    warned_throttled: Mutex<HashSet<iroh::EndpointId>>,
    /// Relays we just sent an anonymous mailbox check-in to. Only those, and
    /// only for a short while, may deliver token-mailbox items to us.
    /// Several contacts may check the same relay at once, so each relay keeps
    /// a list; a delivery is tried against each (decryption authenticates).
    mailbox_waits: Mutex<std::collections::HashMap<iroh::EndpointId, Vec<MailboxWait>>>,
    /// Relays collected from on a schedule (see [`Core::start_mailbox_polling`]).
    mailbox_relays: Mutex<Vec<PeerTicket>>,
    /// A scheduled poll is queued or running: never stack a second one.
    mailbox_poll_busy: std::sync::atomic::AtomicBool,
    /// Where the next poll starts in the contact list (fair rotation).
    mailbox_cursor: std::sync::atomic::AtomicUsize,
    /// Where this device's files live (root key, linked devices, account id).
    pub(crate) data_dir: std::path::PathBuf,
    /// Device linking: the open offer and the requests waiting for approval.
    pub(crate) link_offers: Mutex<crate::linking::LinkOffers>,
    /// Which endpoint each waiting request came from, to answer it.
    pub(crate) link_endpoints: Mutex<std::collections::HashMap<DeviceId, iroh::EndpointId>>,
    /// Set while this (new) device waits to be approved by another.
    pub(crate) link_join: Mutex<Option<crate::link_flow::JoinWait>>,
    /// The code and ticket of the open offer, for the window to show.
    pub(crate) link_shown: Mutex<Option<(String, String)>>,
    /// Extra devices of each contact (see `contact_devices`).
    pub(crate) contact_devices: Mutex<crate::contact_devices::ContactDevices>,
}

/// At most this many contacts are checked per relay per poll; the rest follow
/// on the next ones. Bounds a poll's cost however many contacts exist.
const MAILBOX_POLL_MAX_CONTACTS: usize = 32;

/// A stable device id for a peer whose real one is not known yet.
fn stand_in_device(peer: &PeerTicket) -> DeviceId {
    let key = peer.endpoint_addr.id.as_bytes();
    let mut b = [0u8; 16];
    b.copy_from_slice(&key[16..]);
    DeviceId::from_uuid(uuid::Uuid::from_bytes(b))
}

/// How long after a check-in a relay's deliveries are accepted.
const MAILBOX_WAIT: Duration = Duration::from_secs(30);

/// What a pending anonymous check-in needs to decrypt what comes back.
struct MailboxWait {
    conversation: ConversationId,
    peer: PeerTicket,
    sender_device: DeviceId,
    until: std::time::Instant,
}

/// Boots the core into the UI and runs until the command channel closes
/// (app shutdown).
pub async fn run(boot: Bootstrapped, hub: Arc<StateHub>, commands: mpsc::Receiver<AppCommand>) {
    let core = start(boot, hub).await;
    command_loop(core, commands).await;
}

/// Seeds the UI from the freshly booted core and starts the background
/// tasks. Split from `run` so tests can keep the `Core` and drive its
/// internals (e.g. a simulated restart) directly.
pub async fn start(boot: Bootstrapped, hub: Arc<StateHub>) -> Arc<Core> {
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
        security_repo,
        saved_contacts,
        messages,
        data_dir,
        identity_created_at_millis,
        restored_groups,
        ..
    } = boot;
    let local_key = my_ticket.ed25519_verifying;

    let power = PowerGate::new(siar_power::Deployment::Desktop);
    power.sample_system(); // first decisions use the real state, not defaults
    let core = Arc::new(Core {
        service,
        group_service,
        device_directory,
        contact_repo,
        security_repo,
        messages,
        local_account,
        hub: hub.clone(),
        power: power.clone(),
        peers: Mutex::new(PeerRegistry::new()),
        local_key,
        my_device: device_id,
        read_marks: Mutex::new(ReadMarks::load(&data_dir.join("read-marks.txt"))),
        inbound_conversation: crate::peers::conversation_for(&my_ticket),
        verification: Mutex::new(VerificationStore::load(
            &data_dir.join("verified-contacts.txt"),
        )),
        warned_unknown: Mutex::new(HashSet::new()),
        ingress: siar_messaging::IngressGuard::new(siar_abuse::GateConfig::default()),
        warned_throttled: Mutex::new(HashSet::new()),
        mailbox_waits: Mutex::new(std::collections::HashMap::new()),
        mailbox_relays: Mutex::new(Vec::new()),
        mailbox_poll_busy: std::sync::atomic::AtomicBool::new(false),
        mailbox_cursor: std::sync::atomic::AtomicUsize::new(0),
        data_dir: data_dir.to_path_buf(),
        link_offers: Mutex::new(crate::linking::LinkOffers::new()),
        link_endpoints: Mutex::new(std::collections::HashMap::new()),
        link_join: Mutex::new(None),
        link_shown: Mutex::new(None),
        contact_devices: Mutex::new(crate::contact_devices::ContactDevices::load(&data_dir)),
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
    // Contacts handed over when this device was linked (not saved contacts, so
    // they are remembered in their own small file).
    for text in crate::link_flow::load_imported(&data_dir) {
        if let Ok(ticket) = PeerTicket::decode(&text) {
            let conversation = core.peers().register(ticket.clone());
            seeded.push((
                conversation,
                ticket.endpoint_addr.id.fmt_short().to_string(),
            ));
        }
    }
    // The security log from earlier runs. Restored BEFORE the start-up
    // safety checks run, so a warning that is still standing is recognised
    // as already raised instead of being raised again.
    let restored_events: Vec<_> = match core.security_repo.list() {
        Ok(rows) => rows
            .iter()
            .filter_map(crate::security_log::from_stored)
            .collect(),
        Err(e) => {
            tracing::warn!(error = %e, "couldn’t read the saved security log");
            Vec::new()
        }
    };
    hub.update(|s| s.security_events.restore(restored_events));

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
        // The one device that is certainly ours. Other devices of this account
        // appear here once device linking is wired into the core.
        s.devices.set_devices(crate::link_flow::device_list(
            DeviceSecurityView {
                id: device_id,
                display_name: crate::host::device_label(),
                kind: crate::host::device_kind(),
                status: DeviceTrustState::Trusted,
                added_at_millis: identity_created_at_millis,
                last_active_millis: None,
                current_device: true,
                security_flags: Vec::new(),
            },
            &crate::link_flow::load_linked(&data_dir),
        ));
        for (device, account, safety) in seeded_safety {
            core.apply_safety(s, device, account, safety, true);
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
        // Groups reopened from disk: put them back in the lists.
        for conversation in &restored_groups {
            match core.group_service.group_state(*conversation) {
                Ok(Some(g)) => core.apply_group_summary(s, &g),
                Ok(None) => {
                    tracing::warn!(?conversation, "restored MLS group has no bookkeeping row")
                }
                Err(e) => tracing::warn!(error = %e, "couldn’t read a restored group’s state"),
            }
        }
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
    // Speculative: gated. If it is held, opening a conversation loads its
    // history on demand, so nothing is lost by waiting.
    if core.power.admit(preload_work(), Parked::Preload) != Admission::Parked {
        let preload = core.clone();
        tokio::spawn(async move { preload.preload_history().await });
    }
    tokio::spawn(power_task(core.clone()));
    // A contact that was offline when a device was linked hears about it now.
    {
        let core = core.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(25)).await;
            core.announce_linked_devices().await;
        });
    }
    // An offer that expires (or a request that times out) changes what the
    // window should show without any event; look every few seconds.
    {
        let core = core.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(5));
            loop {
                tick.tick().await;
                core.refresh_link_state();
            }
        });
    }

    tokio::spawn(retry_scheduler(core.service.clone()));
    tokio::spawn(delivery_poller(core.clone()));
    tokio::spawn(incoming_loop(core.clone(), incoming_rx));
    core
}

impl Core {
    /// The toolkit-free state the core publishes to (whatever hosts it reads this).
    #[must_use]
    pub fn state(&self) -> Arc<StateHub> {
        self.hub.clone()
    }

    /// Leave the network politely (idempotent). Hosts that keep command
    /// senders alive past shutdown — a daemon — call this directly.
    /// Generate and persist a post-quantum key package so a peer can open a
    /// forward-secret 1:1 session with this device while it is offline.
    /// Returns `(this device's id, base64 key package)` — both are needed by
    /// the peer's [`Self::pairwise_open`].
    pub fn pairwise_publish(
        &self,
        profile: siar_crypto_mls::PairwiseProfile,
    ) -> Result<(DeviceId, String), String> {
        use base64::Engine as _;
        use siar_messaging::KeyPackageDirectory as _;
        let pw = self
            .service
            .pairwise()
            .ok_or("pairwise sessions are unavailable on this device")?;
        let dir = siar_messaging::InMemoryKeyPackageDirectory::new();
        pw.publish_key_package(profile, &dir)
            .map_err(|e| e.to_string())?;
        let kp = dir
            .take(self.my_device)
            .ok_or("key package was not produced")?;
        Ok((
            self.my_device,
            base64::engine::general_purpose::STANDARD.encode(kp),
        ))
    }

    /// Open a forward-secret 1:1 session for `conversation` with a peer, from
    /// the key package they gave us ([`Self::pairwise_publish`] on their side).
    /// `Ok(false)` means "no usable key package": the conversation keeps the
    /// static-key path. Safe to repeat.
    pub async fn pairwise_open(
        &self,
        conversation: ConversationId,
        peer_ticket: &str,
        peer_device: DeviceId,
        key_package_b64: &str,
        profile: siar_crypto_mls::PairwiseProfile,
    ) -> Result<bool, String> {
        use base64::Engine as _;
        use siar_messaging::KeyPackageDirectory as _;
        let kp = base64::engine::general_purpose::STANDARD
            .decode(key_package_b64.trim())
            .map_err(|e| format!("key package is not base64: {e}"))?;
        let peer = PeerTicket::decode(peer_ticket).map_err(|e| format!("bad ticket: {e}"))?;
        let dir = siar_messaging::InMemoryKeyPackageDirectory::new();
        dir.publish(peer_device, kp);
        self.service
            .open_pairwise_session(conversation, &peer, &dir, peer_device, profile)
            .await
            .map_err(|e| e.to_string())
    }

    pub async fn shutdown_transport(&self) {
        if tokio::time::timeout(Duration::from_secs(5), self.service.shutdown())
            .await
            .is_err()
        {
            tracing::warn!("transport shutdown timed out; exiting anyway");
        }
    }

    /// Part 13: the gate deferred background work waits behind.
    #[must_use]
    pub fn power_gate(&self) -> &Arc<PowerGate> {
        &self.power
    }

    fn peers(&self) -> std::sync::MutexGuard<'_, PeerRegistry> {
        self.peers.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn verification(&self) -> std::sync::MutexGuard<'_, VerificationStore> {
        self.verification
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Safety number + current verification state for one contact.
    /// How much the sender at `endpoint` is trusted, from state we already hold.
    /// Saved contacts and group members are in the device directory; a
    /// contact whose safety number was verified (and whose key is unchanged)
    /// is `Verified`; anyone else is a stranger.
    fn standing_of(&self, endpoint: &iroh::EndpointId) -> siar_abuse::Standing {
        use siar_abuse::Standing;
        // A relay we just asked for our mail is a known peer for now: its
        // answer can be many items, which the stranger budget would cut off.
        if self.waiting_on(endpoint) {
            return Standing::Contact;
        }
        if self
            .contact_devices
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .lookup_endpoint(endpoint)
            .is_some()
        {
            return Standing::Contact;
        }
        let Some((device, key)) = self.device_directory.device_at_endpoint(endpoint) else {
            // A paired peer that is not in the directory yet is still ours.
            let paired = self
                .peers
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .ticket_of_endpoint(endpoint)
                .is_some();
            return if paired {
                Standing::Contact
            } else {
                Standing::Unknown
            };
        };
        match self.verification().state(device, &key) {
            VerificationState::Verified => Standing::Verified,
            _ => Standing::Contact,
        }
    }

    /// A frame was refused by the admission gate. Logged quietly; the user is
    /// told at most once per sender, and only when it is a known contact being
    /// throttled (a stranger being limited is the gate working as intended).
    fn note_throttled(&self, endpoint: &iroh::EndpointId, reject: siar_messaging::IngressReject) {
        tracing::debug!(from = ?endpoint, %reject, "dropped a frame at admission");
        if self.standing_of(endpoint) == siar_abuse::Standing::Unknown {
            return;
        }
        let first_time = self
            .warned_throttled
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(*endpoint);
        if first_time {
            self.hub.notify(
                NoticeLevel::Warning,
                "A contact is sending faster than allowed; some messages were held back and will be retried.",
            );
        }
    }

    /// A relay ticket the user typed. Warns (never panics) on a bad one.
    fn decode_relay(&self, text: &str) -> Option<PeerTicket> {
        match PeerTicket::decode(text.trim()) {
            Ok(t) => Some(t),
            Err(_) => {
                self.hub
                    .notify(NoticeLevel::Warning, "That relay ticket isn’t valid.");
                None
            }
        }
    }

    /// Send `text` to each recorded extra device of `conversation`'s contact.
    /// Best effort per device: a failure is logged, never shown as a failed
    /// send, because the message did reach the contact's primary device.
    /// Send the file to each recorded extra device of the contact, as its own
    /// encrypted blob (same best-effort rules as [`Core::fan_out_text`]).
    async fn fan_out_attachment(
        &self,
        conversation: ConversationId,
        bytes: Vec<u8>,
        media_type: siar_domain::MediaType,
    ) {
        let siblings = self
            .contact_devices
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .siblings_of(conversation);
        for sibling in siblings {
            let hidden = crate::peers::conversation_for(&sibling);
            if let Err(e) = self
                .service
                .send_attachment_detailed(hidden, &sibling, bytes.clone(), media_type)
                .await
            {
                tracing::debug!(error = %e, "couldn’t queue a file for a contact’s other device");
            }
        }
    }

    async fn fan_out_text(&self, conversation: ConversationId, text: &siar_domain::MessageText) {
        let siblings = self
            .contact_devices
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .siblings_of(conversation);
        for sibling in siblings {
            let hidden = crate::peers::conversation_for(&sibling);
            if let Err(e) = self.service.send_text(hidden, &sibling, text.clone()).await {
                tracing::debug!(error = %e, "couldn’t queue a copy for a contact’s other device");
            }
        }
    }

    fn waiting_on(&self, relay: &iroh::EndpointId) -> bool {
        let now = std::time::Instant::now();
        self.mailbox_waits
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(relay)
            .is_some_and(|w| w.iter().any(|w| w.until > now))
    }

    /// Start waiting for a relay's answer to a check-in for `wait.conversation`.
    /// A second check-in for the same conversation replaces the first.
    fn expect_mailbox(&self, relay: iroh::EndpointId, wait: MailboxWait) {
        let mut waits = self
            .mailbox_waits
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let now = std::time::Instant::now();
        // Never grows without bound: expired entries go on every insert.
        waits.retain(|_, list| {
            list.retain(|w| w.until > now);
            !list.is_empty()
        });
        let list = waits.entry(relay).or_default();
        list.retain(|w| w.conversation != wait.conversation);
        list.push(wait);
    }

    /// A token-mailbox item from `relay`. Accepted only from a relay we are
    /// waiting on. Which contact it is for is not on the item, so it is tried
    /// against each contact we asked about; decryption is authenticated, so
    /// only the right one succeeds.
    fn accept_mailbox_item(
        &self,
        relay: &iroh::EndpointId,
        item: &siar_protocol::TokenMailboxEnvelope,
    ) {
        let candidates: Vec<(ConversationId, PeerTicket, DeviceId)> = {
            let now = std::time::Instant::now();
            self.mailbox_waits
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get(relay)
                .map(|list| {
                    list.iter()
                        .filter(|w| w.until > now)
                        .map(|w| (w.conversation, w.peer.clone(), w.sender_device))
                        .collect()
                })
                .unwrap_or_default()
        };
        if candidates.is_empty() {
            tracing::warn!(from = ?relay, "dropping an unsolicited mailbox delivery");
            return;
        }
        for (conversation, peer, sender_device) in candidates {
            match self.service.receive_token_mailbox_envelope(
                conversation,
                sender_device,
                &peer,
                item,
            ) {
                Ok(Some(content)) => {
                    self.hub.update(|s| {
                        s.sendable.insert(conversation);
                        s.record_incoming(conversation, item.id, item.created_at, content);
                    });
                    if self.hub.read(|s| s.selected) == Some(conversation) {
                        self.advance_read(conversation);
                    }
                    return;
                }
                Ok(None) => return, // already stored: a re-delivery, not a new message
                Err(_) => {}        // not this contact's; try the next
            }
        }
        tracing::debug!(from = ?relay, "a mailbox item matched none of the contacts asked about");
    }

    /// Collect from every configured relay, for a rotating slice of contacts.
    async fn poll_mailboxes(self: &Arc<Self>) {
        let relays = self
            .mailbox_relays
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let contacts = self.peers().all();
        if relays.is_empty() || contacts.is_empty() {
            return;
        }
        let take = contacts.len().min(MAILBOX_POLL_MAX_CONTACTS);
        let start = self
            .mailbox_cursor
            .fetch_add(take, std::sync::atomic::Ordering::Relaxed)
            % contacts.len();
        for relay in &relays {
            for i in 0..take {
                let (conversation, peer) = &contacts[(start + i) % contacts.len()];
                let sender_device = self
                    .device_directory
                    .device_at_endpoint(&peer.endpoint_addr.id)
                    .map_or_else(|| stand_in_device(peer), |(d, _)| d);
                self.expect_mailbox(
                    relay.endpoint_addr.id,
                    MailboxWait {
                        conversation: *conversation,
                        peer: peer.clone(),
                        sender_device,
                        until: std::time::Instant::now() + MAILBOX_WAIT,
                    },
                );
                if let Err(e) = self.service.send_anonymous_check_in(peer, relay).await {
                    // A relay that is down is routine, not something to show.
                    tracing::debug!(error = %e, "scheduled mailbox check-in failed");
                    break;
                }
                // Spread the requests out; a burst helps no one.
                tokio::time::sleep(Duration::from_millis(150)).await;
            }
        }
    }

    /// Ask the power gate whether a scheduled poll may run now.
    fn request_poll(self: &Arc<Self>) {
        use std::sync::atomic::Ordering::AcqRel;
        if self.mailbox_poll_busy.swap(true, AcqRel) {
            return; // one already queued or running
        }
        match self.power.admit(mailbox_work(), Parked::MailboxPoll) {
            Admission::Run | Admission::RunThrottled => self.run_poll(),
            Admission::Parked => {} // power_task resumes it; the flag stays set
        }
    }

    fn run_poll(self: &Arc<Self>) {
        let core = self.clone();
        tokio::spawn(async move {
            core.poll_mailboxes().await;
            core.mailbox_poll_busy
                .store(false, std::sync::atomic::Ordering::Release);
        });
    }

    /// Collect waiting relay-mailbox items on a schedule, from `relays`, every
    /// `interval` (first poll shortly after start). Power-gated like other
    /// background work: on low battery or saver a poll is deferred, never
    /// dropped, and at most one is ever queued. Replaces any earlier schedule's
    /// relay list; call it once at start-up.
    pub fn start_mailbox_polling(self: &Arc<Self>, relays: Vec<PeerTicket>, interval: Duration) {
        if relays.is_empty() {
            return;
        }
        *self
            .mailbox_relays
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = relays;
        let core = self.clone();
        tokio::spawn(async move {
            // Let the endpoint settle, then stay out of the start-up burst.
            tokio::time::sleep(Duration::from_secs(20)).await;
            let mut tick = tokio::time::interval(interval.max(Duration::from_secs(60)));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tick.tick().await;
                core.request_poll();
            }
        });
    }

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
        s: &mut siar_ui_model::Slices,
        device: DeviceId,
        account: AccountId,
        safety: ContactSafety,
        at_startup: bool,
    ) {
        let was_changed = s
            .contact_safety
            .get(&device)
            .is_some_and(|c| c.state == VerificationState::KeyChanged);
        // Raise the warning once per transition INTO the changed state, and
        // not at all if the same warning from an earlier run is still open.
        if safety.state == VerificationState::KeyChanged
            && !was_changed
            && !if at_startup {
                // A standing condition the user already saw (even if they
                // resolved it) is not announced again at every launch.
                s.security_events
                    .has_any(SecurityEventKind::IdentityChanged, Some(account))
            } else {
                // During a session a NEW change warns again, unless the
                // same warning is still open.
                s.security_events
                    .has_unresolved(SecurityEventKind::IdentityChanged, Some(account))
            }
        {
            let id = s.security_events.push(
                SecurityEventKind::IdentityChanged,
                now_millis(),
                None,
                Some(account),
                vec![SecurityEventAction::Review],
            );
            if let Some(event) = s.security_events.events().into_iter().find(|e| e.id == id) {
                if let Err(e) = self
                    .security_repo
                    .upsert(&crate::security_log::to_stored(event))
                {
                    tracing::warn!(error = %e, "couldn’t save a security event");
                }
            }
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
        // A contact's extra devices wrote into the same shared inbound stream
        // with their own sessions: try each when opening a stored message.
        let mut openers = vec![peer];
        openers.extend(
            self.contact_devices
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .siblings_of(conversation),
        );
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
                |payload| {
                    openers
                        .iter()
                        .find_map(|p| service.open_stored_payload(p, payload).ok())
                },
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

    fn apply_group_summary(&self, s: &mut siar_ui_model::Slices, g: &siar_domain::GroupState) {
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
    }

    fn upsert_group_summary(&self, g: &siar_domain::GroupState) {
        self.hub.update(|s| self.apply_group_summary(s, g));
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

pub async fn command_loop(core: Arc<Core>, mut commands: mpsc::Receiver<AppCommand>) {
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
            AppCommand::ResolveSecurityEvent { event_id } => {
                match core.security_repo.set_resolved(event_id) {
                    // The daemon owns this state: mark it in the hub too, so a
                    // window that sent the command (rather than resolving its
                    // own copy) sees the change come back as a sync delta.
                    // Idempotent for a local UI that already did it.
                    Ok(()) => core.hub.update(|s| {
                        s.security_events
                            .resolve(siar_ui_state::SecurityEventId::from_raw(event_id));
                    }),
                    Err(e) => core.fail("That change couldn’t be saved", e),
                }
            }
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
                            core.apply_safety(s, device_id, contact.account_id, safety, false)
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
                        // The contact's other devices get their own copy, each
                        // with its own outbox row (retries, delivery tracking),
                        // filed under a hidden conversation of its own so it
                        // never shows twice in this chat.
                        core.fan_out_text(conversation, &text).await;
                        // This account's other devices keep a complete chat too.
                        core.sync_sent_to_linked(conversation, message_id, &text)
                            .await;
                        core.hub.update(|s| {
                            s.record_outgoing(conversation, message_id, MessageContent::Text(text));
                        });
                        core.advance_read(conversation); // replying means you read it
                    }
                    Err(e) => core.fail("Your message couldn’t be sent", e),
                }
            }
            AppCommand::LinkOpenOffer => {
                if let Err(why) = core.link_open_offer() {
                    core.hub.notify(NoticeLevel::Warning, why);
                }
            }
            AppCommand::LinkApprove {
                device,
                share_contacts,
            } => match core.link_approve(device, share_contacts).await {
                Ok(()) => core
                    .hub
                    .notify(NoticeLevel::Info, "Device approved and linked."),
                Err(why) => core.hub.notify(NoticeLevel::Warning, why),
            },
            AppCommand::LinkRevoke { device } => {
                if let Err(why) = core.link_revoke(device).await {
                    core.hub.notify(NoticeLevel::Warning, why);
                }
            }
            AppCommand::LinkReject { device } => {
                core.link_reject(device).await;
            }
            AppCommand::LinkJoin { ticket_text, code } => {
                match core.link_join(&ticket_text, &code).await {
                    Ok(()) => core.hub.notify(
                        NoticeLevel::Info,
                        "Request sent. Approve it on your other device.",
                    ),
                    Err(why) => core.hub.notify(NoticeLevel::Warning, why),
                }
            }
            AppCommand::SendAttachment { conversation, path } => {
                let Some(peer) = core.peers().ticket(conversation).cloned() else {
                    core.hub.notify(
                        NoticeLevel::Warning,
                        "This conversation has no paired peer yet.",
                    );
                    continue;
                };
                match crate::attach::read_for_sending(&path).await {
                    Err(why) => core.hub.notify(NoticeLevel::Warning, &why),
                    Ok((bytes, media_type)) => match core
                        .service
                        .send_attachment_detailed(conversation, &peer, bytes.clone(), media_type)
                        .await
                    {
                        Ok((message_id, reference)) => {
                            core.fan_out_attachment(conversation, bytes, media_type)
                                .await;
                            core.hub.update(|s| {
                                s.record_outgoing(
                                    conversation,
                                    message_id,
                                    MessageContent::Attachment(reference),
                                );
                            });
                            core.advance_read(conversation);
                        }
                        Err(e) => core.fail("Your file couldn’t be sent", e),
                    },
                }
            }
            AppCommand::SendViaRelay {
                conversation,
                relay_ticket_text,
                text,
            } => {
                let (Some(peer), Some(relay)) = (
                    core.peers().ticket(conversation).cloned(),
                    core.decode_relay(&relay_ticket_text),
                ) else {
                    continue;
                };
                match core
                    .service
                    .send_text_via_relay(conversation, &peer, &relay, text.clone())
                    .await
                {
                    Ok(message_id) => {
                        core.hub.update(|s| {
                            s.record_outgoing(conversation, message_id, MessageContent::Text(text));
                        });
                    }
                    Err(e) => core.fail("Your message couldn’t be left at the relay", e),
                }
            }
            AppCommand::CheckRelayMailbox {
                conversation,
                relay_ticket_text,
            } => {
                let (Some(peer), Some(relay)) = (
                    core.peers().ticket(conversation).cloned(),
                    core.decode_relay(&relay_ticket_text),
                ) else {
                    continue;
                };
                // A ticket carries no device id (it is learned from the peer's
                // first envelope), so an unpaired-by-message contact gets a
                // stable stand-in derived from its transport key. It only
                // labels the stored row; nothing is authenticated by it.
                let sender_device = core
                    .device_directory
                    .device_at_endpoint(&peer.endpoint_addr.id)
                    .map_or_else(|| stand_in_device(&peer), |(d, _)| d);
                // Expect the answer BEFORE asking, or a fast relay could win the race.
                core.expect_mailbox(
                    relay.endpoint_addr.id,
                    MailboxWait {
                        conversation,
                        peer: peer.clone(),
                        sender_device,
                        until: std::time::Instant::now() + MAILBOX_WAIT,
                    },
                );
                if let Err(e) = core.service.send_anonymous_check_in(&peer, &relay).await {
                    core.fail("The relay couldn’t be reached", e);
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
                if invite.carries_state {
                    // The invite brought the roster/admin record with it.
                    match core
                        .group_service
                        .join_group_from_invite(conversation, &invite.welcome_bytes)
                    {
                        Ok(group_state) => {
                            core.upsert_group_summary(&group_state);
                            core.hub
                                .notify(NoticeLevel::Success, "You joined the group.");
                        }
                        Err(e) => {
                            // Keep it listed: a transient failure shouldn’t destroy the invite.
                            core.hub.update(|s| s.pending_invites.add(invite));
                            core.fail("Joining the group failed", e);
                        }
                    }
                    continue;
                }
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
                            core.apply_safety(s, device_id, account_id, safety, false);
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
                // Part 13: user-initiated, so saver never blocks it; only
                // hard safety (very hot / critically low battery for a big
                // blob) holds it, and it resumes by itself.
                let admission = core.power.admit(
                    preview_work(&reference),
                    Parked::Preview(Box::new(PreviewJob {
                        peer: peer.clone(),
                        message_id,
                        reference: reference.clone(),
                    })),
                );
                if admission == Admission::Parked {
                    core.hub.update(|s| {
                        s.attachment_previews.set_failed(
                            message_id,
                            "waiting for the device to cool down or charge — it will load by itself"
                                .into(),
                        )
                    });
                    continue;
                }
                // Spawned: fetch may hit the network (plan.md §22) and must
                // not stall sends queued behind it.
                spawn_preview(core.clone(), admission, peer, message_id, reference);
            }
        }
    }
    // The UI (or daemon) is done with us: leave the network politely so peers
    // see an orderly close rather than waiting out a timeout.
    if tokio::time::timeout(Duration::from_secs(5), core.service.shutdown())
        .await
        .is_err()
    {
        tracing::warn!("transport shutdown timed out; exiting anyway");
    }
}

/// Run a preview, one-at-a-time when the power policy asks for throttling.
fn spawn_preview(
    core: Arc<Core>,
    admission: Admission,
    peer: PeerTicket,
    message_id: MessageId,
    reference: siar_domain::AttachmentReference,
) {
    tokio::spawn(async move {
        let _permit = if admission == Admission::RunThrottled {
            core.power.throttle().acquire_owned().await.ok()
        } else {
            None
        };
        load_attachment_preview(core, peer, message_id, reference).await
    });
}

/// Samples the OS power state, mirrors it into the Settings card (only when
/// it changed — no redraw churn), and resumes parked work when allowed.
async fn power_task(core: Arc<Core>) {
    let mut tick = tokio::time::interval(Duration::from_secs(5));
    let mut n: u32 = 0;
    loop {
        tick.tick().await;
        if n.is_multiple_of(6) {
            core.power.sample_system();
        }
        n = n.wrapping_add(1);
        let status = core.power.status();
        if core.hub.read(|s| s.power.as_ref() != Some(&status)) {
            core.hub.update(|s| s.power = Some(status));
        }
        for (what, admission) in core.power.release_due() {
            match what {
                Parked::Preview(job) => {
                    let PreviewJob {
                        peer,
                        message_id,
                        reference,
                    } = *job;
                    core.hub
                        .update(|s| s.attachment_previews.set_loading(message_id));
                    spawn_preview(core.clone(), admission, peer, message_id, reference);
                }
                Parked::Preload => {
                    let c = core.clone();
                    tokio::spawn(async move { c.preload_history().await });
                }
                Parked::MailboxPoll => core.run_poll(),
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
        // Part 28 §61: admission BEFORE any decryption, storage or ACK, keyed on
        // the transport-authenticated sender (never `envelope.sender`). Every
        // wire kind is rationed, not just chat envelopes.
        let standing = core.standing_of(&frame.from);
        if let Err(reject) =
            core.ingress
                .admit_wire(frame.from.as_bytes(), standing, &frame.message)
        {
            core.note_throttled(&frame.from, reject);
            continue;
        }
        let envelope = match frame.message {
            WireMessage::V1(envelope) => envelope,
            // The answer to an anonymous check-in we sent.
            WireMessage::TokenMailboxDeposit(item) => {
                core.accept_mailbox_item(&frame.from, &item);
                continue;
            }
            WireMessage::Link(message) => {
                core.handle_link_message(frame.from, message).await;
                continue;
            }
            // Relay-only traffic: not an ordinary chat client's job.
            WireMessage::Mesh(_)
            | WireMessage::MailboxCheckIn(_)
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
                    // Does the invite carry the group record itself (this
                    // version), or is it a bare welcome from an older client?
                    let carries_state =
                        siar_messaging::WelcomeBundle::decode(&envelope.payload).is_some();
                    core.hub.update(|s| {
                        s.pending_invites.add(PendingInvite {
                            conversation_id: envelope.conversation_id,
                            from_device: envelope.sender,
                            welcome_bytes: envelope.payload,
                            state_input: String::new(),
                            carries_state,
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
                    Ok(None) => {
                        // A commit may have changed the roster (someone was
                        // added): show the new member count and epoch.
                        if let Ok(Some(g)) =
                            core.group_service.group_state(envelope.conversation_id)
                        {
                            core.upsert_group_summary(&g);
                        }
                    }
                    Err(e) => tracing::warn!(error = %e, "failed to handle incoming group frame"),
                },
            }
            continue;
        }

        // 1:1: route by WHO SENT IT, not by whoever is selected. A contact's
        // extra device is the same contact: its frames land in the same chat.
        let routed = match core.peers().ticket_of_endpoint(&frame.from).cloned() {
            Some(t) => {
                let c = crate::peers::conversation_for(&t);
                Some((t, c))
            }
            None => core
                .contact_devices
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .lookup_endpoint(&frame.from)
                .map(|(c, t)| (t, c)),
        };
        let Some((peer, conversation)) = routed else {
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

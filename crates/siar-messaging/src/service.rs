//! `MessageService` (plan.md §111 send flow / §112 receive flow), scoped
//! to what Phase 1 needs: one-to-one text messages, no groups, no
//! multi-device fanout yet (plan.md §124's "Alice ↔ Bob text messaging").

use crate::events::MessagingEvent;
use crate::PeerTicket;
use siar_crypto::{DeviceIdentity, Session};
use siar_domain::{
    backoff_millis, with_jitter, AttachmentReference, BlobSize, CallControlEvent, ConversationId,
    DeliveryState, DeviceId, MediaType, MessageContent, MessageId, MessageText,
};
use siar_event_log::{append_with_retry, EventOrigin, EventStore};
use siar_protocol::v1::{Envelope, EnvelopeKind, CURRENT_VERSION};
use siar_protocol::{MailboxCheckIn, WireMessage};
use siar_storage::{BlobRepository, MessageRepository, OutboxRepository, StoredMessage};
use siar_transport::{PeerTransport, SiarEndpoint};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

/// §33/Phase 3's own append retry budget for this crate's real call
/// sites — see `siar_event_log::retry`'s own doc comment for why
/// blind retry with the same event is safe. Small: a single
/// conversation being written from two places in the same process at
/// once (a send racing an incoming-message's own append) is the only
/// realistic contention this crate has right now, not a real
/// multi-writer workload.
const EVENT_LOG_APPEND_MAX_ATTEMPTS: u32 = 5;

/// plan.md §46: how long we wait for a `DeliveryAck` before treating a
/// transport-accepted send as due for retry. Not part of the failure
/// backoff schedule (`siar_domain::backoff_millis`) — this is the "it
/// probably got there but we haven't heard back" timeout, not "it
/// failed".
const ACK_TIMEOUT_MILLIS: u64 = 5_000;

/// plan.md §33: jitter fraction applied to every scheduled retry, so many
/// queued sends across a reconnect don't all fire in the same instant.
const RETRY_JITTER_FRACTION: f64 = 0.2;

#[derive(Debug, Error)]
pub enum MessageServiceError {
    #[error(transparent)]
    Storage(#[from] siar_storage::StorageError),
    #[error(transparent)]
    Transport(#[from] siar_transport::TransportError),
    #[error(transparent)]
    Crypto(#[from] siar_crypto::CryptoError),
    #[error("could not decode a decrypted message body")]
    Malformed,
    #[error("attachment is too large: {0}")]
    AttachmentTooLarge(#[from] siar_domain::BlobSizeError),
    #[error("requested blob was not found on the peer")]
    BlobNotFound,
}

/// What `handle_incoming` decoded off the wire, widened from a bare
/// `Option<MessageContent>` so call signaling (plan.md §48) can travel
/// the same decode path without pretending a ring/hangup event is a
/// chat message. `GroupEvent` frames are intentionally *not* listed
/// here — they're routed to `GroupService` separately since decoding
/// one needs the per-group `Session`/state `GroupService` owns, not
/// anything `MessageService` has (see `group_service.rs`).
#[derive(Debug, Clone)]
pub enum IncomingEvent {
    Content(MessageContent),
    CallSignal {
        from: DeviceId,
        event: CallControlEvent,
    },
}

pub struct MessageService {
    /// plan.md §7–8: `DeviceId` is our own stable local identifier,
    /// assigned once at provisioning — never derived from key bytes, so
    /// it stays stable across a future key-rotation (plan.md §40).
    device_id: DeviceId,
    identity: DeviceIdentity,
    endpoint: Arc<SiarEndpoint>,
    messages: Arc<dyn MessageRepository + Send + Sync>,
    outbox: Arc<dyn OutboxRepository + Send + Sync>,
    blobs: Arc<dyn BlobRepository + Send + Sync>,
    /// `04-offline-event-log-architecture.md` §33's own real call
    /// sites — see [`Self::with_event_log`]'s own doc comment for why
    /// this is optional rather than a required constructor argument.
    event_log: Option<Arc<dyn EventStore + Send + Sync>>,
}

impl MessageService {
    pub fn new(
        device_id: DeviceId,
        identity: DeviceIdentity,
        endpoint: Arc<SiarEndpoint>,
        messages: Arc<dyn MessageRepository + Send + Sync>,
        outbox: Arc<dyn OutboxRepository + Send + Sync>,
        blobs: Arc<dyn BlobRepository + Send + Sync>,
    ) -> Self {
        Self {
            device_id,
            identity,
            endpoint,
            messages,
            outbox,
            blobs,
            event_log: None,
        }
    }

    /// Opt-in wiring for `siar-event-log`'s per-conversation messaging
    /// event stream (`crate::events`) — additive, not a replacement for
    /// `siar-storage`: this crate's tables remain the system of record
    /// for message content/delivery state (see this type's private
    /// `record_messaging_event` method for exactly what "additive"
    /// means for a failed append). A separate builder method rather than a seventh `new(...)` argument, so existing
    /// callers that don't want the event log keep compiling unchanged
    /// — `new(...)`'s signature is otherwise untouched by this. Takes
    /// and returns `self` by value (not `&mut self`) because
    /// `MessageService` is typically wrapped in an `Arc` immediately
    /// after construction for sharing across async tasks; wiring has
    /// to happen before that `Arc` is taken, not after.
    pub fn with_event_log(mut self, event_log: Arc<dyn EventStore + Send + Sync>) -> Self {
        self.event_log = Some(event_log);
        self
    }

    /// Records one of `crate::events::MessagingEvent`'s nine §33
    /// variants to the event log, if one is wired
    /// ([`Self::with_event_log`]). Best-effort: an append failure is
    /// logged and swallowed, never propagated to the caller. That's a
    /// deliberate scope choice, not an oversight — `siar-storage`'s
    /// tables are still this crate's actual system of record (see
    /// `MessageServiceError`'s own variants, none of which are about
    /// the event log), so a send or receive succeeding or failing
    /// should not hinge on whether an audit-trail write also landed.
    /// Making the event log authoritative (so its own failure really
    /// should fail the caller) is a bigger architectural step — Phase
    /// 4/5's projections/outbox territory, not this round's.
    async fn record_messaging_event(&self, event: MessagingEvent, origin: EventOrigin) {
        let Some(store) = self.event_log.as_deref() else {
            return;
        };
        let stream_id = event.stream_id();
        let new_event = event.into_new_event(origin);
        if let Err(e) =
            append_with_retry(store, stream_id, new_event, EVENT_LOG_APPEND_MAX_ATTEMPTS).await
        {
            tracing::warn!(error = %e, "failed to record messaging event to the event log");
        }
    }

    /// Builds a signed `MailboxCheckIn` for this device — see
    /// `siar_protocol::mailbox`'s own doc comment for exactly what
    /// signing does and doesn't close. Kept here rather than exposing
    /// `self.identity` to callers directly: `DeviceIdentity`'s own doc
    /// comment is explicit that raw key material shouldn't leave
    /// `siar-crypto`, and this is the one signing operation a mailbox-
    /// checking caller (`apps/cli`'s `check-mailbox` command today)
    /// actually needs.
    pub fn sign_mailbox_check_in(&self, now_millis: u64) -> MailboxCheckIn {
        MailboxCheckIn::new(&self.identity, self.device_id, now_millis)
    }

    /// The unlinkable counterpart to
    /// [`sign_mailbox_check_in`](Self::sign_mailbox_check_in) — same
    /// "build the check-in from this device's own identity, no
    /// separate accessor needed" shape, but derived against `peer`
    /// instead of signed with this device's own key (an anonymous
    /// check-in has no signature at all — see
    /// `siar_crypto::mailbox_token`'s doc comment for that
    /// bearer-capability tradeoff). Kept as a method here rather than
    /// exposing `self.identity` to callers: `identity: DeviceIdentity`
    /// stays exactly as private as it's always been, and every caller
    /// that needs a token derivation goes through this or
    /// [`send_text_anon`](Self::send_text_anon), never touching a raw
    /// `DeviceIdentity` itself.
    pub fn build_anonymous_check_in(
        &self,
        peer: &PeerTicket,
        now_millis: u64,
    ) -> siar_protocol::AnonymousMailboxCheckIn {
        let peer_x25519_public = x25519_dalek::PublicKey::from(peer.x25519_public);
        let token_secret =
            siar_crypto::MailboxTokenSecret::establish(&self.identity, &peer_x25519_public);
        siar_protocol::AnonymousMailboxCheckIn::new(&token_secret, now_millis)
    }

    /// plan.md §111: persist first, then attempt delivery. The message is
    /// visible to `timeline()` (and therefore the UI, in later phases)
    /// the instant this returns `Ok`, regardless of whether the network
    /// send below succeeds.
    pub async fn send_text(
        &self,
        conversation: ConversationId,
        peer: &PeerTicket,
        text: MessageText,
    ) -> Result<MessageId, MessageServiceError> {
        let message_id = MessageId::new();
        let now = now_millis();

        let content = MessageContent::Text(text);
        let plaintext = postcard::to_allocvec(&content).expect("MessageContent always serializes");

        let session = self.session_for(peer);
        let ciphertext = session.encrypt(&plaintext)?;

        let stored = StoredMessage {
            message_id,
            conversation_id: conversation,
            sender_device: self.device_id,
            // Phase-1 stand-in for a real per-conversation sequence
            // counter (plan.md §21) — fine for ordering within one
            // two-peer chat's local clock, not for the deterministic
            // distributed ordering groups will need in Phase 5.
            sequence: now,
            timestamp_millis: now,
            delivery_state: DeliveryState::Local,
            payload: ciphertext.clone(),
        };

        // Transactional outbox: message + outbox row commit together
        // (plan.md §16–17) before we ever touch the network.
        self.outbox.enqueue(&stored, &peer.encode())?;

        // §33's own first two named events, in the order they actually
        // happen here: the message existed (`MessageCreated`) the
        // instant its ciphertext was ready, and it was queued
        // (`MessageQueued`) the instant `enqueue` above actually
        // persisted it — both true by the time we reach this line,
        // hence both recorded together rather than split across two
        // separate call sites that would just be adjacent anyway.
        self.record_messaging_event(
            MessagingEvent::MessageCreated {
                conversation_id: conversation,
                message_id,
                sender_device: self.device_id,
                sequence: stored.sequence,
                ciphertext: stored.payload.clone(),
            },
            EventOrigin::LocalDevice(self.device_id),
        )
        .await;
        self.record_messaging_event(
            MessagingEvent::MessageQueued {
                conversation_id: conversation,
                message_id,
            },
            EventOrigin::LocalDevice(self.device_id),
        )
        .await;

        let envelope = Envelope {
            version: CURRENT_VERSION,
            message_id,
            conversation_id: conversation,
            sender: self.device_id,
            timestamp_millis: now,
            sequence: stored.sequence,
            kind: EnvelopeKind::Text,
            payload: ciphertext,
        };

        match self
            .endpoint
            .send(peer.endpoint_addr.clone(), &WireMessage::V1(envelope))
            .await
        {
            Ok(()) => {
                // plan.md §46: transport accepting the bytes is `Sent`,
                // not `Delivered` — the outbox stays populated (so the
                // retry scheduler keeps resending) until a real
                // `DeliveryAck` comes back in `handle_incoming` and calls
                // `outbox.complete`. Marking complete here would silently
                // drop messages whose ACK never arrives. `reschedule`
                // (not `record_failure`) pushes the next retry out
                // without counting this as a failed attempt.
                self.messages
                    .update_delivery_state(message_id, DeliveryState::Sent)?;
                self.outbox
                    .reschedule(message_id, (now_millis() + ACK_TIMEOUT_MILLIS) as i64)?;
            }
            Err(e) => {
                tracing::warn!(error = %e, "send failed, left in outbox for retry");
                let delay = with_jitter(
                    backoff_millis(0),
                    pseudo_unit_random(message_id),
                    RETRY_JITTER_FRACTION,
                );
                self.outbox
                    .record_failure(message_id, (now_millis() + delay) as i64)?;
            }
        }

        Ok(message_id)
    }

    /// The unlinkable counterpart to [`send_text`](Self::send_text) —
    /// same session-encrypted payload, addressed and delivered
    /// completely differently: instead of a `V1::Envelope` sent
    /// directly to `peer.endpoint_addr`, this builds a
    /// `TokenMailboxEnvelope` addressed by a
    /// [`siar_crypto::mailbox_token::MailboxTokenSecret`] token and
    /// hands it to `relay` for pickup — see `siar_crypto::
    /// mailbox_token`'s and `siar_protocol::mailbox::
    /// TokenMailboxEnvelope`'s own doc comments for the full
    /// unlinkability picture and its real, named limits (the relay
    /// still sees *when* something moves and *how much*, and the
    /// bearer-capability model means no delivery signature).
    ///
    /// Deliberately NOT wired through the outbox/retry/ACK machinery
    /// `send_text` uses — this workspace has no `DeliveryAck` path for
    /// a token-addressed delivery to travel back on (that would need
    /// its own rotating-token addressing for the ack itself, which
    /// isn't designed here), so a caller gets a `MessageId` back and
    /// this crate's normal retry scheduler will NOT resend on this
    /// path if the relay drops it. Real follow-up work, named rather
    /// than silently downgraded to "good enough."
    pub async fn send_text_anon(
        &self,
        peer: &PeerTicket,
        relay: &PeerTicket,
        text: MessageText,
    ) -> Result<MessageId, MessageServiceError> {
        let message_id = MessageId::new();
        let now = now_millis();

        let content = MessageContent::Text(text);
        let plaintext = postcard::to_allocvec(&content).expect("MessageContent always serializes");

        // Same session-derived confidentiality as send_text — only the
        // addressing/delivery path differs, not the encryption.
        let session = self.session_for(peer);
        let ciphertext = session.encrypt(&plaintext)?;

        let peer_x25519_public = x25519_dalek::PublicKey::from(peer.x25519_public);
        let token_secret =
            siar_crypto::MailboxTokenSecret::establish(&self.identity, &peer_x25519_public);
        let destination_token = token_secret.token_for_epoch(siar_crypto::epoch_for(now));

        let envelope = siar_protocol::TokenMailboxEnvelope {
            id: message_id,
            destination_token,
            created_at: now,
            // A day is an approximation, not a measured value — same
            // "chosen conservatively, not tuned against real traffic"
            // status every other untuned constant in this workspace
            // carries (see e.g. `siar_crypto::mailbox_token::
            // EPOCH_LENGTH_MILLIS`'s own doc comment). Long enough that
            // a recipient checking in once a day still finds it; short
            // enough that an unclaimed message doesn't sit in a relay's
            // `TokenMailboxStore` forever.
            expires_at: now + 24 * 60 * 60 * 1000,
            hop_limit: 4,
            priority: siar_domain::MessagePriority::Normal,
            payload_hash: *blake3::hash(&ciphertext).as_bytes(),
            ciphertext,
        };

        self.endpoint
            .send(
                relay.endpoint_addr.clone(),
                &WireMessage::TokenMailboxDeposit(envelope),
            )
            .await?;

        Ok(message_id)
    }

    /// Decrypts one [`siar_protocol::TokenMailboxEnvelope`] received in
    /// answer to an anonymous check-in — the receive-side counterpart
    /// to [`send_text_anon`](Self::send_text_anon), using the exact
    /// same [`Session`] derivation (`session_for(peer)`) so decryption
    /// succeeds precisely when the envelope was really encrypted for
    /// this pairing. `peer` here is the *sender* the caller expects
    /// this item came from — an anonymous check-in has no sender field
    /// to read that from (the whole point), so the caller has to
    /// already know who it's checking mail from, same precondition
    /// `siar_crypto::mailbox_token::MailboxTokenSecret`'s own doc
    /// comment names ("a shared secret must already exist").
    pub fn decrypt_token_mailbox_envelope(
        &self,
        peer: &PeerTicket,
        envelope: &siar_protocol::TokenMailboxEnvelope,
    ) -> Result<MessageContent, MessageServiceError> {
        let session = self.session_for(peer);
        let plaintext = session.decrypt(&envelope.ciphertext)?;
        postcard::from_bytes(&plaintext).map_err(|_| MessageServiceError::Malformed)
    }

    /// plan.md §22–23's attachment send flow: encrypt the file under its
    /// own random key, publish the ciphertext to our local blob store
    /// (`BlobProtocolHandler` on `SiarEndpoint` serves it from there to
    /// whoever asks), and send only the small `AttachmentReference` —
    /// never the file bytes — inside the session-encrypted envelope.
    pub async fn send_attachment(
        &self,
        conversation: ConversationId,
        peer: &PeerTicket,
        plaintext: Vec<u8>,
        media_type: MediaType,
    ) -> Result<MessageId, MessageServiceError> {
        let size = BlobSize::parse(plaintext.len() as u64)?;
        let (blob, key) = siar_crypto::encrypt_attachment(&plaintext)?;
        self.blobs.put(blob.hash.as_bytes(), &blob.ciphertext)?;

        let reference = AttachmentReference {
            blob_hash: *blob.hash.as_bytes(),
            encrypted_size: size,
            media_type,
            attachment_key: key.to_bytes(),
            thumbnail: None, // plan.md §25 — thumbnail generation is a later refinement
        };

        let message_id = MessageId::new();
        let now = now_millis();
        let content = MessageContent::Attachment(reference);
        let envelope_plaintext =
            postcard::to_allocvec(&content).expect("MessageContent always serializes");

        let session = self.session_for(peer);
        let ciphertext = session.encrypt(&envelope_plaintext)?;

        let stored = StoredMessage {
            message_id,
            conversation_id: conversation,
            sender_device: self.device_id,
            sequence: now,
            timestamp_millis: now,
            delivery_state: DeliveryState::Local,
            payload: ciphertext.clone(),
        };
        self.outbox.enqueue(&stored, &peer.encode())?;

        let envelope = Envelope {
            version: CURRENT_VERSION,
            message_id,
            conversation_id: conversation,
            sender: self.device_id,
            timestamp_millis: now,
            sequence: stored.sequence,
            kind: EnvelopeKind::Attachment,
            payload: ciphertext,
        };

        match self
            .endpoint
            .send(peer.endpoint_addr.clone(), &WireMessage::V1(envelope))
            .await
        {
            Ok(()) => {
                self.messages
                    .update_delivery_state(message_id, DeliveryState::Sent)?;
                self.outbox
                    .reschedule(message_id, (now_millis() + ACK_TIMEOUT_MILLIS) as i64)?;
            }
            Err(e) => {
                tracing::warn!(error = %e, "attachment send failed, left in outbox for retry");
                let delay = with_jitter(
                    backoff_millis(0),
                    pseudo_unit_random(message_id),
                    RETRY_JITTER_FRACTION,
                );
                self.outbox
                    .record_failure(message_id, (now_millis() + delay) as i64)?;
            }
        }

        Ok(message_id)
    }

    /// plan.md §112: decode -> decrypt -> persist -> ACK. (Identity/replay
    /// verification is a further hardening item — plan.md §68–70 covers
    /// the idempotency half of that, handled here via `insert_if_new`;
    /// Phase 1's CLI trusts that `peer` really is who the transport says
    /// it is, which only holds because the harness pairs two known peers
    /// by hand.)
    ///
    /// Returns `None` for a `DeliveryAck`/`ReadReceipt` frame (nothing to
    /// show a user), `Some(IncomingEvent::Content(_))` for a text/
    /// attachment message worth displaying, `Some(IncomingEvent::CallSignal
    /// { .. })` for call control-plane events (plan.md §48) — the caller
    /// feeds those into `siar_domain::CallState::apply` to drive its own
    /// call UI state machine. `EnvelopeKind::GroupEvent` and the three
    /// `GroupMls*` frames are not handled here at all (see
    /// `IncomingEvent`'s doc comment); a caller wiring up groups
    /// dispatches those to `GroupService` before ever reaching this
    /// function.
    pub async fn handle_incoming(
        &self,
        peer: &PeerTicket,
        envelope: Envelope,
    ) -> Result<Option<IncomingEvent>, MessageServiceError> {
        match envelope.kind {
            EnvelopeKind::CallSignal(event) => {
                // Signaling is deliberately unencrypted-at-this-layer
                // (see `EnvelopeKind::CallSignal`'s doc comment on why
                // it rides as a plain field) — nothing to decrypt, just
                // hand the event to the caller's call-state machine.
                Ok(Some(IncomingEvent::CallSignal {
                    from: envelope.sender,
                    event,
                }))
            }
            EnvelopeKind::GroupEvent
            | EnvelopeKind::GroupMlsCommit
            | EnvelopeKind::GroupMlsWelcome
            | EnvelopeKind::GroupMlsApplication => {
                // Not this function's job — see the doc comment above.
                // A caller that hasn't routed group frames elsewhere
                // yet just doesn't see them; `apps/cli` does route them
                // (to `GroupService`, see that crate's `group_service.rs`)
                // but `apps/desktop` still doesn't have any group UI, so
                // for that caller this remains "a wiring gap in the
                // caller," not a silent drop of something it was
                // otherwise going to handle. The three MLS kinds join
                // `GroupEvent` here for the same reason they exist at
                // all — see `EnvelopeKind::GroupMlsCommit`'s doc
                // comment — `GroupService`'s MLS-path methods are the
                // ones that decode and process them.
                Ok(None)
            }
            EnvelopeKind::Text | EnvelopeKind::Attachment => {
                let session = self.session_for(peer);
                let plaintext = session.decrypt(&envelope.payload)?;
                let content: MessageContent =
                    postcard::from_bytes(&plaintext).map_err(|_| MessageServiceError::Malformed)?;

                let stored = StoredMessage {
                    message_id: envelope.message_id,
                    conversation_id: envelope.conversation_id,
                    sender_device: envelope.sender,
                    sequence: envelope.sequence,
                    timestamp_millis: envelope.timestamp_millis,
                    delivery_state: DeliveryState::Delivered,
                    payload: envelope.payload,
                };
                // plan.md §70: idempotent under duplicate delivery — only
                // ACK (and only surface to the caller) on a genuinely new
                // message, so a retransmitted envelope doesn't spam the
                // sender with redundant ACKs or the UI with a duplicate.
                let is_new = self.messages.insert_if_new(&stored)?;

                if is_new {
                    // §33 `MessageReceived` — real receipt of a
                    // genuinely new message (not a retransmit; see the
                    // comment above `insert_if_new` on why this is
                    // inside the `is_new` branch). `envelope.sender` is
                    // the remote peer's own device — this event is
                    // theirs, not ours, hence `RemoteDevice`.
                    self.record_messaging_event(
                        MessagingEvent::MessageReceived {
                            conversation_id: envelope.conversation_id,
                            message_id: envelope.message_id,
                            sender_device: envelope.sender,
                        },
                        EventOrigin::RemoteDevice(envelope.sender),
                    )
                    .await;

                    let ack = Envelope {
                        version: CURRENT_VERSION,
                        message_id: MessageId::new(),
                        conversation_id: envelope.conversation_id,
                        sender: self.device_id,
                        timestamp_millis: now_millis(),
                        sequence: 0,
                        kind: EnvelopeKind::DeliveryAck {
                            acked_message: envelope.message_id,
                        },
                        payload: Vec::new(),
                    };
                    let endpoint = self.endpoint.clone();
                    let ack_addr = peer.endpoint_addr.clone();
                    tokio::spawn(async move {
                        // Best-effort: a lost ACK just means the sender's
                        // outbox retries and we idempotently no-op the
                        // resend — not a reason to block or fail receive.
                        if let Err(e) = endpoint.send(ack_addr, &WireMessage::V1(ack)).await {
                            tracing::debug!(error = %e, "best-effort delivery ACK send failed");
                        }
                    });
                    Ok(Some(IncomingEvent::Content(content)))
                } else {
                    Ok(None)
                }
            }
            EnvelopeKind::DeliveryAck { acked_message } => {
                self.messages
                    .update_delivery_state(acked_message, DeliveryState::Delivered)?;
                self.outbox.complete(acked_message)?;
                // §33 `MessageDelivered` — the confirmation that this
                // message reached the recipient came from them
                // (`envelope.sender` here is whoever sent the ACK, i.e.
                // the original message's own recipient), so this is
                // their event to originate, not ours, even though it's
                // our own outbound message being confirmed.
                self.record_messaging_event(
                    MessagingEvent::MessageDelivered {
                        conversation_id: envelope.conversation_id,
                        message_id: acked_message,
                    },
                    EventOrigin::RemoteDevice(envelope.sender),
                )
                .await;
                Ok(None)
            }
            EnvelopeKind::ReadReceipt { .. } => {
                // Phase 3 (plan.md §45) — accepted on the wire already so
                // the format doesn't need to change later, not acted on
                // yet.
                Ok(None)
            }
        }
    }

    /// plan.md §22's receive-side fetch: given an `AttachmentReference`
    /// from an already-decrypted message, ask `peer` for the blob and
    /// decrypt it. Deliberately not called automatically from
    /// `handle_incoming` — plan.md §65's tiered cache / §67's bandwidth
    /// policy both assume attachments download on demand (or per a
    /// user's auto-download setting), not unconditionally the instant a
    /// reference arrives.
    pub async fn fetch_attachment(
        &self,
        peer: &PeerTicket,
        reference: &AttachmentReference,
    ) -> Result<Vec<u8>, MessageServiceError> {
        if let Some(cached) = self.blobs.get(&reference.blob_hash)? {
            let key = siar_crypto::AttachmentKey::from_bytes(reference.attachment_key);
            let hash = siar_crypto::BlobHash::from_bytes(reference.blob_hash);
            return Ok(siar_crypto::decrypt_attachment(&cached, hash, &key)?);
        }

        let ciphertext = self
            .endpoint
            .fetch_blob(peer.endpoint_addr.clone(), reference.blob_hash)
            .await?
            .ok_or(MessageServiceError::BlobNotFound)?;

        // plan.md §73: verify (hash + AEAD tag) before trusting it, then
        // cache it locally so a second view of the same attachment
        // doesn't re-fetch over the network.
        let key = siar_crypto::AttachmentKey::from_bytes(reference.attachment_key);
        let hash = siar_crypto::BlobHash::from_bytes(reference.blob_hash);
        let plaintext = siar_crypto::decrypt_attachment(&ciphertext, hash, &key)?;
        self.blobs.put(&reference.blob_hash, &ciphertext)?;

        Ok(plaintext)
    }

    /// plan.md §48's signaling send: Offer/Ring/Accept/Reject/Hangup/etc
    /// travel as their own envelope kind, not through the outbox — a
    /// missed "Ring" a minute late is pointless (plan.md §44's
    /// ephemeral-event rule applies here too, not just typing), so this
    /// is fire-and-forget over the pooled connection rather than
    /// persisted-then-retried like a text message.
    pub async fn send_call_signal(
        &self,
        peer: &PeerTicket,
        event: CallControlEvent,
    ) -> Result<(), MessageServiceError> {
        let envelope = Envelope {
            version: CURRENT_VERSION,
            message_id: MessageId::new(),
            // Call signaling isn't scoped to a conversation the way chat
            // messages are; callers that need to correlate a signal to a
            // specific call use `message_id`/their own call-session id
            // at the application layer, same as any other correlation
            // this layer doesn't model.
            conversation_id: ConversationId::new(),
            sender: self.device_id,
            timestamp_millis: now_millis(),
            sequence: 0,
            kind: EnvelopeKind::CallSignal(event),
            payload: Vec::new(),
        };

        self.endpoint
            .send(peer.endpoint_addr.clone(), &WireMessage::V1(envelope))
            .await?;
        Ok(())
    }

    /// plan.md §33: the retry scheduler's core poll. Call this
    /// periodically (the CLI's background task does, every second or so)
    /// — reconstructs each due message from storage (no second copy of
    /// the ciphertext lives in `outbox`) and resends it.
    pub async fn retry_due(&self) -> Result<usize, MessageServiceError> {
        let due = self.outbox.due(now_millis() as i64, 50)?;
        let mut retried = 0;

        for op in due {
            let Some(stored) = self.messages.get(op.message_id)? else {
                // Shouldn't happen under the transactional-outbox
                // invariant (plan.md §17) — message and outbox rows
                // always commit together — but don't retry forever
                // against a row that no longer exists.
                self.outbox.complete(op.message_id)?;
                continue;
            };
            let peer = match PeerTicket::decode(&op.peer_ticket_hex) {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!(error = %e, "outbox row has an undecodable peer ticket; dropping");
                    self.outbox.complete(op.message_id)?;
                    continue;
                }
            };

            let envelope = Envelope {
                version: CURRENT_VERSION,
                message_id: stored.message_id,
                conversation_id: stored.conversation_id,
                sender: stored.sender_device,
                timestamp_millis: stored.timestamp_millis,
                sequence: stored.sequence,
                // `StoredMessage` doesn't currently persist which
                // `EnvelopeKind` a message originally used, so retries
                // always re-tag as `Text`. Harmless for correctness —
                // `handle_incoming` decodes `Text` and `Attachment`
                // identically, since the real discriminant lives inside
                // the decrypted `MessageContent`, not this outer tag —
                // but it would matter for a future traffic-priority
                // feature (plan.md §66) reading the outer tag without
                // decrypting. Fix: persist the kind alongside `payload`
                // if/when that lands.
                kind: EnvelopeKind::Text,
                payload: stored.payload,
            };

            match self
                .endpoint
                .send(peer.endpoint_addr.clone(), &WireMessage::V1(envelope))
                .await
            {
                Ok(()) => {
                    retried += 1;
                    self.outbox
                        .reschedule(op.message_id, (now_millis() + ACK_TIMEOUT_MILLIS) as i64)?;
                }
                Err(e) => {
                    tracing::warn!(error = %e, "retry send failed");
                    let delay = with_jitter(
                        backoff_millis(op.attempts),
                        pseudo_unit_random(op.message_id),
                        RETRY_JITTER_FRACTION,
                    );
                    self.outbox
                        .record_failure(op.message_id, (now_millis() + delay) as i64)?;
                }
            }
        }

        // plan.md §35: sweep dead pooled connections while we're already
        // on this periodic cadence, rather than adding a second timer.
        self.endpoint.evict_idle_connections();

        Ok(retried)
    }

    /// Static-key session (see siar-crypto's module docs on `Session`).
    /// Deliberately *not* cached: re-deriving from the same two static
    /// keys is a cheap ECDH + BLAKE3, and not caching means there is
    /// nothing here to invalidate when Phase 2 swaps this for a real
    /// ratchet — that swap becomes a one-function change in siar-crypto
    /// instead of also touching a cache-invalidation path here.
    fn session_for(&self, peer: &PeerTicket) -> Session {
        let x25519_public = x25519_dalek::PublicKey::from(peer.x25519_public);
        Session::establish(&self.identity, &x25519_public)
    }
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is before 1970")
        .as_millis() as u64
}

/// Cheap jitter source (plan.md §33 just needs "don't all retry in
/// lockstep", not cryptographic randomness) — avoids pulling in the
/// `rand` crate for one `f64` per retry. Seeded by the message ID plus
/// the current clock, so repeated retries of the same message don't get
/// identical jitter.
fn pseudo_unit_random(seed: MessageId) -> f64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    seed.hash(&mut hasher);
    now_millis().hash(&mut hasher);
    (hasher.finish() as f64) / (u64::MAX as f64)
}

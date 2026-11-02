//! Wire protocol version 1 (plan.md §11).

use serde::{Deserialize, Serialize};
use siar_domain::{CallControlEvent, ConversationId, DeviceId, MessageId};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ProtocolVersion(pub u16);

pub const CURRENT_VERSION: ProtocolVersion = ProtocolVersion(1);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    pub version: ProtocolVersion,
    pub message_id: MessageId,
    pub conversation_id: ConversationId,
    pub sender: DeviceId,
    /// Unix millis. Not trusted for ordering across devices (plan.md
    /// §21) — combined with `sequence` for that.
    pub timestamp_millis: u64,
    pub sequence: u64,
    pub kind: EnvelopeKind,
    /// Application-E2EE ciphertext (plan.md §12). siar-protocol never
    /// looks inside this — that's siar-crypto's job.
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EnvelopeKind {
    Text,
    /// The `AttachmentReference` (which carries the file's decryption
    /// key) is never a field here — it lives only inside the
    /// session-encrypted `Envelope::payload`, exactly like `Text`'s
    /// content does. This variant is a bare discriminant so the receiver
    /// knows to deserialize the decrypted payload as
    /// `MessageContent::Attachment` rather than `::Text`, nothing more —
    /// putting the reference directly on this enum would leak the
    /// attachment key to anything that can see the envelope on the wire
    /// (a relay, a mailbox server — plan.md §31, §23).
    Attachment,
    /// plan.md §27–28's durable group events (member added/removed,
    /// rename, epoch advance), fanned out per-recipient-device exactly
    /// like `Text`/`Attachment` — see `siar-messaging::group_service`.
    /// Same rule as `Attachment`: this is a bare discriminant, the
    /// actual `DurableGroupEvent` only exists inside the session-
    /// encrypted `payload`, never here.
    GroupEvent,
    /// The real, forward-secret group crypto next.md §28 asks for —
    /// `siar-crypto-mls`'s `MlsGroupSession::add_member`/
    /// `remove_member` output, sent to every *existing* member so they
    /// can `process_incoming` and merge the same commit. Unlike
    /// `GroupEvent`, `payload` here is the raw MLS wire message
    /// (`MlsMessageOut::to_bytes()`) directly — **not** additionally
    /// wrapped in a `siar_crypto::Session` ciphertext. RFC 9420's own
    /// `PrivateMessage` framing already provides confidentiality,
    /// integrity, and (crucially, which `Session`'s static-key
    /// encryption does not) forward secrecy and post-compromise
    /// security across membership changes — layering `Session`
    /// encryption on top would add complexity without adding any
    /// security property this envelope doesn't already have from MLS
    /// itself. See `siar-messaging::group_service`'s MLS-path methods.
    GroupMlsCommit,
    /// `MlsGroupSession::add_member`'s welcome half — sent only to the
    /// *newly added* member (never to existing members, unlike
    /// `GroupMlsCommit`), who uses it with
    /// `MlsGroupSession::join_from_welcome`. Same "no outer `Session`
    /// wrapping" rule as `GroupMlsCommit` applies, for the same reason.
    GroupMlsWelcome,
    /// A group application (chat) message encrypted with
    /// `MlsGroupSession::encrypt` — the MLS-path counterpart to `Text`,
    /// for conversations that have real MLS group crypto instead of
    /// (or, during the migration window described in
    /// `siar-messaging::group_service`, alongside) per-device static-
    /// key fanout. Same "no outer `Session` wrapping" rule as
    /// `GroupMlsCommit`.
    GroupMlsApplication,
    /// plan.md §48's call control plane (Offer/Ring/Accept/Reject/
    /// Hangup/...). Small enough to travel as a plain field rather than
    /// forcing a payload round-trip — see `CallControlEvent`'s own docs
    /// for why this is the signaling channel only, not media.
    CallSignal(CallControlEvent),
    DeliveryAck {
        acked_message: MessageId,
    },
    ReadReceipt {
        through_sequence: u64,
    },
    /// The Welcome that opens a 1:1 OpenMLS session
    /// (`siar_crypto_mls::PairwiseSession`), sent once by the initiator.
    /// `payload` is the raw MLS Welcome — no outer `Session` wrapping, for
    /// the same reason as `GroupMlsWelcome`. **Appended last on purpose**:
    /// postcard encodes enum variants by index, so inserting earlier would
    /// change the wire form of every variant after it.
    PairwiseWelcome,
    /// A 1:1 OpenMLS batch (own commit if due, then the message), see
    /// `siar-messaging::PairwiseService`. `payload` is the postcard batch of
    /// raw MLS wire messages; the message id/sequence/timestamp live
    /// *inside* the encrypted plaintext, so a message that was deferred
    /// behind a late commit still arrives with its own identity.
    PairwiseMls,
}

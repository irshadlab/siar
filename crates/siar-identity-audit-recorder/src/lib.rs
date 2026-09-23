#![forbid(unsafe_code)]

//! `04-offline-event-log-architecture.md` §35's own named gap, closed:
//! `siar_identity_multidevice::audit_log` has had a real, tested
//! catalog of eight identity audit events since it was first built —
//! `device_linked_event`, `device_revoked_event`,
//! `revocation_verified_event`, `device_suspended_event`,
//! `device_rotated_event`, `root_rotated_event`,
//! `recovery_used_event`, `fork_detected_event` — but that module's
//! own top doc comment says outright it "deliberately only
//! *constructs* events; it never calls `EventStore::append` itself,"
//! and `ROADMAP.md`'s own §35/§95 rows confirm nothing in this
//! workspace ever did either. This crate is that caller.
//!
//! ## Why this is smaller than [`siar-file-transfer-service`]'s
//! equivalent job for §34
//!
//! Files needed a DECIDE step of its own (`TransferState::transition`,
//! a real state machine with invalid transitions to reject) alongside
//! the RECORD step. Identity doesn't: `audit_log`'s own eight
//! constructors already fire only from real operations elsewhere in
//! `siar-identity-multidevice` that have their own success/failure
//! decisions ([`crate`]'s own re-exported names for what each event
//! follows: `revoke_device`, `suspend_device`, `rotate_device_key`,
//! `rotate_root_key`, `add_device_via_recovery`, `TrustedAccountStore::
//! accept`'s fork detection — all pre-existing and already tested in
//! that crate, not this one's job to re-decide). So
//! [`IdentityAuditRecorder`] is RECORD only: given a `NewEvent`
//! `audit_log` already built correctly (origin included — see that
//! module's own `IdentityAuditPayload::origin`), append it. There is
//! no local decision to protect independently of the event log the
//! way `siar-file-transfer-service`'s in-memory `states` map is, so
//! `event_log` here is a required constructor argument, not an
//! `Option` a caller fills in later.
//!
//! ## What this closes, and what it doesn't
//!
//! Closes §35/§95's "no real caller" gap for identity, and — same
//! caveat as the files round — only the recording half. The actual
//! call sites (`revoke_device` etc. themselves calling THIS crate
//! after they succeed) aren't wired; nothing in this workspace's
//! uploaded `apps/`, or in `siar-identity-multidevice` itself, calls
//! [`IdentityAuditRecorder`] yet. §36/§37 (DTN/emergency) have the
//! identical gap under yet another domain, still open after this.

use siar_domain::AccountId;
use siar_event_log::{append_with_retry, EventStore, EventStoreError, NewEvent};
use siar_identity_multidevice::identity_stream_id;
use std::sync::Arc;

/// Same bound `siar_messaging::service`/`siar-file-transfer-service`
/// both already use — matched, not re-derived.
const EVENT_LOG_APPEND_MAX_ATTEMPTS: u32 = 5;

/// The real caller — see this module's own doc comment.
///
/// Unlike `siar-messaging::service::MessageService`/
/// `siar-file-transfer-service::FileTransferService`'s own optional
/// `event_log`, this one is required at construction: those two
/// services have a real decision (`TransferState::transition`, or
/// messaging's own send flow) that stays correct with no event log
/// configured — recording is an add-on to a decision that already
/// happened. This crate's ENTIRE job is recording; an
/// `IdentityAuditRecorder` with nowhere to record to would have no
/// reason to exist.
pub struct IdentityAuditRecorder {
    event_log: Arc<dyn EventStore + Send + Sync>,
}

impl IdentityAuditRecorder {
    pub fn new(event_log: Arc<dyn EventStore + Send + Sync>) -> Self {
        Self { event_log }
    }

    /// The one real primitive: append an already-constructed
    /// `audit_log` event to `account`'s identity stream, with retry on
    /// a concurrency conflict (same [`append_with_retry`] every other
    /// domain's real caller uses). Every `record_*` convenience method
    /// below is a thin wrapper over this plus one `audit_log`
    /// constructor call — this is the one place that actually touches
    /// [`EventStore::append`].
    pub async fn record(&self, account: AccountId, event: NewEvent) -> Result<(), EventStoreError> {
        let stream_id = identity_stream_id(account);
        append_with_retry(
            self.event_log.as_ref(),
            stream_id,
            event,
            EVENT_LOG_APPEND_MAX_ATTEMPTS,
        )
        .await?;
        Ok(())
    }

    /// Call once [`siar_identity_multidevice::client_api::DeviceClient`]'s
    /// (or whichever real caller's) own linking flow has already
    /// succeeded and produced `new_generation`.
    pub async fn record_device_linked(
        &self,
        account: AccountId,
        device_id: siar_domain::DeviceId,
        new_generation: u64,
    ) -> Result<(), EventStoreError> {
        self.record(
            account,
            siar_identity_multidevice::device_linked_event(device_id, new_generation),
        )
        .await
    }

    /// Call once [`siar_identity_multidevice::revocation::revoke_device`]
    /// has already succeeded.
    pub async fn record_device_revoked(
        &self,
        account: AccountId,
        device_id: siar_domain::DeviceId,
        new_generation: u64,
    ) -> Result<(), EventStoreError> {
        self.record(
            account,
            siar_identity_multidevice::device_revoked_event(device_id, new_generation),
        )
        .await
    }

    /// Call once [`siar_identity_multidevice::revocation::verify_revocation`]
    /// has already succeeded on this device.
    pub async fn record_revocation_verified(
        &self,
        account: AccountId,
        device_id: siar_domain::DeviceId,
        new_generation: u64,
    ) -> Result<(), EventStoreError> {
        self.record(
            account,
            siar_identity_multidevice::revocation_verified_event(device_id, new_generation),
        )
        .await
    }

    /// Call once [`siar_identity_multidevice::device_state::suspend_device`]
    /// has already succeeded.
    pub async fn record_device_suspended(
        &self,
        account: AccountId,
        device_id: siar_domain::DeviceId,
        new_generation: u64,
    ) -> Result<(), EventStoreError> {
        self.record(
            account,
            siar_identity_multidevice::device_suspended_event(device_id, new_generation),
        )
        .await
    }

    /// Call once [`siar_identity_multidevice::rotation::rotate_device_key`]
    /// has already succeeded.
    pub async fn record_device_rotated(
        &self,
        account: AccountId,
        device_id: siar_domain::DeviceId,
        new_generation: u64,
    ) -> Result<(), EventStoreError> {
        self.record(
            account,
            siar_identity_multidevice::device_rotated_event(device_id, new_generation),
        )
        .await
    }

    /// Call once [`siar_identity_multidevice::root_rotation::rotate_root_key`]
    /// has already succeeded and verified — no `device_id`, matching
    /// `IdentityAuditPayload::RootRotated`'s own account-level origin.
    pub async fn record_root_rotated(
        &self,
        account: AccountId,
        new_generation: u64,
    ) -> Result<(), EventStoreError> {
        self.record(
            account,
            siar_identity_multidevice::root_rotated_event(new_generation),
        )
        .await
    }

    /// Call once [`siar_identity_multidevice::recovery::add_device_via_recovery`]
    /// has already succeeded.
    pub async fn record_recovery_used(
        &self,
        account: AccountId,
        device_id: siar_domain::DeviceId,
        new_generation: u64,
    ) -> Result<(), EventStoreError> {
        self.record(
            account,
            siar_identity_multidevice::recovery_used_event(device_id, new_generation),
        )
        .await
    }

    /// Call once `TrustedAccountStore::accept` has already returned
    /// `IdentityError::IdentityForkDetected` — no `device_id`, same
    /// reason as `record_root_rotated`.
    pub async fn record_fork_detected(
        &self,
        account: AccountId,
        generation: u64,
    ) -> Result<(), EventStoreError> {
        self.record(
            account,
            siar_identity_multidevice::fork_detected_event(generation),
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use siar_domain::DeviceId;
    use siar_event_log::{EventOrigin, InMemoryEventStore};
    use siar_identity_multidevice::{
        decode_audit_payload, identity_stream_id, IdentityAuditPayload,
    };

    fn recorder() -> (IdentityAuditRecorder, Arc<dyn EventStore + Send + Sync>) {
        let store: Arc<dyn EventStore + Send + Sync> = Arc::new(InMemoryEventStore::new());
        (IdentityAuditRecorder::new(store.clone()), store)
    }

    #[tokio::test]
    async fn recording_a_device_link_appends_a_decodable_event_with_local_device_origin() {
        let (recorder, store) = recorder();
        let account = AccountId::new();
        let device_id = DeviceId::new();

        recorder
            .record_device_linked(account, device_id, 2)
            .await
            .unwrap();

        let stream_id = identity_stream_id(account);
        let stream = store.read_stream(stream_id, 0, 100).await.unwrap();
        assert_eq!(stream.len(), 1);
        let decoded = decode_audit_payload(
            stream[0].envelope.schema_version,
            &stream[0].envelope.payload,
        )
        .unwrap();
        assert_eq!(
            decoded,
            IdentityAuditPayload::DeviceLinked {
                device_id,
                generation: 2,
            }
        );
        assert_eq!(
            stream[0].envelope.origin,
            EventOrigin::LocalDevice(device_id)
        );
    }

    #[tokio::test]
    async fn account_level_events_get_system_origin_not_a_fabricated_device() {
        let (recorder, store) = recorder();
        let account = AccountId::new();

        recorder.record_root_rotated(account, 5).await.unwrap();
        recorder.record_fork_detected(account, 6).await.unwrap();

        let stream_id = identity_stream_id(account);
        let stream = store.read_stream(stream_id, 0, 100).await.unwrap();
        assert_eq!(stream.len(), 2);
        assert_eq!(stream[0].envelope.origin, EventOrigin::System);
        assert_eq!(stream[1].envelope.origin, EventOrigin::System);
        let first = decode_audit_payload(
            stream[0].envelope.schema_version,
            &stream[0].envelope.payload,
        )
        .unwrap();
        let second = decode_audit_payload(
            stream[1].envelope.schema_version,
            &stream[1].envelope.payload,
        )
        .unwrap();
        assert_eq!(first, IdentityAuditPayload::RootRotated { generation: 5 });
        assert_eq!(second, IdentityAuditPayload::ForkDetected { generation: 6 });
    }

    #[tokio::test]
    async fn two_different_accounts_get_two_different_streams() {
        let (recorder, store) = recorder();
        let account_a = AccountId::new();
        let account_b = AccountId::new();
        let device_id = DeviceId::new();

        recorder
            .record_device_revoked(account_a, device_id, 1)
            .await
            .unwrap();

        let stream_a = store
            .read_stream(identity_stream_id(account_a), 0, 100)
            .await
            .unwrap();
        let stream_b = store
            .read_stream(identity_stream_id(account_b), 0, 100)
            .await
            .unwrap();
        assert_eq!(stream_a.len(), 1);
        assert_eq!(stream_b.len(), 0);
    }

    #[tokio::test]
    async fn all_eight_audit_events_round_trip_through_the_same_recorder() {
        let (recorder, store) = recorder();
        let account = AccountId::new();
        let device_id = DeviceId::new();

        recorder
            .record_device_linked(account, device_id, 1)
            .await
            .unwrap();
        recorder
            .record_device_revoked(account, device_id, 2)
            .await
            .unwrap();
        recorder
            .record_revocation_verified(account, device_id, 3)
            .await
            .unwrap();
        recorder
            .record_device_suspended(account, device_id, 4)
            .await
            .unwrap();
        recorder
            .record_device_rotated(account, device_id, 5)
            .await
            .unwrap();
        recorder.record_root_rotated(account, 6).await.unwrap();
        recorder
            .record_recovery_used(account, device_id, 7)
            .await
            .unwrap();
        recorder.record_fork_detected(account, 8).await.unwrap();

        let stream = store
            .read_stream(identity_stream_id(account), 0, 100)
            .await
            .unwrap();
        assert_eq!(stream.len(), 8);
        for stored in &stream {
            // Every one of the eight actually decodes — the real
            // proof, not just that `record` returned `Ok`.
            decode_audit_payload(stored.envelope.schema_version, &stored.envelope.payload).unwrap();
        }
    }
}

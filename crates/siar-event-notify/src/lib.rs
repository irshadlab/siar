#![forbid(unsafe_code)]

//! `04-offline-event-log-architecture.md` §61's own named gap, closed:
//! `ROADMAP.md`'s own §61 row says outright "no wake/notify mechanism
//! — `record_messaging_event` calls `catch_up` synchronously in the
//! same call stack instead, which works for the one caller that
//! exists today but isn't the decoupled notification pattern this
//! section describes." This crate is that decoupled pattern.
//!
//! [`ChangeNotifier`] is a small `tokio::sync::broadcast` wrapper: any
//! number of subscribers each get their own [`siar_event_log::
//! AppendResult`] the moment something real is appended.
//! [`NotifyingEventStore`] wraps any real `EventStore` and fires a
//! [`ChangeNotifier`] after every `append` that actually appended
//! something — §24's idempotent no-op duplicates (an `AppendResult`
//! whose `local_offsets` are all `None`) notify nobody, since nothing
//! actually changed for a subscriber to react to.
//!
//! ## What this closes, and what it doesn't
//!
//! Closes §61's "no wake/notify mechanism" gap — a real, tested,
//! decoupled notification primitive now exists. It does NOT replace
//! `record_messaging_event`'s own synchronous `catch_up` call, and
//! nothing in this workspace wraps `siar-messaging`'s (or any other
//! domain's) real `EventStore` in a [`NotifyingEventStore`] yet — that
//! wiring, and whichever projection/subscriber ends up on the other
//! end of [`ChangeNotifier::subscribe`], is still open. A
//! `tokio::sync::broadcast` channel also only reaches subscribers
//! that are listening AT THE MOMENT of the append (a subscriber that
//! connects later has missed anything sent before it subscribed, and
//! one that lags past the channel's capacity gets `RecvError::Lagged`
//! rather than silently catching up) — real notification, not a
//! substitute for `catch_up`'s own "read everything since my
//! checkpoint" durability. A subscriber still needs to reconcile
//! against its own checkpoint on wake, the same way it would on a
//! cold start; this crate only tells it WHEN to do that, not instead
//! of doing it.

use async_trait::async_trait;
use siar_event_log::{
    AppendRequest, AppendResult, EventStore, EventStoreError, LocalLogOffset, StoredEvent, StreamId,
};
use tokio::sync::broadcast;

/// §61's own wake/notify primitive. Cloning a [`ChangeNotifier`] (via
/// [`Self::subscribe`], not `Clone` on the type itself — there's
/// deliberately no `#[derive(Clone)]` here, since cloning the sender
/// itself would just be two handles to the same channel, which
/// `subscribe` already gives a caller a cheaper way to get) shares
/// the same underlying broadcast channel.
pub struct ChangeNotifier {
    sender: broadcast::Sender<AppendResult>,
}

impl ChangeNotifier {
    /// `capacity` is the broadcast channel's own lag buffer — see this
    /// module's own doc comment on `RecvError::Lagged`.
    pub fn new(capacity: usize) -> Self {
        let (sender, _receiver) = broadcast::channel(capacity.max(1));
        Self { sender }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<AppendResult> {
        self.sender.subscribe()
    }

    /// Current subscriber count — real diagnostic value for whichever
    /// real caller eventually wires this in (§82 "Metrics," still
    /// `⬜` in the tracker, would want exactly this number).
    pub fn subscriber_count(&self) -> usize {
        self.sender.receiver_count()
    }

    /// Fan out `result` to every current subscriber. A `send` with
    /// zero subscribers is not an error — see `tokio::sync::broadcast`'s
    /// own docs — it's simply discarded, which is correct here: no
    /// harm in appending with nobody listening yet.
    fn notify(&self, result: &AppendResult) {
        let _ = self.sender.send(result.clone());
    }
}

impl Default for ChangeNotifier {
    /// 64 — a round, generous number with no real workload behind it
    /// yet to size against; see [`Self::new`]'s own doc comment for
    /// why a real caller may well want a different one.
    fn default() -> Self {
        Self::new(64)
    }
}

/// §61's own wrapper: any real [`EventStore`] plus a [`ChangeNotifier`]
/// that fires after every append that actually changed something.
/// Read methods (`read_stream`/`read_log`) are pure passthrough — this
/// type adds nothing to reads, only to `append`.
pub struct NotifyingEventStore<S: EventStore + 'static> {
    inner: S,
    notifier: ChangeNotifier,
}

impl<S: EventStore + 'static> NotifyingEventStore<S> {
    pub fn new(inner: S, notifier: ChangeNotifier) -> Self {
        Self { inner, notifier }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<AppendResult> {
        self.notifier.subscribe()
    }
}

#[async_trait]
impl<S: EventStore + 'static> EventStore for NotifyingEventStore<S> {
    async fn append(&self, request: AppendRequest) -> Result<AppendResult, EventStoreError> {
        let result = self.inner.append(request).await?;
        // §24: an all-duplicate batch changed nothing real — see this
        // module's own doc comment for why that means nobody is
        // notified for it.
        if result.local_offsets.iter().any(Option::is_some) {
            self.notifier.notify(&result);
        }
        Ok(result)
    }

    async fn read_stream(
        &self,
        stream: StreamId,
        from_version: u64,
        limit: usize,
    ) -> Result<Vec<StoredEvent>, EventStoreError> {
        self.inner.read_stream(stream, from_version, limit).await
    }

    async fn read_log(
        &self,
        from_offset: LocalLogOffset,
        limit: usize,
    ) -> Result<Vec<StoredEvent>, EventStoreError> {
        self.inner.read_log(from_offset, limit).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use siar_event_log::{
        EventId, EventOrigin, EventTypeId, InMemoryEventStore, NewEvent, Timestamp,
    };

    fn sample_event(event_id: EventId) -> NewEvent {
        NewEvent {
            event_id,
            event_type: EventTypeId(999),
            schema_version: 1,
            created_at: Timestamp::now(),
            origin: EventOrigin::System,
            correlation_id: None,
            causation_id: None,
            payload: vec![1, 2, 3],
        }
    }

    #[tokio::test]
    async fn a_subscriber_is_notified_of_a_real_append() {
        let store = NotifyingEventStore::new(InMemoryEventStore::new(), ChangeNotifier::default());
        let mut receiver = store.subscribe();
        let stream_id = StreamId::from_name("test/stream/a");

        let result = store
            .append(AppendRequest {
                stream_id,
                expected_version: 0,
                events: vec![sample_event(EventId::new())],
            })
            .await
            .unwrap();

        let notified = receiver.recv().await.unwrap();
        assert_eq!(notified, result);
        assert_eq!(notified.stream_id, stream_id);
        assert_eq!(notified.new_version, 1);
    }

    #[tokio::test]
    async fn every_current_subscriber_gets_its_own_copy() {
        let store = NotifyingEventStore::new(InMemoryEventStore::new(), ChangeNotifier::default());
        let mut receiver_a = store.subscribe();
        let mut receiver_b = store.subscribe();
        assert_eq!(store.notifier.subscriber_count(), 2);
        let stream_id = StreamId::from_name("test/stream/b");

        store
            .append(AppendRequest {
                stream_id,
                expected_version: 0,
                events: vec![sample_event(EventId::new())],
            })
            .await
            .unwrap();

        assert_eq!(receiver_a.recv().await.unwrap().stream_id, stream_id);
        assert_eq!(receiver_b.recv().await.unwrap().stream_id, stream_id);
    }

    #[tokio::test]
    async fn an_idempotent_duplicate_notifies_nobody() {
        let store = NotifyingEventStore::new(InMemoryEventStore::new(), ChangeNotifier::default());
        let mut receiver = store.subscribe();
        let stream_id = StreamId::from_name("test/stream/c");
        let event_id = EventId::new();

        store
            .append(AppendRequest {
                stream_id,
                expected_version: 0,
                events: vec![sample_event(event_id)],
            })
            .await
            .unwrap();
        // First append notified — drain it so the duplicate's absence
        // of a notification is the only thing left to observe.
        receiver.recv().await.unwrap();

        let duplicate_result = store
            .append(AppendRequest {
                stream_id,
                expected_version: 1,
                events: vec![sample_event(event_id)],
            })
            .await
            .unwrap();
        assert_eq!(duplicate_result.local_offsets, vec![None]);
        assert_eq!(duplicate_result.new_version, 1);

        // Nothing else was ever sent — a short timeout, not `recv()`
        // outright, since `recv()` with nothing sent would hang
        // forever instead of proving the negative.
        let outcome =
            tokio::time::timeout(std::time::Duration::from_millis(50), receiver.recv()).await;
        assert!(outcome.is_err(), "expected a timeout, not a notification");
    }

    #[tokio::test]
    async fn reads_pass_through_unchanged() {
        let store = NotifyingEventStore::new(InMemoryEventStore::new(), ChangeNotifier::default());
        let stream_id = StreamId::from_name("test/stream/d");
        store
            .append(AppendRequest {
                stream_id,
                expected_version: 0,
                events: vec![sample_event(EventId::new())],
            })
            .await
            .unwrap();

        let stream = store.read_stream(stream_id, 0, 100).await.unwrap();
        assert_eq!(stream.len(), 1);
        let log = store.read_log(LocalLogOffset(0), 100).await.unwrap();
        assert_eq!(log.len(), 1);
    }
}

//! `04-offline-event-log-architecture.md` §73 "Dioxus Boundary", §74
//! "Kotlin / iOS Boundary", §75 "Daemon Compatibility", §76 "Headless
//! Compatibility", §90 "Public API" — the *who may write* half of the
//! design.
//!
//! ## §90: a small API, split by authority
//!
//! §90 names five things: `EventStore`, `EventAppender`, `EventReader`,
//! `ProjectionRunner`, `SnapshotStore`. Until now there was only the
//! first of those as a single trait, which bundles *reading* and
//! *appending*. [`EventReader`] and [`EventAppender`] split it, and
//! that split is what makes the rest of this section expressible:
//!
//! - **§73 Dioxus** "sends `ApplicationCommand` and reads `ViewModel` /
//!   projection; it never writes event records directly." A UI is handed
//!   an [`EventReader`] (or a [`ReadHandle`]) and a *command channel* to
//!   the application layer. It is never handed an [`EventAppender`], so
//!   "never writes event records" is a missing capability, not a rule to
//!   remember.
//! - **§75 daemon** "daemon = sole writer; GUI/CLI = command/query
//!   clients," with "the same event APIs in-process and daemon modes."
//!   [`SoleWriter`] is the one object that can append; it can only be
//!   made once per [`WriterLease`], and it hands out [`ReadHandle`]s
//!   freely. A daemon wraps a `SoleWriter`; a GUI/CLI holds read handles
//!   (in-process) or a remote reader implementing [`EventReader`]
//!   (daemon mode). Code written against the traits cannot tell which.
//! - **§76 headless**: nothing here mentions a UI; a headless node
//!   holds a `SoleWriter` and runs
//!   [`crate::effects::EffectProcessor`] over its outbox.
//!
//! Every [`EventStore`] is automatically an [`EventReader`] *and* an
//! [`EventAppender`] (blanket impls), so existing backends and wrappers
//! (metrics, fault-injection, read-only) need no change.
//!
//! ### What the lease does and doesn't give
//!
//! [`WriterLease`] enforces "one writer" **within a process** — enough
//! to prove the API shape and to keep an in-process app and a embedded
//! daemon-in-a-thread honest. Across processes, the backend itself is
//! the arbiter: `stoolap`'s database lock stops a second process opening
//! the same file for writing, and a real daemon deployment should treat
//! *that* as the authority. The lease is not a distributed lock and does
//! not pretend to be one.
//!
//! ## §74: platform code reports facts, Rust decides
//!
//! "Platform code may report `NetworkChanged`, `AppBackgrounded`,
//! `LowMemory`. Rust decides whether a durable domain event is required.
//! Do not duplicate event semantics in Kotlin or Swift."
//! [`PlatformSignal`] is the *complete* vocabulary platform code may
//! speak — three payload-free facts — and
//! [`PlatformSignalPolicy::requires_durable_event`] is the single Rust
//! place that decides whether one becomes history. The default
//! ([`NeverDurable`]) records nothing: a network flap is telemetry
//! (§77: "do not persist every RTT/path probe as semantic history"),
//! not a fact about the user's data. A deployment that genuinely needs
//! one durable (say, "app was backgrounded mid-transfer") opts that
//! single signal in, in Rust, in review.
//!
//! ## §77-§80: how each consumer uses the log
//!
//! | section | rule | where it is enforced |
//! |---|---|---|
//! | §77 routing | consume *pending durable work*; persist the meaningful result, never every probe | [`crate::outbox`] holds the work; [`crate::effects`] emits only first-deferral/complete/fail transitions, attempts are metrics |
//! | §78 files | semantic transfer events + a high-frequency chunk journal, never an event per chunk | chunk state lives in `siar-blob-manifest`'s transfer journal; this crate's [`crate::limits`] makes a per-chunk event pointless to try |
//! | §79 DTN | persist create / store / forward / deliver / expire; keep encounter telemetry operational | same split: bundle lifecycle events are semantic, encounter counters are [`crate::effects::AttemptMetrics`]-style |
//! | §80 emergency | **commit before any radio/network attempt**; SOS stays pending until delivered, expired or cancelled | [`crate::command_flow::CommandFlow`] (commit first, effects only later) + [`crate::outbox_reconcile`]; proved end-to-end in `tests/acceptance.rs` |

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use thiserror::Error;

use crate::ids::{LocalLogOffset, StreamId};
use crate::store::{AppendRequest, AppendResult, EventStore, EventStoreError, StoredEvent};

/// The read half of [`EventStore`]. Cheap to hand to UIs, CLIs and
/// remote clients: holding one grants no ability to write.
#[async_trait]
pub trait EventReader: Send + Sync {
    async fn read_stream(
        &self,
        stream_id: StreamId,
        from_version: u64,
        limit: usize,
    ) -> Result<Vec<StoredEvent>, EventStoreError>;

    async fn read_log(
        &self,
        from_offset: LocalLogOffset,
        limit: usize,
    ) -> Result<Vec<StoredEvent>, EventStoreError>;

    async fn stream_head(&self, stream: StreamId) -> Result<u64, EventStoreError>;

    async fn latest_offset(&self) -> Result<LocalLogOffset, EventStoreError>;
}

/// The write half. Held by exactly one component (see [`SoleWriter`]).
#[async_trait]
pub trait EventAppender: Send + Sync {
    async fn append(&self, request: AppendRequest) -> Result<AppendResult, EventStoreError>;
}

#[async_trait]
impl<T: EventStore + ?Sized> EventReader for T {
    async fn read_stream(
        &self,
        stream_id: StreamId,
        from_version: u64,
        limit: usize,
    ) -> Result<Vec<StoredEvent>, EventStoreError> {
        EventStore::read_stream(self, stream_id, from_version, limit).await
    }

    async fn read_log(
        &self,
        from_offset: LocalLogOffset,
        limit: usize,
    ) -> Result<Vec<StoredEvent>, EventStoreError> {
        EventStore::read_log(self, from_offset, limit).await
    }

    async fn stream_head(&self, stream: StreamId) -> Result<u64, EventStoreError> {
        EventStore::stream_head(self, stream).await
    }

    async fn latest_offset(&self) -> Result<LocalLogOffset, EventStoreError> {
        EventStore::latest_offset(self).await
    }
}

#[async_trait]
impl<T: EventStore + ?Sized> EventAppender for T {
    async fn append(&self, request: AppendRequest) -> Result<AppendResult, EventStoreError> {
        EventStore::append(self, request).await
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum LeaseError {
    #[error("the event-log writer lease is already held")]
    AlreadyHeld,
}

/// Process-local "who is the writer". See the module doc for exactly
/// what that does and doesn't guarantee.
#[derive(Debug, Clone, Default)]
pub struct WriterLease {
    held: Arc<Mutex<bool>>,
}

impl WriterLease {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_held(&self) -> bool {
        *self.held.lock().expect("writer lease poisoned")
    }

    fn acquire(&self) -> Result<WriterToken, LeaseError> {
        let mut held = self.held.lock().expect("writer lease poisoned");
        if *held {
            return Err(LeaseError::AlreadyHeld);
        }
        *held = true;
        Ok(WriterToken {
            held: Arc::clone(&self.held),
        })
    }
}

/// Releases the lease on drop.
#[derive(Debug)]
struct WriterToken {
    held: Arc<Mutex<bool>>,
}

impl Drop for WriterToken {
    fn drop(&mut self) {
        *self.held.lock().expect("writer lease poisoned") = false;
    }
}

/// A read-only view of a shared store. Cloning is cheap; it is the
/// handle a UI, CLI or query client holds. It has **no** append method
/// and no way to get one.
pub struct ReadHandle<S: EventReader + ?Sized> {
    store: Arc<S>,
}

impl<S: EventReader + ?Sized> Clone for ReadHandle<S> {
    fn clone(&self) -> Self {
        Self {
            store: Arc::clone(&self.store),
        }
    }
}

impl<S: EventReader + ?Sized> ReadHandle<S> {
    pub async fn read_stream(
        &self,
        stream_id: StreamId,
        from_version: u64,
        limit: usize,
    ) -> Result<Vec<StoredEvent>, EventStoreError> {
        self.store.read_stream(stream_id, from_version, limit).await
    }

    pub async fn read_log(
        &self,
        from_offset: LocalLogOffset,
        limit: usize,
    ) -> Result<Vec<StoredEvent>, EventStoreError> {
        self.store.read_log(from_offset, limit).await
    }

    pub async fn stream_head(&self, stream: StreamId) -> Result<u64, EventStoreError> {
        self.store.stream_head(stream).await
    }

    pub async fn latest_offset(&self) -> Result<LocalLogOffset, EventStoreError> {
        self.store.latest_offset().await
    }
}

/// The one component allowed to append. Constructing a second against
/// the same [`WriterLease`] fails until the first is dropped.
pub struct SoleWriter<S: EventStore + ?Sized> {
    store: Arc<S>,
    _token: WriterToken,
}

impl<S: EventStore + ?Sized> SoleWriter<S> {
    pub fn acquire(lease: &WriterLease, store: Arc<S>) -> Result<Self, LeaseError> {
        Ok(Self {
            _token: lease.acquire()?,
            store,
        })
    }

    pub async fn append(&self, request: AppendRequest) -> Result<AppendResult, EventStoreError> {
        EventStore::append(&*self.store, request).await
    }

    /// Any number of read-only views of the same store.
    pub fn reader(&self) -> ReadHandle<S> {
        ReadHandle {
            store: Arc::clone(&self.store),
        }
    }
}

/// §74's complete vocabulary for platform code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PlatformSignal {
    NetworkChanged,
    AppBackgrounded,
    LowMemory,
}

/// The single Rust-side decision point of §74. See the module doc.
pub trait PlatformSignalPolicy: Send + Sync {
    fn requires_durable_event(&self, signal: PlatformSignal) -> bool;
}

/// Records nothing: platform signals are operational telemetry.
#[derive(Debug, Clone, Copy, Default)]
pub struct NeverDurable;

impl PlatformSignalPolicy for NeverDurable {
    fn requires_durable_event(&self, _signal: PlatformSignal) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::EventOrigin;
    use crate::ids::{EventId, EventTypeId, Timestamp};
    use crate::memory_store::InMemoryEventStore;
    use crate::store::NewEvent;

    fn req(version: u64) -> AppendRequest {
        AppendRequest {
            stream_id: StreamId::from_name("s"),
            expected_version: version,
            events: vec![NewEvent {
                event_id: EventId::new(),
                event_type: EventTypeId(1),
                schema_version: 1,
                created_at: Timestamp(0),
                origin: EventOrigin::System,
                correlation_id: None,
                causation_id: None,
                payload: vec![1],
            }],
        }
    }

    #[test]
    fn only_one_writer_may_exist_per_lease_and_dropping_it_releases() {
        let lease = WriterLease::new();
        let store = Arc::new(InMemoryEventStore::new());
        let w = SoleWriter::acquire(&lease, Arc::clone(&store)).unwrap();
        assert!(lease.is_held());
        assert_eq!(
            SoleWriter::acquire(&lease, Arc::clone(&store)).err(),
            Some(LeaseError::AlreadyHeld)
        );
        drop(w);
        assert!(!lease.is_held());
        assert!(SoleWriter::acquire(&lease, store).is_ok());
    }

    #[tokio::test]
    async fn readers_see_the_writers_commits_but_have_no_way_to_write() {
        let lease = WriterLease::new();
        let writer = SoleWriter::acquire(&lease, Arc::new(InMemoryEventStore::new())).unwrap();
        let gui = writer.reader();
        let cli = gui.clone();

        assert_eq!(gui.latest_offset().await.unwrap(), LocalLogOffset(0));
        writer.append(req(0)).await.unwrap();
        writer.append(req(1)).await.unwrap();

        let stream = StreamId::from_name("s");
        assert_eq!(gui.stream_head(stream).await.unwrap(), 2);
        assert_eq!(cli.latest_offset().await.unwrap(), LocalLogOffset(2));
        assert_eq!(cli.read_stream(stream, 0, 10).await.unwrap().len(), 2);
        assert_eq!(gui.read_log(LocalLogOffset(0), 10).await.unwrap().len(), 2);
        // (There is nothing to assert about *appending* from a
        // ReadHandle: the method does not exist. That absence is the
        // property — it is checked by the compiler, not at runtime.)
    }

    #[tokio::test]
    async fn every_event_store_is_a_reader_and_an_appender_including_trait_objects() {
        let store = InMemoryEventStore::new();
        let as_appender: &dyn EventAppender = &store;
        let as_reader: &dyn EventReader = &store;
        as_appender.append(req(0)).await.unwrap();
        assert_eq!(as_reader.latest_offset().await.unwrap(), LocalLogOffset(1));
        assert_eq!(
            as_reader
                .stream_head(StreamId::from_name("s"))
                .await
                .unwrap(),
            1
        );

        let dyn_store: &dyn EventStore = &store;
        assert_eq!(
            EventReader::latest_offset(dyn_store).await.unwrap(),
            LocalLogOffset(1)
        );
    }

    #[test]
    fn platform_signals_are_not_durable_by_default() {
        for s in [
            PlatformSignal::NetworkChanged,
            PlatformSignal::AppBackgrounded,
            PlatformSignal::LowMemory,
        ] {
            assert!(!NeverDurable.requires_durable_event(s));
        }
        // A deployment can opt exactly one in, in Rust.
        struct OnlyBackground;
        impl PlatformSignalPolicy for OnlyBackground {
            fn requires_durable_event(&self, s: PlatformSignal) -> bool {
                s == PlatformSignal::AppBackgrounded
            }
        }
        assert!(OnlyBackground.requires_durable_event(PlatformSignal::AppBackgrounded));
        assert!(!OnlyBackground.requires_durable_event(PlatformSignal::NetworkChanged));
    }
}

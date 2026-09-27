//! `04-offline-event-log-architecture.md` §84 "Crash Injection Tests"
//! needs a way to make one specific, real failure happen
//! deterministically, on demand, in a test — not the whole workspace
//! literally killing its own process mid-append. [`FaultInjectingEventStore`]
//! is that mechanism: a real [`EventStore`] wrapper, same shape
//! [`crate::read_only::ReadOnlyEventStore`]/[`crate::metrics::
//! MetricsEventStore`] already use, that can be told to misbehave in
//! one of two specific, real ways at its next `append` call.
//!
//! See `siar-event-log/tests/crash_injection.rs` for the five
//! scenarios this makes possible, one per point §84's own list names.

use crate::ids::{LocalLogOffset, StreamId};
use crate::store::{AppendRequest, AppendResult, EventStore, EventStoreError, StoredEvent};
use async_trait::async_trait;
use std::sync::atomic::{AtomicBool, Ordering};

/// What [`FaultInjectingEventStore`] does at its next `append` call —
/// see this module's own doc comment and each variant's own for what
/// real crash scenario it stands in for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultMode {
    /// §84's own "before append": refuse before the wrapped store's
    /// own `append` is ever called. The simplest of the two — nothing
    /// happened, so there is nothing to recover.
    FailBeforeAppend,
    /// §84's own "after network effect before success marker" (and,
    /// really, every one of the spec's other crash points that comes
    /// AFTER a real durable write): the wrapped store's real `append`
    /// runs and genuinely succeeds — the data really is durably
    /// written — but this wrapper reports failure to the caller
    /// anyway, simulating a crash or lost response between "storage
    /// committed" and "caller learned of success." A caller can't
    /// tell this apart from a real failure and must retry as if it
    /// were one — proving that retry is SAFE (via §24 idempotency) is
    /// the whole point of testing this mode.
    SucceedInnerButReportFailure,
}

/// See this module's own doc comment.
pub struct FaultInjectingEventStore<S: EventStore> {
    inner: S,
    /// `None` = behave normally. `Some(mode)` = the NEXT `append` call
    /// uses `mode`, then this resets to `None` — a fault fires once,
    /// not on every subsequent call, matching a real crash being one
    /// specific event in time, not a permanently broken store.
    armed: std::sync::Mutex<Option<FaultMode>>,
    /// Set the moment the wrapped store's own `append` was actually
    /// called for the armed request, regardless of what this wrapper
    /// then told the caller — a test's own way to check whether a
    /// simulated crash's write genuinely landed, independent of the
    /// (possibly fake) `Result` the caller received.
    inner_append_was_called: AtomicBool,
}

impl<S: EventStore> FaultInjectingEventStore<S> {
    pub fn new(inner: S) -> Self {
        Self {
            inner,
            armed: std::sync::Mutex::new(None),
            inner_append_was_called: AtomicBool::new(false),
        }
    }

    /// Arms `mode` for exactly the next `append` call.
    pub fn arm(&self, mode: FaultMode) {
        *self.armed.lock().expect("fault lock") = Some(mode);
    }

    /// See [`Self::inner_append_was_called`]'s own doc comment.
    pub fn inner_append_was_called(&self) -> bool {
        self.inner_append_was_called.load(Ordering::SeqCst)
    }

    pub fn into_inner(self) -> S {
        self.inner
    }
}

#[async_trait]
impl<S: EventStore> EventStore for FaultInjectingEventStore<S> {
    async fn append(&self, request: AppendRequest) -> Result<AppendResult, EventStoreError> {
        let mode = self.armed.lock().expect("fault lock").take();
        match mode {
            Some(FaultMode::FailBeforeAppend) => Err(EventStoreError::Backend(
                "injected fault: before append".to_string(),
            )),
            Some(FaultMode::SucceedInnerButReportFailure) => {
                let result = self.inner.append(request).await;
                self.inner_append_was_called.store(true, Ordering::SeqCst);
                // The real write's own outcome is discarded on
                // purpose — even a real failure from the inner store
                // is reported the SAME injected way, because the
                // point here is "the caller cannot trust what it was
                // told," not "and also the real error happened to
                // match."
                let _ = result;
                Err(EventStoreError::Backend(
                    "injected fault: after network effect, before success marker".to_string(),
                ))
            }
            None => {
                let result = self.inner.append(request).await;
                self.inner_append_was_called.store(true, Ordering::SeqCst);
                result
            }
        }
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

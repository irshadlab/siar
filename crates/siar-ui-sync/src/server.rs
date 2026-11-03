//! Daemon side of the sync: publishes deltas on the event bus and answers
//! snapshot / command requests.
//!
//! * **Debounced.** State changes ring a bell; the loop waits a few
//!   milliseconds so a burst of updates becomes one diff.
//! * **Bounded.** Frames ride the runtime's bounded per-subscriber queues; a
//!   slow window gets a `Gap` and re-snapshots instead of growing memory.
//! * **Consistent snapshots.** Taken while holding the sync lock, so the
//!   cursor it carries never runs ahead of its content.
//! * **Paged.** A snapshot too big for one reply is cached briefly and served
//!   page by page under an id.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use siar_runtime::{EventBus, EventPriority, Lifecycle, RuntimeError};
use siar_ui_model::StateHub;
use tokio::sync::Notify;

use crate::commands::WireCommand;
use crate::engine::{capture, SyncServer};
use crate::wire::{decode, encode, SyncFrame, SYNC_TOPIC};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotRequest {
    /// `None` starts a new snapshot (page 0); `Some` continues one.
    pub id: Option<u64>,
    pub page: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotPage {
    pub id: u64,
    pub page: u32,
    pub pages: u32,
    pub frame: SyncFrame,
}

const SNAPSHOT_CACHE: usize = 4;
const DEBOUNCE: Duration = Duration::from_millis(40);

pub struct UiSyncService {
    hub: Arc<StateHub>,
    sync: Mutex<SyncServer>,
    bus: EventBus,
    bell: Arc<Notify>,
    cache: Mutex<VecDeque<(u64, Vec<SyncFrame>)>>,
    next_id: AtomicU64,
    observer: Mutex<Option<Observer>>,
}

/// Called (outside every lock) after each publish pass, with the hub, so a
/// host can react to state changes without a second wake callback.
pub type Observer = Arc<dyn Fn(&StateHub) + Send + Sync>;

impl UiSyncService {
    /// Takes over `hub`'s wake callback (this must be a daemon-owned hub, not
    /// a window's).
    #[must_use]
    pub fn new(hub: Arc<StateHub>, bus: EventBus) -> Arc<Self> {
        let bell = Arc::new(Notify::new());
        let b = bell.clone();
        hub.set_wake(Arc::new(move || b.notify_one()));
        Arc::new(Self {
            sync: Mutex::new(SyncServer::new(bus.instance_id())),
            hub,
            bus,
            bell,
            cache: Mutex::new(VecDeque::new()),
            next_id: AtomicU64::new(1),
            observer: Mutex::new(None),
        })
    }

    /// Register the (single) observer; replaces any previous one.
    pub fn set_observer(&self, f: Observer) {
        *self.observer.lock().unwrap_or_else(PoisonError::into_inner) = Some(f);
    }

    fn observe(&self) {
        let f = self
            .observer
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some(f) = f {
            f(&self.hub);
        }
    }

    /// Publish deltas until the runtime shuts down.
    pub async fn run(self: Arc<Self>, lifecycle: Lifecycle) {
        self.publish_changes(); // baseline
        self.observe();
        loop {
            tokio::select! {
                () = self.bell.notified() => {}
                () = lifecycle.shutdown_requested() => return,
            }
            tokio::time::sleep(DEBOUNCE).await; // coalesce a burst into one diff
            self.publish_changes();
            self.observe();
        }
    }

    /// One diff-and-publish pass. Returns the number of frames sent.
    pub fn publish_changes(&self) -> usize {
        // Lock order is always sync → hub; the core only ever takes the hub.
        let frames = {
            let mut sync = self.sync.lock().unwrap_or_else(PoisonError::into_inner);
            let cap = self.hub.read(capture);
            sync.diff_captured(cap)
        };
        for f in &frames {
            self.bus
                .publish(SYNC_TOPIC, EventPriority::Normal, None, encode(f));
        }
        frames.len()
    }

    /// Answer a snapshot request (`data` is a [`SnapshotRequest`]).
    pub fn snapshot_page(&self, data: &[u8]) -> Result<Vec<u8>, RuntimeError> {
        let req: SnapshotRequest = decode(data)
            .ok_or_else(|| RuntimeError::InvalidCommand("bad snapshot request".into()))?;
        let mut cache = self.cache.lock().unwrap_or_else(PoisonError::into_inner);
        let id = match req.id {
            None => {
                let frames = {
                    let sync = self.sync.lock().unwrap_or_else(PoisonError::into_inner);
                    let cap = self.hub.read(capture);
                    sync.snapshot_captured(cap)
                };
                let id = self.next_id.fetch_add(1, Ordering::Relaxed);
                if cache.len() >= SNAPSHOT_CACHE {
                    cache.pop_front();
                }
                cache.push_back((id, frames));
                id
            }
            Some(id) => id,
        };
        let frames = &cache
            .iter()
            .find(|(i, _)| *i == id)
            .ok_or(RuntimeError::Cancelled)?
            .1;
        let page = if req.id.is_none() { 0 } else { req.page };
        let frame = frames
            .get(page as usize)
            .ok_or_else(|| RuntimeError::InvalidCommand("no such page".into()))?
            .clone();
        Ok(encode(&SnapshotPage {
            id,
            page,
            pages: u32::try_from(frames.len()).unwrap_or(u32::MAX),
            frame,
        }))
    }

    /// Decode, **re-validate** and forward a window command to the core.
    pub fn command(&self, data: &[u8]) -> Result<(), RuntimeError> {
        let cmd: WireCommand =
            decode(data).ok_or_else(|| RuntimeError::InvalidCommand("malformed command".into()))?;
        let cmd = cmd
            .validate()
            .map_err(|e| RuntimeError::InvalidCommand(e.to_string()))?;
        if self.hub.dispatch(cmd.into_app()) {
            Ok(())
        } else {
            Err(RuntimeError::Busy)
        }
    }
}

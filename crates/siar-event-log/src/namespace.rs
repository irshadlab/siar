//! `04-offline-event-log-architecture.md` §64 "Multi-Tenant Isolation"
//! and §65 "Multiple Identities".
//!
//! §64: "`TenantId` must participate in storage/stream namespace. One
//! tenant must never see another tenant's streams or projections." §65:
//! personal/work identities on one device "require isolated stream
//! namespace, projections, replication policy, outbox. Do not
//! accidentally share event state." Same requirement, two names — one
//! mechanism: a [`NamespaceId`].
//!
//! ## Isolation by construction, not by filtering
//!
//! The tempting design is one shared database with a namespace column
//! (or a namespace folded into every [`StreamId`]) and every query
//! filtered. It does not meet §64/§65 here, for a structural reason:
//! [`crate::store::EventStore::read_log`] is a *global* read of the
//! physical log (it is what projections, backup and reconciliation all
//! walk), and a stream id is a one-way hash, so a shared log cannot be
//! filtered back to one tenant's events after the fact. Every consumer
//! of `read_log` would have to remember to filter — and the first that
//! forgets leaks a tenant.
//!
//! So each namespace gets its **own store, own outbox, own snapshots,
//! own cursors, own checkpoints** — whatever the caller's `T` bundles —
//! opened from a path derived from the namespace. A tenant's code is
//! only ever handed its own `T`; there is no handle through which
//! another's data is reachable, and nothing to forget to filter.
//! [`NamespaceRegistry`] is the one place that hands them out.
//!
//! ## Defence in depth: namespaced stream ids
//!
//! [`namespaced_stream_id`] additionally folds the namespace into stream
//! ids. It isn't what provides isolation (separate stores are), but it
//! means an event or cursor that is somehow misrouted — a replication
//! frame delivered to the wrong identity, a backup restored into the
//! wrong store — names a stream that *does not exist* there and is
//! rejected as a gap/divergence rather than silently merging into the
//! other identity's conversation of the same name.
//!
//! ## Namespace ids are untrusted input to the filesystem
//!
//! A tenant or identity name ends up in a path. [`NamespaceId::new`]
//! therefore accepts only `[a-z0-9_-]`, 1-64 characters, not starting
//! with `-`: no `.`, no separators, so `..`, absolute paths and
//! platform-specific tricks cannot be expressed at all, rather than
//! being sanitized after the fact.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use thiserror::Error;

use crate::ids::StreamId;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NamespaceId(String);

#[derive(Debug, Error, PartialEq, Eq)]
pub enum NamespaceError {
    #[error("namespace id must be 1-64 characters of [a-z0-9_-] and not start with '-': {0:?}")]
    Invalid(String),
}

impl NamespaceId {
    pub fn new(id: &str) -> Result<Self, NamespaceError> {
        let ok = !id.is_empty()
            && id.len() <= 64
            && !id.starts_with('-')
            && id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-');
        if ok {
            Ok(Self(id.to_string()))
        } else {
            Err(NamespaceError::Invalid(id.to_string()))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// `base/<namespace>` — safe to join because [`NamespaceId`] cannot
/// contain a separator or `.`.
pub fn namespace_path(base: &Path, namespace: &NamespaceId) -> PathBuf {
    base.join(namespace.as_str())
}

/// Stream id for `name` *within* `namespace`. Length-prefixed so that
/// ("a", "b/c") and ("a/b", "c") can never collide.
pub fn namespaced_stream_id(namespace: &NamespaceId, name: &str) -> StreamId {
    StreamId::from_name(&format!(
        "ns:{}:{}/{}",
        namespace.0.len(),
        namespace.0,
        name
    ))
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum RegistryError<E> {
    #[error(transparent)]
    Namespace(#[from] NamespaceError),
    #[error("opening namespace failed: {0}")]
    Open(E),
}

type Opener<T, E> = Box<dyn Fn(&NamespaceId) -> Result<T, E> + Send + Sync>;

/// Hands out one `T` per namespace, opening it lazily and exactly once.
/// `T` is whatever bundle a deployment isolates (store + outbox +
/// scopes + cursors + snapshots...).
pub struct NamespaceRegistry<T, E> {
    open: Opener<T, E>,
    opened: Mutex<HashMap<NamespaceId, Arc<T>>>,
}

impl<T, E> NamespaceRegistry<T, E> {
    pub fn new(open: impl Fn(&NamespaceId) -> Result<T, E> + Send + Sync + 'static) -> Self {
        Self {
            open: Box::new(open),
            opened: Mutex::new(HashMap::new()),
        }
    }

    /// Validates `id`, then returns that namespace's `T`, opening it on
    /// first use. The same namespace always yields the same `Arc`.
    pub fn get(&self, id: &str) -> Result<Arc<T>, RegistryError<E>> {
        let ns = NamespaceId::new(id)?;
        let mut map = self.opened.lock().expect("namespace registry poisoned");
        if let Some(t) = map.get(&ns) {
            return Ok(Arc::clone(t));
        }
        let t = Arc::new((self.open)(&ns).map_err(RegistryError::Open)?);
        map.insert(ns, Arc::clone(&t));
        Ok(t)
    }

    /// Namespaces opened so far, sorted.
    pub fn opened(&self) -> Vec<NamespaceId> {
        let mut v: Vec<_> = self
            .opened
            .lock()
            .expect("namespace registry poisoned")
            .keys()
            .cloned()
            .collect();
        v.sort();
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::EventOrigin;
    use crate::ids::{EventId, EventTypeId, LocalLogOffset, Timestamp};
    use crate::outbox::{EffectKind, Outbox, OutboxEntry};
    use crate::stoolap_store::StoolapEventStore;
    use crate::store::{AppendRequest, EventStore, EventStoreError, NewEvent};

    fn ns(s: &str) -> NamespaceId {
        NamespaceId::new(s).unwrap()
    }

    #[test]
    fn only_filesystem_safe_ids_are_accepted() {
        for ok in ["personal", "work", "tenant-42", "a_b", "x", &"a".repeat(64)] {
            assert!(NamespaceId::new(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            "..",
            ".",
            "a/b",
            "a\\b",
            "../etc",
            "/abs",
            "-lead",
            "Upper",
            "sp ace",
            "tab\t",
            "nul\0",
            "dot.ted",
            "ünï",
            &"a".repeat(65),
        ] {
            assert!(NamespaceId::new(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn paths_stay_under_the_base() {
        let base = Path::new("/data/siar");
        let p = namespace_path(base, &ns("work"));
        assert_eq!(p, Path::new("/data/siar/work"));
        assert!(p.starts_with(base));
    }

    #[test]
    fn stream_ids_are_namespaced_and_unambiguous() {
        let a = namespaced_stream_id(&ns("personal"), "conversation/1");
        let b = namespaced_stream_id(&ns("work"), "conversation/1");
        assert_ne!(a, b, "same name, different namespace");
        assert_eq!(a, namespaced_stream_id(&ns("personal"), "conversation/1"));
        // Concatenation ambiguity: ("a", "b/c") vs ("a/b"...) cannot arise
        // from valid ids, but ("ab", "c") must still differ from ("a", "bc").
        assert_ne!(
            namespaced_stream_id(&ns("ab"), "c"),
            namespaced_stream_id(&ns("a"), "bc")
        );
    }

    fn ev(payload: &[u8]) -> NewEvent {
        NewEvent {
            event_id: EventId::new(),
            event_type: EventTypeId(1),
            schema_version: 1,
            created_at: Timestamp(0),
            origin: EventOrigin::System,
            correlation_id: None,
            causation_id: None,
            payload: payload.to_vec(),
        }
    }

    fn registry(base: PathBuf) -> NamespaceRegistry<StoolapEventStore, EventStoreError> {
        NamespaceRegistry::new(move |id| {
            std::fs::create_dir_all(&base).ok();
            StoolapEventStore::open(namespace_path(&base, id).to_str().unwrap())
        })
    }

    #[tokio::test]
    async fn two_identities_on_one_device_share_nothing() {
        let base = std::env::temp_dir().join(format!("siar-ns-{}", uuid::Uuid::new_v4()));
        let reg = registry(base.clone());
        let personal = reg.get("personal").unwrap();
        let work = reg.get("work").unwrap();
        assert!(!Arc::ptr_eq(&personal, &work));
        assert!(
            Arc::ptr_eq(&personal, &reg.get("personal").unwrap()),
            "opened once"
        );

        // The SAME conversation name in both identities.
        let name = "conversation/1";
        personal
            .append(AppendRequest {
                stream_id: namespaced_stream_id(&ns("personal"), name),
                expected_version: 0,
                events: vec![ev(b"personal secret")],
            })
            .await
            .unwrap();
        let entry = OutboxEntry::new(
            EventId::new(),
            namespaced_stream_id(&ns("personal"), name),
            EffectKind(1),
            0,
            b"pending personal send".to_vec(),
            Timestamp(0),
        )
        .unwrap();
        personal.outbox().unwrap().enqueue(entry).await.unwrap();

        // Work sees: no events in the global log, no outbox work, an
        // empty stream even under the identical name, and even under
        // the *personal* stream id.
        assert!(work
            .read_log(LocalLogOffset(0), 100)
            .await
            .unwrap()
            .is_empty());
        assert_eq!(work.outbox().unwrap().counts().await.unwrap().pending, 0);
        for id in [
            namespaced_stream_id(&ns("work"), name),
            namespaced_stream_id(&ns("personal"), name),
        ] {
            assert!(work.read_stream(id, 0, 10).await.unwrap().is_empty());
        }
        // Work's own data is independent: version numbering starts fresh.
        work.append(AppendRequest {
            stream_id: namespaced_stream_id(&ns("work"), name),
            expected_version: 0,
            events: vec![ev(b"work")],
        })
        .await
        .unwrap();
        assert_eq!(
            personal
                .read_log(LocalLogOffset(0), 100)
                .await
                .unwrap()
                .len(),
            1
        );
        assert_eq!(reg.opened(), vec![ns("personal"), ns("work")]);

        drop((personal, work, reg));
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn invalid_namespaces_never_reach_the_opener() {
        let called = Arc::new(Mutex::new(0));
        let c = Arc::clone(&called);
        let reg: NamespaceRegistry<(), ()> = NamespaceRegistry::new(move |_| {
            *c.lock().unwrap() += 1;
            Ok(())
        });
        assert!(matches!(
            reg.get("../escape"),
            Err(RegistryError::Namespace(_))
        ));
        assert_eq!(*called.lock().unwrap(), 0);
        assert!(reg.get("ok").is_ok());
        assert!(reg.get("ok").is_ok());
        assert_eq!(*called.lock().unwrap(), 1);
    }

    #[test]
    fn an_opener_failure_is_reported_and_not_cached() {
        let attempts = Arc::new(Mutex::new(0));
        let a = Arc::clone(&attempts);
        let reg: NamespaceRegistry<u8, &'static str> = NamespaceRegistry::new(move |_| {
            let mut n = a.lock().unwrap();
            *n += 1;
            if *n == 1 {
                Err("disk busy")
            } else {
                Ok(7)
            }
        });
        assert_eq!(reg.get("t").err(), Some(RegistryError::Open("disk busy")));
        assert_eq!(*reg.get("t").unwrap(), 7, "a later attempt can succeed");
    }
}

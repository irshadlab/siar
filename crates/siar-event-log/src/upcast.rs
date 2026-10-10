//! `04-offline-event-log-architecture.md` §9 "Versioned Event Schemas"
//! — the migration half, §92 Phase 7 ("migration tests").
//!
//! §9: "never permanently serialize current domain structs … if
//! semantics change: V1 → V2, **or upcast old data deterministically**.
//! Do not silently reinterpret old bytes using changed Rust structs."
//! [`UpcasterRegistry`] is the "upcast deterministically" option: a
//! chain of explicit, registered `V(n) → V(n+1)` byte transforms.
//!
//! ## Rules this enforces
//!
//! - **Explicit steps only.** A payload moves one version at a time
//!   through a registered step. A missing step is
//!   [`UpcastError::NoUpcastPath`] — never a best-effort guess at
//!   reinterpreting bytes.
//! - **Never read the future.** A payload whose `schema_version` is
//!   *newer* than the registered latest (written by a newer build,
//!   replicated from a newer peer, or restored from a newer backup) is
//!   [`UpcastError::FromFuture`]. Treating it as "probably the latest"
//!   is exactly the silent reinterpretation §9 forbids. The caller keeps
//!   the event stored and unread until a build that understands it runs.
//! - **The log is not rewritten.** Upcasting is a read-time transform:
//!   [`upcast_stored`] returns a converted *copy*. The committed bytes
//!   and their §22 checksum stay as written (§10).
//! - **Deterministic and pure.** A step is a plain `fn(&[u8]) ->
//!   Result<Vec<u8>, _>` — no captures, no clock, no IO — so the same
//!   input always yields the same output, which is what makes replay
//!   (§17) and the golden tests below meaningful. (Function pointers,
//!   not boxed closures: no allocation, no dynamic dispatch.)
//!
//! Steps change *encoding*; they cannot invent facts. If V2 needs a
//! field V1 never recorded, the step supplies an explicit documented
//! default — or the domain should model it as a new event type.

use std::collections::HashMap;

use thiserror::Error;

use crate::ids::EventTypeId;
use crate::store::StoredEvent;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum UpcastError {
    #[error("no registered upcast step for event type {event_type:?} from schema version {from}")]
    NoUpcastPath { event_type: EventTypeId, from: u16 },
    #[error(
        "event type {event_type:?} has schema version {found}, newer than the latest known ({latest}); \
         refusing to reinterpret it"
    )]
    FromFuture {
        event_type: EventTypeId,
        found: u16,
        latest: u16,
    },
    #[error("upcast step for {event_type:?} v{from} failed: {reason}")]
    StepFailed {
        event_type: EventTypeId,
        from: u16,
        reason: String,
    },
    #[error("event type {0:?} has no registered latest schema version")]
    UnknownType(EventTypeId),
    #[error("a step for {event_type:?} from v{from} is already registered")]
    DuplicateStep { event_type: EventTypeId, from: u16 },
}

/// One `V(from) → V(from + 1)` payload transform.
pub type UpcastFn = fn(&[u8]) -> Result<Vec<u8>, String>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Upcasted {
    pub schema_version: u16,
    pub payload: Vec<u8>,
    /// Steps applied (0 = already latest).
    pub steps: u16,
}

#[derive(Debug, Default, Clone)]
pub struct UpcasterRegistry {
    steps: HashMap<(EventTypeId, u16), UpcastFn>,
    latest: HashMap<EventTypeId, u16>,
}

impl UpcasterRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Declares the newest schema version this build writes and
    /// understands for `event_type`. Types never declared are
    /// [`UpcastError::UnknownType`] — fail loud, not pass-through.
    pub fn set_latest(&mut self, event_type: EventTypeId, version: u16) {
        self.latest.insert(event_type, version);
    }

    /// Registers the step from `from` to `from + 1`.
    pub fn register(
        &mut self,
        event_type: EventTypeId,
        from: u16,
        step: UpcastFn,
    ) -> Result<(), UpcastError> {
        if self.steps.insert((event_type, from), step).is_some() {
            return Err(UpcastError::DuplicateStep { event_type, from });
        }
        Ok(())
    }

    pub fn upcast(
        &self,
        event_type: EventTypeId,
        from_version: u16,
        payload: &[u8],
    ) -> Result<Upcasted, UpcastError> {
        let latest = *self
            .latest
            .get(&event_type)
            .ok_or(UpcastError::UnknownType(event_type))?;
        if from_version > latest {
            return Err(UpcastError::FromFuture {
                event_type,
                found: from_version,
                latest,
            });
        }
        let mut version = from_version;
        let mut bytes: Option<Vec<u8>> = None;
        let mut steps = 0u16;
        while version < latest {
            let step = self
                .steps
                .get(&(event_type, version))
                .ok_or(UpcastError::NoUpcastPath {
                    event_type,
                    from: version,
                })?;
            let input = bytes.as_deref().unwrap_or(payload);
            let out = step(input).map_err(|reason| UpcastError::StepFailed {
                event_type,
                from: version,
                reason,
            })?;
            bytes = Some(out);
            version += 1;
            steps += 1;
        }
        Ok(Upcasted {
            schema_version: version,
            payload: bytes.unwrap_or_else(|| payload.to_vec()),
            steps,
        })
    }

    /// Checks, at startup, that every type has an unbroken chain from
    /// `oldest_supported` to its latest — turning a runtime
    /// `NoUpcastPath` on some user's old event into a failed unit test.
    pub fn verify_chain(
        &self,
        event_type: EventTypeId,
        oldest_supported: u16,
    ) -> Result<(), UpcastError> {
        let latest = *self
            .latest
            .get(&event_type)
            .ok_or(UpcastError::UnknownType(event_type))?;
        for v in oldest_supported..latest {
            if !self.steps.contains_key(&(event_type, v)) {
                return Err(UpcastError::NoUpcastPath {
                    event_type,
                    from: v,
                });
            }
        }
        Ok(())
    }
}

/// A converted *copy* of `event` at the latest schema version. The
/// stored event is untouched — see the module doc.
pub fn upcast_stored(
    registry: &UpcasterRegistry,
    event: &StoredEvent,
) -> Result<StoredEvent, UpcastError> {
    let env = &event.envelope;
    let up = registry.upcast(env.event_type, env.schema_version, &env.payload)?;
    let mut out = event.clone();
    out.envelope.schema_version = up.schema_version;
    out.envelope.payload = up.payload;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::EventOrigin;
    use crate::ids::{EventId, StreamId, Timestamp};
    use crate::memory_store::InMemoryEventStore;
    use crate::store::{AppendRequest, EventStore, NewEvent};
    use siar_domain::DeviceId;

    const MSG: EventTypeId = EventTypeId(1);

    // A tiny worked schema history for "MessageQueued":
    //   V1: [body bytes]
    //   V2: [flags(1)=0][body bytes]           (added a flags byte)
    //   V3: [flags(1)][len(1)][body bytes]     (added a length prefix)
    fn v1_to_v2(p: &[u8]) -> Result<Vec<u8>, String> {
        let mut out = vec![0u8];
        out.extend_from_slice(p);
        Ok(out)
    }
    fn v2_to_v3(p: &[u8]) -> Result<Vec<u8>, String> {
        let (flags, body) = p.split_first().ok_or("empty v2 payload")?;
        let len = u8::try_from(body.len()).map_err(|_| "body too long for v3")?;
        let mut out = vec![*flags, len];
        out.extend_from_slice(body);
        Ok(out)
    }

    fn registry() -> UpcasterRegistry {
        let mut r = UpcasterRegistry::new();
        r.set_latest(MSG, 3);
        r.register(MSG, 1, v1_to_v2).unwrap();
        r.register(MSG, 2, v2_to_v3).unwrap();
        r
    }

    #[test]
    fn golden_bytes_for_every_starting_version() {
        // These literals ARE the migration contract. If a change to a
        // step alters them, old data would be reinterpreted — the test
        // should fail loudly, and the answer is a new version, not an
        // edit to a shipped step (§9).
        let r = registry();
        let from1 = r.upcast(MSG, 1, b"hi").unwrap();
        assert_eq!(
            from1,
            Upcasted {
                schema_version: 3,
                payload: vec![0, 2, b'h', b'i'],
                steps: 2
            }
        );
        let from2 = r.upcast(MSG, 2, &[7, b'h', b'i']).unwrap();
        assert_eq!(from2.payload, vec![7, 2, b'h', b'i']);
        assert_eq!(from2.steps, 1);
        let from3 = r.upcast(MSG, 3, &[1, 2, b'h', b'i']).unwrap();
        assert_eq!((from3.steps, from3.payload), (0, vec![1, 2, b'h', b'i']));
    }

    #[test]
    fn upcasting_is_deterministic() {
        let r = registry();
        let a = r.upcast(MSG, 1, b"same input").unwrap();
        for _ in 0..50 {
            assert_eq!(r.upcast(MSG, 1, b"same input").unwrap(), a);
        }
    }

    #[test]
    fn a_missing_step_is_an_error_never_a_guess() {
        let mut r = UpcasterRegistry::new();
        r.set_latest(MSG, 3);
        r.register(MSG, 2, v2_to_v3).unwrap(); // v1->v2 forgotten
        assert_eq!(
            r.upcast(MSG, 1, b"x").unwrap_err(),
            UpcastError::NoUpcastPath {
                event_type: MSG,
                from: 1
            }
        );
        assert_eq!(
            r.verify_chain(MSG, 1).unwrap_err(),
            UpcastError::NoUpcastPath {
                event_type: MSG,
                from: 1
            }
        );
        assert!(r.verify_chain(MSG, 2).is_ok());
        assert!(registry().verify_chain(MSG, 1).is_ok());
    }

    #[test]
    fn a_newer_schema_than_this_build_knows_is_refused() {
        assert_eq!(
            registry().upcast(MSG, 4, b"from a newer app").unwrap_err(),
            UpcastError::FromFuture {
                event_type: MSG,
                found: 4,
                latest: 3
            }
        );
    }

    #[test]
    fn unknown_types_and_duplicate_steps_fail_loudly() {
        assert_eq!(
            registry().upcast(EventTypeId(99), 1, b"").unwrap_err(),
            UpcastError::UnknownType(EventTypeId(99))
        );
        let mut r = registry();
        assert_eq!(
            r.register(MSG, 1, v1_to_v2).unwrap_err(),
            UpcastError::DuplicateStep {
                event_type: MSG,
                from: 1
            }
        );
    }

    #[test]
    fn a_failing_step_reports_where_it_failed() {
        let r = registry();
        let long = vec![0u8; 300];
        match r.upcast(MSG, 1, &long).unwrap_err() {
            UpcastError::StepFailed { from, reason, .. } => {
                assert_eq!(from, 2);
                assert!(reason.contains("too long"));
            }
            e => panic!("unexpected {e:?}"),
        }
    }

    #[tokio::test]
    async fn upcasting_a_stored_event_leaves_the_log_untouched() {
        let store = InMemoryEventStore::new();
        let stream = StreamId::from_name("s");
        store
            .append(AppendRequest {
                stream_id: stream,
                expected_version: 0,
                events: vec![NewEvent {
                    event_id: EventId::new(),
                    event_type: MSG,
                    schema_version: 1,
                    created_at: Timestamp(0),
                    origin: EventOrigin::LocalDevice(DeviceId::new()),
                    correlation_id: None,
                    causation_id: None,
                    payload: b"hi".to_vec(),
                }],
            })
            .await
            .unwrap();
        let stored = store.read_stream(stream, 0, 1).await.unwrap().remove(0);

        let up = upcast_stored(&registry(), &stored).unwrap();
        assert_eq!(up.envelope.schema_version, 3);
        assert_eq!(up.envelope.payload, vec![0, 2, b'h', b'i']);
        assert_eq!(up.envelope.event_id, stored.envelope.event_id);
        assert_eq!(up.local_offset, stored.local_offset);

        let again = store.read_stream(stream, 0, 1).await.unwrap().remove(0);
        assert_eq!(
            again.envelope.schema_version, 1,
            "§10: committed bytes unchanged"
        );
        assert_eq!(again.envelope.payload, b"hi");
    }
}

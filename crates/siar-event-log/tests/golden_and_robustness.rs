//! `04-offline-event-log-architecture.md` §86 "Golden Event Tests" and
//! §85 "Fuzzing".
//!
//! ## §86 — pinned bytes
//!
//! "Stable event schemas should have golden encodings. Changing bytes
//! for a stable schema should require explicit protocol/storage
//! review." Every literal below is a *contract*: the exact bytes (or
//! hash) a documented, versioned format produces for fixed inputs.
//! Several are used to compare values **across devices and across
//! releases** — a stream chain anchor, a backup digest, an operation id
//! a receiver deduplicates on — so a silent change doesn't merely fail a
//! test later, it makes a restored backup fail verification or makes an
//! old receiver stop recognising a resent operation.
//!
//! If one of these fails, the right response is *not* to update the
//! literal. It is to decide whether the format is being versioned (new
//! magic / version tag, old reader kept) or whether the change is a bug.
//!
//! ## §85 — robustness against hostile bytes
//!
//! "Fuzz: event envelope decoder, event payload decoders, snapshot
//! decoder, backup/import, event upcasters. All allocations must be
//! bounded." This file is the property-test form: arbitrary and mutated
//! inputs into every decoder/verifier this crate exposes, asserting
//! *no panic* and *output no larger than input*. It runs in CI in
//! seconds. It is **not** a coverage-guided fuzzer: `fuzz/` holds
//! `cargo-fuzz` targets for that, which need a nightly toolchain and
//! were not run in this environment.

use proptest::prelude::*;
use siar_domain::DeviceId;
use siar_event_log::analytics::decode_upload;
use siar_event_log::backup::{export_since, BackupChunk, BACKUP_GENESIS};
use siar_event_log::blobref::{BlobRef, BLOB_REF_LEN};
use siar_event_log::clock::{HlcTimestamp, HLC_ENCODED_LEN};
use siar_event_log::deletion::{KeyId, SealedPayload};
use siar_event_log::envelope::{EventEnvelope, EventOrigin};
use siar_event_log::integrity::{chain_step, ChainCoverage, CHAIN_GENESIS};
use siar_event_log::namespace::{namespaced_stream_id, NamespaceId};
use siar_event_log::outbox::{EffectKind, OperationId};
use siar_event_log::search::{tokenize, SearchLimits};
use siar_event_log::snapshot::Snapshot;
use siar_event_log::upcast::UpcasterRegistry;
use siar_event_log::{
    AppendRequest, CorrelationId, EventId, EventStore, EventTypeId, InMemoryEventStore,
    LocalLogOffset, NewEvent, StreamId, Timestamp,
};

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}
fn uid(n: u8) -> uuid::Uuid {
    uuid::Uuid::from_bytes([n; 16])
}

fn golden_envelope() -> EventEnvelope {
    EventEnvelope {
        event_id: EventId::from_uuid(uid(1)),
        stream_id: StreamId::from_name("golden/stream"),
        stream_version: 3,
        event_type: EventTypeId(42),
        schema_version: 2,
        created_at: Timestamp(1_700_000_000_000),
        origin: EventOrigin::LocalDevice(DeviceId::from_uuid(uid(2))),
        correlation_id: Some(CorrelationId::from_uuid(uid(3))),
        causation_id: Some(EventId::from_uuid(uid(4))),
        payload: b"golden payload".to_vec(),
    }
}

// ---------------------------------------------------------------- §86

#[test]
fn golden_operation_id() {
    // §31's stable id: receivers deduplicate on it across restarts.
    let id = OperationId::derive(EventId::from_uuid(uid(7)), EffectKind(10), 0);
    assert_eq!(hex(id.as_bytes()), "e04e31f464628a2cdd143d1b4789a054");
}

#[test]
fn golden_stream_ids() {
    assert_eq!(
        hex(StreamId::from_name("golden/stream").as_bytes()),
        "3dca9dbe301d7da606ed58589f94bdf736d99d068a449f5f4ed8f0ac218e0b8a"
    );
    assert_eq!(
        hex(
            namespaced_stream_id(&NamespaceId::new("personal").unwrap(), "conversation/1")
                .as_bytes()
        ),
        "b2e258473ca30af522df87169cd6c798cb013e71fbb169938881c8045ae872e1"
    );
}

#[test]
fn golden_chain_hashes() {
    // §66 anchors are stored outside the DB and compared across
    // releases: this hash must not drift.
    let e = golden_envelope();
    assert_eq!(
        hex(&chain_step(&CHAIN_GENESIS, &e, ChainCoverage::Full)),
        "6444a33c7cabdeebdc1f1c23cfe942e925353fa54613156114c3e2a4acb15e46"
    );
    assert_eq!(
        hex(&chain_step(&CHAIN_GENESIS, &e, ChainCoverage::EnvelopeOnly)),
        "f726337d97aaf82747befbaaa1ab0b75d5b85683b17f6dd20fa8df8771c3aa71"
    );
}

#[test]
fn golden_snapshot_checksum() {
    let s = Snapshot::new(
        StreamId::from_name("golden/stream"),
        9,
        3,
        b"state".to_vec(),
    );
    assert_eq!(
        hex(&s.checksum),
        "414a80cf8e3fd1f67996a1ee34a64aed9d4759c75cf1fa90ea2be77e2b5bb8de"
    );
}

#[tokio::test]
async fn golden_backup_digest() {
    // A backup taken today must verify after a release upgrade.
    let store = InMemoryEventStore::new();
    let e = golden_envelope();
    store
        .append(AppendRequest {
            stream_id: e.stream_id,
            expected_version: 0,
            events: vec![NewEvent {
                event_id: e.event_id,
                event_type: e.event_type,
                schema_version: e.schema_version,
                created_at: e.created_at,
                origin: e.origin,
                correlation_id: e.correlation_id,
                causation_id: e.causation_id,
                payload: e.payload.clone(),
            }],
        })
        .await
        .unwrap();
    let c = export_since(&store, LocalLogOffset(0), BACKUP_GENESIS, 10)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        hex(&c.digest),
        "1c9a4ddb4008eff17a807f4e0ea6ac0747e3693bd077bf49eaa6c3173165605c"
    );
}

#[test]
fn golden_fixed_layout_encodings() {
    // BlobRef: BREF1 | blob_id | content_hash | size LE
    let r = BlobRef {
        blob_id: [0x11; 32],
        content_hash: [0x22; 32],
        size: 0x0102030405060708,
    };
    let mut want = b"BREF1".to_vec();
    want.extend([0x11; 32]);
    want.extend([0x22; 32]);
    want.extend(0x0102030405060708u64.to_le_bytes());
    assert_eq!(r.encode().to_vec(), want);
    assert_eq!(BLOB_REF_LEN, 77);

    // HLC: wall BE | counter BE | node
    let t = HlcTimestamp {
        wall_ms: 0x0000_0001_0000_0002,
        counter: 3,
        node: [9; 16],
    };
    let mut want = vec![0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0, 3];
    want.extend([9; 16]);
    assert_eq!(t.to_bytes().to_vec(), want);
    assert_eq!(HLC_ENCODED_LEN, 28);

    // SealedPayload: SEAL1 | key | ciphertext
    let s = SealedPayload {
        key_id: KeyId([5; 16]),
        ciphertext: vec![1, 2],
    };
    let mut want = b"SEAL1".to_vec();
    want.extend([5; 16]);
    want.extend([1, 2]);
    assert_eq!(s.encode(), want);
}

// ---------------------------------------------------------------- §85

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn small_decoders_never_panic_and_never_allocate_more_than_their_input(
        bytes in proptest::collection::vec(any::<u8>(), 0..300),
    ) {
        if let Some(s) = SealedPayload::decode(&bytes) {
            prop_assert!(s.ciphertext.len() <= bytes.len());
            prop_assert_eq!(s.encode(), bytes.clone());
        }
        let _ = BlobRef::decode(&bytes);
        let _ = HlcTimestamp::from_bytes(&bytes);
        if let Some(recs) = decode_upload(&bytes) {
            prop_assert!(recs.len() * 16 <= bytes.len());
        }
    }

    #[test]
    fn tokenizing_arbitrary_text_is_bounded_and_never_panics(text in ".{0,400}") {
        let limits = SearchLimits { max_doc_bytes: 100, max_terms_per_doc: 8, max_term_len: 6, max_query_terms: 4 };
        let terms = tokenize(&text, &limits);
        prop_assert!(terms.len() <= limits.max_terms_per_doc);
        prop_assert!(terms.iter().all(|t| t.chars().count() <= limits.max_term_len));
    }

    #[test]
    fn upcasting_arbitrary_bytes_through_arbitrary_versions_never_panics(
        payload in proptest::collection::vec(any::<u8>(), 0..64),
        from in 0u16..6,
    ) {
        fn dbl(p: &[u8]) -> Result<Vec<u8>, String> {
            if p.len() > 40 { return Err("too long".into()); }
            Ok(p.iter().chain(p.iter()).copied().collect())
        }
        let mut r = UpcasterRegistry::new();
        r.set_latest(EventTypeId(1), 3);
        r.register(EventTypeId(1), 1, dbl).unwrap();
        r.register(EventTypeId(1), 2, dbl).unwrap();
        // Whatever the input: Ok (bounded growth: at most 2 steps => 4x)
        // or a typed error. Never a panic.
        if let Ok(up) = r.upcast(EventTypeId(1), from, &payload) {
            prop_assert!(up.payload.len() <= payload.len() * 4);
        }
    }

    #[test]
    fn snapshot_verification_rejects_every_single_byte_flip(
        state in proptest::collection::vec(any::<u8>(), 1..64),
        at in 0usize..64,
        flip in 1u8..255,
    ) {
        let s = Snapshot::new(StreamId::from_name("p"), 1, 1, state);
        prop_assert!(s.verify());
        let mut t = s.clone();
        let i = at % t.state.len();
        t.state[i] ^= flip;
        prop_assert!(!t.verify());
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// §85 "backup/import": any single-field mutation of a real chunk
    /// is rejected by `verify` (never panics, never accepted).
    #[test]
    fn a_mutated_backup_chunk_never_verifies(
        which in 0usize..6,
        victim in 0usize..4,
        flip in 1u8..255,
    ) {
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let chunk: BackupChunk = rt.block_on(async {
            let store = InMemoryEventStore::new();
            let events = (0..4u8).map(|i| NewEvent {
                event_id: EventId::new(),
                event_type: EventTypeId(1),
                schema_version: 1,
                created_at: Timestamp(i as u64),
                origin: EventOrigin::System,
                correlation_id: None,
                causation_id: None,
                payload: vec![i],
            }).collect();
            store.append(AppendRequest { stream_id: StreamId::from_name("s"), expected_version: 0, events }).await.unwrap();
            export_since(&store, LocalLogOffset(0), BACKUP_GENESIS, 10).await.unwrap().unwrap()
        });
        prop_assert!(chunk.verify(None).is_ok());

        let mut bad = chunk.clone();
        let e = &mut bad.events[victim];
        match which {
            0 => e.envelope.payload.push(flip),
            1 => e.envelope.stream_version += u64::from(flip),
            2 => e.envelope.created_at = Timestamp(e.envelope.created_at.0 + u64::from(flip)),
            3 => e.envelope.event_type = EventTypeId(e.envelope.event_type.0 + u32::from(flip)),
            4 => e.envelope.schema_version += u16::from(flip),
            _ => bad.prev_digest[0] ^= flip,
        }
        prop_assert!(bad.verify(None).is_err());
    }
}

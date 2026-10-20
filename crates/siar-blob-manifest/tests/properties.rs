//! §191 property tests, §192 fuzz-style robustness, §194 corruption,
//! §196 small files. Everything a peer controls goes through a decoder
//! that must (a) never panic and (b) never accept something that breaks
//! an invariant the rest of the system relies on.

use std::collections::HashSet;
use std::sync::Arc;

use bytes::Bytes;
use proptest::prelude::*;
use siar_blob_manifest::blob_io::{BlobStore, BlobWriter};
use siar_blob_manifest::chunked_encryption::{decrypt_chunked, encrypt_chunked};
use siar_blob_manifest::descriptor::{
    BlobDescriptor, ChunkingDescriptor, EncryptionAlgorithm, EncryptionDescriptor,
};
use siar_blob_manifest::encryption::generate_blob_key;
use siar_blob_manifest::fs_store::{DurabilityPolicy, FsBlobStore, FsStoreConfig};
use siar_blob_manifest::gc::sweep;
use siar_blob_manifest::ids::BlobId;
use siar_blob_manifest::limits::ManifestLimits;
use siar_blob_manifest::manifest::build_manifest;
use siar_blob_manifest::metadata_store::InMemoryBlobMetadata;
use siar_blob_manifest::quota::StoreLimits;
use siar_blob_manifest::reference_store::{
    BlobReference, BlobRole, InMemoryReferenceStore, ObjectId, RefOp, ReferenceStore,
};
use siar_blob_manifest::resume::ResumeBitmap;
use siar_blob_manifest::storage_lifecycle::GcPolicy;
use siar_blob_manifest::verify::verify_chunk;
use siar_blob_manifest::wire::{
    decode_control, decode_manifest, validate_manifest_shape, ChunkHeaderV1, WireLimits,
    CHUNK_HEADER_LEN,
};
use siar_domain::BlobSize;
use siar_event_log::namespace::NamespaceId;

fn limits() -> WireLimits {
    WireLimits::default()
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 512, failure_persistence: None, ..ProptestConfig::default() })]

    // ---- §192: decoders never panic on hostile bytes ----

    #[test]
    fn control_decoder_survives_arbitrary_bytes(bytes in proptest::collection::vec(any::<u8>(), 0..2048)) {
        // Any outcome but a panic; and an Ok must already be validated.
        let _ = decode_control(&bytes, &limits());
    }

    #[test]
    fn control_decoder_survives_a_valid_prefix_with_garbage(tail in proptest::collection::vec(any::<u8>(), 0..512)) {
        let mut b = 1u16.to_be_bytes().to_vec();
        b.extend(tail);
        let _ = decode_control(&b, &limits());
    }

    #[test]
    fn manifest_decoder_survives_arbitrary_bytes_and_accepts_only_consistent_manifests(
        bytes in proptest::collection::vec(any::<u8>(), 0..4096)
    ) {
        if let Ok(frame) = decode_manifest(&bytes, &limits()) {
            prop_assert!(validate_manifest_shape(&frame.manifest, &limits().manifest).is_ok());
        }
    }

    #[test]
    fn chunk_headers_round_trip_and_validation_never_panics(raw in proptest::collection::vec(any::<u8>(), CHUNK_HEADER_LEN..=CHUNK_HEADER_LEN)) {
        let arr: [u8; CHUNK_HEADER_LEN] = raw.try_into().unwrap();
        let h = ChunkHeaderV1::from_bytes(&arr);
        prop_assert_eq!(h.to_bytes(), arr);
        let m = build_manifest(&[7u8; 300], 100, &ManifestLimits::default()).unwrap();
        let _ = h.validate(&m, 100);
    }

    // ---- §191: resume bitmap ----

    #[test]
    fn the_bitmap_counts_each_chunk_once_and_ranges_partition_the_file(
        n in 1usize..200,
        marks in proptest::collection::vec(0u32..400, 0..400),
    ) {
        let mut b = ResumeBitmap::new(n);
        let mut model: HashSet<u32> = HashSet::new();
        for m in &marks {
            b.mark_received(*m);              // duplicates and out-of-range allowed
            if (*m as usize) < n { model.insert(*m); }
        }
        prop_assert_eq!(b.received_count(), model.len());
        prop_assert!(b.received_count() <= b.chunk_count());
        prop_assert_eq!(b.is_complete(), model.len() == n);

        let mut covered = vec![0u8; n];
        for (s, e) in b.received_ranges() {
            prop_assert!(s < e && (e as usize) <= n);
            for i in s..e { covered[i as usize] += 1; }
        }
        for (s, e) in b.missing_ranges() {
            prop_assert!(s < e && (e as usize) <= n);
            for i in s..e { covered[i as usize] += 10; }
        }
        for (i, c) in covered.iter().enumerate() {
            let expect = if model.contains(&(i as u32)) { 1 } else { 10 };
            prop_assert_eq!(*c, expect, "chunk {} covered wrongly", i);
        }
    }

    // ---- §191: reassembly, duplicates, order ----

    #[test]
    fn any_order_with_duplicates_reassembles_exactly_the_original(
        data in proptest::collection::vec(any::<u8>(), 1..3000),
        chunk in 1u32..400,
        order_seed in any::<u64>(),
        dup_every in 1usize..5,
    ) {
        let m = build_manifest(&data, chunk, &ManifestLimits::default()).unwrap();
        let mut order: Vec<usize> = (0..m.chunks.len()).collect();
        // deterministic shuffle
        let mut s = order_seed | 1;
        for i in (1..order.len()).rev() {
            s ^= s << 13; s ^= s >> 7; s ^= s << 17;
            order.swap(i, (s as usize) % (i + 1));
        }
        let mut have: Vec<Option<Vec<u8>>> = vec![None; m.chunks.len()];
        let mut verified_bytes = 0u64;
        for (k, &i) in order.iter().enumerate() {
            let d = &m.chunks[i];
            let bytes = &data[d.offset as usize..(d.offset + u64::from(d.size)) as usize];
            prop_assert!(verify_chunk(bytes, d));
            let times = if k % dup_every == 0 { 2 } else { 1 };
            for _ in 0..times {
                if have[i].is_none() {
                    have[i] = Some(bytes.to_vec());
                    verified_bytes += u64::from(d.size);
                }  // a duplicate changes nothing
            }
        }
        prop_assert_eq!(verified_bytes, data.len() as u64);
        let rebuilt: Vec<u8> = have.into_iter().flat_map(|c| c.unwrap()).collect();
        prop_assert_eq!(rebuilt, data);
    }

    // ---- §194: corruption is always detected ----

    #[test]
    fn a_flipped_byte_in_any_chunk_fails_verification(
        data in proptest::collection::vec(any::<u8>(), 1..2000),
        chunk in 1u32..300,
        which in any::<usize>(),
        at in any::<usize>(),
    ) {
        let m = build_manifest(&data, chunk, &ManifestLimits::default()).unwrap();
        let d = &m.chunks[which % m.chunks.len()];
        let mut bytes = data[d.offset as usize..(d.offset + u64::from(d.size)) as usize].to_vec();
        let i = at % bytes.len();
        bytes[i] ^= 0x01;
        prop_assert!(!verify_chunk(&bytes, d));
    }

    #[test]
    fn a_tampered_manifest_entry_is_caught_by_shape_validation_or_blob_id(
        data in proptest::collection::vec(any::<u8>(), 2..2000),
        chunk in 1u32..300,
        which in any::<usize>(),
        field in 0u8..3,
    ) {
        let good = build_manifest(&data, chunk, &ManifestLimits::default()).unwrap();
        let mut m = good.clone();
        let i = which % m.chunks.len();
        match field {
            0 => m.chunks[i].offset = m.chunks[i].offset.wrapping_add(1),
            1 => m.chunks[i].size = m.chunks[i].size.wrapping_add(1),
            _ => m.total_size = m.total_size.wrapping_add(1),
        }
        // Whatever the shape check says, no tampered manifest may pass as
        // *valid and equal to* the real one.
        prop_assert!(m != good);
        if validate_manifest_shape(&m, &ManifestLimits::default()).is_ok() {
            // Structurally plausible — then at least one chunk's own bytes
            // must disagree with the (unchanged) hashes it carries.
            let bad = m.chunks.iter().any(|d| {
                let end = d.offset.saturating_add(u64::from(d.size));
                (end as usize) > data.len()
                    || !verify_chunk(&data[d.offset as usize..end as usize], d)
            });
            prop_assert!(bad);
        }
    }

    // ---- chunked AEAD round trip and mutation ----

    #[test]
    fn v2_round_trips_and_any_mutation_is_rejected(
        data in proptest::collection::vec(any::<u8>(), 0..1500),
        seg in 1u32..200,
        at in any::<usize>(),
        bit in 0u8..8,
    ) {
        let key = generate_blob_key();
        let ct = encrypt_chunked(&key, &data, seg).unwrap();
        prop_assert_eq!(decrypt_chunked(&key, &ct).unwrap(), data);
        let mut bad = ct.clone();
        let i = at % bad.len();
        bad[i] ^= 1 << bit;
        prop_assert!(decrypt_chunked(&key, &bad).is_err());
    }

    // ---- §196: small files carry no heavy overhead ----

    #[test]
    fn tiny_files_are_one_chunk_with_bounded_overhead(len in 1usize..=100_000) {
        let data = vec![9u8; len];
        let class = siar_blob_manifest::chunking::ChunkSizeClass::for_file_size(len as u64);
        let chunk = class.default_chunk_size(len as u64);
        let m = build_manifest(&data, chunk, &ManifestLimits::default()).unwrap();
        prop_assert_eq!(m.chunks.len(), 1, "a file up to 100 KB is a single chunk");
        // v2 framing adds a fixed header plus 16 bytes per segment.
        let ct = encrypt_chunked(&generate_blob_key(), &data, 256 * 1024).unwrap();
        prop_assert!(ct.len() <= len + 15 + 16);
    }
}

// ---- §191: GC never deletes a pinned/referenced/active blob ----

fn ns() -> NamespaceId {
    NamespaceId::new("personal").unwrap()
}

async fn seed(store: &FsBlobStore<InMemoryBlobMetadata>, bytes: &[u8]) -> BlobId {
    let m = build_manifest(bytes, bytes.len() as u32, &ManifestLimits::default()).unwrap();
    let d = BlobDescriptor {
        blob_id: m.blob_id,
        size: BlobSize::parse(bytes.len() as u64).unwrap(),
        chunking: ChunkingDescriptor {
            chunk_size: bytes.len() as u32,
        },
        encryption: EncryptionDescriptor {
            algorithm: EncryptionAlgorithm::ChaCha20Poly1305,
            nonce: [0; 12],
        },
        media_type: None,
        manifest_id: None,
    };
    let mut w = store.begin_write(d).await.unwrap();
    w.write_chunk(0, Bytes::copy_from_slice(bytes))
        .await
        .unwrap();
    w.finalize().await.unwrap();
    m.blob_id
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 24, failure_persistence: None, ..ProptestConfig::default() })]

    #[test]
    fn gc_never_deletes_a_referenced_pinned_or_active_blob(
        // per blob: (referenced, pinned, active)
        flags in proptest::collection::vec((any::<bool>(), any::<bool>(), any::<bool>()), 1..8),
        grace in 0u64..50,
    ) {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async {
            let root = std::env::temp_dir().join(format!("siar-gcprop-{}", uuid::Uuid::new_v4()));
            let store = FsBlobStore::open(
                &root,
                FsStoreConfig {
                    limits: StoreLimits::default(),
                    durability: DurabilityPolicy::None,
                    ..FsStoreConfig::default()
                },
                InMemoryBlobMetadata::new(),
            ).unwrap();
            let refs = Arc::new(InMemoryReferenceStore::new());
            let mut ids = Vec::new();
            for (i, _) in flags.iter().enumerate() {
                ids.push(seed(&store, format!("blob number {i} ............").as_bytes()).await);
            }
            let mut pinned = HashSet::new();
            let mut active = HashSet::new();
            let mut protected = HashSet::new();
            for (n, (id, (referenced, pin, act))) in ids.iter().zip(&flags).enumerate() {
                if *referenced {
                    refs.apply(&[RefOp::Add(BlobReference {
                        owner_namespace: ns(),
                        object_id: ObjectId::new(&format!("obj-{n}")).unwrap(),
                        blob_id: *id,
                        role: BlobRole::MessageAttachment,
                    })]).unwrap();
                }
                if *pin { pinned.insert(*id); }
                if *act { active.insert(*id); }
                if *referenced || *pin || *act { protected.insert(*id); }
            }
            let policy = GcPolicy { grace_period_millis: grace };
            // Several sweeps, time moving forward past any grace period.
            for now in [1u64, 10, 100, 1_000, 10_000] {
                let _ = sweep(&store, &*refs, &pinned, &active, now, &policy, 8);
            }
            for id in &ids {
                if protected.contains(id) {
                    prop_assert!(
                        store.open(*id).await.is_ok(),
                        "a protected blob was deleted"
                    );
                }
            }
            let _ = std::fs::remove_dir_all(root);
            Ok(())
        })?;
    }
}

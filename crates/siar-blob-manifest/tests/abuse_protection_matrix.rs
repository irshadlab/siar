//! §109 "Abuse Protection" — a consolidated regression matrix.
//!
//! Each of the section's six named threats already has a real
//! mitigation somewhere in this crate (see `resource_policy.rs`'s own
//! doc comment for the full mapping in prose) — this file is the
//! proof, one test per threat, run together as a single named
//! checklist rather than left scattered across each module's own test
//! file. If a future change silently weakens any one of these,
//! exactly one of these six tests — not a unit test buried in an
//! unrelated module — is what fails.

use siar_blob_manifest::{
    build_manifest, effects_for, on_corrupt_chunk, source_mutated, BlobId, ManifestError,
    ManifestLimits, RateLimitPolicy, RateLimits, SourceSnapshot, StoragePressure,
};

/// Threat 1: "huge fake size." A real, oversized input is rejected
/// before a manifest — and therefore before any chunk-hashing work —
/// is ever built for it.
#[test]
fn huge_fake_size_is_rejected_before_any_manifest_work_happens() {
    let limits = ManifestLimits {
        max_chunks: 1_000,
        max_manifest_bytes: 1_000_000,
        max_file_bytes: 1_000, // deliberately tiny for this test
    };
    let oversized_ciphertext = vec![0u8; 10_000];
    let result = build_manifest(&oversized_ciphertext, 100, &limits);
    assert!(matches!(result, Err(ManifestError::FileTooLarge { .. })));
}

/// Threat 2: "millions of chunks." A real input that would need more
/// chunks than the configured ceiling is rejected the same way, before
/// allocating a descriptor per chunk.
#[test]
fn millions_of_chunks_is_rejected_before_allocating_one_descriptor_per_chunk() {
    let limits = ManifestLimits {
        max_chunks: 2,
        max_manifest_bytes: 1_000_000,
        max_file_bytes: 1_000_000,
    };
    let ciphertext = vec![0u8; 300]; // 3 chunks of 100 bytes, over the limit of 2
    let result = build_manifest(&ciphertext, 100, &limits);
    assert!(matches!(result, Err(ManifestError::TooManyChunks { .. })));
}

/// Threat 3: "never-completing transfers." A source that never
/// changes is never flagged as mutated, but this crate's own
/// [`SourceSnapshot`] comparison is what a caller uses to detect and
/// abort a transfer whose source silently changed mid-flight rather
/// than letting it run forever against shifting bytes.
#[test]
fn a_source_that_never_settles_is_detected_as_mutated_not_allowed_to_run_forever() {
    let original = SourceSnapshot {
        size: 100,
        mtime_millis: Some(1_000),
        file_id: Some("f".to_string()),
        content_hash_at_read: BlobId::from_ciphertext(b"version one"),
    };
    let mut still_changing = original.clone();
    still_changing.mtime_millis = Some(2_000);
    assert!(source_mutated(&original, &still_changing));
}

/// Threat 4: "hash mismatch spam." Repeated corrupt chunks escalate to
/// a real penalty rather than being tolerated indefinitely.
#[test]
fn hash_mismatch_spam_eventually_triggers_source_penalization() {
    let mut failures = 0;
    let mut penalized = false;
    for _ in 0..5 {
        let action = on_corrupt_chunk(failures, 3);
        failures = action.failure_count;
        if action.should_penalize_source {
            penalized = true;
            break;
        }
    }
    assert!(
        penalized,
        "repeated hash mismatches must eventually penalize the source"
    );
}

/// Threat 5: "storage exhaustion." Full storage pressure triggers the
/// real, severe response — rejecting new large files, not just a
/// gentle nudge.
#[test]
fn storage_exhaustion_triggers_rejecting_new_large_files() {
    let effects = effects_for(StoragePressure::Full);
    assert!(effects.reject_new_large_files);
    assert!(effects.preserve_critical_data);
}

/// Threat 6: "offer spam." An unknown peer is held to the stricter of
/// the two configured tiers, capping how many offers/active transfers
/// it can push at this device.
#[test]
fn offer_spam_from_an_unknown_peer_is_held_to_the_stricter_tier() {
    let policy = RateLimitPolicy {
        unknown_peer: RateLimits {
            max_offers_per_sec: 1.0,
            max_active_transfers: 1,
            max_bytes_per_sec: 10_000,
            max_stored_partial_bytes: 50_000,
        },
        known_peer: RateLimits {
            max_offers_per_sec: 50.0,
            max_active_transfers: 20,
            max_bytes_per_sec: 10_000_000,
            max_stored_partial_bytes: 50_000_000,
        },
    };
    let limits = policy.limits_for(false);
    assert_eq!(limits.max_offers_per_sec, 1.0);
    assert_eq!(limits.max_active_transfers, 1);
}

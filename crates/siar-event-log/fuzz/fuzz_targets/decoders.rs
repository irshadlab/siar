//! §85: every byte-level decoder this crate exposes, driven by
//! coverage-guided fuzzing. Property: no panic, and no decoder returns
//! more bytes than it was given.
//!
//! Run (needs nightly + cargo-fuzz; NOT run when this was written):
//!     cargo +nightly fuzz run decoders
#![no_main]
use libfuzzer_sys::fuzz_target;
use siar_event_log::{analytics, blobref::BlobRef, clock::HlcTimestamp, deletion::SealedPayload};

fuzz_target!(|data: &[u8]| {
    if let Some(s) = SealedPayload::decode(data) {
        assert!(s.ciphertext.len() <= data.len());
        assert_eq!(s.encode(), data);
    }
    let _ = BlobRef::decode(data);
    let _ = HlcTimestamp::from_bytes(data);
    if let Some(r) = analytics::decode_upload(data) {
        assert!(r.len() * 16 <= data.len());
    }
});

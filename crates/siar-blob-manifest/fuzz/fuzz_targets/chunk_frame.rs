#![no_main]
use libfuzzer_sys::fuzz_target;
use siar_blob_manifest::ids::TransferId;
use siar_blob_manifest::limits::ManifestLimits;
use siar_blob_manifest::manifest::build_manifest;
use siar_blob_manifest::wire::{peek_chunk_transfer_id, read_chunk_frame};

fuzz_target!(|data: &[u8]| {
    let manifest = build_manifest(&[7u8; 4096], 1024, &ManifestLimits::default()).unwrap();
    let id = peek_chunk_transfer_id(data).unwrap_or_else(|_| TransferId::new());
    let mut cursor = data;
    // Whatever the bytes, no panic and no allocation beyond the ceiling.
    if let Ok((header, body)) = read_chunk_frame(&mut cursor, id, &manifest, 1024) {
        assert!(body.len() <= 1024);
        assert_eq!(body.len() as u32, header.size);
    }
});

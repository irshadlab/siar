#![no_main]
use libfuzzer_sys::fuzz_target;
use siar_blob_manifest::chunked_encryption::decrypt_chunked;
use siar_blob_manifest::ids::BlobEncryptionKey;

fuzz_target!(|data: &[u8]| {
    // A fixed key: the fuzzer cannot forge a tag, so every input must be
    // rejected — and none may panic or over-allocate.
    let key = BlobEncryptionKey([0x42; 32]);
    assert!(decrypt_chunked(&key, data).is_err());
});

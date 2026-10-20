#![no_main]
use libfuzzer_sys::fuzz_target;
use siar_blob_manifest::wire::{decode_manifest, validate_manifest_shape, WireLimits};

fuzz_target!(|data: &[u8]| {
    let limits = WireLimits::default();
    // An accepted manifest has always passed the shape check.
    if let Ok(frame) = decode_manifest(data, &limits) {
        validate_manifest_shape(&frame.manifest, &limits.manifest).expect("accepted ⇒ valid");
    }
});

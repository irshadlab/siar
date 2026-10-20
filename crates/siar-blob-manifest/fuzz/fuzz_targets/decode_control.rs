#![no_main]
use libfuzzer_sys::fuzz_target;
use siar_blob_manifest::wire::{decode_control, encode_control, WireLimits};

fuzz_target!(|data: &[u8]| {
    let limits = WireLimits::default();
    // Never panics; anything accepted must survive a re-encode/decode.
    if let Ok(msg) = decode_control(data, &limits) {
        let again = encode_control(&msg).expect("an accepted message re-encodes");
        assert_eq!(decode_control(&again, &limits).expect("and decodes"), msg);
    }
});

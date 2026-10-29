//! §193: arbitrary bytes into the IPC frame decoder must only ever return
//! `Err` or a well-formed message — never panic, never allocate beyond the
//! 1 MiB frame limit, and anything that decodes must re-encode and decode to
//! the same value.
#![no_main]

use libfuzzer_sys::fuzz_target;
use siar_runtime::protocol::{decode_frame, encode_frame, ClientMsg, ServerMsg};

fuzz_target!(|data: &[u8]| {
    if let Ok(m) = decode_frame::<ClientMsg>(data) {
        let again = encode_frame(&m).expect("a decoded message must re-encode");
        assert_eq!(decode_frame::<ClientMsg>(&again).expect("round trip"), m);
    }
    if let Ok(m) = decode_frame::<ServerMsg>(data) {
        let again = encode_frame(&m).expect("a decoded message must re-encode");
        assert_eq!(decode_frame::<ServerMsg>(&again).expect("round trip"), m);
    }
});

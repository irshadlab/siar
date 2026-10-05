#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    siar_resource_limits::fuzz_entry::recovery_plan(data);
});

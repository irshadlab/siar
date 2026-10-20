#![no_main]
use libfuzzer_sys::fuzz_target;
use siar_blob_manifest::limits::ManifestLimits;
use siar_blob_manifest::manifest::build_manifest;
use siar_blob_manifest::manifest_builder::ManifestBuilder;

fuzz_target!(|input: (Vec<u8>, u8, u8)| {
    let (data, chunk, feed) = input;
    let chunk = u32::from(chunk).max(1);
    let feed = usize::from(feed).max(1);
    let limits = ManifestLimits::default();
    let Ok(want) = build_manifest(&data, chunk, &limits) else {
        return;
    };
    let mut b = ManifestBuilder::new(chunk, limits);
    for piece in data.chunks(feed) {
        b.update(piece).unwrap();
    }
    assert_eq!(b.finish().unwrap(), want);
});

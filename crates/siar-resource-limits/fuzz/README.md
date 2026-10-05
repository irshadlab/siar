# Fuzzing `siar-resource-limits` (spec 08 §184)

The targets are thin wrappers over `siar_resource_limits::fuzz_entry::*`,
which `assert!` the invariants for every input. The same entry points run
in `cargo test` over a deterministic corpus, so they are exercised without
any fuzzing toolchain.

```sh
cargo install cargo-fuzz
cd crates/siar-resource-limits
cargo +nightly fuzz run frame_decoder        # or any target below
```

Targets: `frame_decoder`, `persisted_settings` (resource config parser),
`quota_update` (quota update messages), `admission_request` (admission
request decoding), `negotiation_offer` (limit negotiation), `file_offer`,
`unknown_hello`, `recovery_plan`.

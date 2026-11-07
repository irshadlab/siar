# `PoKSignature::from_bytes` panics on short input and accepts trailing bytes (zkryptium 0.8.1, `bbsplus`)

Target repository: https://github.com/Cybersecurity-LINKS/zkryptium  (not filed yet — to be filed by the reporter)

## Summary

`BBSplus` proofs are parsed by `PoKSignature::<BBSplus<_>>::from_bytes`
(`src/bbsplus/proof.rs`, ~line 87). That function slices its input at fixed offsets
(`&bytes[0..48]`, `&bytes[48..96]`, … `&bytes[208..240]`) **without checking the length**, so:

1. **Any input shorter than 240 bytes panics** (index out of range) instead of returning
   `Err(Error::InvalidProofOfKnowledgeSignature)`.
2. **Trailing bytes are silently ignored.** After the fixed 240-byte prefix it reads
   `bytes[240..].chunks_exact(32)`, which drops a remainder shorter than 32 bytes. A valid proof
   with one extra byte appended is accepted, so a proof has more than one valid encoding
   (malleability).

Proofs are attacker-controlled input at any verifier, so (1) is a remote crash for anyone who
verifies proofs from untrusted peers without `catch_unwind`.

## Environment

- zkryptium 0.8.1 (released 2026-11-03), features `bbsplus`
- rustc 1.91.1, Linux x86_64

## Reproduction

`zkryptium-proof_parsing-test.rs` (attached; put it in `tests/`). Output against 0.8.1:

```
test from_bytes_never_panics_on_short_input ... FAILED
test from_bytes_rejects_trailing_bytes ... FAILED
PoKSignature::from_bytes PANICKED on a 0-byte input
a proof with 1 trailing byte(s) was accepted (len 337 instead of 336)
```

(The control assertion that a genuine proof parses passes. A genuine proof here is 336 bytes:
240 fixed + 32 per hidden message + 32 challenge, with 2 hidden messages.)

## Suggested fix

`zkryptium-proof-length.patch` (attached): reject up front anything that cannot be a well-formed
proof — shorter than 240 + 32 bytes, or with `(len - 240) % 32 != 0`:

```rust
if bytes.len() < 240 + 32 || (bytes.len() - 240) % 32 != 0 {
    return Err(Error::InvalidProofOfKnowledgeSignature);
}
```

With the patch applied, both tests above pass. (Verified by running the attached test against a
patched copy of the 0.8.1 sources.)

## Possibly related — NOT tested, listed only because the pattern looks the same

Same style of unchecked fixed-offset slicing, found by grep, not individually exercised:

- `src/bbsplus/proof.rs` ~1037 / ~1043 (`bytes[0..32]`, `bytes[32..].chunks_exact(32)`)
- `src/bbsplus/commitment.rs` ~84 / ~86
- `src/bbsplus/pseudonym.rs` ~70
- `src/cl03/keys.rs` (74–76, 116–117) and `src/cl03/signature.rs` (195–200)

A fuzz target over each `from_bytes` would settle whether those are reachable.

## Impact on a downstream user

We verify BBS proofs from unauthenticated mesh peers. We now check the exact proof length before
calling `from_bytes` and wrap the verify call in `catch_unwind`; both are workarounds that
should not be needed.

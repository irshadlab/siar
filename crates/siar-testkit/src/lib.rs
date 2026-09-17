//! The protocol-logic half of Phase 8 ("large-scale simulation, field
//! testing, battery optimization, security audit") — next.md §113–116.
//!
//! [`mesh_sim`] is a deterministic, in-memory simulation over real
//! `siar_dtn_bundle` types (not mocks) that can express next.md's own test
//! scenarios directly: §113's A-B-C-D chain, §114's partition-then-
//! bridge, §115's links appearing/disappearing between ticks. What it
//! proves is protocol-level: hop-limit-respecting eventual delivery,
//! and no duplicate storage at a node reachable by two paths — the
//! "loop prevention" and "eventual delivery" halves of §116's invariant
//! list.
//!
//! What this crate is NOT, and can't be from inside a chat sandbox with
//! no compiler: next.md §116's other invariants (packet loss, device
//! shutdown, corruption, clock skew, low storage, battery constraints,
//! real network changes) and §117's actual performance targets need
//! real hardware, real radios, and real field conditions to mean
//! anything — that half of Phase 8 was already flagged as out of scope
//! for me to build blind, and stays that way. This crate is the part of
//! Phase 8 that's genuinely decidable from the protocol's own rules
//! alone, nothing more.

pub mod mesh_sim;

//! Architecture doc §1 and `siar-media-core`'s own doc comment both draw
//! the same line: the `calls` crate should depend on
//! `VideoEncoder`/`VideoDecoder`/`VideoCapabilities`, "but not whether
//! the implementation is AV1 software or Android MediaCodec." This
//! crate holds to that literally — it has no dependency on
//! `siar-media-av1`, `siar-media-audio`, or (off Android)
//! `siar-media-android` at all. Every `pipeline.rs` function takes an
//! already-constructed `Box<dyn VideoEncoder>` (etc); deciding which
//! concrete codec that is stays one layer up, in whichever app crate
//! negotiated the call (`siar_media_core::negotiate_call`) and knows
//! what's actually available on this device. [`android`] is the one
//! exception, and only because Android's hardware path isn't a direct
//! trait impl to box up in the first place — it's a JNI queue that
//! needs bridging into the same channel shape.
//!
//! What this pass wires up (the piece explicitly deferred in
//! `siar-media-android`'s and `jni_bridge.rs`'s doc comments —
//! "`calls`-crate-level orchestration this file doesn't implement"):
//!
//! - [`pipeline`]: turns the synchronous `encode`/`decode` trait calls
//!   into async, channel-shaped pipelines — one backpressure strategy
//!   for raw capture input (always take the newest frame), a different
//!   one for encoded/decoded output (ordered, bounded, no invented drop
//!   policy).
//! - [`android`] (Android-only): the same channel shape, but backed by
//!   `siar-media-android`'s JNI `MediaSession` queues instead of a
//!   direct trait call — `CallSession` doesn't need to know which one
//!   it's holding.
//! - [`jitter`]: a small, pure, timestamp-ordered reorder buffer for
//!   decoded frames arriving out of order off the network. Pure logic,
//!   no I/O — the one piece of this crate that's actually testable
//!   without hardware, a compiler run, or a network, and has unit tests
//!   to show for it.
//! - [`session`]: ties a [`siar_domain::CallState`] machine to a pair of
//!   pipelines (one per media direction), the quality-ladder logic
//!   already in `siar_domain::adapt_quality`, and — as of this pass —
//!   [`shutdown::CallShutdown`]: `CallSession::apply_control_event`
//!   reaching `CallState::Ended` now signals every background task this
//!   crate spawned for that call to stop, closing what was previously an
//!   open leak-on-hangup gap. Every `spawn_*_pipeline` function in
//!   [`pipeline`] and [`android`] takes a `subscribe()`d shutdown
//!   token and races it against whatever that loop would otherwise
//!   block on.
//!
//! What's still explicitly NOT here, same boundary `siar-domain`'s
//! `call.rs` already drew and for the same reason (needs a real
//! network/device, not guessable from a type signature):
//!
//! - [`transport::MediaTransport`] is a trait, not an implementation —
//!   the actual QUIC media streams live in `siar-transport`, and wiring
//!   real send/receive against a live connection needs a real network
//!   to validate framing, pacing, and loss behavior against.
//! - Camera/microphone capture feeding [`pipeline::LatestFrameSlot`] and
//!   [`pipeline`]'s audio input channel — device access, same as always.
//! - Tuning the channel capacities and jitter-buffer window this crate
//!   picked (documented per-constant, not measured against a real call).
//!
//! And unchanged from every other pass in this workspace: nothing here
//! has been run through `cargo build`, on any target — this environment
//! has no Rust toolchain installed at all. Treat every line as "reads
//! correctly against the trait/type signatures it calls," not
//! "compiled." That now includes `shutdown.rs`'s `tokio::select!` usage
//! and the borrow patterns around it in `pipeline.rs`/`android.rs` — the
//! kind of thing a real `cargo build` finds fast if it's wrong, so treat
//! the next compile as the actual test of this specific addition.

pub mod jitter;
pub mod pipeline;
pub mod session;
pub mod shutdown;
pub mod transport;

#[cfg(target_os = "android")]
pub mod android;

//! Bridges `siar-media-android`'s JNI-facing `MediaSession` queues into
//! the same [`VideoEncodePipeline`]/[`VideoDecodePipeline`] shape
//! `pipeline.rs` uses for the desktop software path, so
//! `session.rs`'s `CallSession` never needs a
//! `#[cfg(target_os = "android")]` of its own.
//!
//! This is exactly the piece both `siar-media-android/src/lib.rs` and
//! `jni_bridge.rs` flagged as intentionally not implemented —
//! "`calls`-crate-level orchestration this file doesn't implement."
//! It's implemented here now, but still unverified beyond "the types
//! line up and the logic reads correctly against `jni_bridge.rs`'s
//! actual field/function signatures": there is no Android target,
//! emulator, or compiler available in this environment to run it
//! against. Treat this module the same as the rest of
//! `siar-media-android` — it needs a real device pass before it's
//! trusted in a live call.
//!
//! Both spawn functions below take a
//! [`crate::shutdown::CallShutdown`] receiver and race it in every task,
//! same as `pipeline.rs`'s software path — the JNI queues themselves
//! have no shutdown concept of their own, so a hung-up call still needs
//! this crate's own signal to stop polling `MediaSession`.

use std::sync::{Arc, Mutex};

use siar_media_android::{
    drain_output, output_ready_notifier, push_input, MediaSession, SessionOutput,
};
use siar_media_core::{DecodedVideoFrame, EncodedVideoFrame, RawVideoFrame};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::pipeline::{select_shutdown, LatestFrameSlot, VideoDecodePipeline, VideoEncodePipeline};

/// Encode direction: raw camera frames in (via `input.put`, same as the
/// software path), Android hardware-encoded bytes out.
///
/// Two background tasks:
/// - the **feeder** takes each new raw frame and hands its packed
///   Y/U/V planes to `push_input`, for Kotlin's `nextRawFrame`/
///   `nextRawFrameTimestampUs` to pull (see `jni_bridge.rs`'s own doc
///   comment on why that pull-pair exists);
/// - the **drainer** awaits `output_ready_notifier` rather than polling
///   `output_queue` on a fixed interval, so there's no invented
///   poll-interval-vs-latency tradeoff to tune.
pub fn spawn_android_hardware_encode_pipeline(
    session: Arc<Mutex<MediaSession>>,
    output_capacity: usize,
    shutdown: CancellationToken,
) -> VideoEncodePipeline {
    let input = LatestFrameSlot::new();
    let (tx, rx) = mpsc::channel(output_capacity);

    let feeder_input = Arc::clone(&input);
    let feeder_session = Arc::clone(&session);
    let feeder_shutdown = shutdown.clone();
    tokio::spawn(async move {
        loop {
            let frame: RawVideoFrame =
                match select_shutdown(&feeder_shutdown, feeder_input.take()).await {
                    Some(frame) => frame,
                    None => return, // shutdown signaled — call ended, stop feeding capture
                };
            // One packed buffer, not three separate `push_input` calls —
            // `jni_bridge.rs`'s `pop_input`/`nextRawFrame` contract pulls
            // one `ByteArray` per frame; Kotlin's `HardwareVideoEncoder`
            // is the side that knows this frame's resolution and
            // therefore how to re-split Y/U/V back out of it.
            let mut packed =
                Vec::with_capacity(frame.y_plane.len() + frame.u_plane.len() + frame.v_plane.len());
            packed.extend_from_slice(&frame.y_plane);
            packed.extend_from_slice(&frame.u_plane);
            packed.extend_from_slice(&frame.v_plane);
            push_input(&feeder_session, packed, frame.timestamp_micros as i64);
        }
    });

    let drainer_shutdown = shutdown;
    tokio::spawn(async move {
        let notify = output_ready_notifier(&session);
        loop {
            // See `LatestFrameSlot::take`'s doc comment for why the
            // `Notified` future is created before draining, not after —
            // same missed-wakeup hazard, same fix.
            let notified = notify.notified();
            for item in drain_output(&session) {
                match item {
                    SessionOutput::Encoded {
                        codec,
                        data,
                        is_keyframe,
                        timestamp_micros,
                    } => {
                        let frame = EncodedVideoFrame {
                            codec,
                            data,
                            is_keyframe,
                            timestamp_micros: timestamp_micros as u64,
                        };
                        if tx.send(frame).await.is_err() {
                            return; // receiver dropped — pipeline shutting down
                        }
                    }
                    SessionOutput::Decoded { .. } => {
                        // A decode result arriving on what this caller
                        // opened as an encode session means `create_session`'s
                        // `kind` and this function's assumption disagree —
                        // a real bug, but one that needs a real session to
                        // ever actually trigger and observe. Flagged rather
                        // than silently matched away.
                        tracing::error!(
                            "encode pipeline received a Decoded output — session kind mismatch"
                        );
                    }
                }
            }
            if select_shutdown(&drainer_shutdown, notified).await.is_none() {
                return; // shutdown signaled — call ended, stop draining
            }
        }
    });

    VideoEncodePipeline { input, output: rx }
}

/// Decode direction: encoded network bytes in, Android hardware-decoded
/// raw frames out. Symmetric to the encode pipeline above, with the
/// feeder/drainer roles swapped relative to which queue each one
/// touches.
pub fn spawn_android_hardware_decode_pipeline(
    session: Arc<Mutex<MediaSession>>,
    input_capacity: usize,
    output_capacity: usize,
    shutdown: CancellationToken,
) -> VideoDecodePipeline {
    let (in_tx, mut in_rx) = mpsc::channel::<EncodedVideoFrame>(input_capacity);
    let (out_tx, out_rx) = mpsc::channel(output_capacity);

    let feeder_session = Arc::clone(&session);
    let feeder_shutdown = shutdown.clone();
    tokio::spawn(async move {
        loop {
            let frame = match select_shutdown(&feeder_shutdown, in_rx.recv()).await {
                Some(Some(frame)) => frame,
                Some(None) => return, // sender dropped — upstream is gone
                None => return,       // shutdown signaled
            };
            push_input(&feeder_session, frame.data, frame.timestamp_micros as i64);
        }
    });

    let drainer_shutdown = shutdown;
    tokio::spawn(async move {
        let notify = output_ready_notifier(&session);
        loop {
            let notified = notify.notified();
            for item in drain_output(&session) {
                match item {
                    SessionOutput::Decoded { frame, .. } => {
                        if out_tx.send(DecodedVideoFrame { frame }).await.is_err() {
                            return;
                        }
                    }
                    SessionOutput::Encoded { .. } => {
                        tracing::error!(
                            "decode pipeline received an Encoded output — session kind mismatch"
                        );
                    }
                }
            }
            if select_shutdown(&drainer_shutdown, notified).await.is_none() {
                return;
            }
        }
    });

    VideoDecodePipeline {
        input: in_tx,
        output: out_rx,
    }
}

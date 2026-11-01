//! Bridges `siar-media-core`'s synchronous `VideoEncoder`/`VideoDecoder`/
//! `AudioEncoder`/`AudioDecoder` traits into the async, channel-based
//! shape the rest of this crate (and `android.rs`'s hardware path, which
//! matches this same shape) works with.
//!
//! Two different backpressure strategies, deliberately:
//! - Raw capture -> encoder input is a [`LatestFrameSlot`]: a live call's
//!   capture almost always produces frames faster than a realtime
//!   encoder consumes them, and the *newest* frame is what's worth
//!   encoding next — not a growing backlog of stale ones. Overwriting is
//!   correct here, not a bug.
//! - Encoded/decoded output is an ordered, bounded `mpsc` channel:
//!   reordering or silently dropping frames on the way to the network
//!   (or the render loop) isn't this layer's call to make. If the
//!   channel fills, that means the consumer on the other end is
//!   stalled — `send`/`recv` will simply apply backpressure, which is
//!   the correct default until a caller has a specific, measured reason
//!   to do something else.
//!
//! Every `encode`/`decode` call runs inside `tokio::task::spawn_blocking`
//! — these are synchronous, CPU-bound (or, for Android, JNI-bound) calls
//! per `siar-media-core`'s own trait signatures, and running them
//! directly on an async task would block that task's executor thread.
//!
//! Every `spawn_*_pipeline` function here also takes a
//! [`crate::shutdown::CallShutdown`] receiver and races it against
//! whatever that loop would otherwise block on, so a hangup
//! (`CallSession::apply_control_event` reaching `CallState::Ended`)
//! actually stops these tasks instead of leaking them for the rest of
//! the process's life.

use std::sync::{Arc, Mutex};

use siar_media_core::{
    AudioDecoder, AudioEncoder, DecodedVideoFrame, EncodedAudioFrame, EncodedVideoFrame,
    RawVideoFrame, VideoDecoder, VideoEncoder,
};
use tokio::sync::{mpsc, Notify};
use tokio_util::sync::CancellationToken;

/// Always holds only the most recently `put` value. `take` waits for a
/// value to become available rather than returning `None` — there's
/// nothing meaningful for a realtime encoder loop to do with "no frame
/// yet" other than wait for one.
pub struct LatestFrameSlot<T> {
    slot: Mutex<Option<T>>,
    notify: Notify,
}

impl<T> LatestFrameSlot<T> {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            slot: Mutex::new(None),
            notify: Notify::new(),
        })
    }

    pub fn put(&self, value: T) {
        *self.slot.lock().expect("LatestFrameSlot poisoned") = Some(value);
        self.notify.notify_one();
    }

    /// Standard `Notify` pattern to avoid a missed-wakeup race: the
    /// `notified()` future is created *before* checking the slot, so a
    /// `put` that lands between the check and the `.await` still wakes
    /// this up (`Notify::notify_one` is not lost just because nobody was
    /// `.await`ing yet at the exact instant it was called, as long as
    /// the `Notified` future already existed).
    pub async fn take(&self) -> T {
        loop {
            let notified = self.notify.notified();
            if let Some(value) = self.slot.lock().expect("LatestFrameSlot poisoned").take() {
                return value;
            }
            notified.await;
        }
    }
}

/// Races `shutdown.cancelled()` against `future`. `Some(value)` is the
/// normal case; `None` means shutdown fired first. Lives here rather
/// than duplicated per call site since every loop below (and every loop
/// in `android.rs`) needs exactly this race.
pub(crate) async fn select_shutdown<F: std::future::Future>(
    shutdown: &CancellationToken,
    future: F,
) -> Option<F::Output> {
    tokio::select! {
        // Shutdown first: once a call has ended, never start another unit of work.
        biased;
        () = shutdown.cancelled() => None,
        value = future => Some(value),
    }
}

pub struct VideoEncodePipeline {
    pub input: Arc<LatestFrameSlot<RawVideoFrame>>,
    pub output: mpsc::Receiver<EncodedVideoFrame>,
}

/// `encoder` is boxed and platform-agnostic on purpose: the caller
/// passes `siar-media-av1`'s `Av1SoftwareEncoder` on desktop today; the
/// same `VideoEncodePipeline` shape is also what [`crate::android`]
/// produces for the hardware path, so `session.rs` never has to care
/// which one it's holding.
pub fn spawn_video_encode_pipeline(
    mut encoder: Box<dyn VideoEncoder>,
    output_capacity: usize,
    shutdown: CancellationToken,
) -> VideoEncodePipeline {
    let input = LatestFrameSlot::new();
    let (tx, rx) = mpsc::channel(output_capacity);

    let task_input = Arc::clone(&input);
    let handle = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || loop {
        let frame = match handle.block_on(select_shutdown(&shutdown, task_input.take())) {
            Some(frame) => frame,
            None => return, // shutdown signaled — call ended, stop encoding
        };
        match encoder.encode(&frame) {
            Ok(encoded) => {
                if tx.blocking_send(encoded).is_err() {
                    return; // receiver dropped — pipeline is shutting down
                }
            }
            Err(err) => {
                tracing::warn!(
                    ?err,
                    "video encode failed for one frame, dropping it and continuing"
                );
            }
        }
    });

    VideoEncodePipeline { input, output: rx }
}

pub struct VideoDecodePipeline {
    pub input: mpsc::Sender<EncodedVideoFrame>,
    pub output: mpsc::Receiver<DecodedVideoFrame>,
}

pub fn spawn_video_decode_pipeline(
    mut decoder: Box<dyn VideoDecoder>,
    input_capacity: usize,
    output_capacity: usize,
    shutdown: CancellationToken,
) -> VideoDecodePipeline {
    let (in_tx, mut in_rx) = mpsc::channel::<EncodedVideoFrame>(input_capacity);
    let (out_tx, out_rx) = mpsc::channel(output_capacity);

    let handle = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || loop {
        let encoded = match handle.block_on(select_shutdown(&shutdown, in_rx.recv())) {
            Some(Some(encoded)) => encoded,
            Some(None) => return, // sender dropped — upstream is gone
            None => return,       // shutdown signaled
        };
        match decoder.decode(&encoded) {
            Ok(decoded) => {
                if out_tx.blocking_send(decoded).is_err() {
                    return;
                }
            }
            Err(err) => {
                tracing::warn!(
                    ?err,
                    "video decode failed for one frame, dropping it and continuing"
                );
            }
        }
    });

    VideoDecodePipeline {
        input: in_tx,
        output: out_rx,
    }
}

pub struct AudioEncodePipeline {
    /// Interleaved 16-bit PCM chunks, matching `AudioEncoder::encode`'s
    /// own input shape — resampling/chunking raw device samples into
    /// this shape is capture's job (`siar-media-audio::capture`), not
    /// this pipeline's.
    pub input: mpsc::Sender<Vec<i16>>,
    pub output: mpsc::Receiver<EncodedAudioFrame>,
}

pub fn spawn_audio_encode_pipeline(
    mut encoder: Box<dyn AudioEncoder>,
    input_capacity: usize,
    output_capacity: usize,
    shutdown: CancellationToken,
) -> AudioEncodePipeline {
    let (in_tx, mut in_rx) = mpsc::channel::<Vec<i16>>(input_capacity);
    let (out_tx, out_rx) = mpsc::channel(output_capacity);

    let handle = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || loop {
        let pcm = match handle.block_on(select_shutdown(&shutdown, in_rx.recv())) {
            Some(Some(pcm)) => pcm,
            Some(None) => return,
            None => return,
        };
        match encoder.encode(&pcm) {
            Ok(encoded) => {
                if out_tx.blocking_send(encoded).is_err() {
                    return;
                }
            }
            Err(err) => {
                tracing::warn!(
                    ?err,
                    "audio encode failed for one chunk, dropping it and continuing"
                );
            }
        }
    });

    AudioEncodePipeline {
        input: in_tx,
        output: out_rx,
    }
}

pub struct AudioDecodePipeline {
    /// `(packet, fec)` mirrors `AudioDecoder::decode`'s own signature —
    /// `fec` set when the caller knows the previous packet was lost, so
    /// Opus can attempt forward-error-correction from this packet's FEC
    /// data instead of concealment.
    pub input: mpsc::Sender<(Vec<u8>, bool)>,
    pub output: mpsc::Receiver<Vec<i16>>,
}

pub fn spawn_audio_decode_pipeline(
    mut decoder: Box<dyn AudioDecoder>,
    input_capacity: usize,
    output_capacity: usize,
    shutdown: CancellationToken,
) -> AudioDecodePipeline {
    let (in_tx, mut in_rx) = mpsc::channel::<(Vec<u8>, bool)>(input_capacity);
    let (out_tx, out_rx) = mpsc::channel(output_capacity);

    let handle = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || loop {
        let (packet, fec) = match handle.block_on(select_shutdown(&shutdown, in_rx.recv())) {
            Some(Some(item)) => item,
            Some(None) => return,
            None => return,
        };
        match decoder.decode(&packet, fec) {
            Ok(pcm) => {
                if out_tx.blocking_send(pcm).is_err() {
                    return;
                }
            }
            Err(err) => {
                tracing::warn!(
                    ?err,
                    "audio decode failed for one packet, dropping it and continuing"
                );
            }
        }
    });

    AudioDecodePipeline {
        input: in_tx,
        output: out_rx,
    }
}

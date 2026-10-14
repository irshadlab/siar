//! §79 "Chunk Compression", §80 "File Compression Architecture", §81
//! "Compression Bomb Protection".
//!
//! ## Decision: DEFLATE via `flate2` (pure-Rust backend)
//!
//! The crate's long-standing note on §79-80 was "pending a codec
//! dependency decision". This is that decision, and it is a *trade*,
//! made explicit so it can be reversed: **`flate2` with its default
//! pure-Rust `miniz_oxide` backend**, because it adds no C code, no
//! build script and no `unsafe` of ours to a crate that is
//! `#![forbid(unsafe_code)]`, and DEFLATE is universally decodable. It
//! gives up Zstandard's better ratio/speed; the choice is isolated to
//! this module and the one-byte [`FrameTag`], so a future `0x02 = zstd`
//! is an addition, not a migration.
//!
//! ## The pipeline order is fixed and explicit (§80)
//!
//! ```text
//! plaintext → [compress] → encrypt → chunk → content-address
//! ```
//!
//! That is [`PIPELINE_ORDER`]. The order is not a style choice, it is a
//! security and correctness constraint: compression **must** precede
//! encryption (ciphertext is indistinguishable from random and does not
//! compress — [`decide`] refuses to try), and compression is applied to
//! the **whole plaintext**, never per chunk. "Chunk compression" in the
//! §79 sense would make chunk boundaries depend on content and break
//! the fixed-geometry, independently-verifiable chunks the whole store
//! relies on ([`crate::fs_store`] derives every offset from
//! `chunk_size` alone). The spec allows "or chunking before encryption
//! depending [on the] chosen format" — this crate's format chose the
//! first; the other is not supported, and nothing here pretends it is.
//!
//! ## Framing: the decoder never guesses
//!
//! Compressed or not, the plaintext that gets encrypted is a *frame*:
//!
//! ```text
//! 0x00 ‖ plaintext                                   (stored)
//! 0x01 ‖ declared_len(u64 LE) ‖ DEFLATE stream       (deflate)
//! ```
//!
//! The first byte says which, so a receiver never infers the format
//! from content and a stored frame cannot be mistaken for a compressed
//! one. `declared_len` is **untrusted input** — it comes off the wire —
//! and is used only as a *ceiling to check against*, never to size an
//! allocation.
//!
//! ## What is compressed, and who decides (§79)
//!
//! "Content-aware and application-controlled." [`decide`] never
//! compresses on its own initiative: the application must ask
//! (`app_requested`). Even then it refuses, with a stated reason, for:
//! media types that are already compressed (PNG, JPEG, WebP, Opus,
//! MP4); container formats recognised by magic number (gzip, zip, xz,
//! zstd, bzip2, 7z, RAR); inputs too small to be worth a header; and —
//! the catch-all that also covers ciphertext — anything that does not
//! shrink by at least [`MIN_SAVINGS_PERCENT`]% when a leading
//! [`SAMPLE_BYTES`] sample is trial-compressed.
//!
//! ## Decompression bombs (§81)
//!
//! "max output size, ratio limits, streaming decode" — all three, in
//! [`decompress_stream`]:
//!
//! - **streaming**: output goes to a caller-supplied `Write` in fixed
//!   64 KiB pieces; no output buffer ever grows with the data;
//! - **max output** ([`DecompressLimits::max_output_bytes`]) and the
//!   frame's own declared length, whichever is smaller, checked after
//!   *every* piece — a bomb is stopped at the limit, not after it has
//!   already been materialized;
//! - **ratio** ([`DecompressLimits::max_ratio`], output ÷ compressed
//!   bytes consumed), enforced once a small floor of output exists so a
//!   legitimate tiny frame is not falsely flagged;
//! - the decoded length must **equal** the declared length, and nothing
//!   may follow the DEFLATE stream.
//!
//! The worst case a hostile frame can cost is therefore bounded by
//! `max_output_bytes` of *writes to the sink* and a constant working
//! buffer — and by `max_ratio × (bytes it actually sent)`, so a few
//! kilobytes of network input cannot become gigabytes of work.

use std::io::{self, BufRead, BufReader, Read, Write};

use flate2::bufread::DeflateDecoder;
use flate2::write::DeflateEncoder;
use flate2::Compression as Level;
use thiserror::Error;

use siar_domain::MediaType;

/// §80, as data: the only order this crate supports.
pub const PIPELINE_ORDER: [PipelineStage; 4] = [
    PipelineStage::Plaintext,
    PipelineStage::OptionalCompression,
    PipelineStage::Encryption,
    PipelineStage::ChunkingAndAddressing,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipelineStage {
    Plaintext,
    OptionalCompression,
    Encryption,
    ChunkingAndAddressing,
}

/// First byte of every frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FrameTag {
    Stored = 0x00,
    Deflate = 0x01,
}

/// Inputs smaller than this are never worth a compression header.
pub const MIN_COMPRESS_BYTES: u64 = 256;
/// Leading bytes trial-compressed to judge compressibility.
pub const SAMPLE_BYTES: usize = 64 * 1024;
/// A sample must shrink by at least this much to be worth compressing.
pub const MIN_SAVINGS_PERCENT: usize = 10;

const PIECE: usize = 64 * 1024;
/// Below this much output the ratio check is not applied.
const RATIO_FLOOR_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    /// The application did not ask for compression.
    NotRequested,
    TooSmall,
    /// The declared media type is already compressed.
    AlreadyCompressedMedia(MediaType),
    /// Recognised by magic number.
    AlreadyCompressedContainer,
    /// The trial sample did not shrink enough — includes ciphertext
    /// and any other high-entropy data.
    Incompressible,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Compress,
    Skip(SkipReason),
}

fn container_magic(head: &[u8]) -> bool {
    const MAGICS: &[&[u8]] = &[
        &[0x1F, 0x8B],                         // gzip
        &[0x50, 0x4B, 0x03, 0x04],             // zip
        &[0x28, 0xB5, 0x2F, 0xFD],             // zstd
        &[0xFD, b'7', b'z', b'X', b'Z', 0x00], // xz
        b"BZh",                                // bzip2
        &[b'7', b'z', 0xBC, 0xAF, 0x27, 0x1C], // 7z
        b"Rar!",                               // rar
    ];
    MAGICS.iter().any(|m| head.starts_with(m))
}

fn media_is_compressed(m: MediaType) -> bool {
    match m {
        MediaType::ImagePng
        | MediaType::ImageJpeg
        | MediaType::ImageWebp
        | MediaType::AudioOpus
        | MediaType::VideoMp4 => true,
        MediaType::Other => false,
    }
}

/// See the module doc. `head` is the start of the plaintext (at least
/// the first [`SAMPLE_BYTES`] if available); `total_len` is its full
/// length.
pub fn decide(
    app_requested: bool,
    media: Option<MediaType>,
    head: &[u8],
    total_len: u64,
) -> Decision {
    if !app_requested {
        return Decision::Skip(SkipReason::NotRequested);
    }
    if total_len < MIN_COMPRESS_BYTES {
        return Decision::Skip(SkipReason::TooSmall);
    }
    if let Some(m) = media {
        if media_is_compressed(m) {
            return Decision::Skip(SkipReason::AlreadyCompressedMedia(m));
        }
    }
    if container_magic(head) {
        return Decision::Skip(SkipReason::AlreadyCompressedContainer);
    }
    let sample = &head[..head.len().min(SAMPLE_BYTES)];
    let mut enc = DeflateEncoder::new(Vec::new(), Level::fast());
    // Writing to a Vec cannot fail.
    let _ = enc.write_all(sample);
    let compressed = enc.finish().map(|v| v.len()).unwrap_or(sample.len());
    // compressed <= sample * (100 - MIN)/100  ⇔  saved at least MIN%.
    if compressed * 100 > sample.len() * (100 - MIN_SAVINGS_PERCENT) {
        return Decision::Skip(SkipReason::Incompressible);
    }
    Decision::Compress
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CompressionError {
    #[error("i/o error: {0:?}")]
    Io(io::ErrorKind),
    #[error("unknown frame tag {0:#04x}")]
    UnknownTag(u8),
    #[error("frame is truncated")]
    Truncated,
    #[error("frame declares {declared} bytes of output, over the {max}-byte limit")]
    DeclaredTooLarge { declared: u64, max: u64 },
    #[error("decompressed output exceeded the {limit}-byte limit")]
    OutputTooLarge { limit: u64 },
    #[error("decompression ratio exceeded {limit}:1")]
    RatioExceeded { limit: u32 },
    #[error("frame declared {declared} bytes but decoded {actual}")]
    LengthMismatch { declared: u64, actual: u64 },
    #[error("data follows the end of the compressed stream")]
    TrailingData,
    #[error("compressed data is corrupt")]
    Corrupt,
}

fn io_err(e: io::Error) -> CompressionError {
    match e.kind() {
        io::ErrorKind::UnexpectedEof => CompressionError::Truncated,
        io::ErrorKind::InvalidData => CompressionError::Corrupt,
        k => CompressionError::Io(k),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecompressLimits {
    pub max_output_bytes: u64,
    /// Output bytes per compressed byte consumed. DEFLATE's theoretical
    /// ceiling is ~1032:1; real text is well under 30:1.
    pub max_ratio: u32,
}

impl Default for DecompressLimits {
    fn default() -> Self {
        Self {
            // Just above the 200 MiB attachment cap so a maximal
            // legitimate attachment always decodes.
            max_output_bytes: 256 * 1024 * 1024,
            max_ratio: 100,
        }
    }
}

/// Writes a **stored** frame (`0x00 ‖ plaintext`) without buffering the
/// plaintext.
pub fn frame_stored<R: Read, W: Write>(
    mut plaintext: R,
    mut out: W,
) -> Result<u64, CompressionError> {
    out.write_all(&[FrameTag::Stored as u8]).map_err(io_err)?;
    let n = io::copy(&mut plaintext, &mut out).map_err(io_err)?;
    Ok(n)
}

/// Writes a **deflate** frame, streaming. `plaintext_len` must be the
/// exact length that will be read (a file's size is known); a mismatch
/// is an error, because the declared length is what the receiver
/// enforces.
pub fn frame_deflate<R: Read, W: Write>(
    mut plaintext: R,
    plaintext_len: u64,
    mut out: W,
) -> Result<u64, CompressionError> {
    out.write_all(&[FrameTag::Deflate as u8]).map_err(io_err)?;
    out.write_all(&plaintext_len.to_le_bytes())
        .map_err(io_err)?;
    let mut enc = DeflateEncoder::new(&mut out, Level::default());
    let mut buf = vec![0u8; PIECE];
    let mut total = 0u64;
    loop {
        let n = match plaintext.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(io_err(e)),
        };
        total += n as u64;
        if total > plaintext_len {
            break;
        }
        enc.write_all(&buf[..n]).map_err(io_err)?;
    }
    if total != plaintext_len {
        return Err(CompressionError::LengthMismatch {
            declared: plaintext_len,
            actual: total,
        });
    }
    enc.finish().map_err(io_err)?;
    Ok(total)
}

/// In-memory convenience: decide, then frame. Returns the frame and
/// the decision taken.
pub fn encode(
    plaintext: &[u8],
    app_requested: bool,
    media: Option<MediaType>,
) -> Result<(Vec<u8>, Decision), CompressionError> {
    let decision = decide(app_requested, media, plaintext, plaintext.len() as u64);
    let mut out = Vec::with_capacity(plaintext.len() / 2 + 16);
    match decision {
        Decision::Compress => {
            frame_deflate(plaintext, plaintext.len() as u64, &mut out)?;
            // A "compressible" sample can still lose on the whole
            // input; never emit a frame bigger than storing it.
            if out.len() > plaintext.len() + 1 {
                out.clear();
                frame_stored(plaintext, &mut out)?;
                return Ok((out, Decision::Skip(SkipReason::Incompressible)));
            }
        }
        Decision::Skip(_) => {
            frame_stored(plaintext, &mut out)?;
        }
    }
    Ok((out, decision))
}

/// Decodes one frame to `out`, enforcing every §81 limit as it goes.
/// Returns the number of plaintext bytes written. See the module doc.
///
/// The decoder is a `bufread` one over a buffer *this function owns*:
/// it consumes exactly the bytes the DEFLATE stream needs and leaves
/// the rest in that buffer, which is how "nothing may follow the stream"
/// can be checked at all (a `read`-based decoder buffers ahead
/// internally and would swallow trailing bytes before they could be
/// seen).
pub fn decompress_stream<R: Read, W: Write>(
    frame: R,
    mut out: W,
    limits: &DecompressLimits,
) -> Result<u64, CompressionError> {
    let mut input = BufReader::with_capacity(PIECE, frame);
    let mut tag = [0u8; 1];
    input.read_exact(&mut tag).map_err(io_err)?;

    match tag[0] {
        t if t == FrameTag::Stored as u8 => {
            // Bounded copy: stop at the limit rather than after it.
            let mut buf = vec![0u8; PIECE];
            let mut total = 0u64;
            loop {
                let n = match input.read(&mut buf) {
                    Ok(0) => return Ok(total),
                    Ok(n) => n,
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) => return Err(io_err(e)),
                };
                total += n as u64;
                if total > limits.max_output_bytes {
                    return Err(CompressionError::OutputTooLarge {
                        limit: limits.max_output_bytes,
                    });
                }
                out.write_all(&buf[..n]).map_err(io_err)?;
            }
        }
        t if t == FrameTag::Deflate as u8 => {
            let mut len = [0u8; 8];
            input.read_exact(&mut len).map_err(io_err)?;
            let declared = u64::from_le_bytes(len);
            if declared > limits.max_output_bytes {
                return Err(CompressionError::DeclaredTooLarge {
                    declared,
                    max: limits.max_output_bytes,
                });
            }
            let mut dec = DeflateDecoder::new(&mut input);
            let mut buf = vec![0u8; PIECE];
            let mut total = 0u64;
            loop {
                let n = match dec.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) => return Err(io_err(e)),
                };
                total += n as u64;
                // Ceiling: the smaller of the frame's own claim and the
                // policy limit — checked before the bytes are emitted.
                if total > declared {
                    return Err(CompressionError::LengthMismatch {
                        declared,
                        actual: total,
                    });
                }
                if total > limits.max_output_bytes {
                    return Err(CompressionError::OutputTooLarge {
                        limit: limits.max_output_bytes,
                    });
                }
                // Ratio over compressed bytes the decoder has actually
                // consumed (not merely buffered), once there is enough
                // output for the ratio to mean something.
                if total > RATIO_FLOOR_BYTES
                    && total / dec.total_in().max(1) > u64::from(limits.max_ratio)
                {
                    return Err(CompressionError::RatioExceeded {
                        limit: limits.max_ratio,
                    });
                }
                out.write_all(&buf[..n]).map_err(io_err)?;
            }
            if total != declared {
                return Err(CompressionError::LengthMismatch {
                    declared,
                    actual: total,
                });
            }
            drop(dec);
            match input.fill_buf() {
                Ok([]) => Ok(total),
                Ok(_) => Err(CompressionError::TrailingData),
                Err(e) => Err(io_err(e)),
            }
        }
        other => Err(CompressionError::UnknownTag(other)),
    }
}

/// In-memory convenience over [`decompress_stream`].
pub fn decode(frame: &[u8], limits: &DecompressLimits) -> Result<Vec<u8>, CompressionError> {
    let mut out = Vec::new();
    decompress_stream(frame, &mut out, limits)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn text(n: usize) -> Vec<u8> {
        "the quick brown fox jumps over the lazy dog. "
            .bytes()
            .cycle()
            .take(n)
            .collect()
    }

    fn noise(n: usize, seed: u32) -> Vec<u8> {
        let mut x = seed.wrapping_mul(2_654_435_761).wrapping_add(1);
        (0..n)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                (x >> 8) as u8
            })
            .collect()
    }

    #[test]
    fn the_order_is_compress_then_encrypt_then_chunk() {
        use PipelineStage::*;
        assert_eq!(
            PIPELINE_ORDER,
            [
                Plaintext,
                OptionalCompression,
                Encryption,
                ChunkingAndAddressing
            ]
        );
        let pos = |s| PIPELINE_ORDER.iter().position(|x| *x == s).unwrap();
        assert!(
            pos(OptionalCompression) < pos(Encryption),
            "ciphertext does not compress"
        );
        assert!(
            pos(Encryption) < pos(ChunkingAndAddressing),
            "addressing is of the ciphertext"
        );
    }

    #[test]
    fn compression_happens_only_when_the_application_asks() {
        let t = text(10_000);
        assert_eq!(
            decide(false, None, &t, 10_000),
            Decision::Skip(SkipReason::NotRequested)
        );
        assert_eq!(decide(true, None, &t, 10_000), Decision::Compress);
    }

    #[test]
    fn already_compressed_media_and_containers_are_never_recompressed() {
        let t = text(10_000);
        for m in [
            MediaType::ImagePng,
            MediaType::ImageJpeg,
            MediaType::ImageWebp,
            MediaType::AudioOpus,
            MediaType::VideoMp4,
        ] {
            assert_eq!(
                decide(true, Some(m), &t, 10_000),
                Decision::Skip(SkipReason::AlreadyCompressedMedia(m))
            );
        }
        // Declared "Other" but actually a gzip/zip: caught by magic.
        let mut gz = vec![0x1F, 0x8B];
        gz.extend(text(10_000));
        assert_eq!(
            decide(true, Some(MediaType::Other), &gz, gz.len() as u64),
            Decision::Skip(SkipReason::AlreadyCompressedContainer)
        );
        let mut zip = vec![0x50, 0x4B, 0x03, 0x04];
        zip.extend(text(10_000));
        assert_eq!(
            decide(true, None, &zip, zip.len() as u64),
            Decision::Skip(SkipReason::AlreadyCompressedContainer)
        );
    }

    #[test]
    fn ciphertext_and_other_high_entropy_data_is_refused_not_wasted_on() {
        // Real ChaCha20-Poly1305 ciphertext: what the pipeline's
        // *next* stage produces, which must never be fed back in.
        use chacha20poly1305::aead::{Aead, KeyInit};
        use chacha20poly1305::{ChaCha20Poly1305, Nonce};
        let cipher = ChaCha20Poly1305::new(&[7u8; 32].into());
        let ct = cipher
            .encrypt(Nonce::from_slice(&[0u8; 12]), text(100_000).as_slice())
            .unwrap();
        assert_eq!(
            decide(true, None, &ct, ct.len() as u64),
            Decision::Skip(SkipReason::Incompressible)
        );
        assert_eq!(
            decide(true, None, &noise(50_000, 1), 50_000),
            Decision::Skip(SkipReason::Incompressible)
        );
    }

    #[test]
    fn tiny_inputs_are_stored() {
        assert_eq!(
            decide(true, None, b"hello", 5),
            Decision::Skip(SkipReason::TooSmall)
        );
    }

    #[test]
    fn text_round_trips_and_actually_shrinks() {
        let t = text(100_000);
        let (frame, d) = encode(&t, true, None).unwrap();
        assert_eq!(d, Decision::Compress);
        assert_eq!(frame[0], FrameTag::Deflate as u8);
        assert!(frame.len() < t.len() / 10, "{} -> {}", t.len(), frame.len());
        assert_eq!(decode(&frame, &DecompressLimits::default()).unwrap(), t);
    }

    #[test]
    fn stored_frames_round_trip_and_are_one_byte_larger() {
        let n = noise(10_000, 2);
        let (frame, d) = encode(&n, true, None).unwrap();
        assert_eq!(d, Decision::Skip(SkipReason::Incompressible));
        assert_eq!(
            (frame[0], frame.len()),
            (FrameTag::Stored as u8, n.len() + 1)
        );
        assert_eq!(decode(&frame, &DecompressLimits::default()).unwrap(), n);
    }

    #[test]
    fn a_frame_is_never_larger_than_storing_would_have_been() {
        // Compressible head, incompressible tail: the sample says yes,
        // the whole says no.
        let mut v = text(SAMPLE_BYTES);
        v.extend(noise(400_000, 3));
        let (frame, _) = encode(&v, true, None).unwrap();
        assert!(frame.len() <= v.len() + 1);
        assert_eq!(decode(&frame, &DecompressLimits::default()).unwrap(), v);
    }

    /// A real bomb: a few KB of DEFLATE that expands to hundreds of MB.
    fn bomb(expanded: u64) -> Vec<u8> {
        let mut deflate = DeflateEncoder::new(Vec::new(), Level::best());
        let zeros = vec![0u8; 1024 * 1024];
        for _ in 0..expanded / (1024 * 1024) {
            deflate.write_all(&zeros).unwrap();
        }
        let body = deflate.finish().unwrap();
        let mut frame = vec![FrameTag::Deflate as u8];
        frame.extend(expanded.to_le_bytes());
        frame.extend(body);
        frame
    }

    /// A sink that records the most it was ever asked to hold.
    struct CountingSink(u64);
    impl Write for CountingSink {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            self.0 += b.len() as u64;
            Ok(b.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_decompression_bomb_is_stopped_by_the_ratio_limit_long_before_it_finishes() {
        let frame = bomb(128 * 1024 * 1024); // claims 128 MiB
        assert!(
            frame.len() < 400 * 1024,
            "the bomb itself is tiny: {} bytes",
            frame.len()
        );
        let limits = DecompressLimits {
            max_output_bytes: 1 << 40,
            max_ratio: 100,
        };
        let mut sink = CountingSink(0);
        let err = decompress_stream(&frame[..], &mut sink, &limits).unwrap_err();
        assert_eq!(err, CompressionError::RatioExceeded { limit: 100 });
        assert!(
            sink.0 < 64 * 1024 * 1024,
            "stopped after only {} bytes of output",
            sink.0
        );
    }

    #[test]
    fn the_output_limit_stops_a_bomb_even_with_the_ratio_check_off() {
        let frame = bomb(64 * 1024 * 1024);
        let limits = DecompressLimits {
            max_output_bytes: 8 * 1024 * 1024,
            max_ratio: u32::MAX,
        };
        let mut sink = CountingSink(0);
        let err = decompress_stream(&frame[..], &mut sink, &limits).unwrap_err();
        // The declared length (64 MiB) already exceeds the limit: the
        // frame is refused before a single byte is decoded.
        assert!(
            matches!(err, CompressionError::DeclaredTooLarge { declared, max } if declared == 64 << 20 && max == 8 << 20)
        );
        assert_eq!(sink.0, 0);
    }

    #[test]
    fn a_lying_declared_length_cannot_smuggle_extra_output() {
        // Declares 1 KiB, deflate stream actually holds 4 MiB.
        let mut deflate = DeflateEncoder::new(Vec::new(), Level::best());
        deflate.write_all(&vec![7u8; 4 * 1024 * 1024]).unwrap();
        let body = deflate.finish().unwrap();
        let mut frame = vec![FrameTag::Deflate as u8];
        frame.extend(1024u64.to_le_bytes());
        frame.extend(body);
        let mut sink = CountingSink(0);
        let err =
            decompress_stream(&frame[..], &mut sink, &DecompressLimits::default()).unwrap_err();
        assert!(
            matches!(err, CompressionError::LengthMismatch { declared: 1024, .. }),
            "{err:?}"
        );
        assert!(
            sink.0 <= 1024 + PIECE as u64,
            "wrote at most one piece past the claim: {}",
            sink.0
        );
    }

    #[test]
    fn a_short_stream_trailing_data_unknown_tags_and_truncation_are_all_errors() {
        let t = text(5_000);
        let (frame, _) = encode(&t, true, None).unwrap();
        let lim = DecompressLimits::default();

        // Declared longer than actual.
        let mut longer = frame.clone();
        longer[1..9].copy_from_slice(&(t.len() as u64 + 10).to_le_bytes());
        assert!(matches!(
            decode(&longer, &lim),
            Err(CompressionError::LengthMismatch { .. })
        ));

        // Trailing garbage after the stream.
        let mut trailing = frame.clone();
        trailing.extend_from_slice(b"extra");
        assert!(matches!(
            decode(&trailing, &lim),
            Err(CompressionError::TrailingData)
        ));

        // Unknown tag, empty input, truncated header, truncated body.
        assert!(matches!(
            decode(&[0x7F, 1, 2, 3], &lim),
            Err(CompressionError::UnknownTag(0x7F))
        ));
        assert!(matches!(
            decode(&[], &lim),
            Err(CompressionError::Truncated)
        ));
        assert!(matches!(
            decode(&[FrameTag::Deflate as u8, 1, 2], &lim),
            Err(CompressionError::Truncated)
        ));
        let cut = &frame[..frame.len() / 2];
        assert!(decode(cut, &lim).is_err());
        // Garbage "compressed" bytes.
        let mut junk = vec![FrameTag::Deflate as u8];
        junk.extend(100u64.to_le_bytes());
        junk.extend(noise(200, 9));
        assert!(decode(&junk, &lim).is_err());
    }

    #[test]
    fn a_stored_frame_is_also_bounded() {
        let mut frame = vec![FrameTag::Stored as u8];
        frame.extend(vec![0u8; 5 * 1024 * 1024]);
        let limits = DecompressLimits {
            max_output_bytes: 1024 * 1024,
            max_ratio: 100,
        };
        let mut sink = CountingSink(0);
        assert_eq!(
            decompress_stream(&frame[..], &mut sink, &limits),
            Err(CompressionError::OutputTooLarge { limit: 1024 * 1024 })
        );
        assert!(sink.0 <= 1024 * 1024);
    }

    #[test]
    fn streaming_encode_checks_the_declared_length() {
        let t = text(10_000);
        let mut out = Vec::new();
        assert!(matches!(
            frame_deflate(&t[..], 9_999, &mut out),
            Err(CompressionError::LengthMismatch {
                declared: 9_999,
                ..
            })
        ));
        let mut out = Vec::new();
        assert!(matches!(
            frame_deflate(&t[..], 10_001, &mut out),
            Err(CompressionError::LengthMismatch {
                declared: 10_001,
                actual: 10_000
            })
        ));
        let mut out = Vec::new();
        frame_deflate(&t[..], 10_000, &mut out).unwrap();
        assert_eq!(decode(&out, &DecompressLimits::default()).unwrap(), t);
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]
        #[test]
        fn any_input_round_trips_whatever_the_decision(
            data in proptest::collection::vec(any::<u8>(), 0..20_000),
            ask in any::<bool>(),
        ) {
            let (frame, _) = encode(&data, ask, None).unwrap();
            prop_assert!(frame.len() <= data.len() + 1 + 9, "never meaningfully larger");
            prop_assert_eq!(decode(&frame, &DecompressLimits::default()).unwrap(), data);
        }

        /// Arbitrary bytes as a frame: an error or a bounded success,
        /// never a panic and never more output than the limit.
        #[test]
        fn hostile_frames_never_panic_or_exceed_the_limit(
            bytes in proptest::collection::vec(any::<u8>(), 0..400),
        ) {
            let limits = DecompressLimits { max_output_bytes: 64 * 1024, max_ratio: 50 };
            let mut sink = CountingSink(0);
            let _ = decompress_stream(&bytes[..], &mut sink, &limits);
            prop_assert!(sink.0 <= limits.max_output_bytes);
        }
    }
}

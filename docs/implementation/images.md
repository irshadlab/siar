# Image Codec & Safe Software Decoding Architecture

## Pure Rust Core + Platform Native Decoding Policy

> **Scope Note:** Blob storage, content-addressed chunking, BLAKE3 deduplication, and encrypted transport are specified in [`sys-arch/05-robust-file-blob-subsystem-architecture.md`](file:///home/irshad/Projects/siar/sys-arch/05-robust-file-blob-subsystem-architecture.md). This document defines the image codec, software decoding, thumbnail generation, and memory sandboxing policy.

---

## 1. Core Codec Policy

For a private messaging application, image codecs are required for formats the application decodes, displays, resizes, compresses, or generates. Unlike realtime video calls (which require negotiated codecs and hardware acceleration, see [`sys-arch/25`](file:///home/irshad/Projects/siar/sys-arch/25-android-direct-hardware-surface-zero-copy-media-architecture.md) and [`sys-arch/29`](file:///home/irshad/Projects/siar/sys-arch/29-realtime-calls-media-session-protocol-architecture.md)), image files are immutable attachments that do not require peer-to-peer codec negotiation. If Alice sends a JPEG, Bob receives that JPEG file as an encrypted blob.

### Supported Format Matrix

| Format | Role | Desktop Strategy | Android Strategy | Priority |
| :--- | :--- | :--- | :--- | :--- |
| **JPEG** | Standard photos | Pure Rust `image` / `jpeg-decoder` | Android native / Rust | **Required (v1)** |
| **PNG** | Graphics / lossless | Pure Rust `png` | Android native / Rust | **Required (v1)** |
| **WebP** | Modern compressed images | Pure Rust `image-webp` | Android native `BitmapFactory` | **Required (v1)** |
| **AVIF** | Next-gen high compression | Software decoder (dav1d/rav1e-based) | Android 12+ native / software | **Optional / Recommended** |
| **GIF** | Simple animations | Software decode (first frame or loop) | Android native / Rust | **Decode Required (v1)** |
| **HEIF/HEIC**| iOS / modern phone photos | Optional platform bridge | Platform native `ImageDecoder` | **Recommended Compatibility** |

---

## 2. Safe Decoding & Processing Pipeline

All incoming image data must be parsed through safe, memory-bounded decoders before rendering in the Dioxus UI:

```text
Incoming Encrypted Blob (from sys-arch/05)
                │
                ▼
      AEAD Decryption to Temp Buffer
                │
                ▼
   Strict Magic-Byte Format Sniffing
   (Prevent Polyglot / MIME Confusion)
                │
                ▼
      Safe Software Decoder
   (Enforce Max Pixel Allocation: 64MP)
                │
                ▼
          RGBA / YUV Frame
         ┌──────┴──────┐
         ▼             ▼
  Thumbnail Gen    Preview Gen
   (128×128 px)    (1080p max)
         │             │
         └──────┬──────┘
                ▼
       Dioxus UI Display
```

---

## 3. Platform Execution Split

### Desktop (Linux, Windows, macOS)
- Pure Rust software decoding pipeline (`image` crate stack) for complete memory safety and zero external shared library dependencies.
- Sandboxed image parsing to prevent native heap corruption from malformed image payloads.
- In-memory thumbnail caching with an LRU cache bounded by memory footprint (default 128 MB max cache).

### Android
- Utilize Android's native `ImageDecoder` / `BitmapFactory` where hardware-assisted decoding and zero-copy surface rendering provide battery and memory advantages.
- Pure Rust decoding fallback for unsupported formats or background processing tasks.
- No requirement for peer-to-peer codec negotiation.

---

## 4. Security & Memory Discipline

1. **Pixel Dimension Limits:** Reject images with decoded dimensions exceeding `8192 × 8192` pixels (64 Megapixels) before full allocation to prevent memory exhaustion (Decompression Bomb / Zip Bomb equivalent).
2. **Metadata Sanitization:** Strip EXIF metadata, GPS coordinates, and camera device serial numbers by default on sending unless explicitly overridden by user preference.
3. **Thumbnail Generation:** Thumbnails are generated locally prior to transmission and uploaded as low-resolution derivative blobs to enable instantaneous preview loading on low-bandwidth and mesh links.
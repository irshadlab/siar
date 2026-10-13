//! §86 "Blob Read API", §87 "Blob Write API", §88 "Blob Store Trait".
//!
//! The three traits the spec gives, with two deliberate departures —
//! both for the same reason the rest of this workspace's async traits
//! look the way they do (`siar-event-log::outbox`):
//!
//! - **Native `async fn`-in-trait (`-> impl Future + Send`), not
//!   `async-trait`.** No per-call heap allocation, static dispatch.
//! - **Associated `Reader`/`Writer` types instead of the spec's
//!   `Box<dyn BlobReader>`/`Box<dyn BlobWriter>`.** A trait with
//!   `impl Future` returns is not object-safe, and boxing every reader
//!   would put an allocation and a vtable on the one path whose whole
//!   point (§37-39) is to be lean. Callers are generic over the store;
//!   nothing in this workspace needs to swap blob stores at runtime.
//!
//! ## What "no entire objects in memory" means in these signatures
//!
//! - [`BlobReader::read_range`] returns **at most**
//!   [`MAX_READ_RANGE_BYTES`] — asking for more is
//!   [`BlobError::RangeTooLarge`], not a giant `Bytes`.
//! - [`BlobReader::stream`] yields bounded pieces
//!   ([`BlobStream`]); there is no `read_all`.
//! - [`BlobWriter::write_chunk`] takes one chunk; the writer never needs
//!   the whole object, only the staging file.
//!
//! `Bytes` (not `Vec<u8>`) is the currency so a chunk can be sliced,
//! shared with a send queue and handed to an encryptor without a copy
//! (§38) — the reader allocates the buffer once and everything after
//! that is a refcount bump.

use std::fs::File;
use std::future::Future;
use std::io::Read;

use bytes::Bytes;

use crate::byte_range::ByteRange;
use crate::descriptor::BlobDescriptor;
use crate::ids::BlobId;
use crate::schema_rows::BlobLifecycleState;
use crate::store_error::{io, BlobError};

/// Largest single `read_range`. Bigger reads must stream.
pub const MAX_READ_RANGE_BYTES: u64 = 16 * 1024 * 1024;

/// Largest piece a [`BlobStream`] will yield.
pub const MAX_STREAM_CHUNK_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlobStat {
    pub blob_id: BlobId,
    pub size: u64,
    pub state: BlobLifecycleState,
    pub last_accessed_millis: u64,
}

/// A bounded sequential read: each item is at most the chunk size the
/// caller asked for, and only one is alive at a time.
#[derive(Debug)]
pub struct BlobStream {
    file: File,
    remaining: u64,
    chunk: usize,
    failed: bool,
}

impl BlobStream {
    pub(crate) fn new(file: File, size: u64, chunk: usize) -> Result<Self, BlobError> {
        if chunk == 0 || chunk > MAX_STREAM_CHUNK_BYTES {
            return Err(BlobError::InvalidArgument(
                "stream chunk size must be 1..=16 MiB",
            ));
        }
        Ok(Self {
            file,
            remaining: size,
            chunk,
            failed: false,
        })
    }

    pub fn remaining_bytes(&self) -> u64 {
        self.remaining
    }
}

impl Iterator for BlobStream {
    type Item = Result<Bytes, BlobError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.failed || self.remaining == 0 {
            return None;
        }
        let want = (self.chunk as u64).min(self.remaining) as usize;
        let mut buf = vec![0u8; want];
        match self.file.read_exact(&mut buf) {
            Ok(()) => {
                self.remaining -= want as u64;
                Some(Ok(Bytes::from(buf)))
            }
            Err(e) => {
                // A file shorter than its recorded size is a damaged
                // object; report it once and stop.
                self.failed = true;
                Some(Err(io("stream blob")(e)))
            }
        }
    }
}

pub trait BlobReader: Send + Sync {
    fn size(&self) -> u64;

    fn read_range(&self, range: ByteRange)
        -> impl Future<Output = Result<Bytes, BlobError>> + Send;

    fn stream(
        &self,
        chunk_size: usize,
    ) -> impl Future<Output = Result<BlobStream, BlobError>> + Send;
}

pub trait BlobWriter: Send {
    /// Writes chunk `index`. Verifies its size (and its hash, when the
    /// writer was given a manifest) *before* it touches storage.
    fn write_chunk(
        &mut self,
        index: u32,
        data: Bytes,
    ) -> impl Future<Output = Result<(), BlobError>> + Send;

    /// Contiguous `[start, end)` runs of chunk indices still missing.
    fn missing_ranges(&self) -> Vec<(u32, u32)>;

    /// Verifies the complete blob and atomically publishes it. Returns
    /// its [`BlobId`]. Nothing is visible to readers before this
    /// succeeds.
    fn finalize(self) -> impl Future<Output = Result<BlobId, BlobError>> + Send;

    /// Discards the partial write and its staging data.
    fn abort(self) -> impl Future<Output = Result<(), BlobError>> + Send;
}

pub trait BlobStore: Send + Sync {
    type Reader: BlobReader;
    type Writer: BlobWriter;

    fn contains(&self, id: BlobId) -> impl Future<Output = Result<bool, BlobError>> + Send;

    fn stat(&self, id: BlobId) -> impl Future<Output = Result<BlobStat, BlobError>> + Send;

    fn open(&self, id: BlobId) -> impl Future<Output = Result<Self::Reader, BlobError>> + Send;

    fn begin_write(
        &self,
        descriptor: BlobDescriptor,
    ) -> impl Future<Output = Result<Self::Writer, BlobError>> + Send;

    /// Idempotent: deleting an absent blob is `Ok` (a GC loop must not
    /// fail because a previous pass already got there).
    fn delete(&self, id: BlobId) -> impl Future<Output = Result<(), BlobError>> + Send;
}

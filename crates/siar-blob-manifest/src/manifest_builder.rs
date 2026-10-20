//! Builds a [`BlobManifest`] from a ciphertext **as it streams past**,
//! holding at most one chunk. The result is byte-for-byte what
//! [`crate::manifest::build_manifest`] would produce for the same bytes
//! (tested), so a memory-bounded import (hash pass, then write pass)
//! yields the same manifest and `BlobId` as the all-in-memory one.

use crate::ids::{BlobId, ChunkHash};
use crate::limits::ManifestLimits;
use crate::manifest::{BlobManifest, ChunkDescriptor, ManifestError};

pub struct ManifestBuilder {
    chunk_size: u32,
    limits: ManifestLimits,
    pending: Vec<u8>,
    offset: u64,
    chunks: Vec<ChunkDescriptor>,
    whole: blake3::Hasher,
}

impl ManifestBuilder {
    pub fn new(chunk_size: u32, limits: ManifestLimits) -> Self {
        Self {
            chunk_size: chunk_size.max(1),
            limits,
            pending: Vec::with_capacity(chunk_size.max(1) as usize),
            offset: 0,
            chunks: Vec::new(),
            whole: blake3::Hasher::new(),
        }
    }

    pub fn update(&mut self, mut data: &[u8]) -> Result<(), ManifestError> {
        self.whole.update(data);
        let size = self.chunk_size as usize;
        while !data.is_empty() {
            let take = (size - self.pending.len()).min(data.len());
            self.pending.extend_from_slice(&data[..take]);
            data = &data[take..];
            if self.pending.len() == size {
                self.seal_chunk()?;
            }
        }
        Ok(())
    }

    fn seal_chunk(&mut self) -> Result<(), ManifestError> {
        let len = self.pending.len() as u64;
        let total = self.offset + len;
        if total > self.limits.max_file_bytes {
            return Err(ManifestError::FileTooLarge {
                actual: total,
                max: self.limits.max_file_bytes,
            });
        }
        if self.chunks.len() as u64 + 1 > u64::from(self.limits.max_chunks) {
            return Err(ManifestError::TooManyChunks {
                actual: self.chunks.len() + 1,
                max: self.limits.max_chunks,
            });
        }
        self.chunks.push(ChunkDescriptor {
            index: self.chunks.len() as u32,
            offset: self.offset,
            size: self.pending.len() as u32,
            hash: ChunkHash::from_ciphertext_chunk(&self.pending),
        });
        self.offset = total;
        self.pending.clear();
        Ok(())
    }

    pub fn finish(mut self) -> Result<BlobManifest, ManifestError> {
        if !self.pending.is_empty() {
            self.seal_chunk()?;
        }
        let estimated = self.chunks.len() as u64 * 32;
        if estimated > self.limits.max_manifest_bytes {
            return Err(ManifestError::ManifestTooLarge {
                actual: estimated,
                max: self.limits.max_manifest_bytes,
            });
        }
        Ok(BlobManifest {
            version: 1,
            blob_id: BlobId::from_raw_digest(*self.whole.finalize().as_bytes()),
            total_size: self.offset,
            chunk_size: self.chunk_size,
            chunks: self.chunks,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::build_manifest;

    fn data(n: usize) -> Vec<u8> {
        (0..n).map(|i| (i.wrapping_mul(31) % 253) as u8).collect()
    }

    #[test]
    fn matches_build_manifest_for_every_alignment_and_feed_pattern() {
        let l = ManifestLimits::default();
        for len in [1usize, 99, 100, 101, 250, 1_000] {
            let d = data(len);
            let want = build_manifest(&d, 100, &l).unwrap();
            for feed in [1usize, 7, 100, 333, 5_000] {
                let mut b = ManifestBuilder::new(100, l);
                for piece in d.chunks(feed) {
                    b.update(piece).unwrap();
                }
                assert_eq!(b.finish().unwrap(), want, "len {len} feed {feed}");
            }
        }
    }

    #[test]
    fn limits_are_enforced_while_streaming() {
        let l = ManifestLimits {
            max_file_bytes: 250,
            ..ManifestLimits::default()
        };
        let mut b = ManifestBuilder::new(100, l);
        assert!(b.update(&data(300)).is_err());
        let l = ManifestLimits {
            max_chunks: 2,
            ..ManifestLimits::default()
        };
        let mut b = ManifestBuilder::new(100, l);
        assert!(b.update(&data(300)).is_err());
    }
}

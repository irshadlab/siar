//! §203/§204: importing a file as an encrypted, content-addressed,
//! referenced blob, and opening it again.

use std::sync::Arc;

use bytes::Bytes;
use siar_blob_manifest::blob_io::{BlobReader, BlobStore, BlobWriter};
use siar_blob_manifest::byte_range::ByteRange;
use siar_blob_manifest::chunking::ChunkSizeClass;
use siar_blob_manifest::descriptor::{
    BlobDescriptor, ChunkingDescriptor, EncryptionAlgorithm, EncryptionDescriptor,
};
use siar_blob_manifest::encryption::{encrypt_blob, generate_blob_key};
use siar_blob_manifest::ids::{BlobEncryptionKey, BlobId, ManifestId};
use siar_blob_manifest::manifest::{build_manifest, BlobManifest};
use siar_blob_manifest::metadata_store::BlobMetadataStore;
use siar_blob_manifest::reference_store::{
    BlobReference, BlobRole, ObjectId, RefOp, ReferenceStore,
};
use siar_domain::{AttachmentReference, BlobSize, MediaType};

use crate::service::FilesService;
use siar_blob_manifest::chunked_encryption::{
    self as v2, ChunkedDecryptor, ChunkedEncryptor, DEFAULT_SEGMENT,
};
use siar_blob_manifest::encryption::decrypt_blob_verified;
use siar_blob_manifest::file_io::read_all_bounded;
use siar_blob_manifest::file_io::read_exact_at;
use siar_blob_manifest::manifest_builder::ManifestBuilder;
use siar_blob_manifest::preprocess::Verified;

use crate::{CiphertextSource, FileChannel, FileSink, FileSource, FilesError};

/// A file that is now an encrypted blob in the local store. Holds the
/// key, so it is never `Debug`-printed with it (the key type redacts)
/// and never serialized by this crate.
pub struct ImportedAttachment {
    pub blob_id: BlobId,
    pub descriptor: BlobDescriptor,
    pub manifest: Arc<BlobManifest>,
    pub media_type: MediaType,
    /// Holds the blob alive until it is bound to a message
    /// ([`FilesService::bind_to_object`]).
    pub import_owner: ObjectId,
    pub(crate) key: BlobEncryptionKey,
    pub(crate) ciphertext_len: u64,
}

impl ImportedAttachment {
    /// §204: what a message carries.
    pub fn reference(&self) -> Result<AttachmentReference, FilesError> {
        Ok(AttachmentReference {
            blob_hash: *self.blob_id.as_bytes(),
            encrypted_size: BlobSize::parse(self.ciphertext_len)?,
            media_type: self.media_type,
            attachment_key: self.key.0,
            thumbnail: None,
        })
    }
}

impl<C, M, R> FilesService<C, M, R>
where
    C: FileChannel + 'static,
    M: BlobMetadataStore + 'static,
    R: ReferenceStore + ?Sized + 'static,
{
    /// §204 `import_attachment`: read (bounded), encrypt once under a
    /// fresh key, chunk, and publish into the store. The blob is
    /// referenced *before* any byte is written (§148) so GC can never
    /// race the import, and the reference is compensated on failure.
    pub async fn import_attachment<S: FileSource + ?Sized>(
        &self,
        source: &S,
        media_type: MediaType,
    ) -> Result<ImportedAttachment, FilesError> {
        let limits = self.limits().manifest;
        let plaintext = read_all_bounded(source, limits.max_file_bytes).await?;
        let key = generate_blob_key();
        let (ciphertext, enc) = encrypt_blob(&key, &plaintext)?;
        drop(plaintext);

        let total = ciphertext.len() as u64;
        let size = BlobSize::parse(total)?;
        let chunk_size = ChunkSizeClass::for_file_size(total).default_chunk_size(total);
        let manifest = build_manifest(&ciphertext, chunk_size, &limits)?;
        let blob_id = manifest.blob_id;
        let descriptor = BlobDescriptor {
            blob_id,
            size,
            chunking: ChunkingDescriptor { chunk_size },
            encryption: EncryptionDescriptor {
                algorithm: EncryptionAlgorithm::ChaCha20Poly1305,
                nonce: enc.nonce,
            },
            media_type: Some(media_type),
            manifest_id: Some(ManifestId::new()),
        };

        let owner = ObjectId::new(&format!("import-{}", uuid::Uuid::new_v4()))?;
        let reference = self.reference_for(&owner, blob_id, BlobRole::Transfer);
        self.refs().apply(&[RefOp::Add(reference.clone())])?;

        if let Err(e) = self.write_blob(descriptor, &manifest, &ciphertext).await {
            if let Err(e2) = self.refs().apply(&[RefOp::Remove(reference)]) {
                tracing::warn!(error = %e2, "could not undo an import reference");
            }
            return Err(e);
        }
        Ok(ImportedAttachment {
            blob_id,
            descriptor,
            manifest: Arc::new(manifest),
            media_type,
            import_owner: owner,
            key,
            ciphertext_len: total,
        })
    }

    async fn write_blob(
        &self,
        descriptor: BlobDescriptor,
        manifest: &BlobManifest,
        ciphertext: &[u8],
    ) -> Result<(), FilesError> {
        let store = self.engine().store();
        let mut w = store.begin_write(descriptor).await?;
        for c in &manifest.chunks {
            let start = c.offset as usize;
            let end = start + c.size as usize;
            if let Err(e) = w
                .write_chunk(c.index, Bytes::copy_from_slice(&ciphertext[start..end]))
                .await
            {
                let _ = w.abort().await;
                return Err(e.into());
            }
        }
        w.finalize().await?;
        Ok(())
    }

    /// Hands the blob from its temporary import owner to a real object
    /// (a message id, a saved-file record) in one atomic batch — there
    /// is no instant at which it has neither.
    pub fn bind_to_object(
        &self,
        imported: &ImportedAttachment,
        object: ObjectId,
        role: BlobRole,
    ) -> Result<(), FilesError> {
        let old = self.reference_for(&imported.import_owner, imported.blob_id, BlobRole::Transfer);
        let new = self.reference_for(&object, imported.blob_id, role);
        self.refs().apply(&[RefOp::Add(new), RefOp::Remove(old)])?;
        Ok(())
    }

    /// §203 `open`: the decrypted bytes behind a message's reference.
    /// Served from the plaintext cache when one is enabled (§127; off by
    /// default), otherwise decrypted on demand.
    pub async fn open_attachment(
        &self,
        reference: &AttachmentReference,
    ) -> Result<Vec<u8>, FilesError> {
        let id = BlobId::from_reference_bytes(reference.blob_hash);
        let now = self.now_millis();
        if let Some(hit) = self.plaintext_get(id, now) {
            return Ok(hit);
        }
        let plaintext = self.open_verified_attachment(reference).await?.into_inner();
        self.plaintext_put(id, &plaintext, now);
        Ok(plaintext)
    }

    /// §129: bytes whose authenticity was checked, for a media decoder
    /// that accepts nothing else.
    pub async fn open_verified_attachment(
        &self,
        reference: &AttachmentReference,
    ) -> Result<Verified<Vec<u8>>, FilesError> {
        let id = BlobId::from_reference_bytes(reference.blob_hash);
        let store = self.engine().store();
        let reader = store.open(id).await?;
        let size = reader.size();
        let max = self.limits().manifest.max_file_bytes;
        if size > max {
            return Err(FilesError::TooLarge { actual: size, max });
        }
        let ciphertext = reader.read_range(ByteRange::new(0, size)).await?;
        let key = BlobEncryptionKey(reference.attachment_key);
        if v2::looks_chunked(&ciphertext) {
            if let Ok(plain) = v2::decrypt_chunked_verified(&key, &ciphertext) {
                return Ok(plain);
            }
            // A v1 blob matches the v2 magic by chance once in 2^32.
        }
        Ok(decrypt_blob_verified(&key, &ciphertext)?)
    }

    /// §125-126: decrypt the internal blob and write it to a
    /// user-selected destination. Nothing becomes visible there unless
    /// the whole file verified and `commit` succeeded.
    ///
    /// A v2 (chunk-wise) blob is decrypted **segment by segment** with
    /// memory bounded by one segment, each segment authenticated before
    /// its plaintext reaches the sink; a v1 blob has a single trailing
    /// tag and must be opened whole first. Either way a failure drops
    /// the sink, which leaves nothing at the destination.
    pub async fn export_attachment<K: FileSink>(
        &self,
        reference: &AttachmentReference,
        mut sink: K,
    ) -> Result<u64, FilesError> {
        /// Plaintext is released in pieces this size.
        const PIECE: usize = 256 * 1024;
        let id = BlobId::from_reference_bytes(reference.blob_hash);
        let key = BlobEncryptionKey(reference.attachment_key);
        let reader = self.engine().store().open(id).await?;
        let size = reader.size();

        if size >= v2::HEADER_LEN as u64 {
            let header = reader
                .read_range(ByteRange::new(0, v2::HEADER_LEN as u64))
                .await?;
            if v2::looks_chunked(&header) {
                if let Ok(mut dec) = ChunkedDecryptor::new(&key, &header) {
                    match stream_export(&reader, &mut dec, size, &mut sink).await {
                        Ok(written) => {
                            dec.finish().map_err(|_| FilesError::Chunked)?;
                            sink.commit().await?;
                            return Ok(written);
                        }
                        // Nothing was written: it may be a v1 blob that
                        // merely starts with the magic bytes.
                        Err(StreamFail::BeforeFirstWrite) => {}
                        Err(StreamFail::Fatal(e)) => return Err(e),
                    }
                }
            }
        }
        let verified = self.open_verified_attachment(reference).await?;
        let bytes = verified.as_inner();
        for piece in bytes.chunks(PIECE) {
            sink.write(piece).await?;
        }
        sink.commit().await?;
        Ok(bytes.len() as u64)
    }

    /// Like [`Self::import_attachment`] but with format v2 and bounded
    /// memory: the file is read twice in segments (a hashing pass, then a
    /// writing pass), never held whole. Needs a source that can be read
    /// twice and that does not change between the passes (a change is
    /// caught by the blob's hash check and refused).
    ///
    /// Peers that only know the v1 format cannot decrypt the result;
    /// keep using `import_attachment` for those.
    pub async fn import_attachment_streaming<S: FileSource + ?Sized>(
        &self,
        source: &S,
        media_type: MediaType,
    ) -> Result<ImportedAttachment, FilesError> {
        let limits = self.limits().manifest;
        let before = source.fingerprint().await?;
        if before.size > limits.max_file_bytes {
            return Err(FilesError::TooLarge {
                actual: before.size,
                max: limits.max_file_bytes,
            });
        }
        let key = generate_blob_key();
        let enc = ChunkedEncryptor::new(&key, DEFAULT_SEGMENT).map_err(|_| FilesError::Chunked)?;
        let total = v2::ciphertext_len(before.size, DEFAULT_SEGMENT).ok_or(FilesError::Chunked)?;
        let size = BlobSize::parse(total)?;
        let chunk_size = ChunkSizeClass::for_file_size(total).default_chunk_size(total);

        // Pass 1: hash and chunk-hash, keeping nothing.
        let mut builder = ManifestBuilder::new(chunk_size, limits);
        let mut stream = SegmentStream::new(source, before.size, enc.replay());
        while let Some(piece) = stream.next().await? {
            builder.update(&piece)?;
        }
        let manifest = builder.finish()?;
        if source.fingerprint().await? != before {
            return Err(FilesError::File(
                siar_blob_manifest::file_io::FileError::Changed,
            ));
        }
        let blob_id = manifest.blob_id;
        let descriptor = BlobDescriptor {
            blob_id,
            size,
            chunking: ChunkingDescriptor { chunk_size },
            encryption: EncryptionDescriptor {
                algorithm: EncryptionAlgorithm::ChaCha20Poly1305Chunked,
                nonce: [0; 12],
            },
            media_type: Some(media_type),
            manifest_id: Some(ManifestId::new()),
        };

        let owner = ObjectId::new(&format!("import-{}", uuid::Uuid::new_v4()))?;
        let reference = self.reference_for(&owner, blob_id, BlobRole::Transfer);
        self.refs().apply(&[RefOp::Add(reference.clone())])?;

        // Pass 2: the identical bytes again, into the store.
        let written = async {
            let store = self.engine().store();
            let mut w = store.begin_write(descriptor).await?;
            let mut stream = SegmentStream::new(source, before.size, enc.replay());
            let mut pending: Vec<u8> = Vec::with_capacity(chunk_size as usize);
            let mut index = 0u32;
            let result: Result<(), FilesError> = async {
                while let Some(piece) = stream.next().await? {
                    let mut rest = &piece[..];
                    while !rest.is_empty() {
                        let take = (chunk_size as usize - pending.len()).min(rest.len());
                        pending.extend_from_slice(&rest[..take]);
                        rest = &rest[take..];
                        if pending.len() == chunk_size as usize {
                            w.write_chunk(index, Bytes::from(std::mem::take(&mut pending)))
                                .await?;
                            pending = Vec::with_capacity(chunk_size as usize);
                            index += 1;
                        }
                    }
                }
                if !pending.is_empty() {
                    w.write_chunk(index, Bytes::from(pending)).await?;
                }
                Ok(())
            }
            .await;
            match result {
                Ok(()) => {
                    // finalize re-verifies the whole blob's hash: a source
                    // that changed between the passes fails here.
                    w.finalize().await?;
                    Ok(())
                }
                Err(e) => {
                    let _ = w.abort().await;
                    Err(e)
                }
            }
        }
        .await;
        if let Err(e) = written {
            if let Err(e2) = self.refs().apply(&[RefOp::Remove(reference)]) {
                tracing::warn!(error = %e2, "could not undo an import reference");
            }
            return Err(e);
        }
        Ok(ImportedAttachment {
            blob_id,
            descriptor,
            manifest: Arc::new(manifest),
            media_type,
            import_owner: owner,
            key,
            ciphertext_len: total,
        })
    }

    /// Rebuilds the descriptor and manifest of a blob this device already
    /// holds, streaming it once with bounded memory, so it can be offered
    /// to a peer that asked for it. The format (v1 whole-blob or v2
    /// chunk-wise) is read from the stored bytes themselves. Fails if the
    /// stored bytes no longer hash to their id.
    pub(crate) async fn describe_stored_blob(
        &self,
        blob: BlobId,
    ) -> Result<(BlobDescriptor, Arc<BlobManifest>), FilesError> {
        if let Some(hit) = self.cached_description(blob) {
            return Ok(hit);
        }
        let limits = self.limits().manifest;
        let reader = self.engine().store().open(blob).await?;
        let total = reader.size();
        if total > limits.max_file_bytes {
            return Err(FilesError::TooLarge {
                actual: total,
                max: limits.max_file_bytes,
            });
        }
        let size = BlobSize::parse(total)?;
        let head_len = total.min(v2::HEADER_LEN as u64);
        let head = reader.read_range(ByteRange::new(0, head_len)).await?;
        let (algorithm, nonce) = if v2::looks_chunked(&head) {
            (EncryptionAlgorithm::ChaCha20Poly1305Chunked, [0u8; 12])
        } else {
            let mut n = [0u8; 12];
            let first = head.get(..12).ok_or(FilesError::Blob(
                siar_blob_manifest::store_error::BlobError::InvalidArgument(
                    "stored blob is too short to be an attachment",
                ),
            ))?;
            n.copy_from_slice(first);
            (EncryptionAlgorithm::ChaCha20Poly1305, n)
        };
        let chunk_size = ChunkSizeClass::for_file_size(total).default_chunk_size(total);
        let mut builder = ManifestBuilder::new(chunk_size, limits);
        let mut offset = 0u64;
        while offset < total {
            let n = u64::from(chunk_size).min(total - offset);
            builder.update(&reader.read_range(ByteRange::new(offset, n)).await?)?;
            offset += n;
        }
        let manifest = builder.finish()?;
        if manifest.blob_id != blob {
            return Err(FilesError::Blob(
                siar_blob_manifest::store_error::BlobError::BlobHashMismatch { declared: blob },
            ));
        }
        let descriptor = BlobDescriptor {
            blob_id: blob,
            size,
            chunking: ChunkingDescriptor { chunk_size },
            encryption: EncryptionDescriptor { algorithm, nonce },
            media_type: None,
            manifest_id: Some(ManifestId::new()),
        };
        let manifest = Arc::new(manifest);
        self.cache_description(blob, descriptor, Arc::clone(&manifest));
        Ok((descriptor, manifest))
    }

    pub(crate) fn reference_for(
        &self,
        object: &ObjectId,
        blob: BlobId,
        role: BlobRole,
    ) -> BlobReference {
        BlobReference {
            owner_namespace: self.namespace().clone(),
            object_id: object.clone(),
            blob_id: blob,
            role,
        }
    }
}

impl<C, M, R> CiphertextSource for FilesService<C, M, R>
where
    C: FileChannel + 'static,
    M: BlobMetadataStore + 'static,
    R: ReferenceStore + ?Sized + 'static,
{
    /// Synchronous because the blob-serving transport's trait is.
    /// Bounded by the store's own recorded size.
    fn ciphertext(&self, blob_hash: &[u8; 32]) -> Option<Vec<u8>> {
        let id = BlobId::from_reference_bytes(*blob_hash);
        let reader = self.engine().store().open_sync(id).ok()?;
        let size = reader.size();
        if size > self.limits().manifest.max_file_bytes {
            return None;
        }
        reader
            .read_range_sync(ByteRange::new(0, size))
            .ok()
            .map(|b| b.to_vec())
    }
}

/// The ciphertext of a v2 blob as a sequence of pieces — the header,
/// then one sealed segment per plaintext segment — produced while reading
/// the source one segment at a time.
struct SegmentStream<'a, S: FileSource + ?Sized> {
    source: &'a S,
    len: u64,
    offset: u64,
    enc: ChunkedEncryptor,
    buf: Vec<u8>,
    header_sent: bool,
    done: bool,
}

impl<'a, S: FileSource + ?Sized> SegmentStream<'a, S> {
    fn new(source: &'a S, len: u64, enc: ChunkedEncryptor) -> Self {
        let seg = enc.segment_size() as usize;
        Self {
            source,
            len,
            offset: 0,
            enc,
            buf: vec![0u8; seg],
            header_sent: false,
            done: false,
        }
    }

    async fn next(&mut self) -> Result<Option<Vec<u8>>, FilesError> {
        if !self.header_sent {
            self.header_sent = true;
            return Ok(Some(self.enc.header().to_vec()));
        }
        if self.done {
            return Ok(None);
        }
        let want = u64::from(self.enc.segment_size()).min(self.len - self.offset) as usize;
        read_exact_at(self.source, self.offset, &mut self.buf[..want]).await?;
        self.offset += want as u64;
        let last = self.offset == self.len;
        self.done = last;
        let sealed = self
            .enc
            .encrypt_segment(&self.buf[..want], last)
            .map_err(|_| FilesError::Chunked)?;
        Ok(Some(sealed))
    }
}

enum StreamFail {
    /// Failed on the first segment, before any plaintext reached the sink.
    BeforeFirstWrite,
    Fatal(FilesError),
}

/// Decrypts a v2 blob segment by segment into `sink`.
async fn stream_export<K: FileSink>(
    reader: &impl BlobReader,
    dec: &mut ChunkedDecryptor,
    size: u64,
    sink: &mut K,
) -> Result<u64, StreamFail> {
    let body = size - v2::HEADER_LEN as u64;
    let stride = dec.stride() as u64;
    let mut offset = v2::HEADER_LEN as u64;
    let mut written = 0u64;
    let mut first = true;
    loop {
        let remaining = size - offset;
        let last = remaining <= stride;
        let take = remaining.min(stride);
        let seg = reader
            .read_range(ByteRange::new(offset, take))
            .await
            .map_err(|e| StreamFail::Fatal(e.into()))?;
        let plain = match dec.decrypt_segment(&seg, last) {
            Ok(p) => p,
            Err(_) if first => return Err(StreamFail::BeforeFirstWrite),
            Err(_) => return Err(StreamFail::Fatal(FilesError::Chunked)),
        };
        first = false;
        sink.write(&plain)
            .await
            .map_err(|e| StreamFail::Fatal(e.into()))?;
        written += plain.len() as u64;
        offset += take;
        if last {
            let _ = body;
            return Ok(written);
        }
    }
}

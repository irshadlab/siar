//! Bridges `siar_storage::BlobRepository` to `siar_transport::BlobStore`.
//! Lives here, not in either lower crate, precisely because it depends
//! on both — `siar-storage` and `siar-transport` don't depend on each
//! other (plan.md §86), so something above both has to be the one that
//! connects them.

use siar_storage::BlobRepository;
use siar_transport::BlobStore;
use std::sync::Arc;

pub struct StorageBlobStore(pub Arc<dyn BlobRepository + Send + Sync>);

impl BlobStore for StorageBlobStore {
    fn get(&self, blob_hash: &[u8; 32]) -> Option<Vec<u8>> {
        match self.0.get(blob_hash) {
            Ok(bytes) => bytes,
            Err(e) => {
                tracing::warn!(error = %e, "blob lookup failed");
                None
            }
        }
    }
}

/// Serves blobs held by the file subsystem (`siar_files::FilesService`,
/// or anything else that can read ciphertext by hash) to peers. Bind the
/// endpoint with this instead of [`StorageBlobStore`] to send imported
/// attachments without keeping a second copy in `siar-storage`.
pub struct FilesBlobStore<T: siar_files::CiphertextSource>(pub Arc<T>);

impl<T: siar_files::CiphertextSource> BlobStore for FilesBlobStore<T> {
    fn get(&self, blob_hash: &[u8; 32]) -> Option<Vec<u8>> {
        self.0.ciphertext(blob_hash)
    }
}

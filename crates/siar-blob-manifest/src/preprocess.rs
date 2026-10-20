//! §115-117, §129: the fixed outgoing order, the staging decision, and a
//! type that says "these bytes were verified".
//!
//! **§115 order** is a value, not a convention: [`PipelineStage`]'s
//! derived order *is* the order, and [`PreprocessPlan::stages`] can only
//! emit it. Compression comes before encryption and never after:
//! ciphertext does not compress, and compressing *after* would be
//! pointless; compressing *before* is what makes ciphertext length
//! depend on content, so a caller should only turn it on for data that
//! does not mix secrets with attacker-influenced text.
//!
//! **§116-117 staging.** [`decide_staging`] picks stage-before-send
//! (stable `BlobId`, easy retry/multi-recipient, restart without the
//! original) unless storage is short. `StreamOnly` needs a chunk-wise
//! AEAD: the v1 blob format is one whole-blob ChaCha20-Poly1305 whose
//! tag arrives last, so it could not stream. Format v2
//! ([`crate::chunked_encryption`]) can, hence
//! [`STREAMING_ENCRYPTION_SUPPORTED`]` = true`. What exists today is
//! the *bounded-memory staged* path (`siar-files`'
//! `import_attachment_streaming`: two passes, one segment in memory,
//! blob staged on disk). A transfer driver that pipes straight to the
//! network without staging is not built; `decide_staging` returning
//! `StreamOnly` is a statement about the format, not a promise that a
//! driver exists.
//!
//! **§129** [`Verified`] can only be produced by an operation that
//! authenticated the bytes ([`crate::encryption::decrypt_blob_verified`]).
//! Media decoders take `&Verified<_>`; there is no way to hand them
//! unverified chunks by accident.

use std::fmt;

use thiserror::Error;

/// Whether a blob format can encrypt incrementally (see module docs).
pub const STREAMING_ENCRYPTION_SUPPORTED: bool = true;

/// §115, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PipelineStage {
    ValidateMetadata,
    GenerateDerivative,
    Compress,
    Encrypt,
    ChunkAndHash,
    Stage,
    Transfer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StagingMode {
    /// §116: prepare the encrypted blob locally, then transfer.
    StageBeforeSend,
    /// §117: encrypt/chunk/send without keeping the whole blob.
    StreamOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreprocessPlan {
    pub derivative: bool,
    pub compress: bool,
    pub staging: StagingMode,
}

impl PreprocessPlan {
    /// The stages this plan runs, always in [`PipelineStage`] order.
    /// Stream-only skips `Stage`.
    pub fn stages(&self) -> Vec<PipelineStage> {
        use PipelineStage::*;
        let mut v = vec![ValidateMetadata];
        if self.derivative {
            v.push(GenerateDerivative);
        }
        if self.compress {
            v.push(Compress);
        }
        v.push(Encrypt);
        v.push(ChunkAndHash);
        if self.staging == StagingMode::StageBeforeSend {
            v.push(Stage);
        }
        v.push(Transfer);
        v
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StagingInputs {
    /// Plaintext size.
    pub size: u64,
    /// Space the store can still use (quota headroom ∧ free disk).
    pub available_bytes: u64,
    /// The application's semantics tolerate a source that cannot be
    /// re-read after a restart (§117 "only when semantics permit").
    pub app_allows_stream_only: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum StagingError {
    #[error("need {needed} bytes to stage, {available} available")]
    InsufficientStorage { needed: u64, available: u64 },
}

/// AEAD nonce plus tag.
const ENCRYPTION_OVERHEAD: u64 = 12 + 16;

/// Stage unless it does not fit; stream only if both the application and
/// the blob format allow it.
pub fn decide_staging(i: StagingInputs) -> Result<StagingMode, StagingError> {
    let needed = i.size.saturating_add(ENCRYPTION_OVERHEAD);
    if needed <= i.available_bytes {
        return Ok(StagingMode::StageBeforeSend);
    }
    if i.app_allows_stream_only && STREAMING_ENCRYPTION_SUPPORTED {
        return Ok(StagingMode::StreamOnly);
    }
    Err(StagingError::InsufficientStorage {
        needed,
        available: i.available_bytes,
    })
}

/// Bytes that were authenticated (§129). Cannot be constructed outside
/// this crate; `Debug` shows only the type, never the content.
pub struct Verified<T>(T);

impl<T> Verified<T> {
    pub(crate) fn new(inner: T) -> Self {
        Self(inner)
    }

    pub fn as_inner(&self) -> &T {
        &self.0
    }

    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> fmt::Debug for Verified<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Verified(..)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(derivative: bool, compress: bool, staging: StagingMode) -> PreprocessPlan {
        PreprocessPlan {
            derivative,
            compress,
            staging,
        }
    }

    #[test]
    fn every_plan_runs_stages_in_the_fixed_order() {
        for d in [false, true] {
            for c in [false, true] {
                for s in [StagingMode::StageBeforeSend, StagingMode::StreamOnly] {
                    let st = plan(d, c, s).stages();
                    assert!(st.windows(2).all(|w| w[0] < w[1]), "{st:?}");
                    assert_eq!(st.first(), Some(&PipelineStage::ValidateMetadata));
                    assert_eq!(st.last(), Some(&PipelineStage::Transfer));
                }
            }
        }
    }

    #[test]
    fn compression_is_always_before_encryption() {
        let st = plan(false, true, StagingMode::StageBeforeSend).stages();
        let pos = |s| st.iter().position(|x| *x == s).unwrap();
        assert!(pos(PipelineStage::Compress) < pos(PipelineStage::Encrypt));
    }

    #[test]
    fn stream_only_skips_staging_and_stage_before_send_has_it() {
        assert!(!plan(false, false, StagingMode::StreamOnly)
            .stages()
            .contains(&PipelineStage::Stage));
        assert!(plan(false, false, StagingMode::StageBeforeSend)
            .stages()
            .contains(&PipelineStage::Stage));
    }

    #[test]
    fn staging_is_chosen_when_it_fits() {
        let m = decide_staging(StagingInputs {
            size: 1_000,
            available_bytes: 1_028,
            app_allows_stream_only: true,
        });
        assert_eq!(m, Ok(StagingMode::StageBeforeSend));
    }

    #[test]
    fn a_blob_that_does_not_fit_streams_only_if_the_app_allows_it() {
        let inputs = |allow| StagingInputs {
            size: 1_000,
            available_bytes: 1_027,
            app_allows_stream_only: allow,
        };
        assert_eq!(decide_staging(inputs(true)), Ok(StagingMode::StreamOnly));
        assert_eq!(
            decide_staging(inputs(false)),
            Err(StagingError::InsufficientStorage {
                needed: 1_028,
                available: 1_027
            })
        );
    }

    #[test]
    fn size_overflow_does_not_wrap_into_fitting() {
        let r = decide_staging(StagingInputs {
            size: u64::MAX,
            available_bytes: 10,
            app_allows_stream_only: false,
        });
        assert!(r.is_err());
    }

    #[test]
    fn verified_never_prints_its_content() {
        let v = Verified::new(vec![1u8, 2, 3]);
        assert_eq!(format!("{v:?}"), "Verified(..)");
        assert_eq!(v.as_inner(), &vec![1, 2, 3]);
    }
}

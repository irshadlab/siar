//! §74 "Source Selection", §75 "Trust of Sources", §77 "Manifest
//! Authentication".
//!
//! §75 gets no function of its own — it's the property that makes
//! every hash check in this crate meaningful, not an additional check
//! itself: "content addressing means untrusted transport/source can
//! provide bytes, but receiver still verifies hashes." That's
//! [`crate::verify::verify_chunk`]/[`crate::verify::verify_complete_blob`]
//! (§76) by construction — both recompute a real hash from the actual
//! bytes on hand and compare, never trust a source's own claim about
//! what it's sending. Naming §75 here as the reason those functions
//! matter, rather than adding a redundant "is this source trusted"
//! flag that would do nothing real.

use serde::{Deserialize, Serialize};

use crate::ids::BlobId;
use crate::manifest::BlobManifest;

/// §74's own four named source kinds. Selection — which one to
/// actually use — is deliberately NOT this crate's job: "routing
/// chooses path/source according to policy" is `siar-routing-policy`'s
/// own domain (Part 03). This is the vocabulary a routing decision
/// would be expressed in, not the decision itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlobSource {
    OriginalSender,
    SendersOtherDevice,
    AuthorizedCache,
    Relay,
}

/// §77: "An attacker must not be able to reorder/replace chunk
/// descriptors." Reading §76's own
/// [`crate::verify::verify_complete_blob`] carefully surfaces exactly
/// the gap §77 is warning about: that function checks a manifest
/// against *itself* (does the reassembled ciphertext match
/// `manifest.blob_id`?) — internally consistent, but it never asks
/// whether `manifest.blob_id` is the identity the receiver actually
/// meant to fetch. An attacker who replaces an entire manifest
/// wholesale (reordered descriptors, a different `blob_id` to match)
/// would pass `verify_complete_blob` cleanly, because that function
/// has no external reference point to catch a wholesale swap.
///
/// This function IS that external reference point: `expected_blob_id`
/// must come from an authenticated source — §77's own suggested
/// mechanism, "manifest covered by E2EE message signature/
/// authentication" (in this crate's own terms: the `blob_id` inside
/// the [`crate::descriptor::BlobDescriptor`] a [`crate::file_offer::FileOffer`]
/// carries, itself protected by whatever signs/encrypts the message
/// that offer arrived in — outside this crate's own scope, see
/// `lib.rs`'s own top-level note on cryptographic integration). A
/// caller that skips this and only calls `verify_complete_blob`
/// against a manifest it pulled from the same untrusted place as the
/// bytes has not actually closed the §77 gap, even though every
/// individual chunk hash would still check out.
pub fn verify_manifest_authenticity(manifest: &BlobManifest, expected_blob_id: BlobId) -> bool {
    manifest.blob_id == expected_blob_id
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::limits::ManifestLimits;
    use crate::manifest::build_manifest;

    #[test]
    fn a_manifest_matching_the_expected_id_is_authentic() {
        let ciphertext = vec![7u8; 300];
        let manifest = build_manifest(&ciphertext, 100, &ManifestLimits::default()).unwrap();
        let expected = manifest.blob_id;
        assert!(verify_manifest_authenticity(&manifest, expected));
    }

    #[test]
    fn a_wholesale_swapped_manifest_fails_against_the_real_expected_id() {
        // §77's own attack: an attacker substitutes an entirely
        // different, internally-consistent manifest (its own blob_id
        // matches ITS OWN reassembled ciphertext — `verify_complete_blob`
        // alone would pass this). Only checking against an
        // authenticated `expected_blob_id` from outside the swapped
        // manifest catches it.
        let real_ciphertext = vec![7u8; 300];
        let real_manifest =
            build_manifest(&real_ciphertext, 100, &ManifestLimits::default()).unwrap();
        let attacker_ciphertext = vec![9u8; 300];
        let attacker_manifest =
            build_manifest(&attacker_ciphertext, 100, &ManifestLimits::default()).unwrap();

        // The swapped manifest is perfectly self-consistent...
        assert!(crate::verify::verify_complete_blob(
            &attacker_ciphertext,
            &attacker_manifest
        ));
        // ...but fails the moment it's checked against the real,
        // externally-known expected id.
        assert!(!verify_manifest_authenticity(
            &attacker_manifest,
            real_manifest.blob_id
        ));
    }
}

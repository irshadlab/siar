//! Key/domain separation registry (Part 28 §44).
//!
//! §44's own rule: "never interpret a signature from one domain as
//! valid in another." This module doesn't retrofit every existing
//! signing/AEAD call site in this workspace to use these constants —
//! several already have their own, independently-chosen domain strings
//! (`mailbox_token.rs`'s `ROOT_SECRET_DOMAIN`/`TOKEN_DOMAIN`,
//! `envelope.rs`'s `MessageType`-tagged AAD) that already achieve real
//! domain separation under different names; renaming those to match
//! this registry would be a wire-format-breaking change for zero
//! functional benefit, since the actual property §44 asks for — two
//! different purposes never sharing one domain string — already holds
//! for them. What this registry *does* provide: one canonical, typed
//! place for any *new* domain-separated construction in this crate (or
//! a sibling crate that mirrors the label string, since not everything
//! that needs a domain label depends on `siar-crypto` — see
//! `siar-blob-manifest::metadata_encryption`'s own doc comment) to draw
//! from, rather than each new module inventing its own ad hoc string.
//!
//! §44's own six domains are included verbatim
//! (`comm/msg/v1`...`comm/authority/v1`); `FileMetadata` is one this
//! crate needed and the spec's own list didn't name (§31's metadata
//! encryption is a distinct purpose from §29's file *content*
//! encryption, and reusing `comm/file/v1` for both would itself violate
//! §44's own rule) — added as an extension, not a deviation, following
//! the same `comm/<purpose>/v1` shape the other six use.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CryptoDomain {
    /// `comm/msg/v1` — one-to-one and small-group message envelopes
    /// (`envelope.rs`). Not currently used as `envelope.rs`'s AAD
    /// literal string — that module domain-separates via
    /// `MessageType` instead — but available for a future construction
    /// that wants a plain byte-string domain label rather than a typed
    /// enum discriminant.
    Message,
    /// `comm/file/v1` — file/blob content encryption
    /// (`siar-blob-manifest::encryption`, §28-29; see this module's own
    /// top doc for why that's a reconciliation, not new code here).
    File,
    /// `comm/file-metadata/v1` — file metadata encryption (§31,
    /// `siar-blob-manifest::metadata_encryption`). Not one of §44's own
    /// six — added because §31 is a real, distinct purpose from §29's
    /// file *content* key and deserves its own domain rather than
    /// silently sharing `File`'s.
    FileMetadata,
    /// `comm/file-keywrap/v1` — the pairwise secret that wraps a blob key
    /// for one recipient (`siar-blob-manifest::key_wrapping`, spec 05
    /// §169-170). Its own domain, so the secret that protects a file key
    /// can never equal the session key or the mailbox-token secret even
    /// though all three come from the same ECDH exchange.
    FileKeyWrap,
    /// `comm/dtn/v1` — DTN bundle-level constructions (reserved; no
    /// bundle-level signing/AEAD construction in this crate uses it
    /// yet — `siar-dtn-bundle` carries only ciphertext/references, see
    /// this module's top doc).
    Dtn,
    /// `comm/device-sync/v1` — device-to-device sync channel material
    /// (reserved; §19's "establishes encrypted device-sync channel" has
    /// no dedicated construction yet — today it would run over an
    /// ordinary `Session`).
    DeviceSync,
    /// `comm/backup/v1` — backup encryption (reserved; §24's restore-
    /// safety logic exists, but backup encryption itself — what
    /// actually protects a backup file at rest — is not built).
    Backup,
    /// `comm/authority/v1` — root/organization-authority signing
    /// (reserved; `RootIdentityKey`'s signatures over certificates/
    /// directories/`DeviceRevocation` records are this domain in
    /// substance, though `siar-identity-multidevice` signs raw
    /// postcard-encoded payloads today rather than prefixing this
    /// label — another case like `Message` above).
    Authority,
}

impl CryptoDomain {
    pub const fn label(self) -> &'static [u8] {
        match self {
            Self::Message => b"comm/msg/v1",
            Self::File => b"comm/file/v1",
            Self::FileMetadata => b"comm/file-metadata/v1",
            Self::FileKeyWrap => b"comm/file-keywrap/v1",
            Self::Dtn => b"comm/dtn/v1",
            Self::DeviceSync => b"comm/device-sync/v1",
            Self::Backup => b"comm/backup/v1",
            Self::Authority => b"comm/authority/v1",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [CryptoDomain; 8] = [
        CryptoDomain::Message,
        CryptoDomain::File,
        CryptoDomain::FileMetadata,
        CryptoDomain::FileKeyWrap,
        CryptoDomain::Dtn,
        CryptoDomain::DeviceSync,
        CryptoDomain::Backup,
        CryptoDomain::Authority,
    ];

    #[test]
    fn every_domain_label_is_unique() {
        for (i, a) in ALL.iter().enumerate() {
            for b in &ALL[i + 1..] {
                assert_ne!(a.label(), b.label(), "{a:?} and {b:?} share a label");
            }
        }
    }

    #[test]
    fn labels_follow_the_comm_purpose_v1_shape() {
        for domain in ALL {
            let label = std::str::from_utf8(domain.label()).unwrap();
            assert!(label.starts_with("comm/"));
            assert!(label.ends_with("/v1"));
        }
    }
}

//! Credential + key-package generation — mirrors `book_code.rs`'s
//! `generate_credential`/`generate_key_package` helpers (anchors
//! `create_basic_credential`, `create_credential_keys`,
//! `create_key_package`) line-for-line in the openmls calls, adapted
//! only to take a `siar_domain::DeviceId` as the credential identity
//! instead of a raw `Vec<u8>` literal.

use crate::CIPHERSUITE;
use openmls::prelude::*;
// Same trait-scope fix as group.rs (see that file's identical import for
// the full explanation): `openmls::prelude::*`'s nested `tls_codec::*`
// glob doesn't reliably surface `Serialize`/`Deserialize` here either —
// `decode_key_package` needs `Deserialize` (`KeyPackageIn::
// tls_deserialize_exact`) and `encode_key_package` needs `Serialize`
// (`KeyPackage::tls_serialize_detached`), confirmed against the real
// compiler errors both calls hit.
use openmls::prelude::tls_codec::{Deserialize as _, Serialize as _};
use openmls_basic_credential::SignatureKeyPair;
use openmls_rust_crypto::OpenMlsRustCrypto;
use siar_domain::DeviceId;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum MlsIdentityError {
    #[error("failed to generate MLS signature keypair: {0}")]
    SignatureKeyGeneration(String),
    #[error("failed to persist MLS signature keypair to the provider's key store: {0}")]
    SignatureKeyStorage(String),
    #[error("failed to build MLS key package: {0}")]
    KeyPackage(String),
}

/// Everything one device needs to participate in MLS groups: its
/// credential (identity, per book_code.rs's `CredentialWithKey`), the
/// matching signing keypair, and a fresh key package other members'
/// `add_member` calls consume to admit this device.
///
/// Deliberately device-scoped, not account-scoped — matches
/// architecture.md §7/§38's account-vs-device split: a multi-device
/// account has one `MlsIdentity` (and, once a device is added to a
/// group, one leaf in that group's tree) per device, the same way it
/// has one `DeviceIdentity`/`Session` set per device for 1:1 chat.
pub struct MlsIdentity {
    pub credential_with_key: CredentialWithKey,
    pub signature_keys: SignatureKeyPair,
    pub key_package: KeyPackageBundle,
}

/// Generates a fresh `MlsIdentity` for `device` against `provider`'s
/// key store. Generic over `P: OpenMlsProvider` (was hardcoded to
/// `OpenMlsRustCrypto` before `persistent.rs` added a second provider
/// type) — `provider` must be the same *instance* this device's
/// `MlsGroupSession`s are later created/joined with, whichever concrete
/// provider type it is: the signing key this writes to
/// `provider.storage()` (book_code.rs:
/// `signature_keys.store(provider.storage()).unwrap()`) has to be
/// readable from there for signing to work later.
pub fn generate_identity<P: OpenMlsProvider>(
    device: DeviceId,
    provider: &P,
) -> Result<MlsIdentity, MlsIdentityError> {
    generate_identity_with_suite(device, provider, CIPHERSUITE)
}

/// [`generate_identity`] for an explicit ciphersuite — the key package is
/// only usable in groups of that suite (1:1 sessions use the post-quantum
/// hybrid [`crate::PQ_CIPHERSUITE`]).
pub fn generate_identity_with_suite<P: OpenMlsProvider>(
    device: DeviceId,
    provider: &P,
    suite: openmls_traits::types::Ciphersuite,
) -> Result<MlsIdentity, MlsIdentityError> {
    // `DeviceId` has no direct byte-conversion method of its own
    // (`crates/siar-domain/src/ids.rs`'s `newtype_id!` macro exposes
    // `as_uuid()` -> `Uuid`, not raw bytes) — go through the `uuid`
    // crate's own well-established `Uuid::as_bytes() -> &[u8; 16]`
    // instead of inventing a method on `DeviceId` this crate doesn't
    // own.
    let identity_bytes = device.as_uuid().as_bytes().to_vec();

    // book_code.rs anchor `create_basic_credential`.
    let credential = BasicCredential::new(identity_bytes);

    // book_code.rs anchor `create_credential_keys`.
    let signature_keys = SignatureKeyPair::new(suite.signature_algorithm())
        .map_err(|e| MlsIdentityError::SignatureKeyGeneration(format!("{e:?}")))?;
    signature_keys
        .store(provider.storage())
        .map_err(|e| MlsIdentityError::SignatureKeyStorage(format!("{e:?}")))?;

    let credential_with_key = CredentialWithKey {
        credential: credential.into(),
        signature_key: signature_keys.to_public_vec().into(),
    };

    // book_code.rs anchor `create_key_package`. `Extensions::default()`
    // (empty) here, not the ExternalSenders/leaf-extension example
    // book_code.rs shows in its `mls_group_create_config_example`
    // anchor — this workspace has no external-sender/authority-signed
    // group concept yet (that's next.md §49's "Authority alerts",
    // unrelated to and out of scope for this pass), so there's nothing
    // real to put in that extension list yet. Leaving it empty is the
    // honest default, not a stand-in for something unimplemented.
    let key_package = KeyPackage::builder()
        .build(
            suite,
            provider,
            &signature_keys,
            credential_with_key.clone(),
        )
        .map_err(|e| MlsIdentityError::KeyPackage(format!("{e:?}")))?;

    Ok(MlsIdentity {
        credential_with_key,
        signature_keys,
        key_package,
    })
}

/// Decodes and validates a serialized key package another device
/// published — the receive-side counterpart to `generate_identity`'s
/// `key_package` output, needed by whoever calls
/// `MlsGroupSession::add_member` with bytes that arrived over the wire
/// rather than one this process generated itself.
///
/// `KeyPackageIn::validate` (verified from `openmls/src/key_packages/
/// key_package_in.rs`) needs an `&impl OpenMlsCrypto` — a *stateless*
/// crypto-operations handle, not the stateful storage/signing-key
/// provider `MlsGroupSession`/`generate_identity` are threaded through
/// elsewhere in this crate. A throwaway `OpenMlsRustCrypto::default()`
/// is the right tool here specifically because `OpenMlsCrypto`'s
/// operations (signature verification, in this call) don't depend on
/// anything the throwaway instance would have needed to have persisted
/// — unlike e.g. `generate_identity`'s provider, which *must* be the
/// long-lived one a signing key was stored against.
pub fn decode_key_package(bytes: &[u8]) -> Result<KeyPackage, MlsIdentityError> {
    let key_package_in = KeyPackageIn::tls_deserialize_exact(bytes)
        .map_err(|e| MlsIdentityError::KeyPackage(format!("failed to deserialize: {e:?}")))?;

    let crypto_provider = OpenMlsRustCrypto::default();
    key_package_in
        .validate(crypto_provider.crypto(), ProtocolVersion::Mls10)
        .map_err(|e| MlsIdentityError::KeyPackage(format!("failed to validate: {e:?}")))
}

/// Serializes a `KeyPackage` (e.g. `generate_identity`'s
/// `identity.key_package.key_package()`) to publishable wire bytes —
/// the send-side counterpart to `decode_key_package`. `KeyPackage`
/// implements `tls_codec`'s `Serialize` trait directly (confirmed from
/// `openmls/src/key_packages/mod.rs`'s `impl TlsSerializeTrait for
/// KeyPackage`, not assumed just because `KeyPackageIn`'s
/// `Deserialize` side works — they're different types), so
/// `tls_serialize_detached()` — the same provided trait method
/// `MlsMessageIn::tls_deserialize_exact`'s `Deserialize` counterpart
/// supplies — is the right call here, not `MlsMessageOut::to_bytes()`
/// (that's a different, message-framing type this bare `KeyPackage`
/// isn't one of).
///
/// **Read `group.rs`'s doc comment on `MlsGroupSession::join_from_welcome`
/// before calling this to publish a key package for later use**: the
/// `SignatureKeyPair`/provider used to build the `KeyPackage` being
/// serialized here must be kept alive and reused verbatim when this
/// device later processes the `Welcome` that consumes it — an MLS
/// `Welcome` is encrypted to that exact key package's private material,
/// not regeneratable from a fresh identity after the fact. This
/// function only serializes; it doesn't enforce that discipline —
/// `siar_messaging::group_service::GroupService::publish_key_package`
/// is where that's actually enforced.
pub fn encode_key_package(key_package: &KeyPackage) -> Result<Vec<u8>, MlsIdentityError> {
    key_package
        .tls_serialize_detached()
        .map_err(|e| MlsIdentityError::KeyPackage(format!("failed to serialize: {e:?}")))
}

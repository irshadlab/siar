//! §72-§76, §129: node roles, per-role feature sets, and headless
//! provisioning.
//!
//! A headless node starts **unprovisioned** (§73): it may run only a limited
//! bootstrap listener — no relay forwarding, no trusted messaging. An admin
//! scans the node's terminal QR (Part 15), which carries a one-time
//! *challenge*; the admin's device answers with a signed [`ProvisionPayload`]
//! naming the node's role and config. Only then does the node activate the
//! role's [`FeatureSet`] (§74-§75).
//!
//! Design points worth stating:
//!
//! * The signature is checked through the [`ProvisionVerifier`] trait, so this
//!   crate takes no stance on the admin key scheme (the daemon wires the real
//!   identity verifier; tests use a fake). **No verifier ⇒ no provisioning**:
//!   there is no default that accepts.
//! * The payload is bound to *this* node by its challenge, so a capture of a
//!   valid payload cannot be replayed to another node, and the challenge is
//!   single-use: once provisioned, the node refuses any further payload until
//!   an explicit local [`Provisioner::reset`].
//! * The signed bytes are domain-separated ([`signing_message`]) so a
//!   signature made for some other purpose cannot be presented as one.
//! * The embedded config is parsed and validated *before* anything is
//!   persisted; a bad config never leaves a half-provisioned node.

use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::config::RuntimeConfig;

/// §74. The role controls which features are enabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NodeRole {
    PersonalDevice,
    RelayGateway,
    DtnNode,
    OrganizationNode,
    StorageNode,
    TestNode,
}

impl NodeRole {
    pub const ALL: [NodeRole; 6] = [
        Self::PersonalDevice,
        Self::RelayGateway,
        Self::DtnNode,
        Self::OrganizationNode,
        Self::StorageNode,
        Self::TestNode,
    ];

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::PersonalDevice => "personal-device",
            Self::RelayGateway => "relay-gateway",
            Self::DtnNode => "dtn-node",
            Self::OrganizationNode => "organization-node",
            Self::StorageNode => "storage-node",
            Self::TestNode => "test-node",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|r| r.name() == s)
    }

    /// §75: what this role runs. Data, not scattered `if`s, so "does a DTN
    /// node run calls?" has one answer.
    #[must_use]
    pub const fn features(self) -> FeatureSet {
        use Feature as F;
        match self {
            Self::PersonalDevice => FeatureSet::of(&[
                F::Iroh,
                F::Ble,
                F::Lan,
                F::Dtn,
                F::Files,
                F::Messaging,
                F::Calls,
                F::Gui,
            ]),
            // §75's own example: Iroh, BLE, LAN, DTN, files/chunks; no calls, no GUI.
            Self::DtnNode => FeatureSet::of(&[F::Iroh, F::Ble, F::Lan, F::Dtn, F::Files]),
            Self::RelayGateway => FeatureSet::of(&[F::Iroh, F::Lan, F::Dtn, F::RelayForwarding]),
            Self::OrganizationNode => FeatureSet::of(&[
                F::Iroh,
                F::Lan,
                F::Dtn,
                F::Files,
                F::Messaging,
                F::RelayForwarding,
            ]),
            Self::StorageNode => FeatureSet::of(&[F::Iroh, F::Lan, F::Files, F::Storage]),
            Self::TestNode => FeatureSet::of(&[
                F::Iroh,
                F::Ble,
                F::Lan,
                F::Dtn,
                F::Files,
                F::Messaging,
                F::Calls,
                F::RelayForwarding,
                F::Storage,
            ]),
        }
    }
}

impl fmt::Display for NodeRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// One switchable capability of a node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum Feature {
    Iroh = 0,
    Ble = 1,
    Lan = 2,
    Dtn = 3,
    Files = 4,
    Messaging = 5,
    Calls = 6,
    /// Forwarding other people's traffic (§73: never while unprovisioned).
    RelayForwarding = 7,
    Gui = 8,
    Storage = 9,
    /// §73: the limited listener an unprovisioned node runs to be enrolled.
    BootstrapListener = 10,
}

impl Feature {
    pub const ALL: [Feature; 11] = [
        Self::Iroh,
        Self::Ble,
        Self::Lan,
        Self::Dtn,
        Self::Files,
        Self::Messaging,
        Self::Calls,
        Self::RelayForwarding,
        Self::Gui,
        Self::Storage,
        Self::BootstrapListener,
    ];

    const fn bit(self) -> u16 {
        1 << (self as u8)
    }
}

/// A set of [`Feature`]s as a `u16` bitset: `Copy`, allocation-free, usable
/// in `const` tables.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct FeatureSet(u16);

impl FeatureSet {
    pub const EMPTY: Self = Self(0);

    #[must_use]
    pub const fn of(features: &[Feature]) -> Self {
        let mut bits = 0u16;
        let mut i = 0;
        while i < features.len() {
            bits |= features[i].bit();
            i += 1;
        }
        Self(bits)
    }

    #[must_use]
    pub const fn contains(self, f: Feature) -> bool {
        self.0 & f.bit() != 0
    }

    #[must_use]
    pub const fn without(self, f: Feature) -> Self {
        Self(self.0 & !f.bit())
    }

    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub fn iter(self) -> impl Iterator<Item = Feature> {
        Feature::ALL.into_iter().filter(move |f| self.contains(*f))
    }
}

impl fmt::Debug for FeatureSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_set().entries(self.iter()).finish()
    }
}

/// §73: all an unprovisioned node may do.
pub const UNPROVISIONED_FEATURES: FeatureSet = FeatureSet::of(&[Feature::BootstrapListener]);

/// Bounds on what a provisioning payload may carry.
pub const MAX_PROVISION_CONFIG: usize = 16 * 1024;
pub const MAX_ISSUER: usize = 128;
/// Longest a payload may stay valid after it was issued (an admin scans the
/// QR, the answer comes back within minutes — not days).
pub const MAX_PROVISION_VALIDITY_MS: u64 = 24 * 60 * 60 * 1000;

const SIGN_DOMAIN: &[u8] = b"siar-provision-v1\0";
const STATE_FILE: &str = "node.toml";
const CHALLENGE_LEN: usize = 16;

/// What the admin's device signs (§72: "signed role/config").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProvisionPayload {
    /// The role the node should take.
    pub role: NodeRole,
    /// A [`RuntimeConfig`] in TOML. Validated before it is accepted.
    pub config_toml: String,
    /// Admin identity (opaque id the [`ProvisionVerifier`] resolves).
    pub issuer: String,
    pub issued_at_ms: u64,
    pub valid_for_ms: u64,
    /// The challenge from the node's QR; binds the payload to one node.
    pub challenge: [u8; CHALLENGE_LEN],
}

/// The exact bytes to sign / verify: a domain tag plus the canonical
/// postcard encoding of the payload.
#[must_use]
pub fn signing_message(p: &ProvisionPayload) -> Vec<u8> {
    let mut m = SIGN_DOMAIN.to_vec();
    // Serialising plain data into a Vec cannot fail; an empty tail would
    // make every signature check fail closed anyway.
    m.extend(postcard::to_allocvec(p).unwrap_or_default());
    m
}

/// Checks that `signature` over `message` was made by `issuer`'s key.
///
/// Must also decide whether `issuer` is *authorised to provision this node*
/// (an allow-list / owner key); a valid signature from a stranger is not
/// enough.
pub trait ProvisionVerifier: Send + Sync + 'static {
    fn verify(&self, issuer: &str, message: &[u8], signature: &[u8]) -> bool;
}

/// The config a node actually runs: the admin's payload config laid over the
/// local file — **except** `[node]` (who the admins are) and `[secrets]`
/// (paths to local key material), which only the local file may set.
#[must_use]
pub fn overlay_config(local: &RuntimeConfig, applied: Option<RuntimeConfig>) -> RuntimeConfig {
    match applied {
        None => local.clone(),
        Some(mut a) => {
            a.node = local.node.clone();
            a.admin = local.admin.clone();
            a.secrets = local.secrets.clone();
            a
        }
    }
}

/// [`ProvisionVerifier`] backed by an allow-list of Ed25519 public keys.
///
/// The payload's `issuer` is the **lowercase hex of the admin's public key**
/// — self-describing, so there is no separate name→key table to keep in
/// sync. A payload is accepted only if that key is on the allow-list *and*
/// the signature verifies (strictly) under it. A FROST group root key is an
/// ordinary Ed25519 key, so a threshold group can be the provisioning admin
/// without any change here.
pub struct Ed25519AdminVerifier {
    keys: HashSet<[u8; 32]>,
}

fn parse_hex32(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 || !s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(out)
}

impl Ed25519AdminVerifier {
    /// Build from hex keys (e.g. `NodeSection::admin_keys`). An empty list is
    /// an error: a verifier that trusts nobody is a misconfiguration, and one
    /// that trusts everybody must not be constructible.
    pub fn from_hex_keys(keys: &[String]) -> Result<Self, String> {
        if keys.is_empty() {
            return Err("no admin keys configured".into());
        }
        let mut set = HashSet::new();
        for k in keys {
            let bytes = parse_hex32(k).ok_or("admin key is not 64 lowercase hex chars")?;
            ed25519_dalek::VerifyingKey::from_bytes(&bytes)
                .map_err(|_| "admin key is not a valid Ed25519 point")?;
            set.insert(bytes);
        }
        Ok(Self { keys: set })
    }
}

impl ProvisionVerifier for Ed25519AdminVerifier {
    fn verify(&self, issuer: &str, message: &[u8], signature: &[u8]) -> bool {
        let Some(key) = parse_hex32(issuer) else {
            return false;
        };
        if !self.keys.contains(&key) {
            return false;
        }
        let (Ok(vk), Ok(sig)) = (
            ed25519_dalek::VerifyingKey::from_bytes(&key),
            <[u8; 64]>::try_from(signature),
        ) else {
            return false;
        };
        vk.verify_strict(message, &ed25519_dalek::Signature::from_bytes(&sig))
            .is_ok()
    }
}

/// What the IPC layer needs from a [`Provisioner`], without its generic
/// verifier parameter.
pub trait ProvisioningService: Send + Sync + 'static {
    fn status(&self) -> ProvisionStatus;
    /// `payload` is the postcard encoding of a [`ProvisionPayload`].
    fn apply_encoded(
        &self,
        payload: &[u8],
        signature: &[u8],
        now_ms: u64,
    ) -> Result<Provisioned, ProvisionError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvisionStatus {
    pub role: Option<NodeRole>,
    /// Hex challenge to put in the QR; `None` once provisioned.
    pub challenge_hex: Option<String>,
}

impl ProvisionStatus {
    #[must_use]
    pub fn render(&self) -> String {
        match (&self.role, &self.challenge_hex) {
            (Some(r), _) => format!("provisioned role={r}\n"),
            (None, Some(c)) => format!("unprovisioned challenge={c}\n"),
            (None, None) => "unprovisioned\n".to_owned(),
        }
    }
}

impl<V: ProvisionVerifier> ProvisioningService for Provisioner<V> {
    fn status(&self) -> ProvisionStatus {
        ProvisionStatus {
            role: match self.state() {
                Provisioning::Provisioned(p) => Some(p.role),
                Provisioning::Unprovisioned => None,
            },
            challenge_hex: self.challenge().map(|c| {
                c.iter().fold(String::with_capacity(32), |mut s, b| {
                    use std::fmt::Write as _;
                    let _ = write!(s, "{b:02x}");
                    s
                })
            }),
        }
    }

    fn apply_encoded(
        &self,
        payload: &[u8],
        signature: &[u8],
        now_ms: u64,
    ) -> Result<Provisioned, ProvisionError> {
        if payload.len() > MAX_PROVISION_CONFIG + 1024 {
            return Err(ProvisionError::Malformed("payload too large"));
        }
        let p: ProvisionPayload = postcard::from_bytes(payload)
            .map_err(|_| ProvisionError::Malformed("payload does not decode"))?;
        // Canonical form only: re-encoding must reproduce the exact bytes the
        // admin signed over, so trailing data or an alternative encoding
        // cannot ride along.
        if postcard::to_allocvec(&p).ok().as_deref() != Some(payload) {
            return Err(ProvisionError::Malformed("payload is not canonical"));
        }
        self.apply(&p, signature, now_ms)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provisioned {
    pub role: NodeRole,
    pub issuer: String,
    pub provisioned_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Provisioning {
    Unprovisioned,
    Provisioned(Provisioned),
}

#[derive(Debug, Error)]
pub enum ProvisionError {
    #[error("node is already provisioned (reset it locally first)")]
    AlreadyProvisioned,
    #[error("signature rejected")]
    BadSignature,
    #[error("payload is for a different node (challenge mismatch)")]
    WrongChallenge,
    #[error("payload expired or not yet valid")]
    Expired,
    #[error("payload malformed: {0}")]
    Malformed(&'static str),
    #[error("embedded config rejected: {0}")]
    BadConfig(String),
    #[error("storage error: {0}")]
    Io(#[from] io::Error),
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Persisted {
    version: u32,
    role: NodeRole,
    issuer: String,
    provisioned_at_ms: u64,
    config_toml: String,
}

/// Owns a node's provisioning state and the activation decision.
pub struct Provisioner<V: ProvisionVerifier> {
    dir: PathBuf,
    verifier: V,
    inner: Mutex<Inner>,
}

struct Inner {
    state: Provisioning,
    /// `Some` only while unprovisioned; consumed by a successful apply.
    challenge: Option<[u8; CHALLENGE_LEN]>,
    config: Option<RuntimeConfig>,
}

fn fresh_challenge() -> [u8; CHALLENGE_LEN] {
    *uuid::Uuid::new_v4().as_bytes()
}

impl<V: ProvisionVerifier> Provisioner<V> {
    /// Open the node's state in `dir`. A missing file is the normal
    /// first-boot case. A **corrupt** file fails closed to unprovisioned (the
    /// restrictive state) and is reported in the returned warning rather than
    /// being silently ignored.
    pub fn open(dir: &Path, verifier: V) -> io::Result<(Self, Option<String>)> {
        fs::create_dir_all(dir)?;
        let (state, config, warning) = match fs::read_to_string(dir.join(STATE_FILE)) {
            Ok(text) => match Self::decode(&text) {
                Ok((p, c)) => (Provisioning::Provisioned(p), Some(c), None),
                Err(e) => (
                    Provisioning::Unprovisioned,
                    None,
                    Some(format!(
                        "{STATE_FILE} unusable ({e}); starting unprovisioned"
                    )),
                ),
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                (Provisioning::Unprovisioned, None, None)
            }
            Err(e) => return Err(e),
        };
        let challenge = matches!(state, Provisioning::Unprovisioned).then(fresh_challenge);
        Ok((
            Self {
                dir: dir.to_owned(),
                verifier,
                inner: Mutex::new(Inner {
                    state,
                    challenge,
                    config,
                }),
            },
            warning,
        ))
    }

    fn decode(text: &str) -> Result<(Provisioned, RuntimeConfig), String> {
        let p: Persisted = toml::from_str(text).map_err(|e| e.to_string())?;
        if p.version != 1 {
            return Err(format!("unknown version {}", p.version));
        }
        let cfg = RuntimeConfig::parse(&p.config_toml).map_err(|e| e.to_string())?;
        cfg.validate().map_err(|e| e.to_string())?;
        Ok((
            Provisioned {
                role: p.role,
                issuer: p.issuer,
                provisioned_at_ms: p.provisioned_at_ms,
            },
            cfg,
        ))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    #[must_use]
    pub fn state(&self) -> Provisioning {
        self.lock().state.clone()
    }

    /// The challenge to render in the terminal QR; `None` once provisioned.
    #[must_use]
    pub fn challenge(&self) -> Option<[u8; CHALLENGE_LEN]> {
        self.lock().challenge
    }

    /// The config the admin supplied, once provisioned.
    #[must_use]
    pub fn config(&self) -> Option<RuntimeConfig> {
        self.lock().config.clone()
    }

    /// The admin's config **only if the payload actually carried one**. An
    /// empty `config_toml` parses to the defaults; treating that as "the admin
    /// chose defaults" would silently wipe the operator's local settings.
    #[must_use]
    pub fn applied_config(&self) -> Option<RuntimeConfig> {
        let g = self.lock();
        let cfg = g.config.as_ref()?;
        (*cfg != RuntimeConfig::default()).then(|| cfg.clone())
    }

    /// §73 / §75: what this node may run **right now**.
    #[must_use]
    pub fn effective_features(&self) -> FeatureSet {
        match &self.lock().state {
            Provisioning::Unprovisioned => UNPROVISIONED_FEATURES,
            Provisioning::Provisioned(p) => p.role.features(),
        }
    }

    #[must_use]
    pub fn allows(&self, f: Feature) -> bool {
        self.effective_features().contains(f)
    }

    /// §72: accept a signed role/config. All checks happen before anything
    /// is written; the on-disk state is replaced atomically.
    pub fn apply(
        &self,
        payload: &ProvisionPayload,
        signature: &[u8],
        now_ms: u64,
    ) -> Result<Provisioned, ProvisionError> {
        let mut g = self.lock();
        let Some(expected) = g.challenge else {
            return Err(ProvisionError::AlreadyProvisioned);
        };
        if payload.config_toml.len() > MAX_PROVISION_CONFIG {
            return Err(ProvisionError::Malformed("config too large"));
        }
        if payload.issuer.is_empty() || payload.issuer.len() > MAX_ISSUER {
            return Err(ProvisionError::Malformed("issuer length"));
        }
        if payload.valid_for_ms == 0 || payload.valid_for_ms > MAX_PROVISION_VALIDITY_MS {
            return Err(ProvisionError::Malformed("validity window"));
        }
        // Constant-shape compare is not needed (the challenge is public in
        // the QR), but the *order* matters: a wrong-node payload is
        // reported before we spend a signature check on it.
        if payload.challenge != expected {
            return Err(ProvisionError::WrongChallenge);
        }
        let expires = payload.issued_at_ms.saturating_add(payload.valid_for_ms);
        if now_ms < payload.issued_at_ms || now_ms >= expires {
            return Err(ProvisionError::Expired);
        }
        if !self
            .verifier
            .verify(&payload.issuer, &signing_message(payload), signature)
        {
            return Err(ProvisionError::BadSignature);
        }
        let cfg = RuntimeConfig::parse(&payload.config_toml)
            .map_err(|e| ProvisionError::BadConfig(e.to_string()))?;
        cfg.validate()
            .map_err(|e| ProvisionError::BadConfig(e.to_string()))?;
        // Who the admins are is decided by the local config file alone. A
        // payload that could rewrite it could hand the node to someone else.
        if cfg.node != crate::config::NodeSection::default() {
            return Err(ProvisionError::BadConfig(
                "a payload may not set the [node] section".into(),
            ));
        }
        // Same for remote-admin operators: a payload must not appoint its own.
        if cfg.admin != crate::config::AdminSection::default() {
            return Err(ProvisionError::BadConfig(
                "a payload may not set the [admin] section".into(),
            ));
        }

        let done = Provisioned {
            role: payload.role,
            issuer: payload.issuer.clone(),
            provisioned_at_ms: now_ms,
        };
        let text = toml::to_string(&Persisted {
            version: 1,
            role: done.role,
            issuer: done.issuer.clone(),
            provisioned_at_ms: done.provisioned_at_ms,
            config_toml: payload.config_toml.clone(),
        })
        .map_err(|e| ProvisionError::Io(io::Error::other(e)))?;
        write_private_atomic(&self.dir.join(STATE_FILE), text.as_bytes())?;

        g.state = Provisioning::Provisioned(done.clone());
        g.challenge = None;
        g.config = Some(cfg);
        Ok(done)
    }

    /// Return to unprovisioned (factory reset). Local operator action only —
    /// there is deliberately no IPC/remote path to this. Issues a fresh
    /// challenge.
    pub fn reset(&self) -> io::Result<()> {
        let mut g = self.lock();
        match fs::remove_file(self.dir.join(STATE_FILE)) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        g.state = Provisioning::Unprovisioned;
        g.challenge = Some(fresh_challenge());
        g.config = None;
        Ok(())
    }
}

fn write_private_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension("tmp");
    {
        let mut f = fs::File::create(&tmp)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            f.set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    fs::rename(tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test-only scheme: FNV-1a over the issuer and the *whole* message, so
    /// any changed byte changes the "signature". A real verifier checks the
    /// admin's key.
    struct Fake;
    fn fake_sig(issuer: &str, msg: &[u8]) -> Vec<u8> {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in issuer.bytes().chain(msg.iter().copied()) {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
        h.to_le_bytes().to_vec()
    }
    impl ProvisionVerifier for Fake {
        fn verify(&self, issuer: &str, message: &[u8], signature: &[u8]) -> bool {
            issuer == "admin-1" && signature == fake_sig(issuer, message)
        }
    }

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("siar-node-{tag}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn payload(ch: [u8; 16], role: NodeRole) -> ProvisionPayload {
        ProvisionPayload {
            role,
            config_toml: String::new(),
            issuer: "admin-1".into(),
            issued_at_ms: 1_000,
            valid_for_ms: 60_000,
            challenge: ch,
        }
    }

    fn sign(p: &ProvisionPayload) -> Vec<u8> {
        fake_sig(&p.issuer, &signing_message(p))
    }

    #[test]
    fn dtn_node_matches_the_spec_example() {
        // §75: Iroh, BLE, LAN, DTN, files/chunks; no calls, no Dioxus/GUI.
        let f = NodeRole::DtnNode.features();
        for yes in [
            Feature::Iroh,
            Feature::Ble,
            Feature::Lan,
            Feature::Dtn,
            Feature::Files,
        ] {
            assert!(f.contains(yes), "{yes:?}");
        }
        for no in [Feature::Calls, Feature::Gui, Feature::Messaging] {
            assert!(!f.contains(no), "{no:?}");
        }
    }

    #[test]
    fn only_personal_devices_get_a_gui_or_calls_by_default() {
        for r in NodeRole::ALL {
            let f = r.features();
            if !matches!(r, NodeRole::PersonalDevice | NodeRole::TestNode) {
                assert!(!f.contains(Feature::Calls), "{r} runs calls");
            }
            if r != NodeRole::PersonalDevice {
                assert!(!f.contains(Feature::Gui), "{r} has a GUI");
            }
            assert!(
                !f.contains(Feature::BootstrapListener),
                "{r} keeps the bootstrap listener"
            );
        }
    }

    #[test]
    fn role_names_round_trip() {
        for r in NodeRole::ALL {
            assert_eq!(NodeRole::parse(r.name()), Some(r));
        }
        assert_eq!(NodeRole::parse("root"), None);
    }

    #[test]
    fn unprovisioned_node_runs_only_the_bootstrap_listener() {
        // §73
        let d = tmp("unprov");
        let (p, warn) = Provisioner::open(&d, Fake).unwrap();
        assert!(warn.is_none());
        assert_eq!(p.state(), Provisioning::Unprovisioned);
        assert_eq!(p.effective_features(), UNPROVISIONED_FEATURES);
        assert!(p.allows(Feature::BootstrapListener));
        for f in Feature::ALL {
            if f != Feature::BootstrapListener {
                assert!(!p.allows(f), "{f:?} allowed while unprovisioned");
            }
        }
        assert!(p.challenge().is_some());
    }

    #[test]
    fn valid_payload_activates_the_role_and_persists() {
        let d = tmp("ok");
        let (p, _) = Provisioner::open(&d, Fake).unwrap();
        let pl = payload(p.challenge().unwrap(), NodeRole::DtnNode);
        let done = p.apply(&pl, &sign(&pl), 2_000).unwrap();
        assert_eq!(done.role, NodeRole::DtnNode);
        assert!(p.allows(Feature::Dtn));
        assert!(!p.allows(Feature::BootstrapListener));
        assert!(!p.allows(Feature::Calls));
        assert!(p.challenge().is_none(), "challenge is single-use");

        // A restart comes back provisioned, with the same role and config.
        let (q, warn) = Provisioner::open(&d, Fake).unwrap();
        assert!(warn.is_none());
        assert_eq!(q.state(), Provisioning::Provisioned(done));
        assert!(q.challenge().is_none());
        assert!(q.config().is_some());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(d.join(STATE_FILE))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn replay_to_a_different_node_is_refused() {
        let (a, _) = Provisioner::open(&tmp("a"), Fake).unwrap();
        let (b, _) = Provisioner::open(&tmp("b"), Fake).unwrap();
        let pl = payload(a.challenge().unwrap(), NodeRole::RelayGateway);
        let sig = sign(&pl);
        assert!(matches!(
            b.apply(&pl, &sig, 2_000),
            Err(ProvisionError::WrongChallenge)
        ));
        assert_eq!(b.state(), Provisioning::Unprovisioned);
        // …and it still works on the node it was made for.
        a.apply(&pl, &sig, 2_000).unwrap();
    }

    #[test]
    fn second_payload_is_refused_until_a_local_reset() {
        let d = tmp("twice");
        let (p, _) = Provisioner::open(&d, Fake).unwrap();
        let ch = p.challenge().unwrap();
        let pl = payload(ch, NodeRole::DtnNode);
        p.apply(&pl, &sign(&pl), 2_000).unwrap();
        // Exact replay of the same signed payload.
        assert!(matches!(
            p.apply(&pl, &sign(&pl), 2_001),
            Err(ProvisionError::AlreadyProvisioned)
        ));
        p.reset().unwrap();
        assert_eq!(p.state(), Provisioning::Unprovisioned);
        assert!(!d.join(STATE_FILE).exists());
        let ch2 = p.challenge().unwrap();
        assert_ne!(ch, ch2, "reset issues a fresh challenge");
        // The old payload is dead: it names the old challenge.
        assert!(matches!(
            p.apply(&pl, &sign(&pl), 2_002),
            Err(ProvisionError::WrongChallenge)
        ));
    }

    #[test]
    fn bad_signature_expiry_and_clock_skew_are_refused() {
        let (p, _) = Provisioner::open(&tmp("bad"), Fake).unwrap();
        let ch = p.challenge().unwrap();
        let pl = payload(ch, NodeRole::DtnNode);
        assert!(matches!(
            p.apply(&pl, b"forged", 2_000),
            Err(ProvisionError::BadSignature)
        ));
        // Tampering after signing (role swapped for a more powerful one).
        let mut tampered = pl.clone();
        tampered.role = NodeRole::TestNode;
        assert!(matches!(
            p.apply(&tampered, &sign(&pl), 2_000),
            Err(ProvisionError::BadSignature)
        ));
        // Expired / from the future.
        assert!(matches!(
            p.apply(&pl, &sign(&pl), 61_000),
            Err(ProvisionError::Expired)
        ));
        assert!(matches!(
            p.apply(&pl, &sign(&pl), 999),
            Err(ProvisionError::Expired)
        ));
        // Unknown issuer.
        let mut other = pl.clone();
        other.issuer = "mallory".into();
        assert!(matches!(
            p.apply(&other, &sign(&other), 2_000),
            Err(ProvisionError::BadSignature)
        ));
        // None of the failures consumed the challenge or changed state.
        assert_eq!(p.challenge(), Some(ch));
        assert_eq!(p.state(), Provisioning::Unprovisioned);
        p.apply(&pl, &sign(&pl), 2_000).unwrap();
    }

    #[test]
    fn oversized_or_implausible_payloads_are_malformed() {
        let (p, _) = Provisioner::open(&tmp("mal"), Fake).unwrap();
        let ch = p.challenge().unwrap();
        let mut big = payload(ch, NodeRole::DtnNode);
        big.config_toml = "#".repeat(MAX_PROVISION_CONFIG + 1);
        assert!(matches!(
            p.apply(&big, &sign(&big), 2_000),
            Err(ProvisionError::Malformed(_))
        ));
        let mut forever = payload(ch, NodeRole::DtnNode);
        forever.valid_for_ms = u64::MAX;
        assert!(matches!(
            p.apply(&forever, &sign(&forever), 2_000),
            Err(ProvisionError::Malformed(_))
        ));
        let mut anon = payload(ch, NodeRole::DtnNode);
        anon.issuer.clear();
        assert!(matches!(
            p.apply(&anon, &sign(&anon), 2_000),
            Err(ProvisionError::Malformed(_))
        ));
    }

    #[test]
    fn a_bad_embedded_config_leaves_nothing_half_provisioned() {
        let d = tmp("cfg");
        let (p, _) = Provisioner::open(&d, Fake).unwrap();
        let mut pl = payload(p.challenge().unwrap(), NodeRole::DtnNode);
        pl.config_toml = "no_such_section = 1".into();
        assert!(matches!(
            p.apply(&pl, &sign(&pl), 2_000),
            Err(ProvisionError::BadConfig(_))
        ));
        assert_eq!(p.state(), Provisioning::Unprovisioned);
        assert!(!d.join(STATE_FILE).exists());
        assert!(
            p.challenge().is_some(),
            "a rejected payload is not consumed"
        );
    }

    #[test]
    fn corrupt_state_fails_closed_and_says_so() {
        let d = tmp("corrupt");
        fs::write(d.join(STATE_FILE), "this is not toml = = =").unwrap();
        let (p, warn) = Provisioner::open(&d, Fake).unwrap();
        assert_eq!(p.state(), Provisioning::Unprovisioned);
        assert!(warn.unwrap().contains("unprovisioned"));
        assert_eq!(p.effective_features(), UNPROVISIONED_FEATURES);
    }

    #[test]
    fn signing_message_is_domain_separated_and_covers_every_field() {
        let base = payload([7; 16], NodeRole::DtnNode);
        let m = signing_message(&base);
        assert!(m.starts_with(SIGN_DOMAIN));
        let mut v = vec![];
        let mut a = base.clone();
        a.role = NodeRole::StorageNode;
        v.push(a);
        let mut a = base.clone();
        a.config_toml = "x".into();
        v.push(a);
        let mut a = base.clone();
        a.issuer = "z".into();
        v.push(a);
        let mut a = base.clone();
        a.issued_at_ms += 1;
        v.push(a);
        let mut a = base.clone();
        a.valid_for_ms += 1;
        v.push(a);
        let mut a = base.clone();
        a.challenge[0] ^= 1;
        v.push(a);
        for changed in v {
            assert_ne!(signing_message(&changed), m);
        }
    }

    #[test]
    fn feature_set_basics() {
        let s = FeatureSet::of(&[Feature::Iroh, Feature::Dtn]);
        assert!(s.contains(Feature::Iroh) && !s.contains(Feature::Ble));
        assert_eq!(s.without(Feature::Iroh), FeatureSet::of(&[Feature::Dtn]));
        assert_eq!(s.iter().count(), 2);
        assert!(FeatureSet::EMPTY.is_empty());
        assert_eq!(format!("{:?}", FeatureSet::of(&[Feature::Dtn])), "{Dtn}");
    }

    // ---- real admin verifier + IPC-facing service ---------------------------

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    fn admin(seed: u8) -> (ed25519_dalek::SigningKey, String) {
        let sk = ed25519_dalek::SigningKey::from_bytes(&[seed; 32]);
        let id = hex(sk.verifying_key().as_bytes());
        (sk, id)
    }

    fn signed(
        sk: &ed25519_dalek::SigningKey,
        issuer: &str,
        ch: [u8; 16],
        role: NodeRole,
    ) -> (ProvisionPayload, Vec<u8>) {
        use ed25519_dalek::Signer;
        let p = ProvisionPayload {
            issuer: issuer.to_owned(),
            ..payload(ch, role)
        };
        let sig = sk.sign(&signing_message(&p)).to_bytes().to_vec();
        (p, sig)
    }

    #[test]
    fn admin_verifier_needs_keys_and_rejects_malformed_ones() {
        assert!(Ed25519AdminVerifier::from_hex_keys(&[]).is_err());
        assert!(Ed25519AdminVerifier::from_hex_keys(&["zz".into()]).is_err());
        assert!(
            Ed25519AdminVerifier::from_hex_keys(&["AB".repeat(32)]).is_err(),
            "uppercase"
        );
        let (_, id) = admin(1);
        assert!(Ed25519AdminVerifier::from_hex_keys(&[id]).is_ok());
    }

    #[test]
    fn only_an_allow_listed_admin_with_a_valid_signature_can_provision() {
        let (sk, id) = admin(1);
        let (other_sk, other_id) = admin(2);
        let v = Ed25519AdminVerifier::from_hex_keys(std::slice::from_ref(&id)).unwrap();
        let (p, _) = Provisioner::open(&tmp("adm"), v).unwrap();
        let ch = p.challenge().unwrap();

        // Valid signature, but from a key that is not on the list.
        let (pl, sig) = signed(&other_sk, &other_id, ch, NodeRole::DtnNode);
        assert!(matches!(
            p.apply(&pl, &sig, 2_000),
            Err(ProvisionError::BadSignature)
        ));
        // Listed admin's issuer id, signature made by somebody else.
        let (pl, _) = signed(&sk, &id, ch, NodeRole::DtnNode);
        let (_, forged) = signed(&other_sk, &id, ch, NodeRole::DtnNode);
        assert!(matches!(
            p.apply(&pl, &forged, 2_000),
            Err(ProvisionError::BadSignature)
        ));
        // Not hex at all / wrong signature length.
        let mut junk = pl.clone();
        junk.issuer = "admin-1".into();
        assert!(matches!(
            p.apply(&junk, &[0; 64], 2_000),
            Err(ProvisionError::BadSignature)
        ));
        assert!(matches!(
            p.apply(&pl, &[0; 63], 2_000),
            Err(ProvisionError::BadSignature)
        ));
        assert_eq!(p.state(), Provisioning::Unprovisioned);

        // The genuine one works.
        let (pl, sig) = signed(&sk, &id, ch, NodeRole::DtnNode);
        assert_eq!(p.apply(&pl, &sig, 2_000).unwrap().issuer, id);
    }

    #[test]
    fn a_payload_cannot_rewrite_who_the_admins_are() {
        let (sk, id) = admin(1);
        let v = Ed25519AdminVerifier::from_hex_keys(std::slice::from_ref(&id)).unwrap();
        let d = tmp("nodesec");
        let (p, _) = Provisioner::open(&d, v).unwrap();
        let (mut pl, _) = signed(&sk, &id, p.challenge().unwrap(), NodeRole::DtnNode);
        pl.config_toml = format!("[node]\nadmin_keys = [\"{}\"]\n", hex(&[7; 32]));
        use ed25519_dalek::Signer;
        let sig = sk.sign(&signing_message(&pl)).to_bytes();
        let e = p.apply(&pl, &sig, 2_000).unwrap_err();
        assert!(
            matches!(e, ProvisionError::BadConfig(ref m) if m.contains("[node]")),
            "{e:?}"
        );
        assert!(!d.join(STATE_FILE).exists());
    }

    #[test]
    fn the_encoded_path_decodes_only_canonical_payloads() {
        let (sk, id) = admin(1);
        let v = Ed25519AdminVerifier::from_hex_keys(std::slice::from_ref(&id)).unwrap();
        let (p, _) = Provisioner::open(&tmp("enc"), v).unwrap();
        let svc: &dyn ProvisioningService = &p;
        let status = svc.status();
        assert_eq!(status.role, None);
        let ch_hex = status.challenge_hex.clone().unwrap();
        assert_eq!(ch_hex.len(), 32);
        assert!(status.render().starts_with("unprovisioned challenge="));

        let (pl, sig) = signed(&sk, &id, p.challenge().unwrap(), NodeRole::StorageNode);
        let bytes = postcard::to_allocvec(&pl).unwrap();
        // Trailing data is refused even though the signed part is intact.
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert!(matches!(
            svc.apply_encoded(&trailing, &sig, 2_000),
            Err(ProvisionError::Malformed(_))
        ));
        assert!(matches!(
            svc.apply_encoded(&[1, 2, 3], &sig, 2_000),
            Err(ProvisionError::Malformed(_))
        ));
        assert!(matches!(
            svc.apply_encoded(&vec![0; MAX_PROVISION_CONFIG + 2048], &sig, 2_000),
            Err(ProvisionError::Malformed(_))
        ));
        let done = svc.apply_encoded(&bytes, &sig, 2_000).unwrap();
        assert_eq!(done.role, NodeRole::StorageNode);
        assert_eq!(svc.status().render(), "provisioned role=storage-node\n");
        assert_eq!(svc.status().challenge_hex, None);
    }

    #[test]
    fn applied_config_is_only_what_the_admin_actually_sent_and_never_touches_node_or_secrets() {
        let (sk, id) = admin(1);
        let v = Ed25519AdminVerifier::from_hex_keys(std::slice::from_ref(&id)).unwrap();

        // Empty payload config → nothing to apply (must not wipe local settings).
        let (p, _) = Provisioner::open(&tmp("cfg-empty"), v).unwrap();
        let (pl, sig) = signed(&sk, &id, p.challenge().unwrap(), NodeRole::DtnNode);
        p.apply(&pl, &sig, 2_000).unwrap();
        assert!(p.applied_config().is_none());

        // A real payload config applies, laid over the local one.
        let v = Ed25519AdminVerifier::from_hex_keys(std::slice::from_ref(&id)).unwrap();
        let (p, _) = Provisioner::open(&tmp("cfg-real"), v).unwrap();
        let (mut pl, _) = signed(&sk, &id, p.challenge().unwrap(), NodeRole::DtnNode);
        pl.config_toml = "[logging]\nlevel = \"debug\"\n".into();
        use ed25519_dalek::Signer;
        let sig = sk.sign(&signing_message(&pl)).to_bytes();
        p.apply(&pl, &sig, 2_000).unwrap();
        let applied = p.applied_config().expect("payload carried a config");
        assert_eq!(applied.logging.level, "debug");

        let mut local = RuntimeConfig::default();
        local.node.require_provisioning = true;
        local.node.admin_keys = vec![id.clone()];
        local.secrets.backup_passphrase = Some(crate::config::SecretRef::Env { name: "X".into() });
        let eff = overlay_config(&local, Some(applied));
        assert_eq!(eff.logging.level, "debug", "the admin's setting applies");
        assert_eq!(eff.node, local.node, "who the admins are stays local");
        assert_eq!(eff.admin, local.admin, "remote operators stay local");
        assert_eq!(eff.secrets, local.secrets, "key-material paths stay local");
        assert_eq!(overlay_config(&local, None), local);
    }
}

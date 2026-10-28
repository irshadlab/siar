//! Contact verification: persisted marks plus safety numbers.
//!
//! A mark records *which key* the user verified, not just that they
//! verified: `device-uuid=<ed25519 public key, hex>`. If the contact's key
//! later differs, the contact is `KeyChanged` — loudly different from
//! "never verified" — and a security event is raised. A bare "verified"
//! boolean would silently keep vouching for a key the user never compared.
//!
//! Safety numbers come from `siar-identity-multidevice`'s
//! `SafetyFingerprint` (symmetric: both sides compute the same number).
//! Today that is derived from the two *device* signing keys, because the
//! desktop app has no account-root key yet; when root keys are wired in,
//! only `safety_number`'s inputs change.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use siar_domain::DeviceId;
use siar_identity_multidevice::root_key::RootPublicKey;
use siar_identity_multidevice::safety_fingerprint::SafetyFingerprint;
use siar_ui_model::VerificationState;

pub fn safety_number(local_key: &[u8; 32], peer_key: &[u8; 32]) -> String {
    SafetyFingerprint::derive(&RootPublicKey(*local_key), &RootPublicKey(*peer_key))
        .display_string()
}

fn hex(bytes: &[u8; 32]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Groups of 4 hex chars: `short` is the first 4 groups, `full` all 16.
pub fn key_fingerprint(key: &[u8; 32]) -> (String, String) {
    let digest = blake3::hash(&[b"siar/identity-fingerprint/v1".as_slice(), key].concat());
    let h = hex(digest.as_bytes());
    let grouped: Vec<&str> = (0..h.len()).step_by(4).map(|i| &h[i..i + 4]).collect();
    (grouped[..4].join(" "), grouped.join(" "))
}

#[derive(Debug, Default)]
pub struct VerificationStore {
    path: Option<PathBuf>,
    marks: HashMap<DeviceId, String>,
}

impl VerificationStore {
    /// A missing or damaged file means "nothing verified yet" — failing
    /// safe: the user is asked to verify again, never shown a false tick.
    pub fn load(path: &Path) -> Self {
        let mut marks = HashMap::new();
        if let Ok(text) = std::fs::read_to_string(path) {
            for line in text.lines() {
                let Some((id, key)) = line.split_once('=') else {
                    continue;
                };
                if let (Ok(id), true) = (id.trim().parse::<DeviceId>(), key.trim().len() == 64) {
                    marks.insert(id, key.trim().to_ascii_lowercase());
                }
            }
        }
        Self {
            path: Some(path.to_path_buf()),
            marks,
        }
    }

    #[cfg(test)]
    pub fn in_memory() -> Self {
        Self::default()
    }

    pub fn state(&self, device: DeviceId, current_key: &[u8; 32]) -> VerificationState {
        match self.marks.get(&device) {
            None => VerificationState::Unverified,
            Some(k) if *k == hex(current_key) => VerificationState::Verified,
            Some(_) => VerificationState::KeyChanged,
        }
    }

    pub fn mark(&mut self, device: DeviceId, key: &[u8; 32]) -> std::io::Result<()> {
        self.marks.insert(device, hex(key));
        self.save()
    }

    pub fn unmark(&mut self, device: DeviceId) -> std::io::Result<()> {
        self.marks.remove(&device);
        self.save()
    }

    fn save(&self) -> std::io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let mut lines: Vec<String> = self.marks.iter().map(|(d, k)| format!("{d}={k}")).collect();
        lines.sort();
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, lines.join("\n") + "\n")?;
        std::fs::rename(&tmp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safety_number_is_symmetric_and_key_dependent() {
        let (a, b, c) = ([1u8; 32], [2u8; 32], [3u8; 32]);
        assert_eq!(safety_number(&a, &b), safety_number(&b, &a));
        assert_ne!(safety_number(&a, &b), safety_number(&a, &c));
        assert_eq!(safety_number(&a, &b).split(' ').count(), 12);
    }

    #[test]
    fn fingerprint_short_is_a_prefix_of_full() {
        let (short, full) = key_fingerprint(&[7; 32]);
        assert!(full.starts_with(&short));
        assert_eq!(short.split(' ').count(), 4);
        assert_eq!(full.split(' ').count(), 16);
        assert_ne!(key_fingerprint(&[7; 32]), key_fingerprint(&[8; 32]));
    }

    #[test]
    fn a_changed_key_is_key_changed_not_unverified() {
        let mut s = VerificationStore::in_memory();
        let d = DeviceId::new();
        assert_eq!(s.state(d, &[1; 32]), VerificationState::Unverified);
        s.mark(d, &[1; 32]).unwrap();
        assert_eq!(s.state(d, &[1; 32]), VerificationState::Verified);
        assert_eq!(s.state(d, &[2; 32]), VerificationState::KeyChanged);
        s.unmark(d).unwrap();
        assert_eq!(s.state(d, &[2; 32]), VerificationState::Unverified);
    }

    #[test]
    fn marks_survive_a_restart_and_garbage_is_ignored() {
        let dir = std::env::temp_dir().join(format!("siar-verif-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("verified.txt");
        let (d, other) = (DeviceId::new(), DeviceId::new());
        let mut s = VerificationStore::load(&path);
        s.mark(d, &[9; 32]).unwrap();
        s.mark(other, &[5; 32]).unwrap();

        let mut text = std::fs::read_to_string(&path).unwrap();
        text.push_str("not a line\nbad-uuid=00\n");
        std::fs::write(&path, text).unwrap();

        let reloaded = VerificationStore::load(&path);
        assert_eq!(reloaded.state(d, &[9; 32]), VerificationState::Verified);
        assert_eq!(
            reloaded.state(other, &[6; 32]),
            VerificationState::KeyChanged
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

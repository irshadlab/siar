//! File-based **root-key ceremony** on top of [`crate::frost_root`]: what a
//! `siar-root` command-line tool needs so that each participant can run their
//! part on their own machine and exchange only small hex files.
//!
//! * Public/broadcast values are written as hex text (copy/paste, QR, USB).
//! * **Secret state** (DKG round secrets, the long-term share, signing
//!   nonces) is sealed on disk with a passphrase: Argon2id → 32-byte key →
//!   XChaCha20-Poly1305, the file's `kind` bound in as associated data so a
//!   sealed share cannot be fed in where a nonce file is expected. These are
//!   the same two upstream crates `siar-backup` already uses; nothing here
//!   is a new primitive.
//! * **Nonces are single-use by construction:** [`sign_round2`] deletes the
//!   nonce file *before* computing the signature share, so a crash or a
//!   second invocation can never sign twice with one nonce (which would leak
//!   the share).
//! * The DKG **round-2 files are secret** (each carries a share-in-
//!   transit for exactly one recipient). Send each only to its recipient over
//!   a channel you trust to be confidential and authentic; this tool cannot
//!   do that for you.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::Path;

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};

use crate::frost_root::{self as fr, Identifier, Threshold};
use crate::{fill_random, Error};

const MAGIC: &[u8; 9] = b"SIARSEAL1";
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 24;
/// Longest hex file we will read (a signing package for a 64-signer group
/// plus a message stays far below this).
const MAX_HEX_FILE: usize = 1 << 20;
pub const MIN_PASSPHRASE_CHARS: usize = 12;

/// Argon2id cost. [`KdfCost::DEFAULT`] is for real use; tests use
/// [`KdfCost::FAST`].
#[derive(Debug, Clone, Copy)]
pub struct KdfCost {
    pub mem_kib: u32,
    pub iterations: u32,
}

impl KdfCost {
    pub const DEFAULT: Self = Self {
        mem_kib: 64 * 1024,
        iterations: 3,
    };
    pub const FAST: Self = Self {
        mem_kib: 8,
        iterations: 1,
    };
}

/// A passphrase that has passed the minimum-length check.
pub struct Passphrase {
    text: String,
    cost: KdfCost,
}

impl Passphrase {
    pub fn new(text: &str, cost: KdfCost) -> Result<Self, Error> {
        if text.chars().count() < MIN_PASSPHRASE_CHARS {
            return Err(Error::InvalidParameter("passphrase is too short"));
        }
        Ok(Self {
            text: text.to_owned(),
            cost,
        })
    }
}

impl Drop for Passphrase {
    fn drop(&mut self) {
        zeroize::Zeroize::zeroize(&mut self.text);
    }
}

fn derive(pw: &Passphrase, salt: &[u8], mem_kib: u32, iters: u32) -> Result<[u8; 32], Error> {
    let params = Params::new(mem_kib, iters, 1, Some(32))
        .map_err(|_| Error::InvalidParameter("kdf parameters"))?;
    let mut key = [0u8; 32];
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(pw.text.as_bytes(), salt, &mut key)
        .map_err(|e| Error::Upstream(e.to_string()))?;
    Ok(key)
}

/// `MAGIC ‖ mem_kib(4) ‖ iters(4) ‖ salt ‖ nonce ‖ ciphertext`.
fn seal(pw: &Passphrase, kind: &str, plaintext: &[u8]) -> Result<Vec<u8>, Error> {
    let mut salt = [0u8; SALT_LEN];
    let mut nonce = [0u8; NONCE_LEN];
    fill_random(&mut salt)?;
    fill_random(&mut nonce)?;
    let mut key = derive(pw, &salt, pw.cost.mem_kib, pw.cost.iterations)?;
    let mut header = Vec::with_capacity(MAGIC.len() + 8 + SALT_LEN + NONCE_LEN);
    header.extend_from_slice(MAGIC);
    header.extend_from_slice(&pw.cost.mem_kib.to_be_bytes());
    header.extend_from_slice(&pw.cost.iterations.to_be_bytes());
    header.extend_from_slice(&salt);
    header.extend_from_slice(&nonce);
    // Everything in the header AND the kind are authenticated.
    let aad = [header.as_slice(), kind.as_bytes()].concat();
    let ct = XChaCha20Poly1305::new(Key::from_slice(&key))
        .encrypt(
            XNonce::from_slice(&nonce),
            Payload {
                msg: plaintext,
                aad: &aad,
            },
        )
        .map_err(|_| Error::Upstream("seal failed".into()));
    zeroize::Zeroize::zeroize(&mut key);
    header.extend_from_slice(&ct?);
    Ok(header)
}

fn unseal(pw: &Passphrase, kind: &str, blob: &[u8]) -> Result<Vec<u8>, Error> {
    let fixed = MAGIC.len() + 8 + SALT_LEN + NONCE_LEN;
    if blob.len() < fixed + 16 || &blob[..MAGIC.len()] != MAGIC {
        return Err(Error::Malformed("not a sealed file"));
    }
    let mem = u32::from_be_bytes(
        blob[9..13]
            .try_into()
            .map_err(|_| Error::Malformed("kdf"))?,
    );
    let iters = u32::from_be_bytes(
        blob[13..17]
            .try_into()
            .map_err(|_| Error::Malformed("kdf"))?,
    );
    // A file we did not write must not be able to make us allocate gigabytes.
    if !(8..=1 << 20).contains(&mem) || !(1..=16).contains(&iters) {
        return Err(Error::Malformed("kdf parameters out of range"));
    }
    let salt = &blob[17..17 + SALT_LEN];
    let nonce = &blob[17 + SALT_LEN..fixed];
    let mut key = derive(pw, salt, mem, iters)?;
    let aad = [&blob[..fixed], kind.as_bytes()].concat();
    let pt = XChaCha20Poly1305::new(Key::from_slice(&key))
        .decrypt(
            XNonce::from_slice(nonce),
            Payload {
                msg: &blob[fixed..],
                aad: &aad,
            },
        )
        .map_err(|_| Error::VerificationFailed);
    zeroize::Zeroize::zeroize(&mut key);
    pt
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    let io = |e: std::io::Error| Error::Upstream(e.to_string());
    let tmp = path.with_extension("tmp");
    {
        let mut f = fs::File::create(&tmp).map_err(io)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            f.set_permissions(fs::Permissions::from_mode(0o600))
                .map_err(io)?;
        }
        f.write_all(bytes).map_err(io)?;
        f.sync_all().map_err(io)?;
    }
    fs::rename(tmp, path).map_err(io)
}

fn read_capped(path: &Path) -> Result<Vec<u8>, Error> {
    let meta = fs::metadata(path).map_err(|e| Error::Upstream(e.to_string()))?;
    if meta.len() as usize > MAX_HEX_FILE {
        return Err(Error::Malformed("file too large"));
    }
    fs::read(path).map_err(|e| Error::Upstream(e.to_string()))
}

pub fn to_hex(b: &[u8]) -> String {
    use std::fmt::Write as _;
    b.iter()
        .fold(String::with_capacity(b.len() * 2), |mut s, x| {
            let _ = write!(s, "{x:02x}");
            s
        })
}

pub fn from_hex(s: &str) -> Result<Vec<u8>, Error> {
    let s = s.trim();
    if !s.len().is_multiple_of(2) || s.len() > 2 * MAX_HEX_FILE {
        return Err(Error::Malformed("hex length"));
    }
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).map_err(|_| Error::Malformed("hex")))
        .collect()
}

/// `id(2, BE) ‖ body`, as hex.
fn tag(id: u16, body: &[u8]) -> String {
    let mut v = id.to_be_bytes().to_vec();
    v.extend_from_slice(body);
    to_hex(&v)
}

fn untag(hex: &str) -> Result<(u16, Vec<u8>), Error> {
    let b = from_hex(hex)?;
    if b.len() < 3 {
        return Err(Error::Malformed("too short"));
    }
    Ok((u16::from_be_bytes([b[0], b[1]]), b[2..].to_vec()))
}

fn up<E: std::fmt::Display>(e: E) -> Error {
    Error::Upstream(e.to_string())
}

fn files(dir: &Path, name: &str) -> std::path::PathBuf {
    dir.join(name)
}

// --------------------------------------------------------------------- DKG

/// DKG step 1. Writes `r1.secret` (sealed) and `r1.public.hex`; returns the
/// public hex to send to every other participant.
pub fn dkg_round1(
    dir: &Path,
    pw: &Passphrase,
    id: u16,
    min: u16,
    max: u16,
) -> Result<String, Error> {
    let t = Threshold::new(min, max)?;
    if id > max {
        return Err(Error::InvalidParameter("participant id exceeds group size"));
    }
    let (secret, public) = fr::dkg_part1(fr::identifier(id)?, t)?;
    let mut body = Vec::new();
    body.extend_from_slice(&id.to_be_bytes());
    body.extend_from_slice(&min.to_be_bytes());
    body.extend_from_slice(&max.to_be_bytes());
    body.extend_from_slice(&secret.serialize().map_err(up)?);
    fs::create_dir_all(dir).map_err(up)?;
    write_private(&files(dir, "r1.secret"), &seal(pw, "dkg-r1", &body)?)?;
    let hex = tag(id, &public.serialize().map_err(up)?);
    fs::write(files(dir, "r1.public.hex"), &hex).map_err(up)?;
    Ok(hex)
}

fn parse_r1(
    hexes: &[String],
    me: u16,
    expect: u16,
) -> Result<BTreeMap<Identifier, fr::round1::Package>, Error> {
    let mut out = BTreeMap::new();
    for h in hexes {
        let (id, body) = untag(h)?;
        if id == me {
            continue; // tolerate our own file being in the pile
        }
        let pkg = fr::round1::Package::deserialize(&body).map_err(up)?;
        if out.insert(fr::identifier(id)?, pkg).is_some() {
            return Err(Error::InvalidParameter("duplicate round-1 package"));
        }
    }
    if out.len() != usize::from(expect) - 1 {
        return Err(Error::InvalidParameter(
            "need exactly one round-1 package from every other participant",
        ));
    }
    Ok(out)
}

fn read_r1_secret(
    dir: &Path,
    pw: &Passphrase,
) -> Result<(u16, u16, u16, fr::round1::SecretPackage), Error> {
    let body = unseal(pw, "dkg-r1", &read_capped(&files(dir, "r1.secret"))?)?;
    if body.len() < 7 {
        return Err(Error::Malformed("round-1 secret"));
    }
    let id = u16::from_be_bytes([body[0], body[1]]);
    let min = u16::from_be_bytes([body[2], body[3]]);
    let max = u16::from_be_bytes([body[4], body[5]]);
    let secret = fr::round1::SecretPackage::deserialize(&body[6..]).map_err(up)?;
    Ok((id, min, max, secret))
}

/// DKG step 2: consume everyone else's round-1 hex. Writes `r2.secret`
/// (sealed) and returns `(recipient id, secret hex)` pairs — **each goes only
/// to its recipient.** The round-1 secret file is removed (it must not be
/// used twice).
pub fn dkg_round2(
    dir: &Path,
    pw: &Passphrase,
    r1_public: &[String],
) -> Result<Vec<(u16, String)>, Error> {
    let (id, _min, max, secret) = read_r1_secret(dir, pw)?;
    let others = parse_r1(r1_public, id, max)?;
    let (r2_secret, out) = fr::dkg_part2(secret, &others)?;
    let mut body = id.to_be_bytes().to_vec();
    body.extend_from_slice(&max.to_be_bytes());
    body.extend_from_slice(&r2_secret.serialize().map_err(up)?);
    write_private(&files(dir, "r2.secret"), &seal(pw, "dkg-r2", &body)?)?;
    let _ = fs::remove_file(files(dir, "r1.secret"));

    let mut result = Vec::new();
    for (to, pkg) in out {
        let to_id = id_to_u16(&to)?;
        let mut b = to_id.to_be_bytes().to_vec(); // intended recipient
        b.extend_from_slice(&pkg.serialize().map_err(up)?);
        result.push((to_id, tag(id, &b)));
    }
    Ok(result)
}

/// Map an [`Identifier`] back to its participant number by searching the
/// (small, bounded) valid range — `Identifier` has no public numeric getter.
fn id_to_u16(id: &Identifier) -> Result<u16, Error> {
    (1..=fr::MAX_PARTICIPANTS)
        .find(|n| fr::identifier(*n).ok().as_ref() == Some(id))
        .ok_or(Error::Malformed("participant id out of range"))
}

/// DKG step 3. `r2_for_me` are the round-2 hex strings *addressed to this
/// participant*. Writes `share.sealed`, `group.public.hex` and `root.pub`;
/// returns the 32-byte root public key. The round-2 secret file is removed.
pub fn dkg_round3(
    dir: &Path,
    pw: &Passphrase,
    r1_public: &[String],
    r2_for_me: &[String],
) -> Result<[u8; 32], Error> {
    let body = unseal(pw, "dkg-r2", &read_capped(&files(dir, "r2.secret"))?)?;
    if body.len() < 5 {
        return Err(Error::Malformed("round-2 secret"));
    }
    let id = u16::from_be_bytes([body[0], body[1]]);
    let max = u16::from_be_bytes([body[2], body[3]]);
    let r2_secret = fr::round2::SecretPackage::deserialize(&body[4..]).map_err(up)?;
    let r1 = parse_r1(r1_public, id, max)?;

    let mut r2 = BTreeMap::new();
    for h in r2_for_me {
        let (from, rest) = untag(h)?;
        if rest.len() < 3 || u16::from_be_bytes([rest[0], rest[1]]) != id {
            return Err(Error::InvalidParameter(
                "a round-2 file is addressed to someone else",
            ));
        }
        let pkg = fr::round2::Package::deserialize(&rest[2..]).map_err(up)?;
        if r2.insert(fr::identifier(from)?, pkg).is_some() {
            return Err(Error::InvalidParameter("duplicate round-2 package"));
        }
    }
    if r2.len() != usize::from(max) - 1 {
        return Err(Error::InvalidParameter(
            "need exactly one round-2 package from every other participant",
        ));
    }
    let (key, public) = fr::dkg_part3(&r2_secret, &r1, &r2)?;
    let root = fr::root_public_key(&public)?;

    let mut share = id.to_be_bytes().to_vec();
    share.extend_from_slice(&key.serialize().map_err(up)?);
    write_private(&files(dir, "share.sealed"), &seal(pw, "share", &share)?)?;
    fs::write(
        files(dir, "group.public.hex"),
        to_hex(&public.serialize().map_err(up)?),
    )
    .map_err(up)?;
    fs::write(files(dir, "root.pub"), to_hex(&root)).map_err(up)?;
    let _ = fs::remove_file(files(dir, "r2.secret"));
    Ok(root)
}

fn read_share(dir: &Path, pw: &Passphrase) -> Result<(u16, fr::KeyPackage), Error> {
    let body = unseal(pw, "share", &read_capped(&files(dir, "share.sealed"))?)?;
    if body.len() < 3 {
        return Err(Error::Malformed("share"));
    }
    let id = u16::from_be_bytes([body[0], body[1]]);
    Ok((id, fr::KeyPackage::deserialize(&body[2..]).map_err(up)?))
}

// ----------------------------------------------------------------- signing

/// Signing step 1 (each signer). Writes `nonces.sealed`; returns the
/// commitment hex to send to the coordinator.
pub fn sign_round1(dir: &Path, pw: &Passphrase) -> Result<String, Error> {
    let (id, key) = read_share(dir, pw)?;
    let (nonces, commitments) = fr::sign_round1(&key);
    let mut body = id.to_be_bytes().to_vec();
    body.extend_from_slice(&nonces.serialize().map_err(up)?);
    write_private(&files(dir, "nonces.sealed"), &seal(pw, "nonces", &body)?)?;
    Ok(tag(id, &commitments.serialize().map_err(up)?))
}

/// Coordinator: bind `message` and the chosen signers' commitments.
pub fn make_signing_package(message: &[u8], commitments: &[String]) -> Result<String, Error> {
    let mut map = BTreeMap::new();
    for h in commitments {
        let (id, body) = untag(h)?;
        let c = fr::SigningCommitments::deserialize(&body).map_err(up)?;
        if map.insert(fr::identifier(id)?, c).is_some() {
            return Err(Error::InvalidParameter("duplicate commitment"));
        }
    }
    if map.len() < 2 {
        return Err(Error::InvalidParameter("need at least two commitments"));
    }
    let pkg = fr::signing_package(map, message);
    Ok(to_hex(&pkg.serialize().map_err(up)?))
}

/// Signing step 2 (each signer). **Deletes `nonces.sealed` first**, then
/// returns this signer's share hex.
pub fn sign_round2(dir: &Path, pw: &Passphrase, package_hex: &str) -> Result<String, Error> {
    let (id, key) = read_share(dir, pw)?;
    let nonce_path = files(dir, "nonces.sealed");
    let blob = read_capped(&nonce_path)?;
    let body = unseal(pw, "nonces", &blob)?;
    if body.len() < 3 || u16::from_be_bytes([body[0], body[1]]) != id {
        return Err(Error::Malformed("nonce file belongs to another share"));
    }
    let nonces = fr::SigningNonces::deserialize(&body[2..]).map_err(up)?;
    // Single use: gone before any signing happens.
    fs::remove_file(&nonce_path).map_err(up)?;

    let pkg_bytes = from_hex(package_hex)?;
    let pkg = fr::SigningPackage::deserialize(&pkg_bytes).map_err(up)?;
    let share = fr::sign_round2(&pkg, nonces, &key)?;
    Ok(tag(id, &share.serialize()))
}

/// Coordinator: combine shares into the final **standard Ed25519 signature**
/// (verified strictly before it is returned).
pub fn aggregate_signature(
    group_public_hex: &str,
    package_hex: &str,
    shares: &[String],
) -> Result<[u8; 64], Error> {
    let public = fr::PublicKeyPackage::deserialize(&from_hex(group_public_hex)?).map_err(up)?;
    let pkg = fr::SigningPackage::deserialize(&from_hex(package_hex)?).map_err(up)?;
    let mut map = BTreeMap::new();
    for h in shares {
        let (id, body) = untag(h)?;
        let s = fr::SignatureShare::deserialize(&body).map_err(up)?;
        if map.insert(fr::identifier(id)?, s).is_some() {
            return Err(Error::InvalidParameter("duplicate signature share"));
        }
    }
    fr::aggregate(&pkg, &map, &public)
}

#[cfg(test)]
mod tests {
    use super::*;
    use siar_identity_multidevice::root_key::RootPublicKey;

    fn pw(s: &str) -> Passphrase {
        Passphrase::new(s, KdfCost::FAST).unwrap()
    }

    fn tmp(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("siar-cer-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn sealing_round_trips_and_binds_kind_and_passphrase() {
        let p = pw("correct horse battery");
        let blob = seal(&p, "share", b"secret").unwrap();
        assert_eq!(unseal(&p, "share", &blob).unwrap(), b"secret");
        assert!(
            unseal(&p, "nonces", &blob).is_err(),
            "kind is authenticated"
        );
        assert!(unseal(&pw("another long passphrase"), "share", &blob).is_err());
        let mut bad = blob.clone();
        let last = bad.len() - 1;
        bad[last] ^= 1;
        assert!(unseal(&p, "share", &bad).is_err(), "tamper detected");
        let mut hdr = blob.clone();
        hdr[10] ^= 1; // header (kdf params) is authenticated too
        assert!(unseal(&p, "share", &hdr).is_err());
        assert!(unseal(&p, "share", b"nope").is_err());
        assert!(Passphrase::new("short", KdfCost::FAST).is_err());
    }

    #[test]
    fn hostile_kdf_parameters_in_a_file_are_refused_not_allocated() {
        let p = pw("correct horse battery");
        let mut blob = seal(&p, "share", b"x").unwrap();
        blob[9..13].copy_from_slice(&u32::MAX.to_be_bytes()); // 4 TiB of memory
        assert!(matches!(
            unseal(&p, "share", &blob),
            Err(Error::Malformed(_))
        ));
    }

    #[test]
    fn hex_helpers_reject_garbage() {
        assert_eq!(from_hex("00ff").unwrap(), [0, 255]);
        assert!(from_hex("0").is_err());
        assert!(from_hex("zz").is_err());
        assert!(untag("0001").is_err());
    }

    /// The whole ceremony, through real files, for 3 participants (2-of-3):
    /// DKG → every participant derives the same root key → 2 sign → the
    /// result verifies under the workspace's own `RootPublicKey::verify`.
    #[test]
    fn full_file_based_ceremony_produces_a_standard_ed25519_root_signature() {
        let root_dir = tmp("full");
        let dirs: Vec<_> = (1..=3).map(|n| root_dir.join(format!("p{n}"))).collect();
        let pws: Vec<_> = (1..=3)
            .map(|n| pw(&format!("participant {n} long passphrase")))
            .collect();

        // DKG round 1: each runs; broadcasts go to everyone.
        let r1: Vec<String> = (0..3)
            .map(|i| dkg_round1(&dirs[i], &pws[i], (i + 1) as u16, 2, 3).unwrap())
            .collect();
        // Round 2: each consumes the others' round 1 (own file in the pile is tolerated).
        let mut inbox: BTreeMap<u16, Vec<String>> = BTreeMap::new();
        for i in 0..3 {
            for (to, hex) in dkg_round2(&dirs[i], &pws[i], &r1).unwrap() {
                inbox.entry(to).or_default().push(hex);
            }
        }
        // Round 3.
        let roots: Vec<[u8; 32]> = (0..3)
            .map(|i| dkg_round3(&dirs[i], &pws[i], &r1, &inbox[&((i + 1) as u16)]).unwrap())
            .collect();
        assert!(
            roots.windows(2).all(|w| w[0] == w[1]),
            "same root key for all"
        );
        let root = roots[0];
        // No leftover round secrets on disk.
        for d in &dirs {
            assert!(!d.join("r1.secret").exists() && !d.join("r2.secret").exists());
            assert!(d.join("share.sealed").exists());
            // The share file is sealed: the root public key bytes are not in it.
            let raw = fs::read(d.join("share.sealed")).unwrap();
            assert!(!raw.windows(32).any(|w| w == root));
        }

        // Sign with participants 1 and 3.
        let msg = b"provision node: relay-gateway";
        let commits: Vec<String> = [0usize, 2]
            .iter()
            .map(|i| sign_round1(&dirs[*i], &pws[*i]).unwrap())
            .collect();
        let package = make_signing_package(msg, &commits).unwrap();
        let shares: Vec<String> = [0usize, 2]
            .iter()
            .map(|i| sign_round2(&dirs[*i], &pws[*i], &package).unwrap())
            .collect();
        let group = fs::read_to_string(dirs[0].join("group.public.hex")).unwrap();
        let sig = aggregate_signature(&group, &package, &shares).unwrap();
        RootPublicKey(root).verify(msg, &sig).unwrap();
        assert!(RootPublicKey(root).verify(b"other", &sig).is_err());

        // Nonces were consumed: a second round-2 with the same package fails.
        assert!(sign_round2(&dirs[0], &pws[0], &package).is_err());
        assert!(!dirs[0].join("nonces.sealed").exists());
    }

    #[test]
    fn wrong_passphrase_and_swapped_files_are_refused() {
        let root_dir = tmp("swap");
        let d = root_dir.join("p1");
        let p = pw("participant 1 long passphrase");
        dkg_round1(&d, &p, 1, 2, 2).unwrap();
        assert!(dkg_round2(&d, &pw("wrong passphrase entirely"), &[]).is_err());
        // A round-1 secret is not accepted as a round-2 secret.
        fs::copy(d.join("r1.secret"), d.join("r2.secret")).unwrap();
        assert!(dkg_round3(&d, &p, &[], &[]).is_err());
    }

    #[test]
    fn round_inputs_are_checked_for_completeness_duplicates_and_addressing() {
        let root_dir = tmp("inputs");
        let dirs: Vec<_> = (1..=3).map(|n| root_dir.join(format!("p{n}"))).collect();
        let pws: Vec<_> = (1..=3)
            .map(|n| pw(&format!("participant {n} long passphrase")))
            .collect();
        let r1: Vec<String> = (0..3)
            .map(|i| dkg_round1(&dirs[i], &pws[i], (i + 1) as u16, 2, 3).unwrap())
            .collect();
        // Missing a participant's round 1.
        assert!(dkg_round2(&dirs[0], &pws[0], &r1[..2]).is_err());
        // Duplicate.
        let dup = vec![r1[1].clone(), r1[1].clone(), r1[2].clone()];
        assert!(dkg_round2(&dirs[0], &pws[0], &dup).is_err());
        // Now do it properly for 1 and 2, then misaddress a round-2 file.
        let mut inbox: BTreeMap<u16, Vec<String>> = BTreeMap::new();
        for i in 0..3 {
            for (to, hex) in dkg_round2(&dirs[i], &pws[i], &r1).unwrap() {
                inbox.entry(to).or_default().push(hex);
            }
        }
        assert!(
            dkg_round3(&dirs[0], &pws[0], &r1, &inbox[&2]).is_err(),
            "addressed to participant 2"
        );
        // And the correct set still works (the failed attempt consumed nothing).
        dkg_round3(&dirs[0], &pws[0], &r1, &inbox[&1]).unwrap();
    }

    #[test]
    fn a_signing_package_needs_at_least_two_distinct_commitments() {
        assert!(make_signing_package(b"m", &[]).is_err());
        assert!(make_signing_package(b"m", &["zz".into()]).is_err());
    }

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
        }
        fn below(&mut self, n: usize) -> usize {
            (self.next() % n.max(1) as u64) as usize
        }
        fn bytes(&mut self, n: usize) -> Vec<u8> {
            (0..n).map(|_| self.next() as u8).collect()
        }
        fn mutate(&mut self, src: &[u8]) -> Vec<u8> {
            let mut v = src.to_vec();
            for _ in 0..=self.below(4) {
                match self.below(4) {
                    0 if !v.is_empty() => {
                        let i = self.below(v.len());
                        v[i] ^= 1 << self.below(8);
                    }
                    1 if !v.is_empty() => {
                        let i = self.below(v.len());
                        v[i] = self.next() as u8;
                    }
                    2 => {
                        let i = self.below(v.len() + 1);
                        v.insert(i, self.next() as u8);
                    }
                    _ if !v.is_empty() => {
                        let i = self.below(v.len());
                        v.remove(i);
                    }
                    _ => {}
                }
            }
            v
        }
    }

    /// Files come from other people's machines. Whatever bytes arrive, the
    /// parsers return an error or a value — never a panic, and a sealed blob
    /// only opens if it is byte-for-byte what `seal` produced.
    #[test]
    fn hostile_files_never_panic_and_only_the_exact_sealed_blob_opens() {
        let p = pw("correct horse battery");
        let blob = seal(&p, "share", b"the secret share bytes").unwrap();
        let mut rng = Rng(0x005E_A1ED);
        for round in 0..1_500 {
            let input = if round % 3 == 0 {
                let n = rng.below(200);
                rng.bytes(n)
            } else {
                rng.mutate(&blob)
            };
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                unseal(&p, "share", &input)
            }));
            let r = r.unwrap_or_else(|_| panic!("unseal PANICKED on {} bytes", input.len()));
            if input == blob {
                assert_eq!(r.unwrap(), b"the secret share bytes");
            } else {
                assert!(r.is_err(), "a MODIFIED sealed file opened (round {round})");
            }
        }
        // Text-side parsers: arbitrary strings.
        let alphabet = b"0123456789abcdefABCDEF xyz\n\t-";
        for _ in 0..3_000 {
            let n = rng.below(120);
            let s: String = (0..n)
                .map(|_| alphabet[rng.below(alphabet.len())] as char)
                .collect();
            let r = std::panic::catch_unwind(|| {
                let _ = from_hex(&s);
                let _ = untag(&s);
            });
            assert!(r.is_ok(), "hex/tag parser PANICKED on {s:?}");
        }
        // Round-1 / signing inputs: garbage must be rejected, not crash.
        for _ in 0..300 {
            let n = rng.below(150);
            let h = to_hex(&rng.bytes(n));
            let r = std::panic::catch_unwind(|| {
                let _ = make_signing_package(b"m", &[h.clone(), h.clone()]);
                let _ = aggregate_signature(&h, &h, std::slice::from_ref(&h));
            });
            assert!(r.is_ok(), "signing-input parser PANICKED on {h}");
        }
    }
}

//! Encrypted backup and restore of a Siar data directory.
//!
//! Why this exists: a device's identity key lives in one file. Lose it
//! (disk failure, reinstall, a new laptop) and the account, every
//! verified contact and the readable history are gone for good — nothing
//! upstream can recover them, by design. A backup is the only safety net.
//!
//! ## What is protected, and how
//!
//! The archive contains the identity key and MLS group keys, so it is
//! encrypted with a key derived from a passphrase (Argon2id, memory-hard,
//! to make guessing expensive) and XChaCha20-Poly1305. The whole header is
//! bound to the ciphertext as associated data, so changing any byte — a
//! KDF parameter, the salt, the timestamp — makes decryption fail instead
//! of silently weakening or redirecting it. A wrong passphrase and a
//! damaged file are deliberately indistinguishable (`Unreadable`): telling
//! them apart would only help an attacker.
//!
//! ## File layout (version 1)
//!
//! ```text
//! "SIARBK1\n"   8   magic
//! version       1   = 1
//! kdf           1   = 1  (Argon2id v0x13)
//! mem_kib       4   BE
//! iterations    4   BE
//! lanes         4   BE
//! created_ms    8   BE  (plain: not secret, useful to show before unlocking)
//! salt         16
//! nonce        24
//! ciphertext    …   AEAD(postcard(Payload)), the rest of the file
//! ```
//!
//! ## What a restore will and won't do
//!
//! It refuses to run unless the target holds no existing identity, writes
//! into a sibling temporary directory first and renames it into place (so
//! a failure never leaves a half-restored data directory), and rejects any
//! archive entry whose path isn't a plain relative path (no `..`, no
//! absolute paths, no duplicates) before touching the disk.
//!
//! ## Why consistency needs the lock
//!
//! The databases are copied as files. That is only safe when nothing is
//! writing to them, so both operations take the same exclusive
//! [`InstanceLock`] the app holds while running: a backup of a running app
//! is refused rather than silently corrupt. (The lock also stops two copies
//! of the app corrupting one data directory — a bug on its own.)

#![forbid(unsafe_code)]

use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit, OsRng, Payload as AeadPayload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use rand_core::RngCore;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

const MAGIC: &[u8; 8] = b"SIARBK1\n";
const VERSION: u8 = 1;
const KDF_ARGON2ID: u8 = 1;
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 24;
const HEADER_LEN: usize = 8 + 1 + 1 + 4 + 4 + 4 + 8 + SALT_LEN + NONCE_LEN;
/// Files the app creates for its own bookkeeping, never worth backing up.
const SKIP_FILES: &[&str] = &["siar.lock"];
/// Hard ceilings, so a hostile or corrupt archive can't exhaust memory.
const MAX_TOTAL_BYTES: u64 = 1 << 30; // 1 GiB of plaintext
const MAX_ENTRIES: usize = 100_000;
/// The least a usable archive must contain.
const REQUIRED: &[&str] = &["identity.bin", "account_id.txt", "device_id.txt"];
/// Longest passphrase worth hashing; longer is rejected rather than truncated.
pub const MIN_PASSPHRASE_CHARS: usize = 12;

#[derive(Debug, thiserror::Error)]
pub enum BackupError {
    #[error("that passphrase is too weak: {0}")]
    WeakPassphrase(&'static str),
    /// Wrong passphrase, or the file is damaged or has been altered. These
    /// are intentionally not distinguished.
    #[error("couldn’t unlock this backup — wrong passphrase, or the file is damaged")]
    Unreadable,
    #[error("this isn’t a Siar backup file")]
    NotABackup,
    #[error("this backup was made by a newer version of Siar (format {0}) and can’t be read here")]
    UnsupportedVersion(u8),
    #[error("the backup file is corrupt or truncated")]
    Corrupt,
    #[error("the data folder is too large to back up in one file (limit {0} MiB)")]
    TooLarge(u64),
    #[error("the backup contains an unsafe path ({0}) and was not restored")]
    UnsafePath(String),
    #[error("the backup is missing {0}, so it can’t restore an account")]
    MissingRequired(&'static str),
    #[error("there is already a Siar account in {0}; restoring would overwrite it. Choose an empty folder")]
    TargetNotEmpty(PathBuf),
    #[error("the backup file must not be saved inside the data folder it is backing up")]
    OutputInsideData,
    #[error("Siar appears to be running on this data folder; close it first")]
    InstanceRunning,
    #[error("file system error: {0}")]
    Io(#[from] std::io::Error),
}

/// Argon2id cost. The defaults are deliberately heavier than a login
/// check would need: this runs once per backup, and the archive may sit on
/// a USB stick or cloud drive for years.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KdfParams {
    pub mem_kib: u32,
    pub iterations: u32,
    pub lanes: u32,
}

impl KdfParams {
    pub const DEFAULT: Self = Self {
        mem_kib: 64 * 1024,
        iterations: 3,
        lanes: 1,
    };
    // Bounds accepted when READING, so a crafted header can't demand
    // gigabytes of RAM or hours of CPU.
    const MAX_MEM_KIB: u32 = 512 * 1024;
    const MAX_ITERATIONS: u32 = 16;
    const MAX_LANES: u32 = 16;
    const MIN_MEM_KIB: u32 = 8;

    fn validate_for_read(self) -> Result<Self, BackupError> {
        if self.mem_kib < Self::MIN_MEM_KIB
            || self.mem_kib > Self::MAX_MEM_KIB
            || self.iterations == 0
            || self.iterations > Self::MAX_ITERATIONS
            || self.lanes == 0
            || self.lanes > Self::MAX_LANES
        {
            return Err(BackupError::Corrupt);
        }
        Ok(self)
    }
}

/// What can be learned from a backup WITHOUT the passphrase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupInfo {
    pub version: u8,
    pub created_at_millis: u64,
    pub kdf: KdfParams,
    pub encrypted_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupSummary {
    pub files: usize,
    pub plaintext_bytes: u64,
    pub created_at_millis: u64,
}

#[derive(Serialize, Deserialize)]
struct Entry {
    path: String,
    data: Vec<u8>,
}

#[derive(Serialize, Deserialize)]
struct Payload {
    entries: Vec<Entry>,
}

/// `Some(reason)` when a passphrase shouldn't protect an identity key.
/// Deliberately blunt (length and trivial repetition) — a strength meter
/// is a UI concern; this is the floor the core insists on.
pub fn passphrase_problem(passphrase: &str) -> Option<&'static str> {
    let chars: Vec<char> = passphrase.chars().collect();
    if chars.len() < MIN_PASSPHRASE_CHARS {
        return Some("use at least 12 characters");
    }
    if chars.iter().all(|c| *c == chars[0]) {
        return Some("it is one repeated character");
    }
    if passphrase.trim().is_empty() {
        return Some("it is blank");
    }
    None
}

fn derive_key(
    passphrase: &str,
    salt: &[u8],
    p: KdfParams,
) -> Result<Zeroizing<[u8; 32]>, BackupError> {
    let params = Params::new(p.mem_kib, p.iterations, p.lanes, Some(32))
        .map_err(|_| BackupError::Corrupt)?;
    let mut key = Zeroizing::new([0u8; 32]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(passphrase.as_bytes(), salt, key.as_mut())
        .map_err(|_| BackupError::Corrupt)?;
    Ok(key)
}

fn encode_header(
    p: KdfParams,
    created_ms: u64,
    salt: &[u8; SALT_LEN],
    nonce: &[u8; NONCE_LEN],
) -> Vec<u8> {
    let mut h = Vec::with_capacity(HEADER_LEN);
    h.extend_from_slice(MAGIC);
    h.push(VERSION);
    h.push(KDF_ARGON2ID);
    h.extend_from_slice(&p.mem_kib.to_be_bytes());
    h.extend_from_slice(&p.iterations.to_be_bytes());
    h.extend_from_slice(&p.lanes.to_be_bytes());
    h.extend_from_slice(&created_ms.to_be_bytes());
    h.extend_from_slice(salt);
    h.extend_from_slice(nonce);
    h
}

struct ParsedHeader {
    info: BackupInfo,
    salt: [u8; SALT_LEN],
    nonce: [u8; NONCE_LEN],
}

fn parse_header(bytes: &[u8]) -> Result<ParsedHeader, BackupError> {
    if bytes.len() < MAGIC.len() || &bytes[..MAGIC.len()] != MAGIC {
        return Err(BackupError::NotABackup);
    }
    if bytes.len() < HEADER_LEN {
        return Err(BackupError::Corrupt);
    }
    let version = bytes[8];
    if version != VERSION {
        return Err(BackupError::UnsupportedVersion(version));
    }
    if bytes[9] != KDF_ARGON2ID {
        return Err(BackupError::Corrupt);
    }
    let be32 = |o: usize| u32::from_be_bytes(bytes[o..o + 4].try_into().unwrap());
    let kdf = KdfParams {
        mem_kib: be32(10),
        iterations: be32(14),
        lanes: be32(18),
    }
    .validate_for_read()?;
    let created = u64::from_be_bytes(bytes[22..30].try_into().unwrap());
    let mut salt = [0u8; SALT_LEN];
    salt.copy_from_slice(&bytes[30..30 + SALT_LEN]);
    let mut nonce = [0u8; NONCE_LEN];
    nonce.copy_from_slice(&bytes[30 + SALT_LEN..HEADER_LEN]);
    Ok(ParsedHeader {
        info: BackupInfo {
            version,
            created_at_millis: created,
            kdf,
            encrypted_bytes: (bytes.len() - HEADER_LEN) as u64,
        },
        salt,
        nonce,
    })
}

/// Plain relative path made only of normal components.
fn safe_relative(path: &str) -> bool {
    if path.is_empty() || path.contains('\\') || path.contains('\0') {
        return false;
    }
    let p = Path::new(path);
    !p.is_absolute() && p.components().all(|c| matches!(c, Component::Normal(_)))
}

fn collect(
    dir: &Path,
    root: &Path,
    out: &mut Vec<Entry>,
    total: &mut u64,
) -> Result<(), BackupError> {
    let mut children: Vec<_> = fs::read_dir(dir)?.collect::<Result<_, _>>()?;
    children.sort_by_key(|e| e.file_name()); // a deterministic archive
    for child in children {
        let path = child.path();
        let ty = child.file_type()?;
        if ty.is_symlink() {
            continue; // never follow a link out of the data directory
        }
        let name = child.file_name().to_string_lossy().into_owned();
        if SKIP_FILES.contains(&name.as_str()) || name.ends_with(".tmp") {
            continue;
        }
        if ty.is_dir() {
            collect(&path, root, out, total)?;
        } else if ty.is_file() {
            let data = fs::read(&path)?;
            *total += data.len() as u64;
            if *total > MAX_TOTAL_BYTES || out.len() >= MAX_ENTRIES {
                return Err(BackupError::TooLarge(MAX_TOTAL_BYTES >> 20));
            }
            let rel = path.strip_prefix(root).map_err(|_| BackupError::Corrupt)?;
            let rel = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            out.push(Entry { path: rel, data });
        }
    }
    Ok(())
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn seal(
    entries: Vec<Entry>,
    passphrase: &str,
    kdf: KdfParams,
    created_ms: u64,
) -> Result<Vec<u8>, BackupError> {
    let mut salt = [0u8; SALT_LEN];
    let mut nonce = [0u8; NONCE_LEN];
    OsRng.fill_bytes(&mut salt);
    OsRng.fill_bytes(&mut nonce);
    let header = encode_header(kdf, created_ms, &salt, &nonce);
    let plaintext = Zeroizing::new(
        postcard::to_allocvec(&Payload { entries }).map_err(|_| BackupError::Corrupt)?,
    );
    let key = derive_key(passphrase, &salt, kdf)?;
    let cipher = XChaCha20Poly1305::new(key.as_ref().into());
    let sealed = cipher
        .encrypt(
            XNonce::from_slice(&nonce),
            AeadPayload {
                msg: &plaintext,
                aad: &header,
            },
        )
        .map_err(|_| BackupError::Corrupt)?;
    let mut out = header;
    out.extend(sealed);
    Ok(out)
}

fn open(bytes: &[u8], passphrase: &str) -> Result<(Payload, BackupInfo), BackupError> {
    let header = parse_header(bytes)?;
    let key = derive_key(passphrase, &header.salt, header.info.kdf)?;
    let cipher = XChaCha20Poly1305::new(key.as_ref().into());
    let plaintext = Zeroizing::new(
        cipher
            .decrypt(
                XNonce::from_slice(&header.nonce),
                AeadPayload {
                    msg: &bytes[HEADER_LEN..],
                    aad: &bytes[..HEADER_LEN],
                },
            )
            .map_err(|_| BackupError::Unreadable)?,
    );
    let payload: Payload = postcard::from_bytes(&plaintext).map_err(|_| BackupError::Corrupt)?;
    Ok((payload, header.info))
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), BackupError> {
    let tmp = path.with_extension("partial");
    let mut f = fs::File::create(&tmp)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        f.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    f.write_all(bytes)?;
    f.sync_all()?;
    fs::rename(&tmp, path)?;
    Ok(())
}

/// Backs `data_dir` up into the encrypted file `out`.
///
/// Fails with [`BackupError::InstanceRunning`] if the app holds the data
/// folder's lock.
pub fn create_backup(
    data_dir: &Path,
    passphrase: &str,
    out: &Path,
) -> Result<BackupSummary, BackupError> {
    create_backup_with(data_dir, passphrase, out, KdfParams::DEFAULT)
}

pub fn create_backup_with(
    data_dir: &Path,
    passphrase: &str,
    out: &Path,
    kdf: KdfParams,
) -> Result<BackupSummary, BackupError> {
    if let Some(reason) = passphrase_problem(passphrase) {
        return Err(BackupError::WeakPassphrase(reason));
    }
    let data_dir = fs::canonicalize(data_dir)?;
    // A backup written inside the folder being backed up would be swept up
    // into the NEXT backup (and into itself, if listed mid-write).
    let out_parent = fs::canonicalize(out.parent().unwrap_or(Path::new(".")))?;
    if out_parent.starts_with(&data_dir) {
        return Err(BackupError::OutputInsideData);
    }
    let _lock = InstanceLock::acquire(&data_dir).map_err(|e| match e {
        LockError::Held => BackupError::InstanceRunning,
        LockError::Io(e) => BackupError::Io(e),
    })?;

    let mut entries = Vec::new();
    let mut total = 0u64;
    collect(&data_dir, &data_dir, &mut entries, &mut total)?;
    for needed in REQUIRED {
        if !entries.iter().any(|e| e.path == *needed) {
            return Err(BackupError::MissingRequired(needed));
        }
    }
    let files = entries.len();
    let created = now_millis();
    let bytes = seal(entries, passphrase, kdf, created)?;
    write_atomic(out, &bytes)?;
    Ok(BackupSummary {
        files,
        plaintext_bytes: total,
        created_at_millis: created,
    })
}

/// Header facts, no passphrase needed.
pub fn inspect(archive: &Path) -> Result<BackupInfo, BackupError> {
    // Only the header is needed; reading the whole file just to size it
    // is cheap enough and keeps `encrypted_bytes` exact.
    Ok(parse_header(&fs::read(archive)?)?.info)
}

/// Restores `archive` into `target`, which must not already hold an
/// account. On any failure `target` is left exactly as it was.
pub fn restore_backup(
    archive: &Path,
    passphrase: &str,
    target: &Path,
) -> Result<BackupSummary, BackupError> {
    let bytes = fs::read(archive)?;
    let (payload, info) = open(&bytes, passphrase)?;

    // Validate EVERYTHING before writing anything.
    if payload.entries.len() > MAX_ENTRIES {
        return Err(BackupError::Corrupt);
    }
    let mut seen = std::collections::HashSet::new();
    let mut total = 0u64;
    for e in &payload.entries {
        if !safe_relative(&e.path) {
            return Err(BackupError::UnsafePath(e.path.chars().take(80).collect()));
        }
        if !seen.insert(e.path.as_str()) {
            return Err(BackupError::UnsafePath(format!(
                "{} (listed twice)",
                e.path
            )));
        }
        total += e.data.len() as u64;
    }
    if total > MAX_TOTAL_BYTES {
        return Err(BackupError::TooLarge(MAX_TOTAL_BYTES >> 20));
    }
    for needed in REQUIRED {
        if !seen.contains(needed) {
            return Err(BackupError::MissingRequired(needed));
        }
    }

    // If the target exists, take its lock: restoring into a folder a
    // running app is using would corrupt that app's data.
    let guard = if target.exists() {
        let guard = InstanceLock::acquire(target).map_err(|e| match e {
            LockError::Held => BackupError::InstanceRunning,
            LockError::Io(e) => BackupError::Io(e),
        })?;
        let occupied = fs::read_dir(target)?
            .filter_map(Result::ok)
            .any(|e| e.file_name() != "siar.lock");
        if occupied {
            return Err(BackupError::TargetNotEmpty(target.to_path_buf()));
        }
        Some(guard)
    } else {
        None
    };
    let parent = target.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let staging = parent.join(format!(
        ".{}.restoring-{}",
        target
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        now_millis()
    ));
    let _ = fs::remove_dir_all(&staging);
    let result = (|| -> Result<(), BackupError> {
        fs::create_dir_all(&staging)?;
        for e in &payload.entries {
            let path = staging.join(&e.path);
            if let Some(dir) = path.parent() {
                fs::create_dir_all(dir)?;
            }
            write_secret(&path, &e.data)?;
        }
        drop(guard);
        if target.exists() {
            // Empty, or holding only the lock file this very call may have
            // just created (checked above).
            let _ = fs::remove_file(target.join("siar.lock"));
            fs::remove_dir(target)?;
        }
        fs::rename(&staging, target)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    result?;
    Ok(BackupSummary {
        files: payload.entries.len(),
        plaintext_bytes: total,
        created_at_millis: info.created_at_millis,
    })
}

fn write_secret(path: &Path, data: &[u8]) -> Result<(), BackupError> {
    let mut f = fs::File::create(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        f.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    f.write_all(data)?;
    f.sync_all()?;
    Ok(())
}

// ---- single-instance lock ---------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum LockError {
    #[error("another process holds the lock")]
    Held,
    #[error("couldn’t use the lock file: {0}")]
    Io(#[from] std::io::Error),
}

/// Exclusive advisory lock on `<data dir>/siar.lock`, held for as long as
/// the value lives. The OS releases it if the process dies, so a crash
/// can never leave the folder locked forever.
#[derive(Debug)]
pub struct InstanceLock {
    _file: fs::File,
}

impl InstanceLock {
    pub fn acquire(data_dir: &Path) -> Result<Self, LockError> {
        fs::create_dir_all(data_dir)?;
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(data_dir.join("siar.lock"))?;
        match file.try_lock() {
            Ok(()) => Ok(Self { _file: file }),
            Err(fs::TryLockError::WouldBlock) => Err(LockError::Held),
            Err(fs::TryLockError::Error(e)) => Err(LockError::Io(e)),
        }
    }
}

#[cfg(test)]
mod tests;

//! Admin key files. The node's admin key and each operator's key are plain
//! Iroh (Ed25519) keys stored as 32 raw bytes, mode 0600, created with
//! `create_new` so an existing key is never overwritten.

use std::fs;
use std::io::{self, Write};
use std::path::Path;

use iroh::SecretKey;

/// Load a key, creating it (from the OS CSPRNG) if the file does not exist.
/// On Unix a key readable by group/others is **refused**, not silently used.
pub fn load_or_create(path: &Path) -> io::Result<SecretKey> {
    match fs::read(path) {
        Ok(bytes) => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = fs::metadata(path)?.permissions().mode();
                if mode & 0o077 != 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        format!(
                            "{} is readable by others (mode {:o}); chmod 600 it",
                            path.display(),
                            mode & 0o777
                        ),
                    ));
                }
            }
            let arr: [u8; 32] = bytes.try_into().map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "admin key file must be exactly 32 bytes",
                )
            })?;
            Ok(SecretKey::from_bytes(&arr))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut seed = [0u8; 32];
            getrandom::fill(&mut seed).map_err(|e| io::Error::other(e.to_string()))?;
            let mut opts = fs::OpenOptions::new();
            opts.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                opts.mode(0o600);
            }
            let mut f = opts.open(path)?;
            f.write_all(&seed)?;
            f.sync_all()?;
            Ok(SecretKey::from_bytes(&seed))
        }
        Err(e) => Err(e),
    }
}

#[must_use]
pub fn to_hex(b: &[u8]) -> String {
    use std::fmt::Write as _;
    b.iter()
        .fold(String::with_capacity(b.len() * 2), |mut s, x| {
            let _ = write!(s, "{x:02x}");
            s
        })
}

/// Strict: exactly 64 lowercase hex characters.
#[must_use]
pub fn parse_hex32(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 || !s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(out)
}

#[must_use]
pub fn public_hex(k: &SecretKey) -> String {
    to_hex(k.public().as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("siar-admin-keys-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn creates_once_then_loads_the_same_key_with_mode_600() {
        let p = tmp("create").join("k.key");
        let a = load_or_create(&p).unwrap();
        let b = load_or_create(&p).unwrap();
        assert_eq!(a.public(), b.public());
        assert_eq!(fs::read(&p).unwrap().len(), 32);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&p).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn a_world_readable_or_malformed_key_is_refused() {
        let d = tmp("bad");
        fs::create_dir_all(&d).unwrap();
        let p = d.join("k.key");
        fs::write(&p, [7u8; 32]).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&p, fs::Permissions::from_mode(0o644)).unwrap();
            assert_eq!(
                load_or_create(&p).unwrap_err().kind(),
                io::ErrorKind::PermissionDenied
            );
            fs::set_permissions(&p, fs::Permissions::from_mode(0o600)).unwrap();
        }
        assert!(load_or_create(&p).is_ok());
        fs::write(&p, [7u8; 31]).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&p, fs::Permissions::from_mode(0o600)).unwrap();
        }
        assert_eq!(
            load_or_create(&p).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn hex_is_strict() {
        let k = SecretKey::generate();
        let h = public_hex(&k);
        assert_eq!(parse_hex32(&h), Some(*k.public().as_bytes()));
        for bad in ["", "ab", &h.to_uppercase(), &format!("{h}00"), &h[..63]] {
            assert_eq!(parse_hex32(bad), None, "{bad:?}");
        }
    }
}

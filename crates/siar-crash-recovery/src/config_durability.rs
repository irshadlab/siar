//! §154 "Corrupt Config", §155 "Config Transactions", §156 "RON
//! Config", §157 "Postcard Durable Data".
//!
//! A broken config file must never cost the user data, and a changed
//! struct must never silently reinterpret old bytes. The format
//! crates (RON, Postcard) stay out of this module on purpose: what is
//! shared is the *envelope* — a version — and the decision of what to
//! do with it.

use crate::atomic_rename::{publish_atomic, PublishError};
use crate::migration_recovery::{check_open_compatibility, OpenCompatibility};
use crate::temp_scan::OpaqueTempId;
use std::io::Write;
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------
// §154
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingClass {
    /// Theme, layout: any sane value is fine.
    Cosmetic,
    /// Ports, intervals, limits.
    Operational,
    /// Anything whose permissive default could weaken security
    /// (trust policy, allowed transports for sensitive traffic).
    SecurityCritical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigFallback {
    LastKnownGood,
    Defaults,
    /// Start with minimal, restrictive behaviour and tell the user.
    SafeMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfigRecovery {
    pub fallback: ConfigFallback,
    /// The unreadable file is set aside for inspection, not deleted.
    pub preserve_corrupt_copy: bool,
    /// Config problems never touch durable user data.
    pub touches_user_data: bool,
}

/// Cosmetic and operational settings prefer last-known-good, then
/// defaults. Security-critical ones prefer last-known-good and
/// otherwise fall to **safe mode** — never to a silently permissive
/// default.
pub const fn on_config_parse_failure(
    class: SettingClass,
    have_last_known_good: bool,
) -> ConfigRecovery {
    let fallback = if have_last_known_good {
        ConfigFallback::LastKnownGood
    } else {
        match class {
            SettingClass::Cosmetic | SettingClass::Operational => ConfigFallback::Defaults,
            SettingClass::SecurityCritical => ConfigFallback::SafeMode,
        }
    };
    ConfigRecovery {
        fallback,
        preserve_corrupt_copy: true,
        touches_user_data: false,
    }
}

// ---------------------------------------------------------------------
// §155
// ---------------------------------------------------------------------

fn lkg_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(".lkg");
    path.with_file_name(name)
}

fn write_temp(dir_of: &Path, bytes: &[u8]) -> std::io::Result<PathBuf> {
    let tmp = dir_of.with_file_name(OpaqueTempId::new().file_name());
    let mut f = std::fs::File::create(&tmp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    Ok(tmp)
}

/// Atomically replaces the config at `path`: write temp → fsync →
/// rename → fsync directory (the §22 publish). Readers see the old or
/// the new file, never a mix.
pub fn write_config_atomic(path: &Path, bytes: &[u8]) -> Result<(), PublishError> {
    let tmp = write_temp(path, bytes)?;
    if let Err(e) = publish_atomic(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

/// Like [`write_config_atomic`], but first preserves the current file
/// as `<name>.lkg` (also atomically) so there is a last-known-good to
/// fall back to if the new one turns out to be bad.
pub fn write_config_keeping_lkg(path: &Path, bytes: &[u8]) -> Result<(), PublishError> {
    match std::fs::read(path) {
        Ok(old) => write_config_atomic(&lkg_path(path), &old)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    write_config_atomic(path, bytes)
}

pub fn last_known_good_path(path: &Path) -> PathBuf {
    lkg_path(path)
}

// ---------------------------------------------------------------------
// §156
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigLoad {
    Accepted,
    /// Could not be parsed at all.
    RejectedMalformed,
    /// Parsed, but unknown fields / wrong types / out-of-range values.
    RejectedInvalid,
    RejectedVersion(OpenCompatibility),
    /// Older but upgradeable: accepted only through the explicit
    /// upcast path.
    NeedsUpcast {
        from: u32,
    },
}

/// §156: version it, validate strictly, never accept malformed config
/// silently. `parsed_version` is `None` when the file could not be
/// parsed; `strict_validation_passed` is the result of the format
/// layer's strict check (e.g. `deny_unknown_fields`).
pub fn config_load_decision(
    parsed_version: Option<u32>,
    strict_validation_passed: bool,
    current: u32,
    upcastable_from: &RangeInclusive<u32>,
) -> ConfigLoad {
    let Some(v) = parsed_version else {
        return ConfigLoad::RejectedMalformed;
    };
    match check_open_compatibility(v, current, upcastable_from) {
        OpenCompatibility::Current => {
            if strict_validation_passed {
                ConfigLoad::Accepted
            } else {
                ConfigLoad::RejectedInvalid
            }
        }
        OpenCompatibility::NeedsMigration { from } => ConfigLoad::NeedsUpcast { from },
        other => ConfigLoad::RejectedVersion(other),
    }
}

// ---------------------------------------------------------------------
// §157
// ---------------------------------------------------------------------

const MAGIC: [u8; 4] = *b"SIAR";
pub const DURABLE_HEADER_LEN: usize = 8;

/// 4-byte magic + little-endian `u32` schema version, prefixed to
/// every durable serialized value.
pub const fn encode_durable_header(schema_version: u32) -> [u8; DURABLE_HEADER_LEN] {
    let v = schema_version.to_le_bytes();
    [
        MAGIC[0], MAGIC[1], MAGIC[2], MAGIC[3], v[0], v[1], v[2], v[3],
    ]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum HeaderError {
    #[error("durable value shorter than its header")]
    TooShort,
    #[error("durable value has no SIAR header (unversioned or foreign data)")]
    BadMagic,
}

/// Splits a durable value into `(schema_version, payload)`.
pub fn parse_durable_header(bytes: &[u8]) -> Result<(u32, &[u8]), HeaderError> {
    if bytes.len() < DURABLE_HEADER_LEN {
        return Err(HeaderError::TooShort);
    }
    if bytes[..4] != MAGIC {
        return Err(HeaderError::BadMagic);
    }
    let v = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    Ok((v, &bytes[DURABLE_HEADER_LEN..]))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodePlan {
    /// Same version as the current struct: decode directly.
    DecodeDirect,
    /// Older: must go through an explicit upcast/migration first.
    UpcastThenDecode {
        from: u32,
    },
    Refuse(OpenCompatibility),
}

/// §157: old persistent bytes are never decoded straight into a
/// changed struct. Only an exact version match decodes directly.
pub fn decode_plan(
    stored_version: u32,
    current: u32,
    upcastable_from: &RangeInclusive<u32>,
) -> DecodePlan {
    match check_open_compatibility(stored_version, current, upcastable_from) {
        OpenCompatibility::Current => DecodePlan::DecodeDirect,
        OpenCompatibility::NeedsMigration { from } => DecodePlan::UpcastThenDecode { from },
        other => DecodePlan::Refuse(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn broken_config_never_touches_data_and_keeps_the_corrupt_file() {
        for class in [
            SettingClass::Cosmetic,
            SettingClass::Operational,
            SettingClass::SecurityCritical,
        ] {
            for lkg in [false, true] {
                let r = on_config_parse_failure(class, lkg);
                assert!(r.preserve_corrupt_copy, "{class:?}");
                assert!(!r.touches_user_data, "{class:?}");
            }
        }
    }

    #[test]
    fn fallback_prefers_last_known_good_then_class_specific() {
        for class in [
            SettingClass::Cosmetic,
            SettingClass::Operational,
            SettingClass::SecurityCritical,
        ] {
            assert_eq!(
                on_config_parse_failure(class, true).fallback,
                ConfigFallback::LastKnownGood
            );
        }
        assert_eq!(
            on_config_parse_failure(SettingClass::Cosmetic, false).fallback,
            ConfigFallback::Defaults
        );
        assert_eq!(
            on_config_parse_failure(SettingClass::Operational, false).fallback,
            ConfigFallback::Defaults
        );
        assert_eq!(
            on_config_parse_failure(SettingClass::SecurityCritical, false).fallback,
            ConfigFallback::SafeMode,
            "never a silently permissive default"
        );
    }

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "siar-cfg-{}-{}-{}",
            name,
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[cfg(unix)]
    #[test]
    fn config_write_is_atomic_replace_with_no_temp_left_behind() {
        let d = scratch("atomic");
        let p = d.join("node.ron");
        write_config_atomic(&p, b"v1").unwrap();
        write_config_atomic(&p, b"version two").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"version two");
        let names: Vec<_> = std::fs::read_dir(&d)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            vec!["node.ron".to_owned()],
            "no stray temp: {names:?}"
        );
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn keeping_lkg_preserves_the_previous_good_version() {
        let d = scratch("lkg");
        let p = d.join("node.ron");
        write_config_keeping_lkg(&p, b"first").unwrap();
        assert!(
            !last_known_good_path(&p).exists(),
            "nothing to preserve yet"
        );
        write_config_keeping_lkg(&p, b"second").unwrap();
        write_config_keeping_lkg(&p, b"third").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"third");
        assert_eq!(std::fs::read(last_known_good_path(&p)).unwrap(), b"second");
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn lkg_path_sits_next_to_the_config() {
        assert_eq!(
            last_known_good_path(Path::new("/etc/siar/node.ron")),
            PathBuf::from("/etc/siar/node.ron.lkg")
        );
    }

    #[test]
    fn config_load_never_accepts_malformed_or_invalid_silently() {
        let up = 1..=2;
        assert_eq!(
            config_load_decision(None, true, 3, &up),
            ConfigLoad::RejectedMalformed
        );
        assert_eq!(
            config_load_decision(Some(3), false, 3, &up),
            ConfigLoad::RejectedInvalid
        );
        assert_eq!(
            config_load_decision(Some(3), true, 3, &up),
            ConfigLoad::Accepted
        );
    }

    #[test]
    fn config_versions_upcast_or_are_rejected() {
        let up = 2..=3;
        assert_eq!(
            config_load_decision(Some(2), true, 4, &up),
            ConfigLoad::NeedsUpcast { from: 2 }
        );
        assert_eq!(
            config_load_decision(Some(5), true, 4, &up),
            ConfigLoad::RejectedVersion(OpenCompatibility::TooNew)
        );
        assert_eq!(
            config_load_decision(Some(1), true, 4, &up),
            ConfigLoad::RejectedVersion(OpenCompatibility::TooOld)
        );
    }

    #[test]
    fn durable_header_round_trips() {
        let mut v = encode_durable_header(7).to_vec();
        v.extend_from_slice(b"payload");
        let (ver, rest) = parse_durable_header(&v).unwrap();
        assert_eq!((ver, rest), (7, &b"payload"[..]));
        assert_eq!(
            parse_durable_header(&encode_durable_header(0)).unwrap(),
            (0, &[][..])
        );
    }

    #[test]
    fn unversioned_or_truncated_durable_data_is_rejected() {
        assert_eq!(parse_durable_header(b"SIA"), Err(HeaderError::TooShort));
        assert_eq!(
            parse_durable_header(b"NOPE\x01\x00\x00\x00rest"),
            Err(HeaderError::BadMagic)
        );
        assert_eq!(parse_durable_header(b""), Err(HeaderError::TooShort));
    }

    #[test]
    fn only_an_exact_version_match_decodes_directly() {
        let up = 2..=4;
        assert_eq!(decode_plan(5, 5, &up), DecodePlan::DecodeDirect);
        assert_eq!(
            decode_plan(3, 5, &up),
            DecodePlan::UpcastThenDecode { from: 3 }
        );
        assert_eq!(
            decode_plan(1, 5, &up),
            DecodePlan::Refuse(OpenCompatibility::TooOld)
        );
        assert_eq!(
            decode_plan(6, 5, &up),
            DecodePlan::Refuse(OpenCompatibility::TooNew)
        );
    }

    #[test]
    fn changing_the_struct_forces_the_old_bytes_off_the_direct_path() {
        // The same stored bytes: direct while current==3, but the day
        // the struct moves to version 4 they must upcast.
        let up = 3..=3;
        assert_eq!(decode_plan(3, 3, &up), DecodePlan::DecodeDirect);
        assert_eq!(
            decode_plan(3, 4, &up),
            DecodePlan::UpcastThenDecode { from: 3 }
        );
    }
}

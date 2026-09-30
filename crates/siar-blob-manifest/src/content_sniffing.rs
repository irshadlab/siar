//! §56 "MIME Type Validation", §57 "Executable Content".
//!
//! §58 "Archive Safety" gets no code anywhere in this crate, by the
//! spec's own explicit instruction: "must be handled in a separate
//! safe extraction subsystem. Blob layer itself stores opaque bytes."
//! Naming it here rather than silently skipping it.

use siar_domain::MediaType;

use crate::descriptor::FileName;

/// §56: "Remote MIME type is only a hint... sniff/inspect content
/// safely before opening." A minimal magic-byte sniffer covering every
/// variant [`MediaType`] actually has except [`MediaType::Other`]
/// (which by definition has no signature to check) — enough to catch
/// the specific failure mode §56 warns about (a peer mislabeling
/// content to get it treated as something trusted), not a general
/// file-type detector.
pub fn sniff_media_type(bytes: &[u8]) -> Option<MediaType> {
    if bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]) {
        return Some(MediaType::ImagePng);
    }
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some(MediaType::ImageJpeg);
    }
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some(MediaType::ImageWebp);
    }
    if bytes.len() >= 8 && &bytes[4..8] == b"ftyp" {
        return Some(MediaType::VideoMp4);
    }
    if bytes.starts_with(b"OggS") {
        return Some(MediaType::AudioOpus);
    }
    None
}

/// §56's own outcome, made explicit rather than a bare bool: "we
/// couldn't check" and "we checked and it's wrong" call for different
/// caller behavior (the first might still be safe to open cautiously;
/// the second is a real red flag), so collapsing them into one boolean
/// would throw away information §56 itself relies on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MimeTrust {
    /// The declared type and the sniffed bytes agree.
    Confirmed,
    /// The declared type and the sniffed bytes disagree — the
    /// specific case §56 exists to catch.
    Mismatch,
    /// Too few bytes, or a signature [`sniff_media_type`] doesn't
    /// recognize (including a declared [`MediaType::Other`], which has
    /// no signature to check by definition).
    Unverifiable,
}

pub fn check_declared_media_type(declared: Option<MediaType>, bytes: &[u8]) -> MimeTrust {
    match (declared, sniff_media_type(bytes)) {
        (Some(declared), Some(sniffed)) if declared == sniffed => MimeTrust::Confirmed,
        (Some(_), Some(_)) => MimeTrust::Mismatch,
        _ => MimeTrust::Unverifiable,
    }
}

/// §57: "Downloads may contain APK/EXE/script/archive... should not
/// auto-execute... opening is an explicit application/user action."
/// [`MediaType`] has no variant for any of these (it's an allow-list of
/// media the UI renders specially, not a general file-type registry —
/// see that type's own doc comment in `siar-domain`), so this checks
/// the one signal this crate actually has for it: the file extension
/// on a [`FileName`]. A caller that finds `true` here is the one §57
/// says should require an explicit user action before opening, rather
/// than whatever it would otherwise have done automatically.
pub fn has_potentially_executable_extension(name: &FileName) -> bool {
    const RISKY_EXTENSIONS: &[&str] = &[
        "exe", "apk", "sh", "bat", "cmd", "msi", "jar", "app", "com", "scr", "ps1", "dmg", "deb",
        "rpm", "AppImage",
    ];
    let Some(extension) = name.as_str().rsplit('.').next() else {
        return false;
    };
    // The whole name has no '.' at all: `rsplit('.').next()` then
    // returns the whole string, which is never itself one of the
    // short, fixed extensions above, so this is safe without an
    // explicit "does it contain a dot" check.
    RISKY_EXTENSIONS
        .iter()
        .any(|risky| extension.eq_ignore_ascii_case(risky))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_known_signature_sniffs_correctly() {
        assert_eq!(
            sniff_media_type(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0, 0]),
            Some(MediaType::ImagePng)
        );
        assert_eq!(
            sniff_media_type(&[0xFF, 0xD8, 0xFF, 0xE0]),
            Some(MediaType::ImageJpeg)
        );
        let mut webp = b"RIFF".to_vec();
        webp.extend_from_slice(&[0, 0, 0, 0]);
        webp.extend_from_slice(b"WEBP");
        assert_eq!(sniff_media_type(&webp), Some(MediaType::ImageWebp));
        let mut mp4 = vec![0, 0, 0, 0x20];
        mp4.extend_from_slice(b"ftypisom");
        assert_eq!(sniff_media_type(&mp4), Some(MediaType::VideoMp4));
        assert_eq!(
            sniff_media_type(b"OggS\0\0\0\0"),
            Some(MediaType::AudioOpus)
        );
    }

    #[test]
    fn unrecognized_or_too_short_bytes_are_unsniffable() {
        assert_eq!(sniff_media_type(b"not a real file"), None);
        assert_eq!(sniff_media_type(&[]), None);
    }

    #[test]
    fn a_jpeg_declared_as_png_is_a_real_mismatch() {
        let jpeg_bytes = [0xFF, 0xD8, 0xFF, 0xE0];
        assert_eq!(
            check_declared_media_type(Some(MediaType::ImagePng), &jpeg_bytes),
            MimeTrust::Mismatch
        );
    }

    #[test]
    fn a_correctly_declared_type_is_confirmed() {
        let jpeg_bytes = [0xFF, 0xD8, 0xFF, 0xE0];
        assert_eq!(
            check_declared_media_type(Some(MediaType::ImageJpeg), &jpeg_bytes),
            MimeTrust::Confirmed
        );
    }

    #[test]
    fn an_other_declared_type_is_never_confirmed_or_mismatched_only_unverifiable() {
        assert_eq!(
            check_declared_media_type(Some(MediaType::Other), b"anything at all"),
            MimeTrust::Unverifiable
        );
    }

    #[test]
    fn no_declared_type_at_all_is_unverifiable() {
        let jpeg_bytes = [0xFF, 0xD8, 0xFF, 0xE0];
        assert_eq!(
            check_declared_media_type(None, &jpeg_bytes),
            MimeTrust::Unverifiable
        );
    }

    #[test]
    fn common_risky_extensions_are_flagged() {
        for name in ["malware.exe", "app.apk", "install.sh", "setup.MSI"] {
            assert!(
                has_potentially_executable_extension(&FileName::new(name).unwrap()),
                "{name} should be flagged"
            );
        }
    }

    #[test]
    fn ordinary_media_files_are_not_flagged() {
        for name in ["photo.jpg", "video.mp4", "notes.txt", "README"] {
            assert!(!has_potentially_executable_extension(
                &FileName::new(name).unwrap()
            ));
        }
    }
}

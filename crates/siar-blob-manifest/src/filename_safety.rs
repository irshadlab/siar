//! §53 "Temp File Naming", §54 "Path Traversal Protection", §55
//! "Filename Sanitization".
//!
//! §54 gets no function of its own here — it's enforced on
//! [`crate::descriptor::FileName`] itself by omission (see that type's
//! own doc comment): there is no path-shaped API on `FileName` to
//! misuse in the first place. What this module provides is the two
//! real, opposite-direction conversions §53/§55 ask for: a safe name
//! to WRITE WITH while a transfer is in progress (never derived from
//! anything remote), and a safe name to export TO once it's done
//! (derived from the remote-supplied [`crate::descriptor::FileName`],
//! but sanitized first).

use crate::ids::TransferId;

/// §53: "Use opaque safe IDs: `<TransferId>.partial`. Do not use
/// untrusted file names as filesystem paths." A transfer's own
/// locally-generated [`TransferId`] is exactly the "opaque safe ID"
/// the section asks for — never anything a remote peer supplied.
pub fn temp_file_name(transfer_id: TransferId) -> String {
    format!("{transfer_id}.partial")
}

/// §55: "sanitize, normalize... keep original display name separately
/// if useful." Replaces path separators and control characters (the
/// two categories that could otherwise turn a display string into
/// something meaningful to a filesystem) with `_`, then falls back to
/// a fixed safe name for the degenerate cases (empty, `.`, `..`) that
/// would otherwise export as an invisible or traversal-shaped file.
/// The original [`crate::descriptor::FileName`] is never mutated or
/// discarded by this function — a caller keeps it for display exactly
/// as the section says, and only ever passes the *returned* `String`
/// to a real filesystem write.
pub fn sanitize_for_export(name: &crate::descriptor::FileName) -> String {
    let cleaned: String = name
        .as_str()
        .chars()
        .map(|c| {
            if c == '/' || c == '\\' || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() || trimmed == "." || trimmed == ".." {
        "unnamed_file".to_string()
    } else {
        trimmed.to_string()
    }
}

/// §55's own worked example: `photo.jpg` → `photo (1).jpg` when a name
/// already exists in the destination directory. Takes the sanitized
/// name (the output of [`sanitize_for_export`], never a raw
/// [`crate::descriptor::FileName`] directly) plus the names already
/// present there, and returns the first available candidate —
/// `desired` itself if it doesn't collide, else `desired` with an
/// incrementing `" (n)"` suffix inserted before the last extension.
pub fn resolve_export_collision(desired: &str, existing_names: &[String]) -> String {
    if !existing_names.iter().any(|n| n == desired) {
        return desired.to_string();
    }
    let (stem, extension) = split_extension(desired);
    for n in 1..=existing_names.len() as u32 + 1 {
        let candidate = match extension {
            Some(ext) => format!("{stem} ({n}).{ext}"),
            None => format!("{stem} ({n})"),
        };
        if !existing_names.iter().any(|x| x == &candidate) {
            return candidate;
        }
    }
    // Unreachable in practice: at most `existing_names.len()` distinct
    // collisions can exist, so the loop above always finds a free
    // candidate within `existing_names.len() + 1` attempts. Falling
    // back to the input rather than panicking if that bound is ever
    // wrong is deliberate — this is a UI-facing display name, not a
    // safety boundary, so degrading rather than crashing is correct.
    desired.to_string()
}

fn split_extension(name: &str) -> (&str, Option<&str>) {
    match name.rfind('.') {
        Some(0) => (name, None), // a leading dot ("`.bashrc`") is not an extension
        Some(idx) => (&name[..idx], Some(&name[idx + 1..])),
        None => (name, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::descriptor::FileName;

    #[test]
    fn a_temp_file_name_never_contains_anything_but_the_transfer_id_and_the_suffix() {
        let id = TransferId::new();
        let name = temp_file_name(id);
        assert_eq!(name, format!("{id}.partial"));
    }

    #[test]
    fn path_traversal_characters_are_neutralized_not_rejected() {
        let name = FileName::new("../../etc/passwd").unwrap();
        let sanitized = sanitize_for_export(&name);
        assert!(!sanitized.contains('/'));
        assert!(!sanitized.contains('\\'));
        assert_eq!(sanitized, ".._.._etc_passwd");
    }

    #[test]
    fn a_windows_style_traversal_attempt_is_also_neutralized() {
        let name = FileName::new("..\\..\\Windows\\System32").unwrap();
        let sanitized = sanitize_for_export(&name);
        assert!(!sanitized.contains('\\'));
    }

    #[test]
    fn degenerate_names_fall_back_to_a_fixed_safe_name() {
        assert_eq!(
            sanitize_for_export(&FileName::new("..").unwrap()),
            "unnamed_file"
        );
        assert_eq!(
            sanitize_for_export(&FileName::new(".").unwrap()),
            "unnamed_file"
        );
        assert_eq!(
            sanitize_for_export(&FileName::new("   ").unwrap()),
            "unnamed_file"
        );
    }

    #[test]
    fn an_ordinary_name_passes_through_unchanged() {
        let name = FileName::new("vacation-photo.jpg").unwrap();
        assert_eq!(sanitize_for_export(&name), "vacation-photo.jpg");
    }

    #[test]
    fn a_non_colliding_name_is_returned_as_is() {
        assert_eq!(
            resolve_export_collision("photo.jpg", &["other.jpg".to_string()]),
            "photo.jpg"
        );
    }

    #[test]
    fn a_colliding_name_gets_the_spec_s_own_worked_example_suffix() {
        assert_eq!(
            resolve_export_collision("photo.jpg", &["photo.jpg".to_string()]),
            "photo (1).jpg"
        );
    }

    #[test]
    fn repeated_collisions_increment_past_already_taken_suffixes() {
        let existing = vec![
            "photo.jpg".to_string(),
            "photo (1).jpg".to_string(),
            "photo (2).jpg".to_string(),
        ];
        assert_eq!(
            resolve_export_collision("photo.jpg", &existing),
            "photo (3).jpg"
        );
    }

    #[test]
    fn a_colliding_name_with_no_extension_still_gets_a_suffix() {
        assert_eq!(
            resolve_export_collision("README", &["README".to_string()]),
            "README (1)"
        );
    }
}

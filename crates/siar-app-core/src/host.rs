//! What this device calls itself in the device list.

use siar_ui_state::DeviceKind;

const MAX_LABEL_CHARS: usize = 64;
const FALLBACK: &str = "This device";

/// A display name for this device: the machine's host name if it has a usable
/// one, otherwise a plain fallback. Never empty, never long, never contains
/// control characters (the name is shown in a list and sent to attached
/// windows).
#[must_use]
pub fn device_label() -> String {
    let raw = std::env::var("HOSTNAME")
        .ok()
        .or_else(|| std::env::var("COMPUTERNAME").ok())
        .or_else(|| {
            std::fs::read_to_string("/etc/hostname")
                .ok()
                .map(|s| s.trim().to_owned())
        })
        .unwrap_or_default();
    clean_label(&raw)
}

fn clean_label(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_LABEL_CHARS)
        .collect();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        FALLBACK.to_owned()
    } else {
        cleaned.to_owned()
    }
}

#[must_use]
pub const fn device_kind() -> DeviceKind {
    if cfg!(target_os = "android") {
        DeviceKind::AndroidPhone
    } else {
        DeviceKind::Desktop
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_are_clean_short_and_never_empty() {
        assert_eq!(clean_label("arch-box\n"), "arch-box");
        assert_eq!(clean_label(""), FALLBACK);
        assert_eq!(clean_label(" \t\n "), FALLBACK);
        assert_eq!(clean_label("a\u{7}b\u{1b}[31mc"), "ab[31mc");
        assert_eq!(
            clean_label(&"x".repeat(500)).chars().count(),
            MAX_LABEL_CHARS
        );
        assert!(!device_label().is_empty());
    }
}

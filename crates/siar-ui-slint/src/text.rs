//! Pure presentation helpers: plain-language labels, stable avatar hues,
//! sizes and relative times. No Slint types, so everything here is
//! unit-testable without a window.
//!
//! ui-ux-15 §46 applies to every label: plain, non-technical language,
//! never a propagated error string or a raw type name.

use siar_domain::{DeliveryState, MediaType};
use siar_ui_state::{
    DeviceKind, DeviceSecurityFlag, DeviceTrustState, SecurityEventKind, SecurityEventSeverity,
};

/// Stable hue (0..360) from any string — FNV-1a, so the same person keeps
/// the same colour across launches and machines.
pub fn hue_for(s: &str) -> i32 {
    let mut h: u32 = 0x811c_9dc5;
    for b in s.bytes() {
        h ^= u32::from(b);
        h = h.wrapping_mul(0x0100_0193);
    }
    (h % 360) as i32
}

/// First letter/digit, upper-cased; `?` when there is none.
pub fn initial(s: &str) -> String {
    s.chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_else(|| "?".to_string())
}

/// Re-flows a space-separated number into lines of `per_line` groups so a
/// 60-digit safety number reads (and is compared) a few groups at a time.
pub fn wrap_groups(s: &str, per_line: usize) -> String {
    let groups: Vec<&str> = s.split_whitespace().collect();
    groups
        .chunks(per_line.max(1))
        .map(|c| c.join(" "))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn format_bytes(n: u64) -> String {
    const KIB: f64 = 1024.0;
    let f = n as f64;
    if f < KIB {
        format!("{n} B")
    } else if f < KIB * KIB {
        format!("{:.0} KB", f / KIB)
    } else if f < KIB * KIB * KIB {
        format!("{:.1} MB", f / (KIB * KIB))
    } else {
        format!("{:.1} GB", f / (KIB * KIB * KIB))
    }
}

pub fn media_label(media: MediaType, encrypted_bytes: u64) -> String {
    let what = match media {
        MediaType::ImagePng | MediaType::ImageJpeg | MediaType::ImageWebp => "Image",
        MediaType::AudioOpus => "Voice message",
        MediaType::VideoMp4 => "Video",
        MediaType::Other => "File",
    };
    format!("{what} · {}", format_bytes(encrypted_bytes))
}

/// Delivery state → (icon code, label). Icon codes match `MessageRow`'s
/// documented `status-icon` values in `app_state.slint`.
///
/// next.md §62: carried-by-peers is explicitly NOT delivery, so it gets
/// its own icon and wording and never reads like a double-check.
pub fn delivery(state: DeliveryState) -> (i32, String) {
    use DeliveryState::*;
    match state {
        Local => (1, "Saving…".into()),
        Queued | Sending => (1, "Sending…".into()),
        Sent => (2, "Sent".into()),
        CarriedByPeers { copies } => (
            6,
            if copies == 1 {
                "Carried by 1 nearby device — not delivered yet".into()
            } else {
                format!("Carried by {copies} nearby devices — not delivered yet")
            },
        ),
        Delivered => (3, "Delivered".into()),
        Read => (4, "Read".into()),
        Failed => (5, "Couldn’t send — retrying".into()),
        Expired => (5, "Expired before it could be delivered".into()),
    }
}

pub fn event_headline(kind: SecurityEventKind) -> &'static str {
    match kind {
        SecurityEventKind::DeviceLinked => "A new device was linked to your account.",
        SecurityEventKind::DeviceRevoked => "A device was removed from your account.",
        SecurityEventKind::DeviceLinkDenied => "A device linking attempt was denied.",
        SecurityEventKind::IdentityChanged => {
            "A contact’s identity has changed. Re-verify before continuing."
        }
        SecurityEventKind::VerificationFailed => "Identity verification failed.",
        SecurityEventKind::RecoveryConfigured => "Account recovery was set up.",
        SecurityEventKind::RecoveryChanged => "Your account recovery settings changed.",
        SecurityEventKind::BackupFailed => "A backup attempt failed.",
        SecurityEventKind::KeyRotation => "Your security keys were rotated.",
        SecurityEventKind::SuspiciousAuthorization => {
            "A suspicious authorization attempt was blocked."
        }
        SecurityEventKind::SecurityPolicyChanged => "Your organization’s security policy changed.",
        SecurityEventKind::KeyExpiryActionRequired => "A security key requires renewal action.",
    }
}

/// (severity code, label) — codes match `EventRow.severity`.
pub fn severity(sev: SecurityEventSeverity) -> (i32, &'static str) {
    match sev {
        SecurityEventSeverity::Info => (0, "Info"),
        SecurityEventSeverity::Warning => (1, "Warning"),
        SecurityEventSeverity::Critical => (2, "Critical"),
    }
}

/// (label, icon code) — codes match `DeviceRow.kind-icon`.
pub fn device_kind(kind: DeviceKind) -> (&'static str, i32) {
    match kind {
        DeviceKind::AndroidPhone => ("Android phone", 1),
        DeviceKind::AndroidTablet => ("Android tablet", 1),
        DeviceKind::Desktop => ("Desktop", 0),
        DeviceKind::Laptop => ("Laptop", 0),
        DeviceKind::ServerNode => ("Relay node", 2),
        DeviceKind::Unknown => ("Unknown device type", 3),
    }
}

/// (status code, label) — codes match `DeviceRow.status`.
pub fn trust(status: DeviceTrustState) -> (i32, &'static str) {
    match status {
        DeviceTrustState::Trusted => (0, "Trusted"),
        DeviceTrustState::Pending => (1, "Waiting for approval"),
        DeviceTrustState::Revoked => (2, "Removed"),
        DeviceTrustState::Compromised => (3, "Possibly compromised"),
        DeviceTrustState::Unknown => (4, "Unknown"),
    }
}

pub fn flag_text(flag: DeviceSecurityFlag) -> &'static str {
    match flag {
        DeviceSecurityFlag::SuspectedCompromise => {
            "Possible compromise — remove this device if you don’t recognise it."
        }
        DeviceSecurityFlag::PendingAuthorization => {
            "Not yet approved — it cannot read your messages."
        }
        DeviceSecurityFlag::UnreviewedSinceLinking => "Linked recently and not reviewed yet.",
    }
}

/// Relative, deliberately coarse ("5 minutes ago"): precise timestamps
/// on a device list add fingerprinting value without helping the user.
pub fn coarse_last_active(now_millis: u64, last: Option<u64>) -> String {
    let Some(last) = last else {
        return "Never active".into();
    };
    let secs = now_millis.saturating_sub(last) / 1000;
    match secs {
        0..=59 => "Active just now".into(),
        60..=3599 => {
            let m = secs / 60;
            format!("Active {m} minute{} ago", if m == 1 { "" } else { "s" })
        }
        3600..=86_399 => {
            let h = secs / 3600;
            format!("Active {h} hour{} ago", if h == 1 { "" } else { "s" })
        }
        _ => {
            let d = secs / 86_400;
            format!("Active {d} day{} ago", if d == 1 { "" } else { "s" })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hue_is_stable_and_in_range() {
        assert_eq!(hue_for("alice"), hue_for("alice"));
        assert_ne!(hue_for("alice"), hue_for("bob"));
        for s in ["", "a", "ünïcode", "0123456789abcdef"] {
            assert!((0..360).contains(&hue_for(s)));
        }
    }

    #[test]
    fn initial_skips_punctuation_and_handles_empty() {
        assert_eq!(initial("alice"), "A");
        assert_eq!(initial("  ~bob"), "B");
        assert_eq!(initial(""), "?");
        assert_eq!(initial("!!!"), "?");
        assert_eq!(initial("élan"), "É");
    }

    #[test]
    fn wrap_groups_breaks_into_even_lines() {
        let n = "11111 22222 33333 44444 55555 66666 77777";
        assert_eq!(
            wrap_groups(n, 3),
            "11111 22222 33333\n44444 55555 66666\n77777"
        );
        assert_eq!(wrap_groups("", 3), "");
        assert_eq!(wrap_groups("a b", 0), "a\nb"); // 0 is clamped, not a panic
    }

    #[test]
    fn bytes_format_at_each_scale() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1023), "1023 B");
        assert_eq!(format_bytes(2048), "2 KB");
        assert_eq!(format_bytes(5 * 1024 * 1024), "5.0 MB");
    }

    #[test]
    fn carried_is_never_worded_or_iconned_as_delivered() {
        let (icon, label) = delivery(DeliveryState::CarriedByPeers { copies: 3 });
        assert_eq!(icon, 6);
        assert!(label.contains("not delivered"));
        assert_ne!(icon, delivery(DeliveryState::Delivered).0);
        assert!(delivery(DeliveryState::CarriedByPeers { copies: 1 })
            .1
            .contains("1 nearby device "));
    }

    #[test]
    fn failed_and_expired_use_the_error_icon() {
        assert_eq!(delivery(DeliveryState::Failed).0, 5);
        assert_eq!(delivery(DeliveryState::Expired).0, 5);
    }

    #[test]
    fn last_active_is_coarse_and_pluralises() {
        let now = 10 * 86_400_000;
        assert_eq!(coarse_last_active(now, None), "Never active");
        assert_eq!(
            coarse_last_active(now, Some(now - 5_000)),
            "Active just now"
        );
        assert_eq!(
            coarse_last_active(now, Some(now - 60_000)),
            "Active 1 minute ago"
        );
        assert_eq!(
            coarse_last_active(now, Some(now - 3 * 3_600_000)),
            "Active 3 hours ago"
        );
        assert_eq!(
            coarse_last_active(now, Some(now - 2 * 86_400_000)),
            "Active 2 days ago"
        );
        // a clock that went backwards must not underflow
        assert_eq!(coarse_last_active(5, Some(10)), "Active just now");
    }

    #[test]
    fn every_headline_is_plain_language() {
        use SecurityEventKind::*;
        for k in [
            DeviceLinked,
            DeviceRevoked,
            DeviceLinkDenied,
            IdentityChanged,
            VerificationFailed,
            RecoveryConfigured,
            RecoveryChanged,
            BackupFailed,
            KeyRotation,
            SuspiciousAuthorization,
            SecurityPolicyChanged,
            KeyExpiryActionRequired,
        ] {
            let h = event_headline(k);
            assert!(h.ends_with('.'));
            assert!(!h.contains("Error") && !h.contains("0x"));
        }
    }
}

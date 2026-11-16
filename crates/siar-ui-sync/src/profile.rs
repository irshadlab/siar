//! Own identity, diagnostics, devices and attachment previews on the wire.
//!
//! All four are **daemon-owned**: the core derives them, the window only
//! shows them. As elsewhere in this crate the wire carries facts in explicit
//! DTOs (never the UI structs), and the window re-derives anything
//! presentational (e.g. the key-health sentence) from the enum.
//!
//! Attachment previews carry JPEG bytes, so they are bounded twice: one
//! preview per section (a section's own size limit applies) and at most
//! [`MAX_PREVIEWS`] synced at once. A preview over [`MAX_PREVIEW_BYTES`] is
//! sent as a failure with an honest reason instead of being truncated (a cut
//! JPEG would render as garbage).

use serde::{Deserialize, Serialize};
use siar_domain::{AccountId, DeviceId};
use siar_ui_state::{
    AttachmentPreview, DeviceKind, DeviceSecurityFlag, DeviceSecurityView, DeviceTrustState,
    IdentityKeyHealth, OwnIdentityView, SecurityDiagnosticDetail,
};

pub const MAX_PREVIEWS: usize = 64;
pub const MAX_PREVIEW_BYTES: usize = 128 * 1024;
const MAX_TEXT: usize = 1024;
const MAX_DEVICES: usize = 256;

fn clip(s: &str) -> String {
    s.chars().take(MAX_TEXT).collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum KeyHealthDto {
    Healthy,
    RotationRecommended,
    RecoveryRequired,
    Compromised,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnIdentityDto {
    pub account: AccountId,
    pub short_fingerprint: String,
    pub full_fingerprint: String,
    pub created_at_millis: u64,
    pub key_health: KeyHealthDto,
}

impl From<&OwnIdentityView> for OwnIdentityDto {
    fn from(v: &OwnIdentityView) -> Self {
        Self {
            account: v.account,
            short_fingerprint: clip(&v.short_fingerprint),
            full_fingerprint: clip(&v.full_fingerprint),
            created_at_millis: v.created_at_millis,
            key_health: match v.key_health {
                IdentityKeyHealth::Healthy => KeyHealthDto::Healthy,
                IdentityKeyHealth::RotationRecommended => KeyHealthDto::RotationRecommended,
                IdentityKeyHealth::RecoveryRequired => KeyHealthDto::RecoveryRequired,
                IdentityKeyHealth::Compromised => KeyHealthDto::Compromised,
            },
        }
    }
}

impl From<&OwnIdentityDto> for OwnIdentityView {
    fn from(d: &OwnIdentityDto) -> Self {
        Self {
            account: d.account,
            short_fingerprint: d.short_fingerprint.clone(),
            full_fingerprint: d.full_fingerprint.clone(),
            created_at_millis: d.created_at_millis,
            key_health: match d.key_health {
                KeyHealthDto::Healthy => IdentityKeyHealth::Healthy,
                KeyHealthDto::RotationRecommended => IdentityKeyHealth::RotationRecommended,
                KeyHealthDto::RecoveryRequired => IdentityKeyHealth::RecoveryRequired,
                KeyHealthDto::Compromised => IdentityKeyHealth::Compromised,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticsDto {
    pub key_id: String,
    pub epoch_id: String,
    pub signature_status: String,
    pub transport_security_details: String,
}

impl From<&SecurityDiagnosticDetail> for DiagnosticsDto {
    fn from(d: &SecurityDiagnosticDetail) -> Self {
        Self {
            key_id: clip(&d.key_id),
            epoch_id: clip(&d.epoch_id),
            signature_status: clip(&d.signature_status),
            transport_security_details: clip(&d.transport_security_details),
        }
    }
}

impl From<&DiagnosticsDto> for SecurityDiagnosticDetail {
    fn from(d: &DiagnosticsDto) -> Self {
        Self {
            key_id: d.key_id.clone(),
            epoch_id: d.epoch_id.clone(),
            signature_status: d.signature_status.clone(),
            transport_security_details: d.transport_security_details.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeviceKindDto {
    AndroidPhone,
    AndroidTablet,
    Desktop,
    Laptop,
    ServerNode,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeviceTrustDto {
    Trusted,
    Pending,
    Revoked,
    Compromised,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeviceFlagDto {
    SuspectedCompromise,
    PendingAuthorization,
    UnreviewedSinceLinking,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceDto {
    pub id: DeviceId,
    pub display_name: String,
    pub kind: DeviceKindDto,
    pub status: DeviceTrustDto,
    pub added_at_millis: u64,
    pub last_active_millis: Option<u64>,
    pub current_device: bool,
    pub flags: Vec<DeviceFlagDto>,
}

impl From<&DeviceSecurityView> for DeviceDto {
    fn from(v: &DeviceSecurityView) -> Self {
        Self {
            id: v.id,
            display_name: clip(&v.display_name),
            kind: match v.kind {
                DeviceKind::AndroidPhone => DeviceKindDto::AndroidPhone,
                DeviceKind::AndroidTablet => DeviceKindDto::AndroidTablet,
                DeviceKind::Desktop => DeviceKindDto::Desktop,
                DeviceKind::Laptop => DeviceKindDto::Laptop,
                DeviceKind::ServerNode => DeviceKindDto::ServerNode,
                DeviceKind::Unknown => DeviceKindDto::Unknown,
            },
            status: match v.status {
                DeviceTrustState::Trusted => DeviceTrustDto::Trusted,
                DeviceTrustState::Pending => DeviceTrustDto::Pending,
                DeviceTrustState::Revoked => DeviceTrustDto::Revoked,
                DeviceTrustState::Compromised => DeviceTrustDto::Compromised,
                DeviceTrustState::Unknown => DeviceTrustDto::Unknown,
            },
            added_at_millis: v.added_at_millis,
            last_active_millis: v.last_active_millis,
            current_device: v.current_device,
            flags: v
                .security_flags
                .iter()
                .map(|f| match f {
                    DeviceSecurityFlag::SuspectedCompromise => DeviceFlagDto::SuspectedCompromise,
                    DeviceSecurityFlag::PendingAuthorization => DeviceFlagDto::PendingAuthorization,
                    DeviceSecurityFlag::UnreviewedSinceLinking => {
                        DeviceFlagDto::UnreviewedSinceLinking
                    }
                })
                .collect(),
        }
    }
}

impl From<&DeviceDto> for DeviceSecurityView {
    fn from(d: &DeviceDto) -> Self {
        Self {
            id: d.id,
            display_name: d.display_name.clone(),
            kind: match d.kind {
                DeviceKindDto::AndroidPhone => DeviceKind::AndroidPhone,
                DeviceKindDto::AndroidTablet => DeviceKind::AndroidTablet,
                DeviceKindDto::Desktop => DeviceKind::Desktop,
                DeviceKindDto::Laptop => DeviceKind::Laptop,
                DeviceKindDto::ServerNode => DeviceKind::ServerNode,
                DeviceKindDto::Unknown => DeviceKind::Unknown,
            },
            status: match d.status {
                DeviceTrustDto::Trusted => DeviceTrustState::Trusted,
                DeviceTrustDto::Pending => DeviceTrustState::Pending,
                DeviceTrustDto::Revoked => DeviceTrustState::Revoked,
                DeviceTrustDto::Compromised => DeviceTrustState::Compromised,
                DeviceTrustDto::Unknown => DeviceTrustState::Unknown,
            },
            added_at_millis: d.added_at_millis,
            last_active_millis: d.last_active_millis,
            current_device: d.current_device,
            security_flags: d
                .flags
                .iter()
                .map(|f| match f {
                    DeviceFlagDto::SuspectedCompromise => DeviceSecurityFlag::SuspectedCompromise,
                    DeviceFlagDto::PendingAuthorization => DeviceSecurityFlag::PendingAuthorization,
                    DeviceFlagDto::UnreviewedSinceLinking => {
                        DeviceSecurityFlag::UnreviewedSinceLinking
                    }
                })
                .collect(),
        }
    }
}

/// At most [`MAX_DEVICES`], stable order, one `this device` at most.
#[must_use]
pub fn devices_for_wire(all: &[DeviceSecurityView]) -> Vec<DeviceDto> {
    let mut out: Vec<DeviceDto> = all.iter().map(DeviceDto::from).collect();
    out.sort_by_key(|d| d.id.as_uuid().as_u128());
    out.truncate(MAX_DEVICES);
    out
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PreviewDto {
    Loading,
    Ready {
        jpeg_bytes: Vec<u8>,
        width: u32,
        height: u32,
    },
    Failed {
        reason: String,
    },
}

/// `None` for a message nobody asked a preview for.
#[must_use]
pub fn preview_for_wire(p: &AttachmentPreview) -> Option<PreviewDto> {
    Some(match p {
        AttachmentPreview::NotRequested => return None,
        AttachmentPreview::Loading => PreviewDto::Loading,
        AttachmentPreview::Ready {
            jpeg_bytes,
            width,
            height,
        } => {
            if jpeg_bytes.len() > MAX_PREVIEW_BYTES {
                PreviewDto::Failed {
                    reason: "This preview is too large to show here.".into(),
                }
            } else {
                PreviewDto::Ready {
                    jpeg_bytes: jpeg_bytes.clone(),
                    width: *width,
                    height: *height,
                }
            }
        }
        AttachmentPreview::Failed { reason } => PreviewDto::Failed {
            reason: clip(reason),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_and_diagnostics_round_trip() {
        let v = OwnIdentityView {
            account: AccountId::new(),
            short_fingerprint: "ab12".into(),
            full_fingerprint: "ab12cd34".into(),
            created_at_millis: 7,
            key_health: IdentityKeyHealth::RecoveryRequired,
        };
        assert_eq!(OwnIdentityView::from(&OwnIdentityDto::from(&v)), v);
        let d = SecurityDiagnosticDetail {
            key_id: "k".into(),
            epoch_id: "e".into(),
            signature_status: "s".into(),
            transport_security_details: "t".into(),
        };
        assert_eq!(SecurityDiagnosticDetail::from(&DiagnosticsDto::from(&d)), d);
    }

    #[test]
    fn every_device_enum_value_survives_the_round_trip() {
        for kind in [
            DeviceKind::AndroidPhone,
            DeviceKind::AndroidTablet,
            DeviceKind::Desktop,
            DeviceKind::Laptop,
            DeviceKind::ServerNode,
            DeviceKind::Unknown,
        ] {
            for status in [
                DeviceTrustState::Trusted,
                DeviceTrustState::Pending,
                DeviceTrustState::Revoked,
                DeviceTrustState::Compromised,
                DeviceTrustState::Unknown,
            ] {
                let v = DeviceSecurityView {
                    id: DeviceId::new(),
                    display_name: "laptop".into(),
                    kind,
                    status,
                    added_at_millis: 1,
                    last_active_millis: Some(2),
                    current_device: false,
                    security_flags: vec![
                        DeviceSecurityFlag::SuspectedCompromise,
                        DeviceSecurityFlag::UnreviewedSinceLinking,
                    ],
                };
                assert_eq!(DeviceSecurityView::from(&DeviceDto::from(&v)), v);
            }
        }
    }

    #[test]
    fn an_oversize_preview_becomes_an_honest_failure_not_a_cut_jpeg() {
        let big = AttachmentPreview::Ready {
            jpeg_bytes: vec![1; MAX_PREVIEW_BYTES + 1],
            width: 10,
            height: 10,
        };
        assert!(matches!(
            preview_for_wire(&big),
            Some(PreviewDto::Failed { .. })
        ));
        let ok = AttachmentPreview::Ready {
            jpeg_bytes: vec![1; MAX_PREVIEW_BYTES],
            width: 10,
            height: 10,
        };
        assert!(matches!(
            preview_for_wire(&ok),
            Some(PreviewDto::Ready { .. })
        ));
        assert_eq!(preview_for_wire(&AttachmentPreview::NotRequested), None);
        assert!(
            crate::wire::encode(&preview_for_wire(&ok).unwrap()).len()
                < crate::wire::MAX_SECTION_BYTES
        );
    }

    #[test]
    fn hostile_long_text_is_clipped() {
        let d = SecurityDiagnosticDetail {
            key_id: "k".repeat(50_000),
            epoch_id: String::new(),
            signature_status: String::new(),
            transport_security_details: String::new(),
        };
        assert_eq!(DiagnosticsDto::from(&d).key_id.chars().count(), MAX_TEXT);
    }
}

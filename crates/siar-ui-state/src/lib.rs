#![forbid(unsafe_code)]

//! siar-ui-state: the view-model layer between Dioxus and `siar-messaging`
//! (plan.md §52–55).
//!
//! Deliberately has zero Dioxus dependency. Everything here is plain
//! Rust structs/enums that a Dioxus signal can wrap — which means this
//! whole crate is unit-testable without a UI toolkit or a system webview,
//! unlike `apps/desktop` (which needs both and can't run headless in any
//! sandbox, not just this one).
//!
//! Rule (plan.md §86): the dependency direction is
//! `Dioxus components -> ViewModels (this crate) -> AppCommand/AppEvent
//! -> MessagingCore`. This crate depends on `siar-domain` only — never on
//! `siar-messaging`, `siar-storage`, or `siar-transport` directly, so it
//! stays reusable if a future frontend replaces Dioxus.

mod accessibility;
mod attachment_preview;
mod command;
mod composer;
mod contact_list;
mod conversation_list;
mod device_lifecycle;
mod diagnostics;
mod empty_states;
mod group_list;
mod identity_verification;
mod network;
mod presentation_api;
mod privacy_and_lock;
mod recovery;
mod recovery_advanced;
mod revocation_honesty;
mod revocation_lifecycle;
mod security_center;
mod security_event;
mod security_status_banner;
mod timeline;
mod ui_effects;

pub use accessibility::{
    device_row_accessibility_label, security_event_severity_icon_name, security_health_icon_name,
    security_overview_accessibility_summary, ConfirmationFriction, DestructiveAction,
    HighRiskConfirmationCopy,
};
pub use attachment_preview::{AttachmentPreview, AttachmentPreviewState};
pub use command::{AppCommand, AppEvent};
pub use composer::ComposerState;
pub use contact_list::{ContactListState, SavedContact};
pub use conversation_list::{ConversationKind, ConversationListState, ConversationSummary};
pub use device_lifecycle::{
    is_last_trusted_device, CompromiseResponseChecklist, CompromiseResponseStep,
    DeviceLossFlowState, DeviceLossFlowStep, DeviceLossKind, ReauthPurpose, ReauthResult,
};
pub use diagnostics::SecurityDiagnosticDetail;
pub use empty_states::{
    device_activity_display, device_display_name, effective_security_health,
    lost_device_revocation_confirmation, trusted_device_count_label, DeviceActivityDisplay,
    BACKUP_MISSING_CTA, BACKUP_MISSING_LABEL, RECOVERY_NOT_CONFIGURED_CTA,
    SECURITY_EVENT_LIST_EMPTY_LABEL,
};
pub use group_list::{
    AddMemberInput, GroupListState, GroupSummary, PendingInvite, PendingInviteState,
};
pub use identity_verification::{
    ContactVerificationIssue, ContactVerificationIssueKind, ContactVerificationSummary,
    IdentityKeyHealth, IdentityResetConfirmation, OwnIdentityView,
};
pub use network::{NetworkState, NetworkStatus, PeerReachability};
pub use presentation_api::{
    DeviceSecurityCapabilities, DeviceSecurityPresentation, ReauthProof, RecoveryCapabilities,
    RecoveryMaterialView, RecoveryPresentation, RecoveryVerificationInput,
    RecoveryVerificationResult, RevocationResultView, SecurityCenterSnapshot, SecurityEventCursor,
    SecurityEventPage, SecurityPresentation, UiError,
};
pub use privacy_and_lock::{
    app_unlock_satisfies_reauth, AdvancedKeyExportGate, AppLockMethod, AppLockSettings,
    AppLockTimeout, PrivacyControlsSummary, ScreenPrivacySettings,
};
pub use recovery::{
    RecoveryKeyDisplayState, RecoveryMethod, RecoveryOverview, RecoveryRotationFlow,
    RecoveryRotationStep, RecoveryStatus,
};
pub use recovery_advanced::{
    BackupSecurityState, LostAllDevicesFlow, LostAllDevicesMethod, LostAllDevicesStep,
    PassphraseAssessment, PassphraseStrengthLevel, RecoveryAndBackupOverview, RecoveryDrillState,
    RecoveryDrillStep, RecoveryQrState,
};
pub use revocation_honesty::{RecoveryScope, RevocationCapabilities, REVOCATION_DATA_DISCLAIMER};
pub use revocation_lifecycle::{
    lost_device_action_label, LocalWipeConfirmation, RevocationProgress, RevocationState,
    WipeCapability,
};
pub use security_center::{
    DeviceKind, DeviceListState, DeviceSecurityFlag, DeviceSecurityView, DeviceTrustState,
    SecurityHealth, SecurityOverview,
};
pub use security_event::{
    SecurityEvent, SecurityEventAction, SecurityEventCategory, SecurityEventFilter,
    SecurityEventId, SecurityEventKind, SecurityEventSeverity, SecurityEventState,
};
pub use security_status_banner::SecurityStatusBanner;
pub use timeline::{TimelineState, TimelineWindow, PAGE_SIZE};
pub use ui_effects::{
    fake_recovery_material_for_screenshot_tests, should_offer_clipboard_clear, ReauthChallenge,
    ReauthChallengeId, RecoveryExportFormat, ReleaseReason, SecurityUiEvent,
    SensitiveRecoveryMaterialHandle, SensitiveUiEffect, CLIPBOARD_CLEAR_OFFER_DELAY_MILLIS,
};

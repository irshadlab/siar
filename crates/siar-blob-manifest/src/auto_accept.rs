//! §45 "Auto-Accept Policy".
//!
//! §45's own text: "Applications can define [a policy]. File subsystem
//! enforces policy supplied by application." This crate has no
//! `Contacts` concept, no network-state query, no idea what "on
//! Wi-Fi" means at runtime — all of that belongs to whatever
//! application supplies the policy. What this module owns is the
//! *enforcement*: [`evaluate_auto_accept`] is a pure function from a
//! policy plus a caller-supplied [`AutoAcceptContext`] (the facts an
//! application already knows) to a decision, so the AND-of-conditions
//! logic itself is written once and tested here, instead of
//! re-implemented at every call site.

use serde::{Deserialize, Serialize};
use siar_domain::MediaType;

use crate::file_offer::FileOfferDecision;

/// §45's own five named policy dimensions, field-for-field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AutoAcceptPolicy {
    pub contacts_only: bool,
    pub max_file_size: Option<u64>,
    pub wifi_only: bool,
    pub allowed_media_types: Option<Vec<MediaType>>,
    pub emergency_thumbnails_allowed: bool,
}

/// The facts an application already has and this crate can't derive on
/// its own — see this module's own top doc comment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutoAcceptContext {
    pub sender_is_contact: bool,
    pub on_wifi: bool,
    pub file_size: u64,
    pub media_type: Option<MediaType>,
    pub is_emergency_thumbnail: bool,
}

/// `None` means "this policy has no automatic answer — fall through to
/// a real (manual) decision," not "reject." Only `Some(Accept)` is
/// ever returned: an *auto-accept* policy has nothing useful to say
/// about auto-rejecting, auto-deferring, or auto-metadata-only — those
/// remain [`FileOfferDecision`]'s other three variants, made by
/// whatever decides when this function doesn't.
///
/// Emergency thumbnails are a named bypass, not one more AND condition
/// — §45 lists them as their own policy dimension precisely because an
/// application enabling `emergency_thumbnails_allowed` wants urgent,
/// small previews through even under an otherwise strict policy (say,
/// `contacts_only` with a stranger's SOS report).
pub fn evaluate_auto_accept(
    policy: &AutoAcceptPolicy,
    context: &AutoAcceptContext,
) -> Option<FileOfferDecision> {
    if context.is_emergency_thumbnail && policy.emergency_thumbnails_allowed {
        return Some(FileOfferDecision::Accept);
    }

    if policy.contacts_only && !context.sender_is_contact {
        return None;
    }
    if let Some(max_size) = policy.max_file_size {
        if context.file_size > max_size {
            return None;
        }
    }
    if policy.wifi_only && !context.on_wifi {
        return None;
    }
    if let Some(allowed) = &policy.allowed_media_types {
        match context.media_type {
            Some(media_type) if allowed.contains(&media_type) => {}
            _ => return None,
        }
    }

    Some(FileOfferDecision::Accept)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn permissive_policy() -> AutoAcceptPolicy {
        AutoAcceptPolicy {
            contacts_only: false,
            max_file_size: None,
            wifi_only: false,
            allowed_media_types: None,
            emergency_thumbnails_allowed: true,
        }
    }

    fn baseline_context() -> AutoAcceptContext {
        AutoAcceptContext {
            sender_is_contact: true,
            on_wifi: true,
            file_size: 1_000,
            media_type: Some(MediaType::ImageJpeg),
            is_emergency_thumbnail: false,
        }
    }

    #[test]
    fn a_fully_permissive_policy_accepts_a_normal_file() {
        assert_eq!(
            evaluate_auto_accept(&permissive_policy(), &baseline_context()),
            Some(FileOfferDecision::Accept)
        );
    }

    #[test]
    fn contacts_only_blocks_a_stranger() {
        let policy = AutoAcceptPolicy {
            contacts_only: true,
            ..permissive_policy()
        };
        let context = AutoAcceptContext {
            sender_is_contact: false,
            ..baseline_context()
        };
        assert_eq!(evaluate_auto_accept(&policy, &context), None);
    }

    #[test]
    fn a_file_over_the_size_limit_is_not_auto_accepted() {
        let policy = AutoAcceptPolicy {
            max_file_size: Some(500),
            ..permissive_policy()
        };
        assert_eq!(evaluate_auto_accept(&policy, &baseline_context()), None);
    }

    #[test]
    fn wifi_only_blocks_a_cellular_context() {
        let policy = AutoAcceptPolicy {
            wifi_only: true,
            ..permissive_policy()
        };
        let context = AutoAcceptContext {
            on_wifi: false,
            ..baseline_context()
        };
        assert_eq!(evaluate_auto_accept(&policy, &context), None);
    }

    #[test]
    fn an_unlisted_media_type_is_not_auto_accepted() {
        let policy = AutoAcceptPolicy {
            allowed_media_types: Some(vec![MediaType::ImagePng]),
            ..permissive_policy()
        };
        assert_eq!(evaluate_auto_accept(&policy, &baseline_context()), None);
    }

    #[test]
    fn an_emergency_thumbnail_bypasses_an_otherwise_strict_policy() {
        let policy = AutoAcceptPolicy {
            contacts_only: true,
            wifi_only: true,
            emergency_thumbnails_allowed: true,
            ..permissive_policy()
        };
        let context = AutoAcceptContext {
            sender_is_contact: false,
            on_wifi: false,
            is_emergency_thumbnail: true,
            ..baseline_context()
        };
        assert_eq!(
            evaluate_auto_accept(&policy, &context),
            Some(FileOfferDecision::Accept)
        );
    }

    #[test]
    fn an_emergency_thumbnail_is_not_exempt_when_the_policy_disallows_it() {
        let policy = AutoAcceptPolicy {
            emergency_thumbnails_allowed: false,
            contacts_only: true,
            ..permissive_policy()
        };
        let context = AutoAcceptContext {
            sender_is_contact: false,
            is_emergency_thumbnail: true,
            ..baseline_context()
        };
        assert_eq!(evaluate_auto_accept(&policy, &context), None);
    }
}

//! Conversion between the in-memory security log (`siar-ui-state`) and its
//! stored rows (`siar-storage`). Kept apart so the rules — skip rows this
//! build can't read, never trust stored text — are testable without a
//! database or a window.

use siar_domain::{AccountId, DeviceId};
use siar_storage::StoredSecurityEvent;
use siar_ui_state::{SecurityEvent, SecurityEventAction, SecurityEventId, SecurityEventKind};

pub fn to_stored(e: &SecurityEvent) -> StoredSecurityEvent {
    StoredSecurityEvent {
        event_id: e.id.get(),
        kind: e.kind.as_str().to_string(),
        occurred_at_millis: e.occurred_at_millis,
        resolved: e.resolved,
        related_device: e.related_device.map(|d| d.as_uuid().to_string()),
        related_account: e.related_contact.map(|a| a.as_uuid().to_string()),
        actions: e
            .actions
            .iter()
            .map(|a| a.as_str())
            .collect::<Vec<_>>()
            .join(","),
    }
}

/// `None` when the row can't be understood by this build (a kind written by
/// a newer version, say). The caller drops it: a log that loads minus one
/// unknown entry beats an app that won't start.
pub fn from_stored(s: &StoredSecurityEvent) -> Option<SecurityEvent> {
    let kind = SecurityEventKind::parse(&s.kind)?;
    Some(SecurityEvent {
        id: SecurityEventId::from_raw(s.event_id),
        kind,
        severity: kind.severity(), // derived, never stored (a caller can't assert it)
        occurred_at_millis: s.occurred_at_millis,
        resolved: s.resolved,
        related_device: s
            .related_device
            .as_deref()
            .and_then(|d| d.parse::<uuid::Uuid>().ok())
            .map(DeviceId::from_uuid),
        related_contact: s
            .related_account
            .as_deref()
            .and_then(|a| a.parse::<uuid::Uuid>().ok())
            .map(AccountId::from_uuid),
        actions: s
            .actions
            .split(',')
            .filter_map(SecurityEventAction::parse)
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use siar_ui_state::SecurityEventState;

    #[test]
    fn every_kind_round_trips_with_all_fields() {
        let mut state = SecurityEventState::new();
        for (i, kind) in SecurityEventKind::ALL.into_iter().enumerate() {
            state.push(
                kind,
                1_000 + i as u64,
                Some(DeviceId::new()),
                Some(AccountId::new()),
                vec![SecurityEventAction::Review, SecurityEventAction::Dismiss],
            );
        }
        let first = state.events().last().map(|e| e.id).unwrap();
        state.resolve(first);
        for e in state.events() {
            let back = from_stored(&to_stored(e)).expect("known kind");
            assert_eq!(&back, e);
        }
    }

    #[test]
    fn an_unknown_kind_is_skipped_not_fatal() {
        let row = StoredSecurityEvent {
            event_id: 1,
            kind: "invented_by_a_future_version".into(),
            occurred_at_millis: 1,
            resolved: false,
            related_device: None,
            related_account: None,
            actions: String::new(),
        };
        assert!(from_stored(&row).is_none());
    }

    #[test]
    fn malformed_ids_and_actions_degrade_without_failing() {
        let row = StoredSecurityEvent {
            event_id: 3,
            kind: "key_rotation".into(),
            occurred_at_millis: 5,
            resolved: true,
            related_device: Some("not-a-uuid".into()),
            related_account: Some("".into()),
            actions: "review,bogus,,dismiss".into(),
        };
        let e = from_stored(&row).expect("the kind is known");
        assert_eq!(e.related_device, None);
        assert_eq!(e.related_contact, None);
        assert_eq!(
            e.actions,
            vec![SecurityEventAction::Review, SecurityEventAction::Dismiss]
        );
        assert!(e.resolved);
    }

    #[test]
    fn severity_comes_from_the_kind_not_from_storage() {
        let row = StoredSecurityEvent {
            event_id: 1,
            kind: "identity_changed".into(),
            occurred_at_millis: 1,
            resolved: false,
            related_device: None,
            related_account: None,
            actions: String::new(),
        };
        assert_eq!(
            from_stored(&row).unwrap().severity,
            siar_ui_state::SecurityEventSeverity::Critical
        );
    }
}

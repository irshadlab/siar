//! §151-§158: desktop notifications from a headless daemon.
//!
//! With no window attached, nobody sees an incoming message or a key-change
//! warning — unless the daemon says so. This module is split so the part that
//! *decides* is pure and testable without a desktop:
//!
//! * [`NotificationEngine`] — looks at synced state ([`Slices`]) and says
//!   what, if anything, to announce. No I/O, no clock of its own.
//! * [`Sink`] / [`DesktopSink`] — shows it (`notify-rust`: D-Bus via the
//!   pure-Rust `zbus` on Linux, toasts on Windows, Notification Center on
//!   macOS). A failure to show (no session bus, headless server) is logged
//!   once and never fatal.
//!
//! Privacy defaults: the notification shows **no sender and no message text**
//! ([`Detail::Private`]); previews are an explicit opt-in. Security
//! notifications use fixed wording that never names a contact. Whatever is
//! on screen can be read by anyone looking at the lock screen.
//!
//! Quiet when watched: unless told otherwise, nothing is shown while a window
//! is attached (the window already shows it), except **Critical** security
//! events, which always are.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};

use siar_domain::ConversationId;
use siar_ui_model::Slices;
use siar_ui_state::SecurityEventSeverity;

/// How much a notification reveals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Detail {
    /// "New message" — nothing about who or what.
    Private,
    /// Conversation name and a short preview of the last message.
    Preview,
}

#[derive(Debug, Clone, Copy)]
pub struct NotifyPolicy {
    pub enabled: bool,
    pub detail: Detail,
    /// Don't announce what an attached window is already showing.
    pub only_when_unattended: bool,
    /// At most one message notification per conversation per this many ms.
    pub cooldown_ms: u64,
}

impl Default for NotifyPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            detail: Detail::Private,
            only_when_unattended: true,
            cooldown_ms: 30_000,
        }
    }
}

impl NotifyPolicy {
    /// `SIAR_NOTIFICATIONS`: `off`, `private` (default) or `preview`.
    #[must_use]
    pub fn from_env_value(v: Option<&str>) -> Self {
        let mut p = Self::default();
        match v.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
            Some("off" | "0" | "false" | "no") => p.enabled = false,
            Some("preview") => p.detail = Detail::Preview,
            _ => {}
        }
        p
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    pub title: String,
    pub body: String,
    /// Shown even when a window is attached; the desktop may keep it on
    /// screen longer.
    pub urgent: bool,
}

#[derive(Debug)]
pub struct NotificationEngine {
    policy: NotifyPolicy,
    /// `None` until the first observation: whatever was already unread, or
    /// already flagged, when the daemon started is not news.
    baseline: Option<Baseline>,
    last_notified_ms: HashMap<ConversationId, u64>,
}

#[derive(Debug, Default)]
struct Baseline {
    unread: HashMap<ConversationId, u32>,
    events_seen: HashSet<u64>,
}

const PREVIEW_CHARS: usize = 120;

impl NotificationEngine {
    #[must_use]
    pub fn new(policy: NotifyPolicy) -> Self {
        Self {
            policy,
            baseline: None,
            last_notified_ms: HashMap::new(),
        }
    }

    /// Observe the latest state. `attached` is the number of connected
    /// windows/subscribers; `now_ms` is any monotone-enough millisecond clock.
    pub fn observe(&mut self, s: &Slices, attached: usize, now_ms: u64) -> Vec<Notification> {
        let mut now_unread = HashMap::new();
        for c in s.conversations.ordered() {
            now_unread.insert(c.id, c.unread_count);
        }
        let now_events: HashSet<u64> = s
            .security_events
            .events()
            .iter()
            .filter(|e| !e.resolved)
            .map(|e| e.id.get())
            .collect();

        let Some(base) = self.baseline.take() else {
            self.baseline = Some(Baseline {
                unread: now_unread,
                events_seen: now_events,
            });
            return Vec::new();
        };

        let mut out = Vec::new();
        if self.policy.enabled {
            let watched = self.policy.only_when_unattended && attached > 0;
            if !watched {
                for c in s.conversations.ordered() {
                    let before = base.unread.get(&c.id).copied().unwrap_or(0);
                    if c.unread_count <= before {
                        continue;
                    }
                    let last = self.last_notified_ms.get(&c.id).copied();
                    if last.is_some_and(|t| now_ms.saturating_sub(t) < self.policy.cooldown_ms) {
                        continue; // counted as seen below: a burst is one ping
                    }
                    self.last_notified_ms.insert(c.id, now_ms);
                    out.push(self.message_notification(c));
                }
            }
            for e in s.security_events.events() {
                if e.resolved || base.events_seen.contains(&e.id.get()) {
                    continue;
                }
                if e.severity == SecurityEventSeverity::Info {
                    continue; // routine (e.g. key rotation): not alarming
                }
                let critical = e.severity == SecurityEventSeverity::Critical;
                if watched_by_window(self.policy, attached) && !critical {
                    continue;
                }
                out.push(Notification {
                    title: "Siar security alert".into(),
                    body: if critical {
                        "A security setting needs your attention.".into()
                    } else {
                        "Siar noticed a security change. Open Siar to review it.".into()
                    },
                    urgent: critical,
                });
            }
        }

        self.baseline = Some(Baseline {
            unread: now_unread,
            events_seen: now_events,
        });
        out
    }

    fn message_notification(&self, c: &siar_ui_state::ConversationSummary) -> Notification {
        match self.policy.detail {
            Detail::Private => Notification {
                title: "Siar".into(),
                body: "New message".into(),
                urgent: false,
            },
            Detail::Preview => Notification {
                title: c.display_name.chars().take(60).collect(),
                body: c
                    .last_message_preview
                    .as_deref()
                    .unwrap_or("New message")
                    .chars()
                    .take(PREVIEW_CHARS)
                    .collect(),
                urgent: false,
            },
        }
    }
}

fn watched_by_window(p: NotifyPolicy, attached: usize) -> bool {
    p.only_when_unattended && attached > 0
}

/// Where notifications go.
pub trait Sink: Send + Sync {
    fn show(&self, n: &Notification);
}

/// The OS notification service.
#[derive(Debug, Default)]
pub struct DesktopSink {
    warned: AtomicBool,
}

impl Sink for DesktopSink {
    fn show(&self, n: &Notification) {
        let mut note = notify_rust::Notification::new();
        note.appname("Siar").summary(&n.title).body(&n.body);
        if n.urgent {
            #[cfg(all(unix, not(target_os = "macos")))]
            note.urgency(notify_rust::Urgency::Critical);
        }
        if let Err(e) = note.show() {
            // Headless server, no session bus, notifications disabled…: say so
            // once, then stay quiet.
            if !self.warned.swap(true, Ordering::Relaxed) {
                tracing::warn!(error = %e, "desktop notifications unavailable");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use siar_ui_state::{
        ConversationKind, ConversationSummary, SecurityEventAction, SecurityEventKind,
    };

    fn conv(s: &mut Slices, name: &str, unread: u32, preview: &str) -> ConversationId {
        let id = ConversationId::new();
        s.conversations.upsert(ConversationSummary {
            id,
            display_name: name.into(),
            last_message_preview: Some(preview.into()),
            unread_count: unread,
            kind: ConversationKind::Direct,
        });
        id
    }

    fn engine() -> NotificationEngine {
        NotificationEngine::new(NotifyPolicy::default())
    }

    #[test]
    fn what_was_already_unread_at_startup_is_not_news() {
        let mut s = Slices::new();
        conv(&mut s, "Alice", 5, "old");
        s.security_events
            .push(SecurityEventKind::IdentityChanged, 1, None, None, vec![]);
        let mut e = engine();
        assert!(e.observe(&s, 0, 0).is_empty());
        assert!(
            e.observe(&s, 0, 1).is_empty(),
            "and still not on the next look"
        );
    }

    #[test]
    fn a_new_unread_message_pings_privately_by_default() {
        let mut s = Slices::new();
        let a = conv(&mut s, "Alice", 0, "hi");
        let mut e = engine();
        e.observe(&s, 0, 0);
        s.conversations.set_unread(a, 1);
        let n = e.observe(&s, 0, 1_000);
        assert_eq!(
            n,
            vec![Notification {
                title: "Siar".into(),
                body: "New message".into(),
                urgent: false
            }]
        );
        assert!(!n[0].title.contains("Alice") && !n[0].body.contains("hi"));
    }

    #[test]
    fn previews_are_an_explicit_opt_in_and_are_truncated() {
        let mut s = Slices::new();
        let a = conv(&mut s, "Alice", 0, &"x".repeat(500));
        let mut e = NotificationEngine::new(NotifyPolicy::from_env_value(Some("preview")));
        e.observe(&s, 0, 0);
        s.conversations.set_unread(a, 1);
        let n = e.observe(&s, 0, 1);
        assert_eq!(n[0].title, "Alice");
        assert_eq!(n[0].body.chars().count(), PREVIEW_CHARS);
    }

    #[test]
    fn nothing_is_shown_while_a_window_is_attached_but_critical_alerts_always_are() {
        let mut s = Slices::new();
        let a = conv(&mut s, "Alice", 0, "hi");
        let mut e = engine();
        e.observe(&s, 1, 0);
        s.conversations.set_unread(a, 1);
        s.security_events
            .push(SecurityEventKind::IdentityChanged, 2, None, None, vec![]);
        let n = e.observe(&s, 1, 10);
        assert_eq!(n.len(), 1, "{n:?}");
        assert!(n[0].urgent);
        assert!(!n[0].body.to_lowercase().contains("alice"));
    }

    #[test]
    fn a_burst_in_one_conversation_is_one_ping_and_the_next_burst_pings_again() {
        let mut s = Slices::new();
        let a = conv(&mut s, "Alice", 0, "hi");
        let mut e = engine();
        e.observe(&s, 0, 0);
        let mut pings = 0;
        for (i, t) in [1_000u64, 2_000, 3_000].into_iter().enumerate() {
            s.conversations.set_unread(a, i as u32 + 1);
            pings += e.observe(&s, 0, t).len();
        }
        assert_eq!(pings, 1);
        s.conversations.set_unread(a, 9);
        assert_eq!(e.observe(&s, 0, 40_000).len(), 1, "cooldown elapsed");
    }

    #[test]
    fn reading_messages_never_notifies_and_resets_the_baseline() {
        let mut s = Slices::new();
        let a = conv(&mut s, "Alice", 3, "hi");
        let mut e = engine();
        e.observe(&s, 0, 0);
        s.conversations.set_unread(a, 0);
        assert!(e.observe(&s, 0, 1).is_empty());
        s.conversations.set_unread(a, 1);
        assert_eq!(e.observe(&s, 0, 100_000).len(), 1);
    }

    #[test]
    fn routine_info_events_resolved_events_and_a_disabled_policy_stay_silent() {
        let mut s = Slices::new();
        let a = conv(&mut s, "Alice", 0, "hi");
        let mut e = engine();
        e.observe(&s, 0, 0);
        s.security_events
            .push(SecurityEventKind::KeyRotation, 1, None, None, vec![]);
        let id = s.security_events.push(
            SecurityEventKind::IdentityChanged,
            2,
            None,
            None,
            vec![SecurityEventAction::Review],
        );
        s.security_events.resolve(id);
        assert!(e.observe(&s, 0, 1).is_empty());

        let mut off = NotificationEngine::new(NotifyPolicy::from_env_value(Some("off")));
        off.observe(&s, 0, 0);
        s.conversations.set_unread(a, 4);
        assert!(off.observe(&s, 0, 1).is_empty());
    }

    #[test]
    fn a_new_security_event_is_announced_once() {
        let mut s = Slices::new();
        let mut e = engine();
        e.observe(&s, 0, 0);
        s.security_events
            .push(SecurityEventKind::IdentityChanged, 1, None, None, vec![]);
        assert_eq!(e.observe(&s, 0, 1).len(), 1);
        assert!(e.observe(&s, 0, 2).is_empty());
    }

    #[test]
    fn env_values_parse_with_a_private_default() {
        assert!(!NotifyPolicy::from_env_value(Some("OFF")).enabled);
        assert_eq!(
            NotifyPolicy::from_env_value(Some("preview")).detail,
            Detail::Preview
        );
        assert_eq!(NotifyPolicy::from_env_value(None).detail, Detail::Private);
        assert_eq!(
            NotifyPolicy::from_env_value(Some("garbage")).detail,
            Detail::Private
        );
    }
}

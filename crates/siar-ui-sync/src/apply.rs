//! Client side: mirror daemon sections into the window's own `Slices`.
//!
//! Only daemon-owned state is touched. Selection, composer text, filter and
//! banner dismissals stay exactly as the user left them.

use siar_ui_model::{
    ContactSafety, HistoryPaging, LocalIdentity, PowerStatus, Slices, VerificationState,
};
use siar_ui_state::{
    ContactListState, ConversationKind, ConversationListState, ConversationSummary, NetworkState,
    NetworkStatus, PeerReachability, SavedContact, TimelineState, TimelineWindow,
};

use crate::wire::*;

/// What to do with an incoming delta.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Apply,
    /// Already covered by what we have (a delta older than our snapshot).
    Stale,
    /// A gap, or a different daemon instance: fetch a snapshot.
    Resync,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ClientCursor {
    instance: Option<u128>,
    last_seq: u64,
}

impl ClientCursor {
    #[must_use]
    pub fn verdict(&self, f: &SyncFrame) -> Verdict {
        if f.full {
            return Verdict::Apply;
        }
        if self.instance != Some(f.instance_id) {
            return Verdict::Resync;
        }
        match f.seq {
            s if s <= self.last_seq => Verdict::Stale,
            s if s == self.last_seq + 1 => Verdict::Apply,
            _ => Verdict::Resync,
        }
    }

    #[must_use]
    pub const fn last_seq(&self) -> u64 {
        self.last_seq
    }

    #[must_use]
    pub const fn has_snapshot(&self) -> bool {
        self.instance.is_some()
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ApplyError {
    #[error("a snapshot must start with a full frame and stay within one instance and sequence")]
    BadSnapshot,
}

/// Replace all synced state from a snapshot's frames.
pub fn apply_snapshot(
    s: &mut Slices,
    cursor: &mut ClientCursor,
    frames: &[SyncFrame],
) -> Result<(), ApplyError> {
    let first = frames.first().ok_or(ApplyError::BadSnapshot)?;
    if !first.full
        || frames
            .iter()
            .any(|f| f.instance_id != first.instance_id || f.seq != first.seq)
        || frames[1..].iter().any(|f| f.full)
    {
        return Err(ApplyError::BadSnapshot);
    }
    reset_synced(s);
    for f in frames {
        for u in &f.updates {
            apply_update(s, u);
        }
    }
    *cursor = ClientCursor {
        instance: Some(first.instance_id),
        last_seq: first.seq,
    };
    Ok(())
}

/// Apply one delta if it is the next in sequence.
pub fn apply_delta(s: &mut Slices, cursor: &mut ClientCursor, f: &SyncFrame) -> Verdict {
    let v = cursor.verdict(f);
    if v == Verdict::Apply && !f.full {
        for u in &f.updates {
            apply_update(s, u);
        }
        cursor.last_seq = f.seq;
    }
    v
}

fn reset_synced(s: &mut Slices) {
    s.conversations = ConversationListState::new();
    s.timelines.clear();
    s.history.clear();
    s.sendable.clear();
    s.contacts = ContactListState::new();
    s.contact_safety.clear();
    s.network = NetworkStatus::new();
    s.power = None;
    s.identity = LocalIdentity::default();
    s.ready = false;
    s.startup_error = None;
}

fn apply_update(s: &mut Slices, u: &SectionUpdate) {
    let Some(section) = &u.section else {
        if let SectionKey::Timeline(c) = u.key {
            s.timelines.remove(&c);
            s.history.remove(&c);
            s.sendable.remove(&c);
        }
        return;
    };
    match (u.key, section) {
        (SectionKey::Core, Section::Core(d)) => {
            s.ready = d.ready;
            s.startup_error = d.startup_error.clone();
        }
        (SectionKey::Identity, Section::Identity(d)) => {
            s.identity = LocalIdentity {
                account: d.account,
                device: d.device,
                ticket: d.ticket.clone(),
                key_package: d.key_package.clone(),
            };
        }
        (SectionKey::Conversations, Section::Conversations(list)) => {
            let mut c = ConversationListState::new();
            for d in list {
                c.upsert(ConversationSummary {
                    id: d.id,
                    display_name: d.display_name.clone(),
                    last_message_preview: d.preview.clone(),
                    unread_count: d.unread,
                    kind: if d.group {
                        ConversationKind::Group
                    } else {
                        ConversationKind::Direct
                    },
                });
            }
            s.conversations = c;
        }
        (SectionKey::Contacts, Section::Contacts(d)) => {
            let mut c = ContactListState::new();
            c.load(
                d.contacts
                    .iter()
                    .map(|x| SavedContact {
                        device_id: x.device_id,
                        account_id: x.account_id,
                        display_name: x.display_name.clone(),
                        ticket_text: x.ticket_text.clone(),
                        key_package_b64: x.key_package_b64.clone(),
                    })
                    .collect(),
            );
            s.contacts = c;
            s.contact_safety = d
                .safety
                .iter()
                .map(|(dev, x)| {
                    let state = match x.state {
                        VerifyDto::Unverified => VerificationState::Unverified,
                        VerifyDto::Verified => VerificationState::Verified,
                        VerifyDto::KeyChanged => VerificationState::KeyChanged,
                    };
                    (
                        *dev,
                        ContactSafety {
                            number: x.number.clone(),
                            state,
                        },
                    )
                })
                .collect();
        }
        (SectionKey::Network, Section::Network(d)) => {
            let mut n = NetworkStatus::new();
            if let Some(o) = d.overall {
                n.set_overall(match o {
                    NetDto::Online => NetworkState::Online,
                    NetDto::Offline => NetworkState::Offline,
                });
            }
            for (dev, r) in &d.peers {
                n.set_peer(
                    *dev,
                    match r {
                        ReachDto::Connected => PeerReachability::Connected,
                        ReachDto::Connecting => PeerReachability::Connecting,
                        ReachDto::Unreachable => PeerReachability::Unreachable,
                    },
                );
            }
            s.network = n;
        }
        (SectionKey::Power, Section::Power(p)) => {
            s.power = p.as_ref().map(|d| PowerStatus {
                headline: d.headline.clone(),
                details: d.details.clone(),
                waiting: d.waiting,
            });
        }
        (SectionKey::Timeline(c), Section::Timeline(d)) => {
            let entries = d
                .entries
                .iter()
                .map(|e| TimelineWindow {
                    message_id: e.message_id,
                    sequence: e.sequence,
                    content: e.content.clone(),
                    delivery_state: e.delivery,
                    from_me: e.from_me,
                })
                .collect();
            s.timelines
                .insert(c, TimelineState::from_snapshot(entries, d.has_more));
            s.history.insert(
                c,
                HistoryPaging {
                    loaded: d.paging.loaded,
                    next_before: d.paging.next_before,
                    exhausted: d.paging.exhausted,
                },
            );
            if d.sendable {
                s.sendable.insert(c);
            } else {
                s.sendable.remove(&c);
            }
        }
        // A key and a section of different kinds can only come from a
        // buggy or hostile peer; ignoring it is the safe answer.
        _ => {}
    }
}

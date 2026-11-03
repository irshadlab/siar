//! Daemon side: turn `Slices` into sections, and sections into deltas.

use std::collections::HashMap;

use siar_ui_model::Slices;
use siar_ui_state::{ConversationKind, NetworkState, PeerReachability};

use crate::security::SecurityEventDto;
use crate::wire::*;

/// Bounds on list-shaped sections (a hostile or buggy store can't make one
/// frame arbitrarily large).
pub const MAX_LISTED: usize = 2_000;

/// Capture every daemon-owned section from the current state.
#[must_use]
pub fn capture(s: &Slices) -> Vec<(SectionKey, Section)> {
    let mut out: Vec<(SectionKey, Section)> = Vec::new();
    out.push((
        SectionKey::Core,
        Section::Core(CoreDto {
            ready: s.ready,
            startup_error: s.startup_error.clone(),
        }),
    ));
    out.push((
        SectionKey::Identity,
        Section::Identity(IdentityDto {
            account: s.identity.account,
            device: s.identity.device,
            ticket: s.identity.ticket.clone(),
            key_package: s.identity.key_package.clone(),
        }),
    ));
    out.push((
        SectionKey::Conversations,
        Section::Conversations(
            s.conversations
                .ordered()
                .iter()
                .take(MAX_LISTED)
                .map(|c| ConversationDto {
                    id: c.id,
                    display_name: c.display_name.clone(),
                    preview: c.last_message_preview.clone(),
                    unread: c.unread_count,
                    group: c.kind == ConversationKind::Group,
                })
                .collect(),
        ),
    ));

    let mut safety: Vec<(siar_domain::DeviceId, SafetyDto)> = s
        .contact_safety
        .iter()
        .map(|(d, c)| {
            use siar_ui_model::VerificationState as V;
            let state = match c.state {
                V::Unverified => VerifyDto::Unverified,
                V::Verified => VerifyDto::Verified,
                V::KeyChanged => VerifyDto::KeyChanged,
            };
            (
                *d,
                SafetyDto {
                    number: c.number.clone(),
                    state,
                },
            )
        })
        .collect();
    safety.sort_by_key(|(d, _)| d.as_uuid().as_u128());
    out.push((
        SectionKey::Contacts,
        Section::Contacts(ContactsDto {
            contacts: s
                .contacts
                .ordered()
                .iter()
                .take(MAX_LISTED)
                .map(|c| ContactDto {
                    device_id: c.device_id,
                    account_id: c.account_id,
                    display_name: c.display_name.clone(),
                    ticket_text: c.ticket_text.clone(),
                    key_package_b64: c.key_package_b64.clone(),
                })
                .collect(),
            safety,
        }),
    ));

    let mut peers: Vec<(siar_domain::DeviceId, ReachDto)> = s
        .network
        .peers()
        .map(|(d, r)| {
            (
                d,
                match r {
                    PeerReachability::Connected => ReachDto::Connected,
                    PeerReachability::Connecting => ReachDto::Connecting,
                    PeerReachability::Unreachable => ReachDto::Unreachable,
                },
            )
        })
        .collect();
    peers.sort_by_key(|(d, _)| d.as_uuid().as_u128());
    out.push((
        SectionKey::Network,
        Section::Network(NetworkDto {
            overall: s.network.overall().map(|o| match o {
                NetworkState::Online => NetDto::Online,
                NetworkState::Offline => NetDto::Offline,
            }),
            peers,
        }),
    ));

    out.push((
        SectionKey::Power,
        Section::Power(s.power.as_ref().map(|p| PowerDto {
            headline: p.headline.clone(),
            details: p.details.clone(),
            waiting: p.waiting,
        })),
    ));

    // Newest events win the bound (a flood of old ones can't push out a fresh
    // Critical); sent in id order so equal state captures equally.
    let mut events: Vec<SecurityEventDto> = s
        .security_events
        .events()
        .into_iter()
        .map(SecurityEventDto::from)
        .collect();
    events.sort_by_key(|e| e.id);
    if events.len() > MAX_LISTED {
        events.drain(..events.len() - MAX_LISTED);
    }
    out.push((SectionKey::SecurityEvents, Section::SecurityEvents(events)));

    let mut convs: Vec<_> = s
        .timelines
        .keys()
        .chain(s.history.keys())
        .chain(s.sendable.iter())
        .copied()
        .collect();
    convs.sort_by_key(|c| c.as_uuid().as_u128());
    convs.dedup();
    for c in convs {
        let (entries, has_more) = s.timelines.get(&c).map_or((Vec::new(), true), |t| {
            (
                t.visible()
                    .map(|e| EntryDto {
                        message_id: e.message_id,
                        sequence: e.sequence,
                        content: e.content.clone(),
                        delivery: e.delivery_state,
                        from_me: e.from_me,
                    })
                    .collect(),
                t.has_more_history(),
            )
        });
        let h = s.history.get(&c).copied().unwrap_or_default();
        let dto = TimelineDto {
            entries,
            has_more,
            paging: PagingDto {
                loaded: h.loaded,
                next_before: h.next_before,
                exhausted: h.exhausted,
            },
            sendable: s.sendable.contains(&c),
        };
        out.push((
            SectionKey::Timeline(c),
            Section::Timeline(fit_timeline(dto)),
        ));
    }
    out
}

/// Drop the *oldest* entries until the section fits, and say so: the window
/// can still page back, it just doesn't mirror more than it can carry.
fn fit_timeline(mut dto: TimelineDto) -> TimelineDto {
    let size = |d: &TimelineDto| encode(&Section::Timeline(d.clone())).len();
    let mut guard = 0;
    while size(&dto) > MAX_SECTION_BYTES && dto.entries.len() > 1 && guard < 64 {
        let k = (dto.entries.len() / 8).max(1);
        dto.entries.drain(..k);
        dto.has_more = true;
        dto.paging.exhausted = false;
        dto.paging.next_before = dto.entries.first().map(|e| e.sequence);
        guard += 1;
    }
    dto
}

fn hash16(bytes: &[u8]) -> [u8; 16] {
    let h = blake3::hash(bytes);
    let mut out = [0u8; 16];
    out.copy_from_slice(&h.as_bytes()[..16]);
    out
}

#[derive(Debug, Clone, Copy)]
struct Tracked {
    version: u64,
    hash: [u8; 16],
}

/// Tracks what clients have been told. One per daemon instance.
#[derive(Debug)]
pub struct SyncServer {
    instance_id: u128,
    seq: u64,
    tracked: HashMap<SectionKey, Tracked>,
}

impl SyncServer {
    #[must_use]
    pub fn new(instance_id: u128) -> Self {
        Self {
            instance_id,
            seq: 0,
            tracked: HashMap::new(),
        }
    }

    /// The sequence number of the last published frame.
    #[must_use]
    pub const fn seq(&self) -> u64 {
        self.seq
    }

    #[must_use]
    pub const fn instance_id(&self) -> u128 {
        self.instance_id
    }

    /// Frames for whatever changed since the last call (empty if nothing).
    /// Each frame gets the next sequence number; frames are split to stay
    /// under [`MAX_FRAME_PAYLOAD`].
    pub fn diff(&mut self, slices: &Slices) -> Vec<SyncFrame> {
        self.diff_captured(capture(slices))
    }

    /// [`Self::diff`] over an already-captured state, so the (cheap) capture
    /// can happen under the state lock and the (costlier) hashing outside it.
    pub fn diff_captured(&mut self, captured: Vec<(SectionKey, Section)>) -> Vec<SyncFrame> {
        let mut updates = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for (key, section) in captured {
            seen.insert(key);
            let h = hash16(&encode(&section));
            match self.tracked.get_mut(&key) {
                Some(t) if t.hash == h => {}
                Some(t) => {
                    t.version += 1;
                    t.hash = h;
                    updates.push(SectionUpdate {
                        key,
                        version: t.version,
                        section: Some(section),
                    });
                }
                None => {
                    self.tracked.insert(
                        key,
                        Tracked {
                            version: 1,
                            hash: h,
                        },
                    );
                    updates.push(SectionUpdate {
                        key,
                        version: 1,
                        section: Some(section),
                    });
                }
            }
        }
        let gone: Vec<SectionKey> = self
            .tracked
            .keys()
            .filter(|k| !seen.contains(k))
            .copied()
            .collect();
        for key in gone {
            let t = self.tracked.remove(&key).expect("tracked key");
            updates.push(SectionUpdate {
                key,
                version: t.version + 1,
                section: None,
            });
        }
        if updates.is_empty() {
            return Vec::new();
        }
        updates.sort_by_key(|u| u.key.sort_key());
        chunk(updates)
            .into_iter()
            .map(|u| {
                self.seq += 1;
                SyncFrame {
                    instance_id: self.instance_id,
                    seq: self.seq,
                    full: false,
                    updates: u,
                }
            })
            .collect()
    }

    /// A complete picture, consistent with "deltas from `seq() + 1`".
    /// Does **not** change what other clients are considered to know.
    #[must_use]
    pub fn snapshot(&self, slices: &Slices) -> Vec<SyncFrame> {
        self.snapshot_captured(capture(slices))
    }

    #[must_use]
    pub fn snapshot_captured(&self, captured: Vec<(SectionKey, Section)>) -> Vec<SyncFrame> {
        let mut updates: Vec<SectionUpdate> = captured
            .into_iter()
            .map(|(key, section)| SectionUpdate {
                key,
                version: self.tracked.get(&key).map_or(0, |t| t.version),
                section: Some(section),
            })
            .collect();
        updates.sort_by_key(|u| u.key.sort_key());
        chunk(updates)
            .into_iter()
            .enumerate()
            .map(|(i, u)| SyncFrame {
                instance_id: self.instance_id,
                seq: self.seq,
                full: i == 0,
                updates: u,
            })
            .collect()
    }
}

/// Split updates into groups whose encoded size stays under the payload
/// target (a single over-size update travels alone; sections are already
/// bounded by [`MAX_SECTION_BYTES`]).
fn chunk(updates: Vec<SectionUpdate>) -> Vec<Vec<SectionUpdate>> {
    let mut out: Vec<Vec<SectionUpdate>> = Vec::new();
    let mut cur: Vec<SectionUpdate> = Vec::new();
    let mut size = 0usize;
    for u in updates {
        let n = encode(&u).len();
        if !cur.is_empty() && size + n > MAX_FRAME_PAYLOAD {
            out.push(std::mem::take(&mut cur));
            size = 0;
        }
        size += n;
        cur.push(u);
    }
    if !cur.is_empty() || out.is_empty() {
        out.push(cur);
    }
    out
}

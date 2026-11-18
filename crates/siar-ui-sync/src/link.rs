//! Device-linking state on the wire (what the window shows; no keys).

use serde::{Deserialize, Serialize};
use siar_domain::DeviceId;
use siar_ui_state::{LinkOffer, LinkState, PendingLinkRow};

const MAX_TEXT: usize = 4096;
const MAX_PENDING: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkDto {
    pub offer: Option<(String, String)>,
    pub pending: Vec<(DeviceId, String, String)>,
    pub restart_needed: bool,
}

fn clip(s: &str) -> String {
    s.chars().take(MAX_TEXT).collect()
}

impl From<&LinkState> for LinkDto {
    fn from(l: &LinkState) -> Self {
        Self {
            offer: l.offer.as_ref().map(|o| (clip(&o.code), clip(&o.ticket))),
            pending: l
                .pending
                .iter()
                .take(MAX_PENDING)
                .map(|p| (p.device_id, clip(&p.name), clip(&p.fingerprint)))
                .collect(),
            restart_needed: l.restart_needed,
        }
    }
}

impl From<&LinkDto> for LinkState {
    fn from(d: &LinkDto) -> Self {
        Self {
            offer: d.offer.as_ref().map(|(code, ticket)| LinkOffer {
                code: code.clone(),
                ticket: ticket.clone(),
            }),
            pending: d
                .pending
                .iter()
                .map(|(device_id, name, fingerprint)| PendingLinkRow {
                    device_id: *device_id,
                    name: name.clone(),
                    fingerprint: fingerprint.clone(),
                })
                .collect(),
            restart_needed: d.restart_needed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_state_round_trips() {
        let s = LinkState {
            offer: Some(LinkOffer {
                code: "ABCD-EFGH-JKMN-PQRS".into(),
                ticket: "t".into(),
            }),
            pending: vec![PendingLinkRow {
                device_id: DeviceId::new(),
                name: "phone".into(),
                fingerprint: "ab12cd34".into(),
            }],
            restart_needed: true,
        };
        assert_eq!(LinkState::from(&LinkDto::from(&s)), s);
    }
}

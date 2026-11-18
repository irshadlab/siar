//! What the window shows about linking another device to this account.
//!
//! The daemon owns the rules and the keys (`siar-app-core`); this is only the
//! picture of them: the one-time code to read out, the requests waiting for a
//! person's yes or no, and whether a restart is needed to finish joining.

use siar_domain::DeviceId;

/// A one-time offer to show to the person setting up the other device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkOffer {
    pub code: String,
    pub ticket: String,
}

/// A device asking to join, waiting for approval.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingLinkRow {
    pub device_id: DeviceId,
    pub name: String,
    /// Short fingerprint of its key, to compare with the other screen.
    pub fingerprint: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LinkState {
    pub offer: Option<LinkOffer>,
    pub pending: Vec<PendingLinkRow>,
    /// This device joined an account; restarting finishes it.
    pub restart_needed: bool,
}

//! Trust classification for emergency data — next.md §49–50, §97–98.
//!
//! This enum is the *result* a UI shows (next.md §97: "display Verified
//! / Known contact / Unverified clearly"), not the verification logic
//! itself. Producing an [`AlertTrust`] correctly needs real signature
//! checking against `siar-crypto` — an authority alert's signature
//! chaining to a configured authority key (§49), or a user report's
//! signature matching a known contact's account key (§50) — which this
//! crate deliberately doesn't have a dependency to do (see
//! `Cargo.toml`). What this module *can* pin down without any crypto
//! at all is the rule for what must NEVER produce
//! [`AlertTrust::VerifiedAuthority`]: next.md §49 in as many words —
//! "Never infer authenticity from `display_name = \"Police\"`." A
//! display name is user-controlled, arbitrary text; nothing in this
//! crate's types even has a field a caller could mistakenly wire up to
//! that check in the first place.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AlertTrust {
    /// Signature chains to a configured authority key (§49).
    VerifiedAuthority,
    /// Signed by an account already in the recipient's contact/
    /// relationship graph (§50: "this message came from Alice's
    /// verified device even if it travelled through ten unknown relay
    /// phones" — the *signature* is what's verified, independent of
    /// how many mesh hops carried the ciphertext).
    KnownContact,
    /// Signature doesn't resolve to either of the above, or couldn't be
    /// checked at all (e.g. sender not yet known). next.md §98: mesh
    /// networks are vulnerable to fake SOS/evacuation/spam/
    /// misinformation, so this is the default a caller should assume
    /// for anything it hasn't actively verified — never upgrade to a
    /// higher trust level as a fallback "probably fine" guess.
    Unverified,
}

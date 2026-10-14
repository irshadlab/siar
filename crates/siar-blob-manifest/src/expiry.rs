//! §154 "Expiry".
//!
//! "Transfers and blobs can have offer expiry, transfer expiry, relay
//! expiry, cache expiry. These are different. Do not use one timestamp
//! for all semantics."
//!
//! The way to make "do not use one timestamp" hold is for the compiler
//! to refuse it: each expiry is its **own type**, with no conversion
//! between them. A function that decides whether a *relay* may delete a
//! blob takes a [`RelayExpiry`]; handing it an [`OfferExpiry`] is a type
//! error, not a runtime surprise discovered when a still-wanted offer
//! vanishes because its cache copy aged out.
//!
//! What each one means, and who consults it:
//!
//! | type | question it answers | consulted by |
//! |---|---|---|
//! | [`OfferExpiry`] | may the recipient still *accept* this offer? | the receive path ([`crate::file_offer`]) |
//! | [`TransferExpiry`] | may this in-flight transfer keep going / be resumed? | partial-blob retention ([`crate::gc::sweep_staging`]) |
//! | [`RelayExpiry`] | may a relay still hold this for someone else? | relay storage ([`crate::relay_and_dtn`]) |
//! | [`CacheExpiry`] | is this *cached copy* still worth keeping? | cache eviction ([`crate::storage_lifecycle`]) |
//!
//! An offer that expired does not delete an already-accepted blob; a
//! cache copy that expired does not cancel the transfer that produced
//! it. That independence is the entire content of the section.
//!
//! All four are millisecond timestamps with the same *expired* test
//! (`now >= at`: expiry is the first instant it is no longer valid), so
//! the boundary is identical and tested once per type.

macro_rules! expiry_type {
    ($(#[$m:meta])* $name:ident) => {
        $(#[$m])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(pub u64);

        impl $name {
            /// `true` from the expiry instant onward.
            pub const fn is_expired(&self, now_millis: u64) -> bool {
                now_millis >= self.0
            }

            /// Milliseconds left, `0` once expired.
            pub const fn remaining_millis(&self, now_millis: u64) -> u64 {
                self.0.saturating_sub(now_millis)
            }
        }
    };
}

expiry_type!(
    /// §154 "offer expiry".
    OfferExpiry
);
expiry_type!(
    /// §154 "transfer expiry".
    TransferExpiry
);
expiry_type!(
    /// §154 "relay expiry".
    RelayExpiry
);
expiry_type!(
    /// §154 "cache expiry".
    CacheExpiry
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expiry_is_the_first_instant_it_is_no_longer_valid_for_every_kind() {
        macro_rules! boundary {
            ($t:ident) => {
                let e = $t(1_000);
                assert!(!e.is_expired(999));
                assert!(e.is_expired(1_000));
                assert!(e.is_expired(u64::MAX));
                assert_eq!(e.remaining_millis(400), 600);
                assert_eq!(e.remaining_millis(5_000), 0);
            };
        }
        boundary!(OfferExpiry);
        boundary!(TransferExpiry);
        boundary!(RelayExpiry);
        boundary!(CacheExpiry);
    }

    #[test]
    fn the_four_kinds_are_distinct_types_with_independent_values() {
        // The same instant under four different meanings: an expired
        // offer says nothing about the cache copy, and vice versa.
        let now = 5_000;
        let offer = OfferExpiry(4_000); // lapsed
        let transfer = TransferExpiry(9_000); // still resumable
        let relay = RelayExpiry(6_000);
        let cache = CacheExpiry(5_000); // just lapsed
        assert!(offer.is_expired(now));
        assert!(!transfer.is_expired(now));
        assert!(!relay.is_expired(now));
        assert!(cache.is_expired(now));
        // (Passing an OfferExpiry where a RelayExpiry is required does
        // not compile — that is the property; nothing to assert at
        // runtime.)
    }
}

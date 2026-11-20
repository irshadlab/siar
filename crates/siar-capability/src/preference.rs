//! §71 Preference Ordering.
//!
//! For a selectable mode (a codec, a transfer strategy) the order of
//! authority is *local preference → remote support → policy*, and "do not let
//! the peer alone choose the mode". The remote list is therefore only ever a
//! **membership test**: its order, and anything it lists that we did not
//! offer, cannot influence the result.

/// The first entry of `local_preference` that the remote supports and policy
/// allows, or `None` if there is no such mode.
///
/// Deterministic and independent of the order of `remote_supports`, so both
/// sides that hold the same local preference and see the same support pick the
/// same mode (§72).
pub fn choose_by_preference<T: PartialEq + Clone>(
    local_preference: &[T],
    remote_supports: &[T],
    policy_allows: impl Fn(&T) -> bool,
) -> Option<T> {
    local_preference
        .iter()
        .find(|m| remote_supports.contains(m) && policy_allows(m))
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn local_order_wins_over_the_peers_order() {
        let local = ["av1", "vp9", "h264"];
        // The peer lists its own favourite first; it must not matter.
        let remote = ["h264", "vp9", "av1"];
        assert_eq!(choose_by_preference(&local, &remote, |_| true), Some("av1"));
    }

    #[test]
    fn unsupported_and_policy_denied_modes_are_skipped_in_order() {
        let local = ["av1", "vp9", "h264"];
        assert_eq!(
            choose_by_preference(&local, &["vp9", "h264"], |_| true),
            Some("vp9")
        );
        assert_eq!(
            choose_by_preference(&local, &["av1", "vp9", "h264"], |m| *m != "av1"
                && *m != "vp9"),
            Some("h264")
        );
        assert_eq!(choose_by_preference(&local, &["vp8"], |_| true), None);
        assert_eq!(choose_by_preference(&local, &["av1"], |_| false), None);
    }

    #[test]
    fn a_mode_only_the_peer_offers_is_never_chosen() {
        assert_eq!(choose_by_preference(&["a"], &["b", "c"], |_| true), None);
    }

    proptest! {
        #[test]
        fn result_is_in_both_lists_allowed_and_independent_of_remote_order(
            local in proptest::collection::vec(0u8..8, 0..8),
            remote in proptest::collection::vec(0u8..8, 0..8),
            deny in 0u8..8,
        ) {
            let allow = |m: &u8| *m != deny;
            let got = choose_by_preference(&local, &remote, allow);
            if let Some(m) = got {
                prop_assert!(local.contains(&m) && remote.contains(&m) && m != deny);
            }
            let mut reversed = remote.clone();
            reversed.reverse();
            prop_assert_eq!(got, choose_by_preference(&local, &reversed, allow));
        }
    }
}

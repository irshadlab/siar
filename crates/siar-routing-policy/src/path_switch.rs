//! §168 "Graceful Path Switch", §169 "Make-Before-Break", §170
//! "Break-Before-Make".
//!
//! §168's own five-step process —
//!
//! ```text
//! prepare new path → authenticate → transfer state → switch → close old path
//! ```
//!
//! — is entirely session-layer mechanics this crate has no session to
//! perform them on: "authenticate," "transfer state," and "close" are
//! all actions on a live transport handle, the same kind of thing
//! §120 "Transport Manager API" already puts outside this crate's
//! scope (see [`crate::engine`]'s own doc comment on §120 for the
//! fuller reasoning). Nothing here attempts that process.
//!
//! What *is* this crate's to decide is which of the two named
//! strategies (§169/§170) a switch should even try to follow —
//! [`PathSwitchStrategy`]/[`path_switch_strategy_for`]. §170's own two
//! named triggers ("resource constrained," "security requires old
//! path termination") are treated as overriding §169's — a resource
//! or security constraint is a hard requirement, not a preference
//! competing with "reduces interruption" on equal footing; §169's own
//! "and policy permits" qualifier is exactly this asymmetry read the
//! other way, since it implies Make-Before-Break can be *refused* by
//! policy but §170's own conditions have no equivalent "unless policy
//! says otherwise" language.

use crate::types::DeliveryClass;

/// §169/§170, transcribed exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathSwitchStrategy {
    MakeBeforeBreak,
    BreakBeforeMake,
}

/// §169's own two named use cases are [`DeliveryClass::Realtime`]
/// ("call") and [`DeliveryClass::Bulk`] ("large transfer") — the same
/// two classes [`crate::requirements::DeliveryRequirements::realtime_media`]/
/// [`crate::requirements::DeliveryRequirements::file_chunk`] already
/// use for exactly those two cases elsewhere in this crate.
pub fn path_switch_strategy_for(
    class: DeliveryClass,
    resource_constrained: bool,
    security_requires_old_path_termination: bool,
    policy_permits_overlap: bool,
) -> PathSwitchStrategy {
    if resource_constrained || security_requires_old_path_termination {
        return PathSwitchStrategy::BreakBeforeMake;
    }
    if matches!(class, DeliveryClass::Realtime | DeliveryClass::Bulk) && policy_permits_overlap {
        PathSwitchStrategy::MakeBeforeBreak
    } else {
        PathSwitchStrategy::BreakBeforeMake
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_169_a_call_with_policy_permission_and_no_constraints_makes_before_breaking() {
        assert_eq!(
            path_switch_strategy_for(DeliveryClass::Realtime, false, false, true),
            PathSwitchStrategy::MakeBeforeBreak
        );
    }

    #[test]
    fn spec_169_a_large_transfer_with_policy_permission_makes_before_breaking() {
        assert_eq!(
            path_switch_strategy_for(DeliveryClass::Bulk, false, false, true),
            PathSwitchStrategy::MakeBeforeBreak
        );
    }

    #[test]
    fn spec_169_policy_can_still_refuse_make_before_break_even_for_a_call() {
        assert_eq!(
            path_switch_strategy_for(DeliveryClass::Realtime, false, false, false),
            PathSwitchStrategy::BreakBeforeMake
        );
    }

    #[test]
    fn spec_170_resource_constraint_overrides_an_otherwise_qualifying_call() {
        assert_eq!(
            path_switch_strategy_for(DeliveryClass::Realtime, true, false, true),
            PathSwitchStrategy::BreakBeforeMake
        );
    }

    #[test]
    fn spec_170_security_requirement_overrides_an_otherwise_qualifying_transfer() {
        assert_eq!(
            path_switch_strategy_for(DeliveryClass::Bulk, false, true, true),
            PathSwitchStrategy::BreakBeforeMake
        );
    }

    #[test]
    fn an_ordinary_reliable_message_breaks_before_making_by_default() {
        assert_eq!(
            path_switch_strategy_for(DeliveryClass::Reliable, false, false, true),
            PathSwitchStrategy::BreakBeforeMake
        );
    }
}

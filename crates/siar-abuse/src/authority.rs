//! §67: emergency authority is **granted locally, never requested remotely**.
//!
//! A [`CriticalGrant`] can only be produced by [`EmergencyAuthority::grant`],
//! which is local policy code. Anything that arrives over the wire — a peer's
//! "this is critical" flag, a plugin's declared priority — is just a
//! *request*, and [`effective`] ignores a request that carries no grant.

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Authority {
    Normal,
    Critical,
}

/// Proof that local policy authorised critical priority for one purpose.
/// There is deliberately no public constructor and no `Default`/`From`.
#[derive(Debug, Clone, Copy)]
pub struct CriticalGrant {
    _private: (),
}

/// Local, user-controlled emergency policy.
#[derive(Debug, Clone, Copy)]
pub struct EmergencyAuthority {
    user_enabled: bool,
}

impl EmergencyAuthority {
    #[must_use]
    pub const fn new(user_enabled: bool) -> Self {
        Self { user_enabled }
    }

    /// Only the owner of this device, having turned emergency mode on, can
    /// authorise critical handling — and only for what local code asks about.
    #[must_use]
    pub const fn grant(&self) -> Option<CriticalGrant> {
        if self.user_enabled {
            Some(CriticalGrant { _private: () })
        } else {
            None
        }
    }
}

/// What a peer or plugin *asked for* is never what it *gets* on its own.
#[must_use]
pub const fn effective(requested: Authority, grant: Option<&CriticalGrant>) -> Authority {
    match (requested, grant) {
        (Authority::Critical, Some(_)) => Authority::Critical,
        _ => Authority::Normal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asking_for_critical_gets_nothing_without_a_local_grant() {
        assert_eq!(effective(Authority::Critical, None), Authority::Normal);
        assert_eq!(effective(Authority::Normal, None), Authority::Normal);
    }

    #[test]
    fn a_grant_only_exists_when_the_user_enabled_emergency_mode() {
        assert!(EmergencyAuthority::new(false).grant().is_none());
        let on = EmergencyAuthority::new(true).grant();
        assert_eq!(
            effective(Authority::Critical, on.as_ref()),
            Authority::Critical
        );
        assert_eq!(
            effective(Authority::Normal, on.as_ref()),
            Authority::Normal,
            "a grant never upgrades normal traffic by itself"
        );
    }
}

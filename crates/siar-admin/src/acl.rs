//! The node's operator allow-list: which Iroh keys may administer it, and as
//! what. Empty means "nobody" — there is no anonymous or default operator.

use std::collections::HashMap;

use thiserror::Error;

use crate::protocol::AdminRole;

pub const MAX_OPERATORS: usize = 64;
pub const MAX_LABEL: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Operator {
    pub key: [u8; 32],
    pub role: AdminRole,
    /// Shown in logs and the audit trail instead of the key.
    pub label: String,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum AclError {
    #[error("too many operators (max {MAX_OPERATORS})")]
    TooMany,
    #[error("duplicate operator key")]
    DuplicateKey,
    #[error("duplicate operator label")]
    DuplicateLabel,
    #[error("operator label must be 1-{MAX_LABEL} characters of a-z, 0-9, '.', '_', '-'")]
    BadLabel,
    #[error("operator key is not a valid Ed25519 public key")]
    BadKey,
}

#[derive(Debug, Clone, Default)]
pub struct OperatorTable {
    by_key: HashMap<[u8; 32], Operator>,
}

impl OperatorTable {
    pub fn new(operators: Vec<Operator>) -> Result<Self, AclError> {
        if operators.len() > MAX_OPERATORS {
            return Err(AclError::TooMany);
        }
        let mut by_key = HashMap::new();
        let mut labels = std::collections::HashSet::new();
        for o in operators {
            let ok = !o.label.is_empty()
                && o.label.len() <= MAX_LABEL
                && o.label
                    .bytes()
                    .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' | b'-'));
            if !ok {
                return Err(AclError::BadLabel);
            }
            iroh::PublicKey::from_bytes(&o.key).map_err(|_| AclError::BadKey)?;
            if !labels.insert(o.label.clone()) {
                return Err(AclError::DuplicateLabel);
            }
            if by_key.insert(o.key, o).is_some() {
                return Err(AclError::DuplicateKey);
            }
        }
        Ok(Self { by_key })
    }

    #[must_use]
    pub fn lookup(&self, key: &[u8; 32]) -> Option<&Operator> {
        self.by_key.get(key)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.by_key.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_key.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> [u8; 32] {
        iroh::SecretKey::generate().public().as_bytes().to_owned()
    }

    fn op(label: &str, role: AdminRole) -> Operator {
        Operator {
            key: key(),
            role,
            label: label.into(),
        }
    }

    #[test]
    fn lookup_finds_exactly_the_listed_keys() {
        let a = op("laptop", AdminRole::Operator);
        let t = OperatorTable::new(vec![a.clone(), op("ci", AdminRole::Viewer)]).unwrap();
        assert_eq!(t.len(), 2);
        assert_eq!(t.lookup(&a.key), Some(&a));
        assert_eq!(t.lookup(&key()), None);
        assert!(OperatorTable::default().is_empty(), "default trusts nobody");
    }

    #[test]
    fn invalid_tables_are_rejected() {
        let a = op("a", AdminRole::Owner);
        let dup_key = Operator {
            label: "b".into(),
            ..a.clone()
        };
        assert_eq!(
            OperatorTable::new(vec![a.clone(), dup_key]).unwrap_err(),
            AclError::DuplicateKey
        );
        let dup_label = op("a", AdminRole::Viewer);
        assert_eq!(
            OperatorTable::new(vec![a, dup_label]).unwrap_err(),
            AclError::DuplicateLabel
        );
        for bad in ["", "Upper", "with space", "a/b", &"x".repeat(MAX_LABEL + 1)] {
            assert_eq!(
                OperatorTable::new(vec![op(bad, AdminRole::Viewer)]).unwrap_err(),
                AclError::BadLabel,
                "{bad:?}"
            );
        }
        let many: Vec<_> = (0..=MAX_OPERATORS)
            .map(|i| op(&format!("op{i}"), AdminRole::Viewer))
            .collect();
        assert_eq!(OperatorTable::new(many).unwrap_err(), AclError::TooMany);
        let junk = Operator {
            key: {
                let mut k = [0u8; 32];
                k[0] = 2;
                k
            }, /* y = 2 is not on the curve */
            role: AdminRole::Owner,
            label: "x".into(),
        };
        assert_eq!(
            OperatorTable::new(vec![junk]).unwrap_err(),
            AclError::BadKey
        );
    }
}

//! §77-§79, §81-§84: per-user runtime directories and account/tenant
//! namespaces. A namespace id becomes a **path component**, so it is
//! validated hard: no separators, no `..`, no absolute paths, no Unicode
//! tricks — a hostile id must never address a directory outside its own.

use std::path::{Path, PathBuf};

use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum NamespaceError {
    #[error("namespace id must be 1-64 characters of a-z, 0-9, '-' or '_'")]
    Invalid,
}

/// Account or tenant id (§81, §84).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Namespace(String);

impl Namespace {
    pub fn new(id: &str) -> Result<Self, NamespaceError> {
        let ok = !id.is_empty()
            && id.len() <= 64
            && id
                .bytes()
                .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_'))
            && !id.starts_with('-');
        ok.then(|| Self(id.to_owned()))
            .ok_or(NamespaceError::Invalid)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Directory layout for one namespace. Everything is derived from two roots
/// the host chose; nothing is ever built from caller-supplied paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeDirs {
    data_root: PathBuf,
    runtime_root: PathBuf,
}

impl RuntimeDirs {
    #[must_use]
    pub fn new(data_root: impl Into<PathBuf>, runtime_root: impl Into<PathBuf>) -> Self {
        Self {
            data_root: data_root.into(),
            runtime_root: runtime_root.into(),
        }
    }

    /// §82: cross-account isolation starts with disjoint directories.
    #[must_use]
    pub fn data_dir(&self, ns: &Namespace) -> PathBuf {
        self.data_root.join("accounts").join(ns.as_str())
    }

    /// §78: the per-user runtime directory holds sockets and the instance file.
    #[must_use]
    pub fn runtime_dir(&self, ns: &Namespace) -> PathBuf {
        self.runtime_root.join("siar").join(ns.as_str())
    }

    #[must_use]
    pub fn socket_path(&self, ns: &Namespace) -> PathBuf {
        self.runtime_dir(ns).join("daemon.sock")
    }

    /// Does `p` live inside this namespace's directories? (A defence-in-depth
    /// check for code that receives paths from elsewhere.)
    #[must_use]
    pub fn contains(&self, ns: &Namespace, p: &Path) -> bool {
        let inside = |root: PathBuf| {
            p.starts_with(&root)
                && !p
                    .components()
                    .any(|c| matches!(c, std::path::Component::ParentDir))
        };
        inside(self.data_dir(ns)) || inside(self.runtime_dir(ns))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_plain_ids_rejects_everything_path_like() {
        for ok in ["alice", "a", "tenant-1", "t_9", &"x".repeat(64)] {
            assert!(Namespace::new(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            "..",
            ".",
            "a/b",
            "a\\b",
            "/abs",
            "a b",
            "A",
            "ünï",
            "-lead",
            "a\0b",
            "a\n",
            &"x".repeat(65),
            "~",
            "a:b",
        ] {
            assert_eq!(Namespace::new(bad), Err(NamespaceError::Invalid), "{bad:?}");
        }
    }

    #[test]
    fn namespaces_get_disjoint_directories() {
        let d = RuntimeDirs::new("/data", "/run/user/1000");
        let (a, b) = (Namespace::new("a").unwrap(), Namespace::new("b").unwrap());
        assert_ne!(d.data_dir(&a), d.data_dir(&b));
        assert_ne!(d.socket_path(&a), d.socket_path(&b));
        assert_eq!(
            d.socket_path(&a),
            PathBuf::from("/run/user/1000/siar/a/daemon.sock")
        );
        assert!(!d.data_dir(&a).starts_with(d.data_dir(&b)));
    }

    #[test]
    fn containment_check_rejects_escapes_and_siblings() {
        let d = RuntimeDirs::new("/data", "/run");
        let a = Namespace::new("a").unwrap();
        assert!(d.contains(&a, &d.data_dir(&a).join("db/x")));
        assert!(!d.contains(&a, Path::new("/data/accounts/b/db")));
        assert!(!d.contains(&a, &d.data_dir(&a).join("../b/db")));
        assert!(!d.contains(&a, Path::new("/etc/passwd")));
    }
}

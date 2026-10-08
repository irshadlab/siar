//! §65 "File Export Recovery" and §66 "Notification Recovery".
//!
//! Both are effects on systems we don't control (the user's
//! filesystem, the OS notification centre). Export uses temp +
//! atomic commit, or an explicit user-visible duplicate policy;
//! notifications are never authoritative and are deduplicated by id.

use crate::messaging_recovery::Deduplicator;

// ---------------------------------------------------------------------
// §65
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExportPolicy {
    /// Write to a temp destination, then atomically commit/rename.
    #[default]
    AtomicCommit,
    /// If completion is uncertain, re-export under a visibly
    /// different (numbered) name instead of overwriting or guessing.
    UserVisibleDuplicate,
}

/// What recovery found for one interrupted export.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExportFacts {
    pub temp_exists: bool,
    /// The temp file was verified complete (length/hash against the
    /// source). Meaningless if `temp_exists` is false.
    pub temp_complete: bool,
    pub final_exists: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportRecoveryAction {
    /// Export already committed; nothing to do.
    Done,
    /// Temp is complete but the commit didn't happen: atomic rename.
    CommitTemp,
    /// Temp is partial/unverified: delete it and export again.
    DiscardTempAndReexport,
    /// Nothing exists yet: export from scratch.
    Reexport,
    /// Final exists *and* a stray temp does: the commit won; drop the
    /// leftover temp.
    DiscardLeftoverTemp,
    /// `UserVisibleDuplicate` policy with uncertain completion:
    /// export again under a numbered name.
    ReexportAsNumberedCopy,
}

pub const fn plan_export_recovery(policy: ExportPolicy, f: ExportFacts) -> ExportRecoveryAction {
    use ExportRecoveryAction::*;
    match policy {
        ExportPolicy::AtomicCommit => match (f.final_exists, f.temp_exists, f.temp_complete) {
            (true, false, _) => Done,
            (true, true, _) => DiscardLeftoverTemp,
            (false, true, true) => CommitTemp,
            (false, true, false) => DiscardTempAndReexport,
            (false, false, _) => Reexport,
        },
        ExportPolicy::UserVisibleDuplicate => {
            if f.final_exists {
                // A file by that name exists but we can't prove it is
                // ours and complete: never overwrite, number instead.
                ReexportAsNumberedCopy
            } else {
                Reexport
            }
        }
    }
}

/// `name.ext` → `name (2).ext`, `name (3).ext`, … — the first
/// candidate for which `exists` is false. The base name itself is
/// tried first.
pub fn unique_numbered_name(file_name: &str, exists: impl Fn(&str) -> bool) -> String {
    if !exists(file_name) {
        return file_name.to_owned();
    }
    let (stem, ext) = match file_name.rfind('.') {
        Some(i) if i > 0 => (&file_name[..i], &file_name[i..]),
        _ => (file_name, ""),
    };
    let mut n: u32 = 2;
    loop {
        let candidate = format!("{stem} ({n}){ext}");
        if !exists(&candidate) {
            return candidate;
        }
        n = n.saturating_add(1);
        if n == u32::MAX {
            return format!("{stem} ({n}){ext}");
        }
    }
}

// ---------------------------------------------------------------------
// §66
// ---------------------------------------------------------------------

/// Notification identity: the id of the notification or the event it
/// announces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NotificationKey(pub uuid::Uuid);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationOutcome {
    Display,
    /// Already shown (or already replaced in place): suppress.
    Suppress,
}

/// "Do not treat notification display as authoritative state." This
/// type is a best-effort duplicate suppressor only; losing it costs a
/// possible duplicate banner, never a lost or wrong state change.
#[derive(Debug, Clone, Default)]
pub struct NotificationDeduper {
    seen: Deduplicator<NotificationKey>,
}

impl NotificationDeduper {
    pub fn new() -> Self {
        Self::default()
    }

    /// `platform_replaces_by_id`: the OS collapses a repost with the
    /// same id into the existing notification ("where platform
    /// allows"), so re-posting is already harmless and always allowed.
    pub fn on_event(
        &mut self,
        key: NotificationKey,
        platform_replaces_by_id: bool,
    ) -> NotificationOutcome {
        let first = self.seen.try_admit(key);
        if first || platform_replaces_by_id {
            NotificationOutcome::Display
        } else {
            NotificationOutcome::Suppress
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ExportRecoveryAction::*;

    fn f(temp: bool, complete: bool, fin: bool) -> ExportFacts {
        ExportFacts {
            temp_exists: temp,
            temp_complete: complete,
            final_exists: fin,
        }
    }

    #[test]
    fn atomic_commit_is_the_default_policy() {
        assert_eq!(ExportPolicy::default(), ExportPolicy::AtomicCommit);
    }

    #[test]
    fn atomic_policy_table() {
        let p = ExportPolicy::AtomicCommit;
        assert_eq!(plan_export_recovery(p, f(false, false, true)), Done);
        assert_eq!(
            plan_export_recovery(p, f(true, true, true)),
            DiscardLeftoverTemp
        );
        assert_eq!(
            plan_export_recovery(p, f(true, false, true)),
            DiscardLeftoverTemp
        );
        assert_eq!(plan_export_recovery(p, f(true, true, false)), CommitTemp);
        assert_eq!(
            plan_export_recovery(p, f(true, false, false)),
            DiscardTempAndReexport
        );
        assert_eq!(plan_export_recovery(p, f(false, false, false)), Reexport);
    }

    #[test]
    fn visible_duplicate_policy_never_overwrites() {
        let p = ExportPolicy::UserVisibleDuplicate;
        assert_eq!(
            plan_export_recovery(p, f(false, false, true)),
            ReexportAsNumberedCopy
        );
        assert_eq!(plan_export_recovery(p, f(false, false, false)), Reexport);
    }

    #[test]
    fn numbered_names_skip_taken_ones_and_keep_the_extension() {
        let taken = ["a.txt", "a (2).txt", "a (3).txt"];
        let n = unique_numbered_name("a.txt", |c| taken.contains(&c));
        assert_eq!(n, "a (4).txt");
        assert_eq!(unique_numbered_name("fresh.txt", |_| false), "fresh.txt");
        assert_eq!(unique_numbered_name("noext", |c| c == "noext"), "noext (2)");
        assert_eq!(
            unique_numbered_name(".hidden", |c| c == ".hidden"),
            ".hidden (2)"
        );
    }

    #[test]
    fn duplicate_notifications_are_suppressed_when_platform_cannot_replace() {
        let mut d = NotificationDeduper::new();
        let k = NotificationKey(uuid::Uuid::new_v4());
        assert_eq!(d.on_event(k, false), NotificationOutcome::Display);
        assert_eq!(d.on_event(k, false), NotificationOutcome::Suppress);
        assert_eq!(
            d.on_event(NotificationKey(uuid::Uuid::new_v4()), false),
            NotificationOutcome::Display
        );
    }

    #[test]
    fn replace_by_id_platforms_always_repost() {
        let mut d = NotificationDeduper::new();
        let k = NotificationKey(uuid::Uuid::new_v4());
        assert_eq!(d.on_event(k, true), NotificationOutcome::Display);
        assert_eq!(d.on_event(k, true), NotificationOutcome::Display);
    }
}

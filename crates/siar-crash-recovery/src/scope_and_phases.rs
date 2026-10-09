//! §180 "Suggested Crate Structure", §182 "Initial Production Scope",
//! §183 "Implementation Phases", §185 "Relationship to Earlier Parts".
//!
//! Spec sections that are *plans* are kept as data here, and the tests
//! check the data against reality: every module named in a table must
//! exist as a file; every source file must be accounted for; every
//! claimed crate dependency must be in `Cargo.toml` (and every
//! "modelled" one must not be). So these tables can't quietly drift
//! away from the code they describe.
//!
//! An *evidence* string is either a module name (`src/<name>.rs`) or
//! a path starting with `tests/` relative to the crate root.

// ---------------------------------------------------------------------
// §180
// ---------------------------------------------------------------------

/// The spec suggests a 15-file layout. This crate grew finer-grained
/// modules (one per spec section group), so each suggested file maps
/// to the modules that implement it.
pub const STRUCTURE: [(&str, &[&str]); 15] = [
    ("lib.rs", &["lib"]),
    ("coordinator.rs", &["coordinator", "runtime_api"]),
    (
        "phase.rs",
        &["cleanup_quarantine", "progressive", "state_machine"],
    ),
    (
        "context.rs",
        &["coordinator", "recovery_metadata", "shutdown_marker"],
    ),
    ("report.rs", &["reporting"]),
    ("health.rs", &["runtime_api", "recovery_mode"]),
    (
        "storage.rs",
        &[
            "migration_recovery",
            "storage_catalog",
            "fault_sim",
            "config_durability",
            "locks",
        ],
    ),
    ("outbox.rs", &["derived_recovery", "messaging_recovery"]),
    ("projection.rs", &["derived_recovery"]),
    (
        "files.rs",
        &[
            "chunk_recovery",
            "finalization_recovery",
            "atomic_rename",
            "operation_state",
            "staged_intent",
        ],
    ),
    ("dtn.rs", &["dtn_recovery"]),
    (
        "identity.rs",
        &[
            "identity_recovery",
            "recovery_security",
            "key_recovery",
            "device_linking",
        ],
    ),
    (
        "cleanup.rs",
        &["cleanup_quarantine", "temp_scan", "blob_refs"],
    ),
    ("quarantine.rs", &["cleanup_quarantine"]),
    ("error.rs", &["error_model"]),
];

/// Modules with no counterpart in the suggested layout (spec sections
/// the suggested tree didn't anticipate).
pub const EXTRA_MODULES: &[&str] = &[
    "backup_restore",
    "compat_boundaries",
    "config_durability",
    "corruption",
    "crash_injection",
    "definition_of_done",
    "effect_identity",
    "effect_journal",
    "effects_io",
    "ephemeral_recovery",
    "ephemeral_state",
    "extensions",
    "failpoint",
    "fault_policy",
    "fault_scenarios",
    "idempotent_steps",
    "integrity_maintenance",
    "interop",
    "invariants",
    "local_first",
    "platform_recovery",
    "reevaluation",
    "recovery_scheduler",
    "repair_tools",
    "restore_scope",
    "route_resource_recovery",
    "scope_and_phases",
    "session_recovery",
    "startup_policy",
    "supervision",
    "timing_recovery",
    "transaction_group",
];

// ---------------------------------------------------------------------
// §182
// ---------------------------------------------------------------------

/// "Implement first", each with the evidence that it exists.
pub const IMPLEMENT_FIRST: [(&str, &[&str]); 14] = [
    (
        "unclean-shutdown detection",
        &["shutdown_marker", "recovery_metadata"],
    ),
    ("recovery coordinator", &["coordinator", "runtime_api"]),
    ("outbox reconciliation", &["derived_recovery"]),
    ("projection resume/rebuild", &["derived_recovery"]),
    ("file staging scan", &["temp_scan", "chunk_recovery"]),
    ("chunk re-verification", &["chunk_recovery", "corruption"]),
    (
        "blob finalization repair",
        &["finalization_recovery", "atomic_rename"],
    ),
    ("DTN bundle restore", &["dtn_recovery"]),
    (
        "identity integrity/revocation restore",
        &["identity_recovery"],
    ),
    (
        "storage reservation cleanup",
        &["route_resource_recovery", "blob_refs"],
    ),
    ("ephemeral session invalidation", &["session_recovery"]),
    ("typed recovery report", &["reporting"]),
    (
        "fault injection",
        &[
            "crash_injection",
            "failpoint",
            "fault_sim",
            "fault_scenarios",
        ],
    ),
    ("process-kill tests", &["tests/process_kill.rs"]),
];

/// "Defer initially" — deliberately *not* built. Recorded so absence
/// is a decision, not an omission.
pub const DEFERRED: [&str; 3] = [
    "complex peer-assisted database repair",
    "automatic authoritative-event surgery",
    "cluster-wide distributed recovery",
];

// ---------------------------------------------------------------------
// §183
// ---------------------------------------------------------------------

pub struct Phase {
    pub number: u8,
    pub name: &'static str,
    pub items: &'static [&'static str],
    pub evidence: &'static [&'static str],
}

pub const PHASES: [Phase; 7] = [
    Phase {
        number: 1,
        name: "Recovery Metadata",
        items: &["shutdown marker", "recovery generation", "health states"],
        evidence: &["shutdown_marker", "recovery_metadata", "runtime_api"],
    },
    Phase {
        number: 2,
        name: "Event/Outbox",
        items: &[
            "projection checkpoints",
            "outbox reconciliation",
            "idempotent resend",
        ],
        evidence: &["derived_recovery", "effect_identity", "messaging_recovery"],
    },
    Phase {
        number: 3,
        name: "Files",
        items: &[
            "staging scan",
            "chunk verify",
            "finalization recovery",
            "orphan cleanup",
        ],
        evidence: &[
            "temp_scan",
            "chunk_recovery",
            "finalization_recovery",
            "atomic_rename",
        ],
    },
    Phase {
        number: 4,
        name: "DTN",
        items: &["bundle restore", "expiry", "tombstones", "forward history"],
        evidence: &["dtn_recovery"],
    },
    Phase {
        number: 5,
        name: "Identity",
        items: &[
            "generation",
            "revocation",
            "fork detection",
            "secure store validation",
        ],
        evidence: &["identity_recovery", "recovery_security", "corruption"],
    },
    Phase {
        number: 6,
        name: "Resource Cleanup",
        items: &[
            "storage reservations",
            "stale temp",
            "runtime permits reset",
        ],
        evidence: &["route_resource_recovery", "temp_scan"],
    },
    Phase {
        number: 7,
        name: "Hardening",
        items: &[
            "failpoints",
            "SIGKILL tests",
            "disk full",
            "corruption",
            "migration interruption",
        ],
        evidence: &[
            "failpoint",
            "tests/process_kill.rs",
            "fault_policy",
            "corruption",
            "migration_recovery",
        ],
    },
];

// ---------------------------------------------------------------------
// §185
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Coupling {
    /// This crate depends on the earlier part's crate and uses its
    /// types.
    CrateDependency(&'static str),
    /// Built on the earlier part's *concepts* via plain summary types,
    /// with no crate dependency (so the earlier crate can evolve
    /// without breaking recovery, and vice versa).
    ModelledByPlainTypes,
}

/// "Part 09 builds on" 01-08.
pub const BUILDS_ON: [(u8, &str, Coupling); 8] = [
    (
        1,
        "Protocol Extension System",
        Coupling::ModelledByPlainTypes,
    ),
    (2, "Multi-Device Identity", Coupling::ModelledByPlainTypes),
    (
        3,
        "Transport & Routing Policy Engine",
        Coupling::CrateDependency("siar-routing-policy"),
    ),
    (
        4,
        "Offline Event Log",
        Coupling::CrateDependency("siar-event-log"),
    ),
    (
        5,
        "Robust File / Blob Subsystem",
        Coupling::CrateDependency("siar-blob-manifest"),
    ),
    (
        6,
        "DTN / Store-Carry-Forward",
        Coupling::CrateDependency("siar-dtn-bundle"),
    ),
    (7, "Capability Negotiation", Coupling::ModelledByPlainTypes),
    (
        8,
        "Resource Limits & Backpressure",
        Coupling::ModelledByPlainTypes,
    ),
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::path::{Path, PathBuf};

    fn root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
    }

    fn evidence_exists(e: &str) -> bool {
        if e.starts_with("tests/") {
            root().join(e).is_file()
        } else {
            root().join("src").join(format!("{e}.rs")).is_file()
        }
    }

    fn src_modules() -> HashSet<String> {
        std::fs::read_dir(root().join("src"))
            .unwrap()
            .filter_map(|e| {
                let p = e.unwrap().path();
                (p.extension()? == "rs")
                    .then(|| p.file_stem().unwrap().to_string_lossy().into_owned())
            })
            .collect()
    }

    #[test]
    fn every_module_named_in_the_structure_table_exists() {
        for (suggested, ours) in STRUCTURE {
            assert!(!ours.is_empty(), "{suggested} maps to nothing");
            for m in ours {
                assert!(evidence_exists(m), "{suggested} → missing src/{m}.rs");
            }
        }
    }

    #[test]
    fn every_source_file_is_accounted_for_in_the_docs() {
        let mut known: HashSet<&str> = HashSet::new();
        for (_, ours) in STRUCTURE {
            known.extend(ours.iter().copied());
        }
        known.extend(EXTRA_MODULES.iter().copied());
        let actual = src_modules();
        let unaccounted: Vec<_> = actual
            .iter()
            .filter(|m| !known.contains(m.as_str()))
            .collect();
        assert!(
            unaccounted.is_empty(),
            "undocumented modules: {unaccounted:?}"
        );
        let phantom: Vec<_> = known.iter().filter(|m| !actual.contains(**m)).collect();
        assert!(
            phantom.is_empty(),
            "documented but missing modules: {phantom:?}"
        );
    }

    #[test]
    fn every_suggested_file_from_the_spec_is_mapped() {
        let names: Vec<_> = STRUCTURE.iter().map(|(n, _)| *n).collect();
        for expected in [
            "lib.rs",
            "coordinator.rs",
            "phase.rs",
            "context.rs",
            "report.rs",
            "health.rs",
            "storage.rs",
            "outbox.rs",
            "projection.rs",
            "files.rs",
            "dtn.rs",
            "identity.rs",
            "cleanup.rs",
            "quarantine.rs",
            "error.rs",
        ] {
            assert!(names.contains(&expected), "{expected}");
        }
    }

    #[test]
    fn all_fourteen_implement_first_items_have_real_evidence() {
        assert_eq!(IMPLEMENT_FIRST.len(), 14);
        for (item, ev) in IMPLEMENT_FIRST {
            assert!(!ev.is_empty(), "{item}");
            for e in ev {
                assert!(evidence_exists(e), "{item}: missing evidence {e}");
            }
        }
    }

    #[test]
    fn deferred_items_are_recorded_and_not_built() {
        assert_eq!(DEFERRED.len(), 3);
        // No module is named for a deferred capability.
        let mods = src_modules();
        for m in &mods {
            for bad in [
                "peer_db_repair",
                "event_surgery",
                "cluster_recovery",
                "distributed",
            ] {
                assert!(!m.contains(bad), "{m} looks like a deferred feature");
            }
        }
    }

    #[test]
    fn the_seven_phases_are_in_order_and_each_has_evidence() {
        for (i, p) in PHASES.iter().enumerate() {
            assert_eq!(p.number as usize, i + 1);
            assert!(!p.items.is_empty() && !p.evidence.is_empty(), "{}", p.name);
            for e in p.evidence {
                assert!(
                    evidence_exists(e),
                    "phase {} {}: missing {e}",
                    p.number,
                    p.name
                );
            }
        }
        assert_eq!(PHASES[6].name, "Hardening");
    }

    fn cargo_toml() -> String {
        std::fs::read_to_string(root().join("Cargo.toml")).unwrap()
    }

    fn declares_dependency(toml: &str, name: &str) -> bool {
        // a `[dependencies]`-style line beginning with the crate name
        toml.lines().any(|l| {
            l.trim_start().starts_with(&format!("{name} "))
                || l.trim_start().starts_with(&format!("{name}="))
        })
    }

    #[test]
    fn claimed_dependencies_are_real_and_modelled_parts_have_none() {
        let toml = cargo_toml();
        for (part, name, coupling) in BUILDS_ON {
            match coupling {
                Coupling::CrateDependency(c) => {
                    assert!(
                        declares_dependency(&toml, c),
                        "part {part} {name}: {c} not in Cargo.toml"
                    );
                }
                Coupling::ModelledByPlainTypes => {
                    for forbidden in [
                        "siar-protocol-ext",
                        "siar-identity-multidevice",
                        "siar-capability",
                        "siar-resource-limits",
                    ] {
                        assert!(
                            !declares_dependency(&toml, forbidden),
                            "part {part} {name} is documented as modelled, but {forbidden} is a dependency"
                        );
                    }
                }
            }
        }
        assert_eq!(BUILDS_ON.len(), 8);
        let _ = Path::new("");
    }

    #[test]
    fn claimed_dependencies_are_actually_used_in_code() {
        // An unused dependency would make the §185 claim hollow.
        let all_src: String = src_modules()
            .iter()
            .map(|m| std::fs::read_to_string(root().join("src").join(format!("{m}.rs"))).unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        for (part, name, coupling) in BUILDS_ON {
            if let Coupling::CrateDependency(c) = coupling {
                let ident = c.replace('-', "_");
                assert!(
                    all_src.contains(&format!("{ident}::")),
                    "part {part} {name}: {c} is never used"
                );
            }
        }
    }
}

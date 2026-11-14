//! `cargo xtask` — repository automation in pure Rust. Replaces every shell
//! script (the old `scripts/*.sh`); see `cargo xtask --help`.

mod android;
mod audit_pure;
mod ci;
mod e2e;
mod elf;
mod harness;
mod mls_storage;
mod musl;
mod scenarios;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "xtask",
    about = "Siar repository automation (no shell scripts)"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Regenerate `crates/siar-crypto-mls/src/persistent_delegate.rs` from the
    /// `openmls_traits::storage::StorageProvider` source. With `--check`,
    /// fail (exit 1) if the committed file is out of date instead of writing.
    GenMlsStorage {
        #[arg(long)]
        check: bool,
    },
    /// Audit the resolved dependency graph for C/C++ (build steps, native
    /// links). Fails if any non-allowed native dependency is reachable from
    /// the given workspace packages.
    AuditPure {
        /// Target triple to resolve for (default: the host).
        #[arg(long)]
        target: Option<String>,
        /// Only these workspace packages (default: all).
        #[arg(long = "package", short = 'p')]
        packages: Vec<String>,
        /// Also list the exempt (Slint) and inactive entries.
        #[arg(long)]
        verbose: bool,
        /// Fail on TRACKED violations too, not only unexplained ones.
        #[arg(long)]
        strict: bool,
    },
    /// Two daemons pair and exchange messages (was `scripts/daemon-smoke.sh`).
    DaemonSmoke {
        /// Use the binaries already in target/debug.
        #[arg(long)]
        no_build: bool,
    },
    /// The window-facing `ui-state` sync protocol (was `scripts/daemon-sync-smoke.sh`).
    DaemonSyncSmoke {
        #[arg(long)]
        no_build: bool,
    },
    /// Config validation, `--replace` takeover, SIGHUP reload, clean stop
    /// (was `scripts/daemon-upgrade-smoke.sh`).
    DaemonUpgradeSmoke {
        #[arg(long)]
        no_build: bool,
    },
    /// FROST ceremony -> enrol a daemon with a threshold signature -> it
    /// restarts into its role and the role's child is supervised.
    E2eProvision {
        #[arg(long, default_value = "relay-gateway")]
        role: String,
        #[arg(long)]
        no_build: bool,
    },
    /// Remote administration: roles, audit, no shell, shutdown reply.
    E2eAdmin {
        #[arg(long)]
        no_build: bool,
    },
    /// The durable hash-chained audit log: verify, continue, detect tampering.
    E2eAudit {
        #[arg(long)]
        no_build: bool,
    },
    /// Live backup and staged restore.
    E2eBackup {
        #[arg(long)]
        no_build: bool,
    },
    /// Everything CI runs: fmt, generated-code drift, native-dependency audit,
    /// clippy `-D warnings`, tests. A workflow file is just `cargo xtask ci`.
    Ci {
        #[arg(long = "package", short = 'p')]
        packages: Vec<String>,
        #[arg(long)]
        skip_tests: bool,
    },
    /// Are the prerequisites for a musl build present (std for the target, musl-gcc)?
    CheckMusl {
        #[arg(long, default_value = musl::DEFAULT_TARGET)]
        target: String,
    },
    /// Build the headless binaries for musl and verify each is statically
    /// linked. (Slint/desktop-audio stay on glibc by design.)
    BuildMusl {
        #[arg(long, default_value = musl::DEFAULT_TARGET)]
        target: String,
        #[arg(long)]
        release: bool,
        #[arg(long = "package", short = 'p')]
        packages: Vec<String>,
    },
    /// Is this executable statically linked? (reads the ELF program headers)
    CheckStatic { file: std::path::PathBuf },
    /// Cross-compile the Android JNI libraries with cargo-ndk
    /// (was `apps/android/build-native.sh`).
    AndroidNative {
        /// Print the `cargo ndk` command instead of running it.
        #[arg(long)]
        dry_run: bool,
    },
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::GenMlsStorage { check } => mls_storage::run(check),
        Cmd::AuditPure {
            target,
            packages,
            verbose,
            strict,
        } => audit_pure::run_strict(target.as_deref(), &packages, verbose, strict),
        Cmd::DaemonSmoke { no_build } => {
            scenarios::daemon_smoke(&harness::Bins::locate(!no_build, &["siar-daemon"])?)
        }
        Cmd::DaemonSyncSmoke { no_build } => {
            scenarios::daemon_sync_smoke(&harness::Bins::locate(!no_build, &["siar-daemon"])?)
        }
        Cmd::DaemonUpgradeSmoke { no_build } => {
            scenarios::daemon_upgrade_smoke(&harness::Bins::locate(!no_build, &["siar-daemon"])?)
        }
        Cmd::AndroidNative { dry_run } => android::run(dry_run),
        Cmd::Ci {
            packages,
            skip_tests,
        } => ci::run(&packages, skip_tests),
        Cmd::E2eProvision { role, no_build } => e2e::e2e_provision(
            &harness::Bins::locate(
                !no_build,
                &["siar-daemon", "siar-emergency-node", "siar-root"],
            )?,
            &role,
        ),
        Cmd::E2eAdmin { no_build } => {
            e2e::e2e_admin(&harness::Bins::locate(!no_build, &["siar-daemon"])?)
        }
        Cmd::E2eAudit { no_build } => {
            e2e::e2e_audit(&harness::Bins::locate(!no_build, &["siar-daemon"])?)
        }
        Cmd::E2eBackup { no_build } => {
            e2e::e2e_backup(&harness::Bins::locate(!no_build, &["siar-daemon"])?)
        }
        Cmd::CheckMusl { target } => musl::check(&target),
        Cmd::BuildMusl {
            target,
            release,
            packages,
        } => musl::build(&target, release, &packages),
        Cmd::CheckStatic { file } => musl::check_static(&file),
    }
}

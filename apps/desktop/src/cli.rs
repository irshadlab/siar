//! Command-line side of the desktop binary: backup and restore of the data
//! folder. (No window, no core: these must work on a machine where the app
//! can't start — that is exactly when a restore is needed.)
//!
//! ```text
//! siar-desktop                       run the app
//! siar-desktop backup  <file>        encrypt the data folder into <file>
//! siar-desktop restore <file> [dir]  restore into an EMPTY folder (default: the data folder)
//! siar-desktop backup-info <file>    show what can be read without the passphrase
//! ```
//!
//! The passphrase is asked for without echo (twice when creating). For
//! scripts, `SIAR_BACKUP_PASSPHRASE` is honoured instead — convenient, but
//! environment variables are visible to other processes of the same user,
//! so the prompt is the safer default.

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use siar_backup::BackupError;

pub const USAGE: &str = "\
Usage:
  siar-desktop                       run the app
  siar-desktop backup  <file>        back up your account and data into an encrypted file
  siar-desktop restore <file> [dir]  restore a backup into an empty folder
                                     (default: this device's data folder)
  siar-desktop backup-info <file>    show a backup's details (no passphrase needed)

Close the app before backing up or restoring.";

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Run,
    Backup {
        out: PathBuf,
    },
    Restore {
        archive: PathBuf,
        target: Option<PathBuf>,
    },
    BackupInfo {
        file: PathBuf,
    },
    Help,
}

pub fn parse(args: &[String]) -> Result<Command, String> {
    let mut it = args.iter().map(String::as_str);
    let Some(first) = it.next() else {
        return Ok(Command::Run);
    };
    let rest: Vec<&str> = it.collect();
    match (first, rest.as_slice()) {
        ("-h" | "--help" | "help", _) => Ok(Command::Help),
        ("backup", [out]) => Ok(Command::Backup { out: out.into() }),
        ("restore", [archive]) => Ok(Command::Restore {
            archive: archive.into(),
            target: None,
        }),
        ("restore", [archive, dir]) => Ok(Command::Restore {
            archive: archive.into(),
            target: Some(dir.into()),
        }),
        ("backup-info", [file]) => Ok(Command::BackupInfo { file: file.into() }),
        ("backup" | "restore" | "backup-info", _) => {
            Err(format!("wrong number of arguments for `{first}`"))
        }
        (other, _) => Err(format!("unknown command `{other}`")),
    }
}

fn passphrase(confirm: bool) -> Result<String> {
    if let Ok(p) = std::env::var("SIAR_BACKUP_PASSPHRASE") {
        return Ok(p);
    }
    let first =
        rpassword::prompt_password("Backup passphrase: ").context("reading the passphrase")?;
    if confirm {
        if let Some(problem) = siar_backup::passphrase_problem(&first) {
            bail!("that passphrase is too weak: {problem}");
        }
        let again =
            rpassword::prompt_password("Repeat passphrase: ").context("reading the passphrase")?;
        if again != first {
            bail!("the two passphrases didn’t match; nothing was written");
        }
    }
    Ok(first)
}

fn describe(millis: u64) -> String {
    // No date library in this binary; seconds since the epoch is exact and
    // unambiguous, and the file name carries the human-friendly date.
    format!("{} s since 1970-01-01 UTC", millis / 1000)
}

fn friendly(e: BackupError) -> anyhow::Error {
    anyhow::anyhow!("{e}")
}

pub fn execute(command: Command) -> Result<()> {
    match command {
        Command::Run => unreachable!("the app itself is started by main"),
        Command::Help => println!("{USAGE}"),
        Command::BackupInfo { file } => {
            let info = siar_backup::inspect(&file).map_err(friendly)?;
            println!("Siar backup, format {}", info.version);
            println!("Created: {}", describe(info.created_at_millis));
            println!("Encrypted size: {} bytes", info.encrypted_bytes);
            println!(
                "Key stretching: Argon2id, {} MiB, {} passes",
                info.kdf.mem_kib / 1024,
                info.kdf.iterations
            );
            println!("The contents can only be read with the passphrase.");
        }
        Command::Backup { out } => {
            let data_dir = siar_app_core::bootstrap::data_dir_path()?;
            if !data_dir.exists() {
                bail!(
                    "there is no Siar data folder at {} — nothing to back up",
                    data_dir.display()
                );
            }
            let pass = passphrase(true)?;
            println!("Backing up {} …", data_dir.display());
            let summary = siar_backup::create_backup(&data_dir, &pass, &out).map_err(friendly)?;
            println!(
                "Done: {} files ({} bytes) → {}",
                summary.files,
                summary.plaintext_bytes,
                out.display()
            );
            println!("Keep the passphrase safe: without it this file cannot be opened, and nobody can recover it for you.");
        }
        Command::Restore { archive, target } => {
            let target = match target {
                Some(t) => t,
                None => siar_app_core::bootstrap::data_dir_path()?,
            };
            let pass = passphrase(false)?;
            println!("Restoring into {} …", target.display());
            let summary =
                siar_backup::restore_backup(&archive, &pass, &target).map_err(friendly)?;
            println!(
                "Done: {} files restored. Start Siar to use this account.",
                summary.files
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(a: &[&str]) -> Result<Command, String> {
        parse(&a.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn no_arguments_runs_the_app() {
        assert_eq!(p(&[]), Ok(Command::Run));
    }

    #[test]
    fn each_subcommand_parses_with_its_arguments() {
        assert_eq!(
            p(&["backup", "a.bak"]),
            Ok(Command::Backup {
                out: "a.bak".into()
            })
        );
        assert_eq!(
            p(&["restore", "a.bak"]),
            Ok(Command::Restore {
                archive: "a.bak".into(),
                target: None
            })
        );
        assert_eq!(
            p(&["restore", "a.bak", "/tmp/new"]),
            Ok(Command::Restore {
                archive: "a.bak".into(),
                target: Some("/tmp/new".into())
            })
        );
        assert_eq!(
            p(&["backup-info", "a.bak"]),
            Ok(Command::BackupInfo {
                file: "a.bak".into()
            })
        );
        assert_eq!(p(&["--help"]), Ok(Command::Help));
    }

    #[test]
    fn wrong_argument_counts_and_unknown_commands_are_errors_not_runs() {
        for bad in [
            &["backup"][..],
            &["backup", "a", "b"],
            &["restore"],
            &["restore", "a", "b", "c"],
            &["backup-info"],
            &["frobnicate"],
        ] {
            assert!(p(bad).is_err(), "{bad:?}");
        }
    }
}

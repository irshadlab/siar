//! Part 09 §138-§139: was the previous run shut down cleanly?
//!
//! `begin` drops a marker file in the data folder when the daemon starts;
//! `finish` removes it on a graceful stop, *after* the core has shut down and
//! *before* the data-folder lock is released. A marker still there at the next
//! start means the process died (crash, kill -9, power loss), which is what
//! `siar_crash_recovery::coordinator::startup_mode` turns into a thorough
//! start. A first run has no marker and counts as clean.
//!
//! The marker is deliberately not removed on `Drop`: a panic unwinding through
//! `run` is a crash, not a clean stop.

use siar_crash_recovery::coordinator::{startup_mode, RecoveryMode, StartupFacts};
use std::io;
use std::path::{Path, PathBuf};

const FILE: &str = "run.marker";

#[derive(Debug)]
pub struct RunMarker {
    path: PathBuf,
}

impl RunMarker {
    /// Creates the marker and reports whether the previous run ended cleanly.
    /// If the marker's presence can't be determined the answer is "not clean":
    /// the conservative side is the thorough start.
    pub fn begin(dir: &Path) -> io::Result<(Self, bool)> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join(FILE);
        let previous_clean = matches!(path.try_exists(), Ok(false));
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis());
        std::fs::write(
            &path,
            format!("pid={} started_ms={now}\n", std::process::id()),
        )?;
        Ok((Self { path }, previous_clean))
    }

    /// Graceful stop: the next start will see a clean shutdown.
    pub fn finish(self) {
        match std::fs::remove_file(&self.path) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => {
                tracing::warn!(error = %e, "couldn’t remove the run marker; the next start will do a thorough check")
            }
        }
    }
}

/// §138: the recovery mode this start runs in. No operator-forced read-only
/// or repair request exists in the daemon yet, so only an unclean previous
/// shutdown changes the answer.
#[must_use]
pub fn mode_for(previous_clean: bool) -> RecoveryMode {
    startup_mode(StartupFacts {
        previous_shutdown_clean: previous_clean,
        force_read_only: false,
        admin_repair_requested: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        std::env::temp_dir().join(format!("siar-runmarker-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn first_run_is_clean_and_a_graceful_stop_keeps_it_clean() {
        let d = tmp();
        let (m, clean) = RunMarker::begin(d.as_path()).unwrap();
        assert!(clean, "no marker yet: first run");
        assert!(d.as_path().join(FILE).exists());
        m.finish();
        assert!(!d.as_path().join(FILE).exists());
        let (_m2, clean2) = RunMarker::begin(d.as_path()).unwrap();
        assert!(clean2, "finished marker: previous stop was graceful");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_leftover_marker_means_the_previous_run_crashed() {
        let d = tmp();
        let (m, _) = RunMarker::begin(d.as_path()).unwrap();
        drop(m); // no finish(): simulates a crash
        let (_m2, clean) = RunMarker::begin(d.as_path()).unwrap();
        assert!(!clean);
        assert_eq!(mode_for(clean), RecoveryMode::Thorough);
        assert_eq!(mode_for(true), RecoveryMode::Normal);
        let _ = std::fs::remove_dir_all(&d);
    }
}

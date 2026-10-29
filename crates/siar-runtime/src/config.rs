//! §85-§88: runtime configuration.
//!
//! * **TOML, strict.** Unknown keys are errors (`deny_unknown_fields`), so a
//!   typo can't silently run with a default, and a secret pasted into the
//!   file as a value is rejected rather than ignored.
//! * **Secrets are referenced, never embedded** (§86). The file may say
//!   *where* a secret lives (a file, an environment variable); it cannot
//!   hold one. Secret files must be owner-only on Unix.
//! * **Validated before use** (§88), with every problem reported at once.
//! * **Hot reload is classified** (§87): only values that can change safely
//!   at runtime are applied live; the rest are reported as *restart required*
//!   instead of being pretended into effect.

use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct IpcConfig {
    pub max_connections: usize,
    pub requests_per_sec: u32,
    pub write_timeout_ms: u64,
    pub hello_timeout_ms: u64,
}

impl Default for IpcConfig {
    fn default() -> Self {
        Self {
            max_connections: 32,
            requests_per_sec: 200,
            write_timeout_ms: 10_000,
            hello_timeout_ms: 5_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ShutdownConfig {
    pub grace_secs: u64,
}

impl Default for ShutdownConfig {
    fn default() -> Self {
        Self { grace_secs: 10 }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct PowerSection {
    pub allow_background_bulk: bool,
    pub allow_metered_bulk: bool,
    pub multipath_on_battery: bool,
    pub emergency_override: bool,
}

impl Default for PowerSection {
    fn default() -> Self {
        Self {
            allow_background_bulk: true,
            allow_metered_bulk: false,
            multipath_on_battery: false,
            emergency_override: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct LoggingConfig {
    pub level: String,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".into(),
        }
    }
}

/// Where a secret lives. **Never the secret itself.**
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum SecretRef {
    File { path: PathBuf },
    Env { name: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct SecretsConfig {
    pub identity_key: Option<SecretRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct RuntimeConfig {
    pub ipc: IpcConfig,
    pub shutdown: ShutdownConfig,
    pub power: PowerSection,
    pub logging: LoggingConfig,
    pub secrets: SecretsConfig,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ConfigError {
    #[error("config is not valid TOML for this schema: {0}")]
    Parse(String),
    #[error("config has {} problem(s): {}", .0.len(), .0.join("; "))]
    Invalid(Vec<String>),
}

const LOG_LEVELS: [&str; 5] = ["error", "warn", "info", "debug", "trace"];

impl RuntimeConfig {
    /// Parse **and** validate.
    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        let c: Self =
            toml::from_str(text).map_err(|e| ConfigError::Parse(e.message().to_owned()))?;
        c.validate().map(|()| c)
    }

    #[must_use]
    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).unwrap_or_default()
    }

    /// Report every problem, not just the first.
    pub fn validate(&self) -> Result<(), ConfigError> {
        let mut p = Vec::new();
        if !(1..=1024).contains(&self.ipc.max_connections) {
            p.push("ipc.max_connections must be 1..=1024".to_owned());
        }
        if !(1..=100_000).contains(&self.ipc.requests_per_sec) {
            p.push("ipc.requests_per_sec must be 1..=100000".to_owned());
        }
        if !(100..=300_000).contains(&self.ipc.write_timeout_ms) {
            p.push("ipc.write_timeout_ms must be 100..=300000".to_owned());
        }
        if !(100..=60_000).contains(&self.ipc.hello_timeout_ms) {
            p.push("ipc.hello_timeout_ms must be 100..=60000".to_owned());
        }
        if !(1..=600).contains(&self.shutdown.grace_secs) {
            p.push("shutdown.grace_secs must be 1..=600".to_owned());
        }
        if !LOG_LEVELS.contains(&self.logging.level.as_str()) {
            p.push(format!(
                "logging.level must be one of {}",
                LOG_LEVELS.join(", ")
            ));
        }
        match &self.secrets.identity_key {
            Some(SecretRef::Env { name })
                if name.is_empty() || name.contains('=') || name.contains('\0') =>
            {
                p.push("secrets.identity_key env name is not a valid variable name".to_owned());
            }
            Some(SecretRef::File { path }) if path.as_os_str().is_empty() => {
                p.push("secrets.identity_key file path is empty".to_owned());
            }
            _ => {}
        }
        if p.is_empty() {
            Ok(())
        } else {
            Err(ConfigError::Invalid(p))
        }
    }

    #[cfg(unix)]
    #[must_use]
    pub fn server_config(&self) -> crate::ipc::ServerConfig {
        crate::ipc::ServerConfig {
            max_connections: self.ipc.max_connections,
            requests_per_sec: self.ipc.requests_per_sec,
            write_timeout: Duration::from_millis(self.ipc.write_timeout_ms),
            hello_timeout: Duration::from_millis(self.ipc.hello_timeout_ms),
            ..Default::default()
        }
    }

    #[must_use]
    pub fn shutdown_grace(&self) -> Duration {
        Duration::from_secs(self.shutdown.grace_secs)
    }

    /// Overlay the user-editable power section onto the deployment's base.
    #[must_use]
    pub fn power_policy(
        &self,
        base: siar_power::PowerPolicyConfig,
    ) -> siar_power::PowerPolicyConfig {
        siar_power::PowerPolicyConfig {
            allow_background_bulk: self.power.allow_background_bulk,
            allow_metered_bulk: self.power.allow_metered_bulk,
            multipath_on_battery: self.power.multipath_on_battery,
            emergency_override: self.power.emergency_override,
            ..base
        }
    }
}

/// §87: what a reload did.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReloadPlan {
    /// Applied immediately.
    pub applied: Vec<&'static str>,
    /// Changed in the file but only effective after a restart.
    pub restart_required: Vec<&'static str>,
}

/// Compare two *valid* configs.
#[must_use]
pub fn plan_reload(old: &RuntimeConfig, new: &RuntimeConfig) -> ReloadPlan {
    let mut p = ReloadPlan::default();
    let mut hot = |changed: bool, name| {
        if changed {
            p.applied.push(name);
        }
    };
    hot(old.power != new.power, "power");
    hot(old.logging != new.logging, "logging.level");
    hot(old.shutdown != new.shutdown, "shutdown.grace_secs");
    let mut cold = |changed: bool, name| {
        if changed {
            p.restart_required.push(name);
        }
    };
    cold(old.ipc != new.ipc, "ipc");
    cold(old.secrets != new.secrets, "secrets");
    p
}

/// A resolved secret. `Debug` never prints the bytes.
pub struct Secret(Vec<u8>);

impl Secret {
    #[must_use]
    pub fn expose(&self) -> &[u8] {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SecretError {
    #[error("secret is not available")]
    Missing,
    #[error("secret file is readable by other users; run chmod 600")]
    TooPermissive,
}

impl SecretRef {
    pub fn resolve(&self) -> Result<Secret, SecretError> {
        match self {
            Self::Env { name } => std::env::var_os(name)
                .map(|v| Secret(v.to_string_lossy().into_owned().into_bytes()))
                .ok_or(SecretError::Missing),
            Self::File { path } => {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let mode = std::fs::metadata(path)
                        .map_err(|_| SecretError::Missing)?
                        .permissions()
                        .mode();
                    if mode & 0o077 != 0 {
                        return Err(SecretError::TooPermissive);
                    }
                }
                std::fs::read(path)
                    .map(Secret)
                    .map_err(|_| SecretError::Missing)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_config_is_all_defaults_and_round_trips() {
        let c = RuntimeConfig::parse("").unwrap();
        assert_eq!(c, RuntimeConfig::default());
        assert_eq!(RuntimeConfig::parse(&c.to_toml()).unwrap(), c);
    }

    #[test]
    fn typos_and_embedded_secrets_are_rejected_not_ignored() {
        for bad in [
            "[ipc]\nmax_connection = 5",
            "password = \"hunter2\"",
            "[secrets]\nidentity_key = \"abc\"",
            "[secrets.identity_key]\nkind = \"file\"\npath = \"/x\"\nvalue = \"s3cret\"",
        ] {
            assert!(
                matches!(RuntimeConfig::parse(bad), Err(ConfigError::Parse(_))),
                "{bad}"
            );
        }
    }

    #[test]
    fn every_validation_problem_is_reported_at_once() {
        let e = RuntimeConfig::parse("[ipc]\nmax_connections = 0\nrequests_per_sec = 0\n[shutdown]\ngrace_secs = 9999\n[logging]\nlevel = \"loud\"").unwrap_err();
        let ConfigError::Invalid(p) = e else { panic!() };
        assert_eq!(p.len(), 4, "{p:?}");
    }

    #[test]
    fn hot_and_cold_changes_are_classified_honestly() {
        let old = RuntimeConfig::default();
        let mut new = old.clone();
        new.power.allow_metered_bulk = true;
        new.logging.level = "debug".into();
        new.ipc.max_connections = 64;
        let p = plan_reload(&old, &new);
        assert_eq!(p.applied, vec!["power", "logging.level"]);
        assert_eq!(p.restart_required, vec!["ipc"]);
        assert_eq!(plan_reload(&old, &old), ReloadPlan::default());
    }

    #[test]
    fn power_section_overlays_but_keeps_the_deployment() {
        let mut c = RuntimeConfig::default();
        c.power.allow_metered_bulk = true;
        let base = siar_power::PowerPolicyConfig {
            deployment: siar_power::Deployment::AlwaysOn,
            ..Default::default()
        };
        let p = c.power_policy(base);
        assert!(p.allow_metered_bulk);
        assert_eq!(p.deployment, siar_power::Deployment::AlwaysOn);
    }

    #[test]
    fn secrets_are_resolved_by_reference_and_never_printed() {
        std::env::set_var("SIAR_TEST_SECRET_X", "hunter2");
        let s = SecretRef::Env {
            name: "SIAR_TEST_SECRET_X".into(),
        }
        .resolve()
        .unwrap();
        assert_eq!(s.expose(), b"hunter2");
        assert!(!format!("{s:?}").contains("hunter2"));
        assert_eq!(
            SecretRef::Env {
                name: "SIAR_NOPE_NOT_SET".into()
            }
            .resolve()
            .unwrap_err(),
            SecretError::Missing
        );
    }

    #[cfg(unix)]
    #[test]
    fn world_readable_secret_files_are_refused() {
        use std::os::unix::fs::PermissionsExt;
        let p = std::env::temp_dir().join(format!("siar-secret-{}", std::process::id()));
        std::fs::write(&p, b"k").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
        let r = SecretRef::File { path: p.clone() };
        assert_eq!(r.resolve().unwrap_err(), SecretError::TooPermissive);
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(r.resolve().unwrap().expose(), b"k");
        let _ = std::fs::remove_file(p);
    }
}

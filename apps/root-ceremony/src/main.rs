//! `siar-root` — the root-key ceremony tool.
//!
//! Each participant runs their own steps on their own machine and passes
//! small hex files around. Everything secret on disk is passphrase-sealed;
//! nothing here invents cryptography (it drives `frost-ed25519`).
//!
//! Passphrase: `--passphrase-file <file>` (must be mode 0600) or the
//! `SIAR_ROOT_PASSPHRASE` environment variable. There is deliberately no
//! `--passphrase <text>` flag: command lines are world-readable in `ps`.
//!
//! ```text
//! # every participant i of n (threshold m):
//! siar-root dkg1 --dir ~/root --id i --min m --max n      # → r1.public.hex  (send to all)
//! siar-root dkg2 --dir ~/root --r1 r1-from-*.hex          # → out/r2-for-<j>.SECRET.hex (send j only!)
//! siar-root dkg3 --dir ~/root --r1 r1-from-*.hex --r2 r2-for-me-*.hex   # prints the root public key
//!
//! # to sign (e.g. provision a node): coordinator + m signers
//! siar-root payload --role relay-gateway --challenge <node QR hex> --admin-pubkey <root pk> --out-dir work
//! siar-root sign1 --dir ~/root                             # each signer → commitment.hex
//! siar-root package --message-file work/payload.message --commitment c1.hex --commitment c2.hex --out package.hex
//! siar-root sign2 --dir ~/root --package package.hex       # each signer → sig-share.hex
//! siar-root aggregate --group ~/root/group.public.hex --package package.hex --share s1.hex --share s2.hex
//! siar-ctl provision work/payload.bin <signature hex>
//! ```

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use clap::{Parser, Subcommand};
use siar_crypto_advanced::ceremony::{self as c, KdfCost, Passphrase};
use siar_runtime::node::{signing_message, NodeRole, ProvisionPayload};

#[derive(Parser)]
#[command(
    name = "siar-root",
    about = "FROST root-key ceremony (standard Ed25519 output)"
)]
struct Cli {
    /// File holding the passphrase (mode 0600). Else `SIAR_ROOT_PASSPHRASE`.
    #[arg(long, global = true)]
    passphrase_file: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// DKG round 1.
    Dkg1 {
        #[arg(long)]
        dir: PathBuf,
        #[arg(long)]
        id: u16,
        #[arg(long)]
        min: u16,
        #[arg(long)]
        max: u16,
    },
    /// DKG round 2 (needs every other participant's round-1 file).
    Dkg2 {
        #[arg(long)]
        dir: PathBuf,
        #[arg(long = "r1", required = true)]
        r1: Vec<PathBuf>,
    },
    /// DKG round 3 (needs the round-2 files addressed to you).
    Dkg3 {
        #[arg(long)]
        dir: PathBuf,
        #[arg(long = "r1", required = true)]
        r1: Vec<PathBuf>,
        #[arg(long = "r2", required = true)]
        r2: Vec<PathBuf>,
    },
    /// Signing round 1 (each signer).
    Sign1 {
        #[arg(long)]
        dir: PathBuf,
    },
    /// Coordinator: build the signing package for a message.
    Package {
        #[arg(long)]
        message_file: PathBuf,
        #[arg(long = "commitment", required = true)]
        commitments: Vec<PathBuf>,
        #[arg(long)]
        out: PathBuf,
    },
    /// Signing round 2 (each signer). Consumes your nonces.
    Sign2 {
        #[arg(long)]
        dir: PathBuf,
        #[arg(long)]
        package: PathBuf,
    },
    /// Coordinator: combine shares into an Ed25519 signature (hex on stdout).
    Aggregate {
        #[arg(long)]
        group: PathBuf,
        #[arg(long)]
        package: PathBuf,
        #[arg(long = "share", required = true)]
        shares: Vec<PathBuf>,
    },
    /// Verify an Ed25519 signature (any signer, FROST or not).
    Verify {
        #[arg(long)]
        pubkey: String,
        #[arg(long)]
        message_file: PathBuf,
        #[arg(long)]
        sig: String,
    },
    /// Build a node-provisioning payload and the exact message to sign.
    Payload {
        #[arg(long)]
        role: String,
        /// The node's challenge (hex), from `ctl provision-status`.
        #[arg(long)]
        challenge: String,
        /// The admin key that will sign (hex Ed25519 public key).
        #[arg(long)]
        admin_pubkey: String,
        /// Optional RuntimeConfig TOML to apply.
        #[arg(long)]
        config_file: Option<PathBuf>,
        #[arg(long, default_value_t = 600_000)]
        valid_for_ms: u64,
        #[arg(long)]
        out_dir: PathBuf,
    },
}

fn passphrase(cli: &Cli) -> Result<Passphrase> {
    let text = if let Some(p) = &cli.passphrase_file {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(p)?.permissions().mode();
            if mode & 0o077 != 0 {
                bail!("{} is readable by others; run chmod 600", p.display());
            }
        }
        fs::read_to_string(p).with_context(|| format!("reading {}", p.display()))?
    } else {
        std::env::var("SIAR_ROOT_PASSPHRASE")
            .map_err(|_| anyhow!("set SIAR_ROOT_PASSPHRASE or pass --passphrase-file"))?
    };
    Passphrase::new(text.trim_end_matches(['\n', '\r']), KdfCost::DEFAULT)
        .map_err(|e| anyhow!("{e}"))
}

fn read_all(paths: &[PathBuf]) -> Result<Vec<String>> {
    paths
        .iter()
        .map(|p| fs::read_to_string(p).with_context(|| format!("reading {}", p.display())))
        .collect()
}

fn hex32(s: &str, what: &str) -> Result<[u8; 32]> {
    c::from_hex(s)
        .ok()
        .and_then(|b| <[u8; 32]>::try_from(b).ok())
        .ok_or_else(|| anyhow!("{what} must be 64 hex characters"))
}

fn write_text(path: &Path, text: &str) -> Result<()> {
    fs::write(path, text).with_context(|| format!("writing {}", path.display()))
}

fn run(cli: &Cli) -> Result<String> {
    Ok(match &cli.cmd {
        Cmd::Dkg1 { dir, id, min, max } => {
            let hex = c::dkg_round1(dir, &passphrase(cli)?, *id, *min, *max)
                .map_err(|e| anyhow!("{e}"))?;
            format!(
                "wrote {}/r1.public.hex — send it to every other participant\n{hex}\n",
                dir.display()
            )
        }
        Cmd::Dkg2 { dir, r1 } => {
            let out = c::dkg_round2(dir, &passphrase(cli)?, &read_all(r1)?)
                .map_err(|e| anyhow!("{e}"))?;
            fs::create_dir_all(dir.join("out"))?;
            let mut msg = String::from("round-2 files are SECRET: send each only to its recipient, over a confidential channel\n");
            for (to, hex) in out {
                let p = dir.join("out").join(format!("r2-for-{to}.SECRET.hex"));
                write_text(&p, &hex)?;
                msg.push_str(&format!("  {} → participant {to}\n", p.display()));
            }
            msg
        }
        Cmd::Dkg3 { dir, r1, r2 } => {
            let root = c::dkg_round3(dir, &passphrase(cli)?, &read_all(r1)?, &read_all(r2)?)
                .map_err(|e| anyhow!("{e}"))?;
            format!(
                "root public key (Ed25519): {}\nshare sealed in {}/share.sealed\n",
                c::to_hex(&root),
                dir.display()
            )
        }
        Cmd::Sign1 { dir } => {
            let hex = c::sign_round1(dir, &passphrase(cli)?).map_err(|e| anyhow!("{e}"))?;
            write_text(&dir.join("commitment.hex"), &hex)?;
            format!(
                "wrote {}/commitment.hex — send it to the coordinator\n",
                dir.display()
            )
        }
        Cmd::Package {
            message_file,
            commitments,
            out,
        } => {
            let msg = fs::read(message_file)
                .with_context(|| format!("reading {}", message_file.display()))?;
            let hex = c::make_signing_package(&msg, &read_all(commitments)?)
                .map_err(|e| anyhow!("{e}"))?;
            write_text(out, &hex)?;
            format!("wrote {} — send it to the signers\n", out.display())
        }
        Cmd::Sign2 { dir, package } => {
            let pkg = fs::read_to_string(package)?;
            let hex = c::sign_round2(dir, &passphrase(cli)?, &pkg).map_err(|e| anyhow!("{e}"))?;
            write_text(&dir.join("sig-share.hex"), &hex)?;
            format!("wrote {}/sig-share.hex — send it to the coordinator (your nonces are now consumed)\n", dir.display())
        }
        Cmd::Aggregate {
            group,
            package,
            shares,
        } => {
            let sig = c::aggregate_signature(
                &fs::read_to_string(group)?,
                &fs::read_to_string(package)?,
                &read_all(shares)?,
            )
            .map_err(|e| anyhow!("{e}"))?;
            format!("{}\n", c::to_hex(&sig))
        }
        Cmd::Verify {
            pubkey,
            message_file,
            sig,
        } => {
            let pk = hex32(pubkey, "pubkey")?;
            let sig: [u8; 64] = c::from_hex(sig)
                .ok()
                .and_then(|b| b.try_into().ok())
                .ok_or_else(|| anyhow!("sig must be 128 hex characters"))?;
            let msg = fs::read(message_file)?;
            siar_crypto_advanced::frost_root::verify_ed25519_strict(&pk, &msg, &sig)
                .map_err(|e| anyhow!("{e}"))?;
            "signature OK\n".to_owned()
        }
        Cmd::Payload {
            role,
            challenge,
            admin_pubkey,
            config_file,
            valid_for_ms,
            out_dir,
        } => {
            let role = NodeRole::parse(role).ok_or_else(|| {
                anyhow!(
                    "unknown role; one of: {}",
                    NodeRole::ALL.map(NodeRole::name).join(", ")
                )
            })?;
            let challenge: [u8; 16] = c::from_hex(challenge)
                .ok()
                .and_then(|b| b.try_into().ok())
                .ok_or_else(|| anyhow!("challenge must be 32 hex characters"))?;
            let admin = hex32(admin_pubkey, "admin pubkey")?;
            let payload = ProvisionPayload {
                role,
                config_toml: match config_file {
                    Some(p) => fs::read_to_string(p)?,
                    None => String::new(),
                },
                issuer: c::to_hex(&admin),
                issued_at_ms: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)?
                    .as_millis() as u64,
                valid_for_ms: *valid_for_ms,
                challenge,
            };
            fs::create_dir_all(out_dir)?;
            fs::write(
                out_dir.join("payload.bin"),
                postcard::to_allocvec(&payload)?,
            )?;
            fs::write(out_dir.join("payload.message"), signing_message(&payload))?;
            format!(
                "wrote payload.bin and payload.message in {}\nsign payload.message with the admin key (FROST: package/sign2/aggregate), then: siar-ctl provision payload.bin <signature hex>\nvalid for {} ms from now\n",
                out_dir.display(),
                valid_for_ms
            )
        }
    })
}

fn main() {
    let cli = Cli::parse();
    match run(&cli) {
        Ok(out) => print!("{out}"),
        Err(e) => {
            eprintln!("error: {e:#}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn a_payload_is_what_the_daemon_will_verify() {
        let dir = std::env::temp_dir().join(format!("siar-root-pl-{}", std::process::id()));
        let cli = Cli::parse_from([
            "siar-root",
            "payload",
            "--role",
            "dtn-node",
            "--challenge",
            &"ab".repeat(16),
            "--admin-pubkey",
            &"cd".repeat(32),
            "--out-dir",
            dir.to_str().unwrap(),
        ]);
        let msg = run(&cli).unwrap();
        assert!(msg.contains("payload.message"));
        let bytes = fs::read(dir.join("payload.bin")).unwrap();
        let p: ProvisionPayload = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(p.role, NodeRole::DtnNode);
        assert_eq!(p.issuer, "cd".repeat(32));
        // The message file is exactly the domain-separated signing bytes.
        assert_eq!(
            fs::read(dir.join("payload.message")).unwrap(),
            signing_message(&p)
        );
        // Bad inputs are errors.
        for bad in [
            [
                "siar-root",
                "payload",
                "--role",
                "root",
                "--challenge",
                &"ab".repeat(16),
                "--admin-pubkey",
                &"cd".repeat(32),
                "--out-dir",
                "/tmp/x",
            ],
            [
                "siar-root",
                "payload",
                "--role",
                "dtn-node",
                "--challenge",
                "abcd",
                "--admin-pubkey",
                &"cd".repeat(32),
                "--out-dir",
                "/tmp/x",
            ],
            [
                "siar-root",
                "payload",
                "--role",
                "dtn-node",
                "--challenge",
                &"ab".repeat(16),
                "--admin-pubkey",
                "nothex",
                "--out-dir",
                "/tmp/x",
            ],
        ] {
            assert!(run(&Cli::parse_from(bad)).is_err());
        }
    }
}

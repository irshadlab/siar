//! `cargo xtask e2e-provision | e2e-admin | e2e-audit | e2e-backup` — the
//! black-box checks of the headless-node work, against the real binaries.
//! (These began life as throwaway bash; they live here now, with assertions.)

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{bail, Context, Result};

use crate::harness::{children_of, poll, wait_exit, Bins, Check, Node, Out};

const SECS: fn(u64) -> Duration = Duration::from_secs;

fn data(n: &Node) -> PathBuf {
    n.root.join("data/siar")
}

fn private_file(p: &Path, text: &str) -> Result<()> {
    fs::write(p, text)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(p, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

// ------------------------------------------------------------ FROST ceremony

/// `siar-root …` with a passphrase file; returns stdout.
fn root_cmd(bin: &Path, pw: &Path, args: &[&str]) -> Result<String> {
    let o = Command::new(bin)
        .arg("--passphrase-file")
        .arg(pw)
        .args(args)
        .output()
        .with_context(|| format!("running siar-root {args:?}"))?;
    if !o.status.success() {
        bail!(
            "siar-root {args:?} failed: {}",
            String::from_utf8_lossy(&o.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&o.stdout).into_owned())
}

struct Ceremony {
    bin: PathBuf,
    dirs: Vec<PathBuf>,
    pws: Vec<PathBuf>,
    root_hex: String,
}

/// A real 2-of-3 DKG through the CLI; returns the shared root public key.
fn ceremony(dir: &Path, bin: PathBuf) -> Result<Ceremony> {
    let (mut dirs, mut pws) = (Vec::new(), Vec::new());
    for n in 1..=3 {
        let d = dir.join(format!("p{n}"));
        fs::create_dir_all(&d)?;
        let pw = d.join("pw");
        private_file(&pw, &format!("participant-{n}-long-passphrase\n"))?;
        dirs.push(d);
        pws.push(pw);
    }
    let r1: Vec<PathBuf> = dirs.iter().map(|d| d.join("r1.public.hex")).collect();
    for i in 0..3 {
        root_cmd(
            &bin,
            &pws[i],
            &[
                "dkg1",
                "--dir",
                &dirs[i].display().to_string(),
                "--id",
                &(i + 1).to_string(),
                "--min",
                "2",
                "--max",
                "3",
            ],
        )?;
    }
    let r1_args: Vec<String> = r1
        .iter()
        .flat_map(|p| ["--r1".to_owned(), p.display().to_string()])
        .collect();
    for i in 0..3 {
        let mut a = vec![
            "dkg2".to_owned(),
            "--dir".into(),
            dirs[i].display().to_string(),
        ];
        a.extend(r1_args.clone());
        root_cmd(
            &bin,
            &pws[i],
            &a.iter().map(String::as_str).collect::<Vec<_>>(),
        )?;
    }
    let mut root_hex = String::new();
    for i in 0..3 {
        let mut a = vec![
            "dkg3".to_owned(),
            "--dir".into(),
            dirs[i].display().to_string(),
        ];
        a.extend(r1_args.clone());
        for (j, dj) in dirs.iter().enumerate() {
            if j != i {
                a.extend([
                    "--r2".into(),
                    dj.join("out")
                        .join(format!("r2-for-{}.SECRET.hex", i + 1))
                        .display()
                        .to_string(),
                ]);
            }
        }
        root_cmd(
            &bin,
            &pws[i],
            &a.iter().map(String::as_str).collect::<Vec<_>>(),
        )?;
        let this = fs::read_to_string(dirs[i].join("root.pub"))?;
        if i == 0 {
            root_hex = this;
        } else if this != root_hex {
            bail!("participants derived different root keys");
        }
    }
    Ok(Ceremony {
        bin,
        dirs,
        pws,
        root_hex,
    })
}

impl Ceremony {
    /// Threshold-sign `message_file` with the given participants (0-based).
    fn sign(&self, signers: &[usize], message_file: &Path, work: &Path) -> Result<String> {
        let mut commit_args = vec![
            "package".to_owned(),
            "--message-file".into(),
            message_file.display().to_string(),
        ];
        for &i in signers {
            root_cmd(
                &self.bin,
                &self.pws[i],
                &["sign1", "--dir", &self.dirs[i].display().to_string()],
            )?;
            commit_args.extend([
                "--commitment".into(),
                self.dirs[i].join("commitment.hex").display().to_string(),
            ]);
        }
        let package = work.join("package.hex");
        commit_args.extend(["--out".into(), package.display().to_string()]);
        root_cmd(
            &self.bin,
            &self.pws[0],
            &commit_args.iter().map(String::as_str).collect::<Vec<_>>(),
        )?;
        let mut agg = vec![
            "aggregate".to_owned(),
            "--group".into(),
            self.dirs[signers[0]]
                .join("group.public.hex")
                .display()
                .to_string(),
            "--package".into(),
            package.display().to_string(),
        ];
        for &i in signers {
            root_cmd(
                &self.bin,
                &self.pws[i],
                &[
                    "sign2",
                    "--dir",
                    &self.dirs[i].display().to_string(),
                    "--package",
                    &package.display().to_string(),
                ],
            )?;
            agg.extend([
                "--share".into(),
                self.dirs[i].join("sig-share.hex").display().to_string(),
            ]);
        }
        Ok(root_cmd(
            &self.bin,
            &self.pws[0],
            &agg.iter().map(String::as_str).collect::<Vec<_>>(),
        )?
        .trim()
        .to_owned())
    }
}

/// FROST ceremony → enrol a daemon with a threshold signature → it restarts
/// into its role (here: a relay) and the role's child is supervised.
pub fn e2e_provision(bins: &Bins, role: &str) -> Result<()> {
    let mut c = Check::new(&format!(
        "provisioning a node as {role} with a FROST-signed payload"
    ));
    let n = Node::new("prov", bins)?;
    let cer = ceremony(&n.root.join("ceremony"), bins.path("siar-root")?)?;
    c.ok(
        cer.root_hex.len() == 64,
        "a 2-of-3 DKG gives every participant the same Ed25519 root key",
    );
    n.write_config(&format!(
        "[node]\nrequire_provisioning = true\nadmin_keys = [\"{}\"]\n",
        cer.root_hex
    ))?;

    let mut d1 = n.spawn(&[], "d1")?;
    n.wait_ready(SECS(60))?;
    let st = n.ctl(&["provision-status"])?.stdout;
    let challenge = st
        .split("challenge=")
        .nth(1)
        .map(|s| s.trim().to_owned())
        .unwrap_or_default();
    c.ok(
        st.starts_with("unprovisioned") && challenge.len() == 32,
        "the node starts unprovisioned and shows a challenge",
    );

    let work = n.root.join("work");
    fs::create_dir_all(&work)?;
    let root_bin = bins.path("siar-root")?;
    let pw = &cer.pws[0];
    root_cmd(
        &root_bin,
        pw,
        &[
            "payload",
            "--role",
            role,
            "--challenge",
            &challenge,
            "--admin-pubkey",
            &cer.root_hex,
            "--out-dir",
            &work.display().to_string(),
        ],
    )?;
    let payload = work.join("payload.bin").display().to_string();

    let forged = n.ctl(&["provision", &payload, &"00".repeat(64)])?;
    c.ok(
        !forged.ok && forged.text().to_lowercase().contains("unauthorized"),
        "a forged signature is refused as unauthorized",
    );

    let sig = cer.sign(&[0, 2], &work.join("payload.message"), &work)?;
    let done = n.ctl(&["provision", &payload, &sig])?;
    c.ok(
        done.ok && done.stdout.contains(role),
        "a 2-of-3 FROST signature provisions the node",
    );
    c.ok(
        wait_exit(&mut d1, SECS(30))? == Some(75),
        "the daemon exits 75 so the service manager restarts it into the role",
    );

    let mut d2 = n.spawn(&[], "d2")?;
    n.wait_ready(SECS(60))?;
    c.ok(
        n.ctl(&["provision-status"])?
            .stdout
            .contains(&format!("provisioned role={role}")),
        "after the restart the node reports its role",
    );
    let replay = n.ctl(&["provision", &payload, &sig])?;
    c.ok(
        !replay.ok && replay.text().to_lowercase().contains("busy"),
        "replaying the payload is refused",
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(data(&n).join("node/node.toml"))?
            .permissions()
            .mode()
            & 0o777;
        c.ok(
            mode == 0o600,
            format!("the node state file is private (mode {mode:o})"),
        );
    }

    if role.contains("relay") || role == "dtn-node" {
        let relay = |pid: u32| {
            children_of(pid)
                .into_iter()
                .find(|(_, cmd)| cmd.contains("siar-emergency-node"))
        };
        c.ok(
            poll(SECS(20), || relay(d2.id()).is_some()).is_ok(),
            "the relay engine runs as a child of the daemon",
        );
        c.ok(
            poll(SECS(20), || data(&n).join("relay/identity.bin").is_file()).is_ok(),
            "the relay creates its own persistent identity",
        );
        c.ok(
            !data(&n).join("identity.bin").exists(),
            "no chat identity was created for this role",
        );
        c.ok(
            n.ctl(&["health"])?.stdout.contains("role-relay: Healthy"),
            "health reports the relay worker",
        );
        if let Some((pid, _)) = relay(d2.id()) {
            let _ = Command::new("kill")
                .args(["-KILL", &pid.to_string()])
                .status();
            c.ok(
                poll(SECS(20), || relay(d2.id()).is_some_and(|(p, _)| p != pid)).is_ok(),
                "a killed relay is restarted under a new pid",
            );
        }
    }
    c.ok(n.stop(&mut d2)? == Some(0), "a normal shutdown exits 0");
    c.ok(
        children_of(d2.id()).is_empty(),
        "no child process is left running",
    );
    c.finish()
}

// -------------------------------------------------------------- remote admin

struct Admin {
    keys: PathBuf,
    owner: String,
    viewer: String,
}

fn admin_setup(n: &Node, extra_config: &str) -> Result<Admin> {
    let keys = n.root.join("keys");
    fs::create_dir_all(&keys)?;
    let gen = |name: &str| -> Result<String> {
        let o = n.run(&[
            "admin-keygen",
            "--out",
            &keys.join(name).display().to_string(),
        ])?;
        if !o.ok {
            bail!("admin-keygen failed: {}", o.stderr);
        }
        Ok(o.stdout.trim().to_owned())
    };
    let (owner, viewer) = (gen("owner.key")?, gen("viewer.key")?);
    gen("stranger.key")?;
    n.write_config(&format!(
        "[admin]\nenabled = true\n[[admin.operators]]\nkey = \"{owner}\"\nrole = \"owner\"\nlabel = \"alice\"\n[[admin.operators]]\nkey = \"{viewer}\"\nrole = \"viewer\"\nlabel = \"ci\"\n{extra_config}"
    ))?;
    Ok(Admin {
        keys,
        owner,
        viewer,
    })
}

/// Wait for the admin endpoint and return `(node id, 127.0.0.1:port)`.
fn admin_endpoint(n: &Node) -> Result<(String, String)> {
    let (id, addrs) = (
        data(n).join("admin/endpoint.id"),
        data(n).join("admin/endpoint.addrs"),
    );
    poll(SECS(30), || {
        id.is_file() && fs::read_to_string(&addrs).is_ok_and(|a| !a.trim().is_empty())
    })?;
    let port = fs::read_to_string(&addrs)?
        .lines()
        .next()
        .and_then(|l| l.rsplit(':').next())
        .unwrap_or("0")
        .to_owned();
    Ok((
        fs::read_to_string(id)?.trim().to_owned(),
        format!("127.0.0.1:{port}"),
    ))
}

fn admin_run(n: &Node, a: &Admin, key: &str, ep: &(String, String), op: &[&str]) -> Result<Out> {
    let kf = a.keys.join(format!("{key}.key")).display().to_string();
    let mut args = vec!["admin", "--key-file", &kf, "--addr", &ep.1, &ep.0];
    args.extend_from_slice(op);
    n.run(&args)
}

pub fn e2e_admin(bins: &Bins) -> Result<()> {
    let mut c = Check::new("remote administration over comm-admin/1");
    let n = Node::new("admin", bins)?;
    let a = admin_setup(&n, "")?;
    let mut d = n.spawn(&[], "d")?;
    n.wait_ready(SECS(60))?;
    let ep = admin_endpoint(&n)?;
    let ask = |key: &str, op: &[&str]| admin_run(&n, &a, key, &ep, op);

    c.ok(ask("viewer", &["health"])?.ok, "a viewer can read health");
    let denied = ask("viewer", &["maintenance", "on"])?;
    c.ok(
        !denied.ok && denied.text().contains("does not allow"),
        "a viewer cannot change state",
    );
    c.ok(
        ask("owner", &["maintenance", "on"])?.ok,
        "an owner can enter maintenance",
    );
    c.ok(
        n.ctl(&["status"])?.stdout.contains("Maintenance"),
        "the daemon is really in maintenance (checked locally)",
    );
    c.ok(
        ask("owner", &["maintenance", "off"])?.ok,
        "…and leave it again",
    );
    c.ok(
        !ask("stranger", &["health"])?.ok,
        "a key that is not an operator gets nothing",
    );
    let shell = ask("owner", &["sh", "-c", "id"])?;
    c.ok(
        !shell.ok && shell.text().contains("no remote shell"),
        "there is no remote shell",
    );
    let audit = ask("owner", &["audit"])?.stdout;
    c.ok(
        audit.contains("remote:ci:maintenance:on") && audit.contains("Denied"),
        "the refused attempt is audited under the operator's label",
    );
    c.ok(
        audit.contains("remote:alice:maintenance:on"),
        "the allowed one too",
    );
    let bye = ask("owner", &["shutdown"])?;
    c.ok(
        bye.ok && bye.stdout.trim() == "ok",
        "an owner's shutdown is acknowledged (reply survives the shutdown)",
    );
    c.ok(
        wait_exit(&mut d, SECS(30))? == Some(0),
        "the daemon exits 0",
    );
    let _ = (&a.owner, &a.viewer);
    c.finish()
}

// --------------------------------------------------------------- audit chain

fn audit_head(ctl_audit: &str) -> Option<String> {
    let l = ctl_audit
        .lines()
        .rev()
        .find(|l| l.starts_with("# persisted head"))?;
    let seq = l.split("seq=").nth(1)?.split_whitespace().next()?;
    let chain = l.split("chain=").nth(1)?.split_whitespace().next()?;
    Some(format!("{seq} {chain}"))
}

pub fn e2e_audit(bins: &Bins) -> Result<()> {
    let mut c = Check::new("durable, hash-chained audit log");
    let n = Node::new("audit", bins)?;
    let a = admin_setup(&n, "")?;
    let aud = data(&n).join("audit");
    let mut d = n.spawn(&[], "d1")?;
    n.wait_ready(SECS(60))?;
    let ep = admin_endpoint(&n)?;
    let ask = |key: &str, op: &[&str]| admin_run(&n, &a, key, &ep, op);
    ask("owner", &["maintenance", "on"])?;
    ask("owner", &["maintenance", "off"])?;
    ask("viewer", &["maintenance", "on"])?;
    ask("stranger", &["health"])?;
    n.ctl(&["maintenance", "on"])?;
    n.ctl(&["maintenance", "off"])?;

    let v = n.run(&["audit-verify", &aud.display().to_string()])?;
    c.ok(
        v.ok && v.stdout.starts_with("OK:"),
        "audit-verify accepts the log",
    );
    let head_ctl = audit_head(&n.ctl(&["audit"])?.stdout);
    let head_file = v
        .stdout
        .lines()
        .find(|l| l.starts_with("head seq="))
        .map(|l| {
            let seq = l
                .split("seq=")
                .nth(1)
                .unwrap_or("")
                .split_whitespace()
                .next()
                .unwrap_or("");
            let chain = l
                .split("chain=")
                .nth(1)
                .unwrap_or("")
                .split_whitespace()
                .next()
                .unwrap_or("");
            format!("{seq} {chain}")
        });
    c.ok(
        head_ctl.is_some() && head_ctl == head_file,
        "the head `ctl audit` shows equals the head audit-verify computes",
    );
    let log = fs::read_to_string(aud.join("audit.log"))?;
    c.ok(
        log.contains("remote:alice:maintenance:on")
            && log.contains("remote:ci:maintenance:on")
            && log.contains("remote:unknown:connect"),
        "remote, refused and unknown-peer actions are all on disk",
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        c.ok(
            fs::metadata(aud.join("audit.log"))?.permissions().mode() & 0o777 == 0o600,
            "the audit file is mode 600",
        );
    }

    ask("owner", &["shutdown"])?;
    wait_exit(&mut d, SECS(30))?;
    let seq_before: u64 = head_file
        .as_deref()
        .and_then(|h| h.split(' ').next())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);

    let mut d = n.spawn(&[], "d2")?;
    n.wait_ready(SECS(60))?;
    let ep = admin_endpoint(&n)?;
    admin_run(&n, &a, "viewer", &ep, &["health"])?;
    let v2 = n.run(&["audit-verify", &aud.display().to_string()])?;
    let seq_after: u64 = v2
        .stdout
        .lines()
        .find(|l| l.starts_with("head seq="))
        .and_then(|l| l.split("seq=").nth(1))
        .and_then(|s| s.split_whitespace().next())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    c.ok(
        v2.ok && seq_after > seq_before,
        "after a restart the chain continues and still verifies",
    );

    admin_run(&n, &a, "owner", &ep, &["shutdown"])?;
    wait_exit(&mut d, SECS(30))?;
    let text = fs::read_to_string(aud.join("audit.log"))?;
    fs::write(aud.join("audit.log"), text.replacen("Ok", "Denied", 1))?;
    let bad = n.run(&["audit-verify", &aud.display().to_string()])?;
    c.ok(
        !bad.ok && bad.text().contains("does NOT verify"),
        "a tampered first entry fails verification (non-zero exit)",
    );

    let mut d = n.spawn(&[], "d3")?;
    n.wait_ready(SECS(60))?;
    c.ok(
        n.log("d3").contains("audit log failed verification"),
        "the daemon logs the failure on the next start",
    );
    let quarantined = fs::read_dir(&aud)?.flatten().any(|e| {
        e.file_name()
            .to_string_lossy()
            .starts_with("audit.corrupt-")
    });
    c.ok(quarantined, "the evidence is moved aside, not deleted");
    let fresh = fs::read_to_string(aud.join("audit.log"))?;
    c.ok(
        fresh
            .lines()
            .next()
            .is_some_and(|l| l.contains("audit:chain-restarted")),
        "the new chain starts with a visible restart marker",
    );
    c.ok(
        n.run(&["audit-verify", &aud.display().to_string()])?.ok,
        "the new chain verifies",
    );
    n.ctl(&["shutdown"])?;
    wait_exit(&mut d, SECS(30))?;
    c.finish()
}

// ----------------------------------------------------------- backup / restore

pub fn e2e_backup(bins: &Bins) -> Result<()> {
    let mut c = Check::new("live backup and staged restore");
    let n = Node::new("backup", bins)?;
    private_file(&n.root.join("bpw"), "backup-passphrase-for-test\n")?;
    n.write_config(&format!(
        "[secrets.backup_passphrase]\nkind = \"file\"\npath = \"{}\"\n",
        n.root.join("bpw").display()
    ))?;
    let out = n.root.join("backups");
    let mut d = n.spawn(&[], "d1")?;
    n.wait_ready(SECS(60))?;
    let id_file = data(&n).join("device_id.txt");
    let original = fs::read_to_string(&id_file)?;

    let b = n.ctl(&["backup", &out.display().to_string()])?;
    c.ok(b.ok, "a backup runs while the daemon is serving");
    c.ok(
        fs::read_dir(&out).map(|d| d.count()).unwrap_or(0) == 1,
        "…and produced one archive",
    );
    let early = n.ctl(&["restore", &out.display().to_string()])?;
    c.ok(
        !early.ok && early.text().contains("not ready"),
        "restore is refused outside maintenance",
    );
    c.ok(n.ctl(&["maintenance", "on"])?.ok, "enter maintenance");
    fs::write(&id_file, "tampered-device-id")?;
    c.ok(
        n.ctl(&["restore", &out.display().to_string()])?.ok,
        "restore is staged in maintenance",
    );
    let parent = data(&n).parent().map(Path::to_path_buf).unwrap_or_default();
    c.ok(
        parent.join("siar.restore-pending").exists(),
        "a pending-restore marker is written",
    );
    n.ctl(&["shutdown"])?;
    wait_exit(&mut d, SECS(30))?;

    let mut d = n.spawn(&[], "d2")?;
    n.wait_ready(SECS(60))?;
    c.ok(
        fs::read_to_string(&id_file)? == original,
        "the restart applied the restore (identity is back)",
    );
    c.ok(
        n.log("d2").contains("restore applied"),
        "the daemon logs that it applied a restore",
    );
    let kept = fs::read_dir(&parent)?.flatten().any(|e| {
        e.file_name()
            .to_string_lossy()
            .starts_with("siar.pre-restore-")
    });
    c.ok(kept, "the previous data was kept, not deleted");
    c.ok(
        !parent.join("siar.restore-pending").exists()
            && !parent.join("siar.restore-staging").exists(),
        "marker and staging are cleaned up",
    );
    c.ok(n.stop(&mut d)? == Some(0), "clean shutdown");
    c.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_audit_head_is_read_from_the_last_matching_line() {
        let t = "1\\tuid=1\\tAdmin\\tx\\tOk\n# persisted head seq=7 chain=abcd write-failures=0";
        assert_eq!(audit_head(t).as_deref(), Some("7 abcd"));
        assert_eq!(audit_head("no head here"), None);
    }

    #[test]
    fn private_files_are_mode_600() {
        let p = std::env::temp_dir().join(format!("xt-pf-{}", std::process::id()));
        private_file(&p, "x").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&p).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
}

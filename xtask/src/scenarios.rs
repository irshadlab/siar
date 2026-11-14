//! Black-box scenarios against the real binaries — the old
//! `scripts/daemon-*.sh`, as Rust with assertions. Run with
//! `cargo xtask daemon-smoke | daemon-sync-smoke | daemon-upgrade-smoke`.

use std::time::Duration;

use anyhow::Result;

use crate::harness::{poll, sockets_in, Bins, Check, Node};

const SECS: fn(u64) -> Duration = Duration::from_secs;

fn first_conversation(n: &Node) -> Option<String> {
    let out = n.ctl(&["conversations"]).ok()?;
    let id = out
        .stdout
        .lines()
        .next()?
        .split('\t')
        .next()?
        .trim()
        .to_owned();
    (!id.is_empty()).then_some(id)
}

/// Poll `ctl messages <conv>` on `n` until it contains `needle`.
fn wait_message(n: &Node, conv: &str, needle: &str) -> bool {
    poll(SECS(40), || {
        n.ctl(&["messages", conv])
            .map(|o| o.stdout.contains(needle))
            .unwrap_or(false)
    })
    .is_ok()
}

/// Two daemons pair and exchange messages end to end.
pub fn daemon_smoke(bins: &Bins) -> Result<()> {
    let mut c = Check::new("two daemons pair and exchange messages");
    let (a, b) = (Node::new("a", bins)?, Node::new("b", bins)?);
    let mut pa = a.spawn(&[], "daemon")?;
    let mut pb = b.spawn(&[], "daemon")?;
    a.wait_ready(SECS(60))?;
    b.wait_ready(SECS(60))?;

    let ta = a.ctl(&["ticket"])?.stdout.trim().to_owned();
    let tb = b.ctl(&["ticket"])?.stdout.trim().to_owned();
    c.ok(
        !ta.is_empty() && !tb.is_empty() && ta != tb,
        "each daemon prints its own pairing ticket",
    );
    c.ok(
        a.ctl(&["pair", &tb])?.ok && b.ctl(&["pair", &ta])?.ok,
        "both sides accept the other's ticket",
    );

    let (mut ca, mut cb) = (None, None);
    poll(SECS(30), || {
        ca = first_conversation(&a);
        cb = first_conversation(&b);
        ca.is_some() && cb.is_some()
    })
    .ok();
    let (Some(ca), Some(cb)) = (ca, cb) else {
        c.ok(false, "pairing created a conversation on both sides");
        return c.finish();
    };
    c.ok(true, "pairing created a conversation on both sides");

    c.ok(
        a.ctl(&["send", &ca, "hello from daemon A"])?.ok,
        "A sends a message",
    );
    c.ok(
        wait_message(&b, &cb, "hello from daemon A"),
        "B receives A's message",
    );
    c.ok(b.ctl(&["send", &cb, "ack from daemon B"])?.ok, "B replies");
    c.ok(
        wait_message(&a, &ca, "ack from daemon B"),
        "A receives the reply",
    );
    let a_view = a.ctl(&["messages", &ca])?.stdout;
    c.ok(
        a_view.contains("hello from daemon A") && a_view.contains("ack from daemon B"),
        "A's history has both messages",
    );

    c.ok(a.stop(&mut pa)? == Some(0), "A shuts down with exit code 0");
    c.ok(b.stop(&mut pb)? == Some(0), "B shuts down with exit code 0");
    c.ok(
        sockets_in(&a.root.join("run/siar")) + sockets_in(&b.root.join("run/siar")) == 0,
        "no control sockets are left behind",
    );
    c.finish()
}

/// `sync seq: N` from `ctl ui-state`.
fn sync_seq(ui: &str) -> Option<u64> {
    ui.lines()
        .find_map(|l| l.strip_prefix("sync seq: ")?.trim().parse().ok())
}

/// The window-facing sync protocol (`ctl ui-state`) reflects activity — as
/// counts and a growing sequence number, never as message text.
pub fn daemon_sync_smoke(bins: &Bins) -> Result<()> {
    const BODY: &str = "hello through the sync protocol";
    let mut c = Check::new("ui-state (the sync protocol a window attaches to)");
    let (a, b) = (Node::new("a", bins)?, Node::new("b", bins)?);
    let mut pa = a.spawn(&[], "daemon")?;
    let mut pb = b.spawn(&[], "daemon")?;
    a.wait_ready(SECS(60))?;
    b.wait_ready(SECS(60))?;

    let before = a.ctl(&["ui-state"])?.stdout;
    c.ok(before.contains("ready: true"), "A reports ready");
    c.ok(before.contains("ticket: present"), "A has a pairing ticket");
    c.ok(
        before.contains("conversations: 0"),
        "no conversations before pairing",
    );
    let seq_before = sync_seq(&before);
    c.ok(seq_before.is_some(), "a sync sequence number is reported");

    let ta = a.ctl(&["ticket"])?.stdout.trim().to_owned();
    let tb = b.ctl(&["ticket"])?.stdout.trim().to_owned();
    a.ctl(&["pair", &tb])?;
    b.ctl(&["pair", &ta])?;
    let mut ca = None;
    poll(SECS(30), || {
        ca = first_conversation(&a);
        ca.is_some()
    })
    .ok();
    let Some(ca) = ca else {
        c.ok(false, "A has a conversation after pairing");
        return c.finish();
    };
    a.ctl(&["send", &ca, BODY])?;

    let one_message = |n: &Node| {
        n.ctl(&["ui-state"])
            .map(|o| o.stdout.contains("conversations: 1") && o.stdout.contains("messages=1"))
            .unwrap_or(false)
    };
    c.ok(
        poll(SECS(40), || one_message(&a)).is_ok(),
        "A's ui-state shows 1 conversation with 1 message",
    );
    c.ok(
        poll(SECS(40), || one_message(&b)).is_ok(),
        "B's ui-state shows 1 conversation with 1 message",
    );

    let (ua, ub) = (a.ctl(&["ui-state"])?.stdout, b.ctl(&["ui-state"])?.stdout);
    c.ok(sync_seq(&ua) > seq_before, "A's sync sequence advanced");
    c.ok(
        sync_seq(&ub).is_some_and(|v| v > 0),
        "B's sync sequence is live",
    );
    c.ok(
        !ua.contains(BODY) && !ub.contains(BODY),
        "message text never appears in the sync summary",
    );

    c.ok(
        a.stop(&mut pa)? == Some(0) && b.stop(&mut pb)? == Some(0),
        "both shut down with exit code 0",
    );
    c.finish()
}

/// Config validation, takeover with `--replace`, SIGHUP reload, clean stop.
pub fn daemon_upgrade_smoke(bins: &Bins) -> Result<()> {
    let mut c = Check::new("upgrade behaviour: config, takeover, reload, clean stop");
    let n = Node::new("up", bins)?;

    c.step("1. an invalid config is refused with every problem listed");
    n.write_config("[ipc]\nmax_connections = 0\n[logging]\nlevel = \"loud\"\n")?;
    let o = n.run(&["run"])?;
    let t = o.text().to_lowercase();
    c.ok(!o.ok, "the daemon refuses to start");
    c.ok(
        t.contains("problem") && t.contains("max_connections") && t.contains("level"),
        "…listing BOTH problems",
    );

    c.step("2. an embedded secret / typo is refused, and not echoed");
    n.write_config("password = \"hunter2\"\n")?;
    let o = n.run(&["run"])?;
    c.ok(
        !o.ok && o.text().to_lowercase().contains("unknown field"),
        "an unknown key is rejected",
    );
    c.ok(
        !o.text().contains("hunter2"),
        "the secret value is not echoed back",
    );
    n.write_config("")?;

    c.step("3. start A");
    let mut pa = n.spawn(&[], "a")?;
    n.wait_ready(SECS(60))?;
    c.ok(
        n.ctl(&["status"])?
            .stdout
            .lines()
            .next()
            .is_some_and(|l| l.contains("Ready")),
        "status reports Ready",
    );
    let inst = std::fs::read_to_string(n.root.join("run/siar/instance.toml")).unwrap_or_default();
    c.ok(
        inst.contains("build"),
        "the instance file records the build",
    );
    let _ = n.ctl(&["send", "00000000-0000-0000-0000-000000000000", "hi"]);

    c.step("4. metrics count the requests");
    let m = n.ctl(&["metrics"])?.stdout;
    c.ok(
        m.contains("siar_ipc_requests_total") && m.contains("siar_commands_total"),
        "request and command counters exist",
    );

    c.step("5. the audit log recorded the attempt");
    c.ok(
        !n.ctl(&["audit"])?.stdout.trim().is_empty(),
        "audit is not empty",
    );

    c.step("6. B with --replace: A stops gracefully, B takes over");
    let mut pb = n.spawn(&["--replace"], "b")?;
    c.ok(
        crate::harness::wait_exit(&mut pa, SECS(30))? == Some(0),
        "A exits with code 0",
    );
    n.wait_ready(SECS(60))?;
    c.ok(
        n.log("b").contains("previous daemon stopped") || n.log("b").contains("taking over"),
        "B logs the takeover",
    );

    c.step("7. SIGHUP: power is hot-applied, ipc needs a restart");
    n.write_config("[power]\nallow_metered_bulk = true\n[ipc]\nmax_connections = 64\n")?;
    n.signal(&pb, "HUP")?;
    c.ok(
        poll(SECS(10), || n.log("b").contains("configuration reloaded")).is_ok(),
        "the reload is logged",
    );
    c.ok(
        n.log("b")
            .lines()
            .any(|l| l.contains("configuration reloaded") && l.contains("ipc")),
        "…and names ipc as restart-required",
    );

    c.step("8. a bad reload is rejected and the daemon keeps running");
    n.write_config("[shutdown]\ngrace_secs = 0\n")?;
    n.signal(&pb, "HUP")?;
    c.ok(
        poll(SECS(10), || n.log("b").contains("reload rejected")).is_ok(),
        "the bad reload is rejected",
    );
    c.ok(
        n.ctl(&["status"])?.stdout.contains("Ready"),
        "the daemon is still Ready",
    );

    c.step("9. graceful stop cleans up");
    c.ok(n.stop(&mut pb)? == Some(0), "exit code 0");
    let left = std::fs::read_dir(n.root.join("run/siar"))
        .map(|d| d.count())
        .unwrap_or(0);
    c.ok(
        left == 0,
        format!("nothing left in the runtime dir ({left} file(s))"),
    );
    c.finish()
}

#[cfg(test)]
mod tests {
    use super::sync_seq;

    #[test]
    fn sync_seq_is_read_from_the_summary() {
        assert_eq!(sync_seq("ready: true\nsync seq: 5\n"), Some(5));
        assert_eq!(sync_seq("sync seq: x"), None);
        assert_eq!(sync_seq("nothing"), None);
    }
}

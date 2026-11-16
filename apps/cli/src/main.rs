//! `siar` — command-line client for `siar-daemon`.
//!
//! The daemon is the node: it owns networking, the database, the keys and the
//! MLS state. This program only sends it requests over the local control
//! socket, so it can run next to a window or another CLI without ever
//! touching the same files.
//!
//! ```text
//! siar status | ticket | identity | key-package | conversations | groups
//! siar pair <ticket>                      # prints the conversation id
//! siar send <conversation-id|ticket> <text>
//! siar send-file <conversation-id|ticket> <path>
//! siar send-anon <conversation-id|ticket> <relay-ticket> <text>
//! siar check-mailbox-anon <conversation-id|ticket> <relay-ticket>
//! siar messages <conversation-id>
//! siar listen <conversation-id>           # follow new messages
//! siar group-create
//! siar group-add <conv> <ticket> <device-id> <account-id> <key-package>
//! siar group-send <conv> <text>
//! ```
//!
//! Add `--spawn` to start the daemon if it is not running, `--socket PATH`
//! to use a non-default socket.
//!
//! Not offered: `check-mailbox` (the signed device check-in). Its replies are
//! relay-addressed bundles this client cannot decrypt, so it was only ever a
//! harness probe; `send-anon`/`check-mailbox-anon` are the working path.

mod args;
mod client;

use std::time::Duration;

use anyhow::Result;
use args::{Args, Command};
use clap::Parser as _;
use client::{classify_target, fresh_lines, socket_or_default, Daemon, Target};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args = Args::parse();
    if let Err(e) = run(args).await {
        eprintln!("siar: {e:#}");
        std::process::exit(1);
    }
}

async fn run(args: Args) -> Result<()> {
    let socket = socket_or_default(args.socket);
    let mut d = Daemon::connect(&socket, args.spawn).await?;
    match args.command {
        Command::Status => println!("{}", d.status().await?),
        Command::Ticket => println!("{}", d.query("ticket", "").await?),
        Command::Identity => println!("{}", d.query("identity", "").await?),
        Command::KeyPackage => println!("{}", d.query("key-package", "").await?),
        Command::Conversations => print_lines(&d.query("conversations", "").await?),
        Command::Groups => print_lines(&d.query("groups", "").await?),
        Command::Messages { conversation } => {
            print_lines(&d.query("messages", &conversation).await?);
        }
        Command::Pair { ticket } => println!("{}", d.command("pair", &ticket).await?),
        Command::Send { target, text } => {
            let conversation = match classify_target(&target) {
                Target::Conversation(id) => id.to_owned(),
                // Pairing is idempotent; the daemon applies commands in order,
                // so the send below runs after the pairing.
                Target::Ticket(t) => d.command("pair", t).await?,
            };
            d.command("send", &format!("{conversation} {text}")).await?;
            println!("queued for {conversation}");
        }
        Command::SendFile { target, path } => {
            // The daemon reads the file, so it needs an absolute path.
            let path = std::fs::canonicalize(&path)
                .map_err(|e| anyhow::anyhow!("cannot read {}: {e}", path.display()))?;
            let conversation = match classify_target(&target) {
                Target::Conversation(id) => id.to_owned(),
                Target::Ticket(t) => d.command("pair", t).await?,
            };
            if !std::fs::metadata(&path).is_ok_and(|m| m.is_file()) {
                anyhow::bail!("{} is not a regular file", path.display());
            }
            d.command("send-file", &format!("{conversation} {}", path.display()))
                .await?;
            println!("queued for {conversation}");
        }
        Command::SendAnon {
            target,
            relay_ticket,
            text,
        } => {
            let conversation = conversation_of(&mut d, &target).await?;
            d.command(
                "send-anon",
                &format!("{conversation} {relay_ticket} {text}"),
            )
            .await?;
            println!("left at the relay for {conversation}");
        }
        Command::CheckMailboxAnon {
            target,
            relay_ticket,
        } => {
            let conversation = conversation_of(&mut d, &target).await?;
            let before = lines_of(&d.query("messages", &conversation).await?);
            d.command(
                "check-mailbox-anon",
                &format!("{conversation} {relay_ticket}"),
            )
            .await?;
            // The relay answers over the next few seconds; show what arrives.
            let mut shown = before.clone();
            for _ in 0..8 {
                tokio::time::sleep(Duration::from_secs(1)).await;
                let now = lines_of(&d.query("messages", &conversation).await?);
                for line in fresh_lines(&shown, &now) {
                    println!("{line}");
                }
                shown = now;
            }
            if shown == before {
                println!("nothing waiting");
            }
        }
        Command::Listen { conversation } => listen(&mut d, &conversation).await?,
        Command::GroupCreate => {
            let before = d.query("groups", "").await?;
            d.command("group-create", "").await?;
            // The core applies the command asynchronously: wait for the group.
            for _ in 0..40 {
                let now = d.query("groups", "").await?;
                if let Some(new) = now.lines().find(|l| !before.lines().any(|b| b == *l)) {
                    println!("{}", new.split('\t').next().unwrap_or(new));
                    return Ok(());
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            anyhow::bail!("the daemon accepted the request but no new group appeared");
        }
        Command::GroupAdd {
            conversation,
            peer_ticket,
            peer_device_id,
            peer_account_id,
            base64_key_package,
        } => {
            d.command(
                "group-add",
                &format!(
                    "{conversation} {peer_ticket} {peer_device_id} {peer_account_id} {base64_key_package}"
                ),
            )
            .await?;
            println!("queued");
        }
        Command::GroupSend { conversation, text } => {
            d.command("group-send", &format!("{conversation} {text}"))
                .await?;
            println!("queued");
        }
    }
    Ok(())
}

/// A conversation id, pairing first if `target` is a ticket.
async fn conversation_of(d: &mut Daemon, target: &str) -> Result<String> {
    Ok(match classify_target(target) {
        Target::Conversation(id) => id.to_owned(),
        Target::Ticket(t) => d.command("pair", t).await?,
    })
}

fn lines_of(s: &str) -> Vec<String> {
    s.lines().map(str::to_owned).collect()
}

fn print_lines(s: &str) {
    if !s.is_empty() {
        println!("{s}");
    }
}

/// Follow a conversation until interrupted (Ctrl-C) or the daemon goes away.
async fn listen(d: &mut Daemon, conversation: &str) -> Result<()> {
    let mut last: Vec<String> = Vec::new();
    let mut first = true;
    loop {
        let snapshot: Vec<String> = d
            .query("messages", conversation)
            .await?
            .lines()
            .map(str::to_owned)
            .collect();
        // The first snapshot is history: show it, then only what follows.
        for line in fresh_lines(&last, &snapshot) {
            println!("{line}");
        }
        if first {
            first = false;
        }
        last = snapshot;
        tokio::select! {
            () = tokio::time::sleep(Duration::from_secs(1)) => {}
            _ = tokio::signal::ctrl_c() => return Ok(()),
        }
    }
}

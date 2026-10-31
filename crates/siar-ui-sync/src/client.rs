//! Window side: keep a local [`StateHub`] mirrored from a daemon.
//!
//! Two connections, deliberately: one only *listens* (frame reads are not
//! cancel-safe, so they live in their own task), one carries requests. A
//! dropped connection is not fatal — the session ends, the window says so
//! once, and the loop reconnects with backoff and takes a fresh snapshot.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use siar_runtime::ipc::{ClientError, IpcClient};
use siar_runtime::protocol::{Command, Query, Reply, Role};
use siar_runtime::{CommandId, Delivery};
use siar_ui_model::{NoticeLevel, StateHub};
use siar_ui_state::AppCommand;
use tokio::sync::{mpsc, watch};

use crate::apply::{apply_delta, apply_snapshot, ClientCursor, Verdict};
use crate::commands::{WireCommand, WireCommandError};
use crate::server::{SnapshotPage, SnapshotRequest};
use crate::wire::{decode, encode, SyncFrame, SYNC_TOPIC};

#[derive(Debug, thiserror::Error)]
enum SessionError {
    #[error(transparent)]
    Client(#[from] ClientError),
    #[error("the daemon sent something the window can’t understand")]
    Protocol,
    #[error("the connection to the daemon closed")]
    Closed,
}

/// Run until `shutdown` flips or the UI's command channel closes.
pub async fn run_remote(
    socket: PathBuf,
    hub: Arc<StateHub>,
    mut commands: mpsc::Receiver<AppCommand>,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut delay = Duration::from_millis(250);
    let mut told_user = false;
    loop {
        if *shutdown.borrow() {
            return;
        }
        match session(&socket, &hub, &mut commands, &mut shutdown, &mut told_user).await {
            Ok(()) => return, // clean stop
            Err(e) => {
                tracing::warn!(error = %e, "background-service session ended");
                if !told_user {
                    hub.notify(
                        NoticeLevel::Warning,
                        "Lost the connection to Siar’s background service. Reconnecting…",
                    );
                    told_user = true;
                }
            }
        }
        tokio::select! {
            () = tokio::time::sleep(delay) => {}
            _ = shutdown.changed() => return,
        }
        delay = (delay * 2).min(Duration::from_secs(5));
    }
}

async fn session(
    socket: &Path,
    hub: &Arc<StateHub>,
    commands: &mut mpsc::Receiver<AppCommand>,
    shutdown: &mut watch::Receiver<bool>,
    told_user: &mut bool,
) -> Result<(), SessionError> {
    let mut sub = IpcClient::connect_unix(socket, Role::Observer, "window-events").await?;
    let mut cmd = IpcClient::connect_unix(socket, Role::Operator, "window").await?;
    sub.subscribe(vec![SYNC_TOPIC], 1024).await?;

    let mut cursor = ClientCursor::default();
    fetch_snapshot(&mut cmd, hub, &mut cursor).await?;
    *told_user = false; // reconnected: the next outage deserves a notice again

    // The listener owns `sub`; the main loop only sees complete deliveries.
    let (tx, mut rx) = mpsc::channel::<Delivery>(256);
    let reader = tokio::spawn(async move {
        while let Ok(d) = sub.next_delivery().await {
            if tx.send(d).await.is_err() {
                break;
            }
        }
    });

    let result = loop {
        tokio::select! {
            _ = shutdown.changed() => break Ok(()),
            d = rx.recv() => match d {
                None => break Err(SessionError::Closed),
                Some(Delivery::Gap { .. }) => {
                    if let Err(e) = fetch_snapshot(&mut cmd, hub, &mut cursor).await { break Err(e); }
                }
                Some(Delivery::Event(e)) => {
                    let Some(frame) = decode::<SyncFrame>(&e.payload) else { break Err(SessionError::Protocol) };
                    let verdict = hub.update(|s| apply_delta(s, &mut cursor, &frame));
                    if verdict == Verdict::Resync {
                        if let Err(e) = fetch_snapshot(&mut cmd, hub, &mut cursor).await { break Err(e); }
                    }
                }
            },
            c = commands.recv() => match c {
                None => break Ok(()),
                Some(c) => forward(&mut cmd, hub, &c).await,
            },
        }
    };
    reader.abort();
    result
}

async fn fetch_snapshot(
    cmd: &mut IpcClient,
    hub: &Arc<StateHub>,
    cursor: &mut ClientCursor,
) -> Result<(), SessionError> {
    let ask = |id, page| Query::App {
        name: "ui-snapshot".into(),
        data: encode(&SnapshotRequest { id, page }),
    };
    let mut frames: Vec<SyncFrame> = Vec::new();
    let first = page(cmd.query(ask(None, 0)).await?)?;
    let (id, pages) = (first.id, first.pages.clamp(1, 4096));
    frames.push(first.frame);
    for p in 1..pages {
        frames.push(page(cmd.query(ask(Some(id), p)).await?)?.frame);
    }
    hub.update(|s| apply_snapshot(s, cursor, &frames))
        .map_err(|_| SessionError::Protocol)
}

fn page(r: Reply) -> Result<SnapshotPage, SessionError> {
    match r {
        Reply::Bytes(b) => decode(&b).ok_or(SessionError::Protocol),
        _ => Err(SessionError::Protocol),
    }
}

async fn forward(cmd: &mut IpcClient, hub: &Arc<StateHub>, c: &AppCommand) {
    let wire = match WireCommand::try_from(c) {
        Ok(w) => w,
        Err(WireCommandError::Unsupported) => {
            hub.notify(
                NoticeLevel::Warning,
                "That isn’t available through the background service yet.",
            );
            return;
        }
        Err(_) => return,
    };
    let id = CommandId(rand_id());
    let req = Command::App {
        name: "ui-command".into(),
        data: encode(&wire),
    };
    if let Err(e) = cmd.command(id, req).await {
        tracing::warn!(error = %e, "command was not accepted");
        hub.notify(
            NoticeLevel::Error,
            "Siar’s background service didn’t accept that. Try again.",
        );
    }
}

fn rand_id() -> u128 {
    // Per-intent uniqueness is all idempotency needs; time + counter is enough
    // and avoids another dependency.
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(1);
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    (t << 20)
        ^ u128::from(N.fetch_add(1, Ordering::Relaxed))
        ^ (u128::from(std::process::id()) << 100)
}

/// One-shot: connect, take a snapshot, and describe it in plain text. Used by
/// `siar-daemon ctl ui-state` to check what a window would see.
pub async fn snapshot_summary(socket: &Path) -> Result<String, String> {
    let (tx, _rx) = mpsc::channel(1);
    let hub = Arc::new(StateHub::new(tx));
    let mut cmd = IpcClient::connect_unix(socket, Role::Observer, "ui-state")
        .await
        .map_err(|e| e.to_string())?;
    let mut cursor = ClientCursor::default();
    fetch_snapshot(&mut cmd, &hub, &mut cursor)
        .await
        .map_err(|e| e.to_string())?;
    Ok(hub.read(|s| {
        let mut out = format!(
            "ready: {}\nticket: {}\nconversations: {}\nsync seq: {}",
            s.ready,
            if s.identity.ticket.is_empty() {
                "none"
            } else {
                "present"
            },
            s.conversations.ordered().len(),
            cursor.last_seq()
        );
        for c in s.conversations.ordered() {
            let n = s.timelines.get(&c.id).map_or(0, |t| t.visible().count());
            out.push_str(&format!(
                "\n  {}\t{}\tunread={}\tmessages={}",
                c.id, c.display_name, c.unread_count, n
            ));
        }
        out
    }))
}

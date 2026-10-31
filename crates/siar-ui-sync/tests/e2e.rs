//! Window ⇄ daemon over real Unix sockets.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use siar_domain::{ConversationId, DeliveryState, MessageContent, MessageId, MessageText};
use siar_runtime::ipc::{serve_unix, ServerConfig};
use siar_runtime::{AppService, BoxFuture, Runtime, RuntimeError};
use siar_ui_model::{NoticeLevel, StateHub};
use siar_ui_state::{AppCommand, ConversationKind, ConversationSummary, TimelineWindow};
use siar_ui_sync::{capture, run_remote, UiSyncService};
use tokio::sync::{mpsc, watch};

#[derive(Clone, Default)]
struct SyncApp(Arc<OnceLock<Arc<UiSyncService>>>);

impl AppService for SyncApp {
    fn command<'a>(
        &'a self,
        name: &'a str,
        data: Vec<u8>,
    ) -> BoxFuture<'a, Result<Vec<u8>, RuntimeError>> {
        Box::pin(async move {
            match (name, self.0.get()) {
                ("ui-command", Some(s)) => s.command(&data).map(|()| Vec::new()),
                _ => Err(RuntimeError::Unsupported),
            }
        })
    }
    fn query<'a>(
        &'a self,
        name: &'a str,
        data: Vec<u8>,
    ) -> BoxFuture<'a, Result<Vec<u8>, RuntimeError>> {
        Box::pin(async move {
            match (name, self.0.get()) {
                ("ui-snapshot", Some(s)) => s.snapshot_page(&data),
                _ => Err(RuntimeError::Unsupported),
            }
        })
    }
}

struct Daemon {
    rt: Runtime,
    hub: Arc<StateHub>,
    commands: mpsc::Receiver<AppCommand>,
    serve: tokio::task::JoinHandle<std::io::Result<()>>,
}

async fn start_daemon(sock: &std::path::Path) -> Daemon {
    let app = SyncApp::default();
    let rt = Runtime::builder().app(app.clone()).build();
    rt.start(async { Ok::<_, String>(()) }, async { Ok::<_, String>(()) })
        .await
        .unwrap();
    let (tx, commands) = mpsc::channel(64);
    let hub = Arc::new(StateHub::new(tx));
    let svc = UiSyncService::new(hub.clone(), rt.bus.clone());
    let _ = app.0.set(svc.clone());
    tokio::spawn(svc.run(rt.lifecycle.clone()));
    let (rt2, p2) = (rt.clone(), sock.to_path_buf());
    let serve = tokio::spawn(async move { serve_unix(rt2, &p2, ServerConfig::default()).await });
    for _ in 0..200 {
        if sock.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    Daemon {
        rt,
        hub,
        commands,
        serve,
    }
}

fn sock(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("siar-sync-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d.join("d.sock")
}

fn msg(seq: u64, text: &str) -> TimelineWindow {
    TimelineWindow {
        message_id: MessageId::new(),
        sequence: seq,
        content: MessageContent::Text(MessageText::parse(text.to_owned()).unwrap()),
        delivery_state: DeliveryState::Sent,
        from_me: seq.is_multiple_of(2),
    }
}

fn populate(hub: &StateHub, conversations: usize, per: u64, body: usize) -> ConversationId {
    let mut first = None;
    hub.update(|s| {
        s.ready = true;
        s.identity.ticket = "daemon-ticket".into();
        for i in 0..conversations {
            let c = ConversationId::new();
            first.get_or_insert(c);
            s.conversations.upsert(ConversationSummary {
                id: c,
                display_name: format!("chat {i}"),
                last_message_preview: Some("hello".into()),
                unread_count: 1,
                kind: ConversationKind::Direct,
            });
            for j in 1..=per {
                s.timeline_mut(c)
                    .push_latest(msg(j, &format!("{i}/{j} {}", "x".repeat(body))));
            }
            s.sendable.insert(c);
        }
    });
    first.unwrap()
}

async fn converged(a: &StateHub, b: &StateHub) -> bool {
    let end = Instant::now() + Duration::from_secs(8);
    while Instant::now() < end {
        if a.read(capture) == b.read(capture) && b.read(|s| s.ready) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    false
}

struct Window {
    hub: Arc<StateHub>,
    stop: watch::Sender<bool>,
    task: tokio::task::JoinHandle<()>,
}

fn open_window(sock: &std::path::Path) -> Window {
    let (tx, rx) = mpsc::channel(64);
    let hub = Arc::new(StateHub::new(tx));
    let (stop, stop_rx) = watch::channel(false);
    let task = tokio::spawn(run_remote(sock.to_path_buf(), hub.clone(), rx, stop_rx));
    Window { hub, stop, task }
}

#[tokio::test]
async fn window_mirrors_the_daemon_and_follows_live_changes() {
    let path = sock("live");
    let d = start_daemon(&path).await;
    let a = populate(&d.hub, 3, 5, 20);
    let w = open_window(&path);
    assert!(converged(&d.hub, &w.hub).await, "initial snapshot");

    // Local, window-only state survives every sync.
    w.hub.update(|s| s.selected = Some(a));
    d.hub.update(|s| {
        s.timeline_mut(a)
            .push_latest(msg(99, "a brand new message"));
        s.conversations.set_unread(a, 7);
    });
    assert!(converged(&d.hub, &w.hub).await, "live delta");
    assert_eq!(w.hub.read(|s| s.selected), Some(a));
    assert_eq!(w.hub.read(|s| s.timelines[&a].visible().count()), 6);

    let _ = w.stop.send(true);
    let _ = w.task.await;
    d.rt.shutdown(Duration::from_secs(1)).await;
}

#[tokio::test]
async fn commands_cross_the_boundary_validated_and_unsupported_ones_say_so() {
    let path = sock("cmd");
    let mut d = start_daemon(&path).await;
    let a = populate(&d.hub, 1, 1, 5);
    let w = open_window(&path);
    assert!(converged(&d.hub, &w.hub).await);

    assert!(w.hub.dispatch(AppCommand::SendMessage {
        conversation: a,
        text: MessageText::parse("over the socket".into()).unwrap()
    }));
    let got = tokio::time::timeout(Duration::from_secs(5), d.commands.recv())
        .await
        .unwrap()
        .unwrap();
    match got {
        AppCommand::SendMessage { conversation, text } => {
            assert_eq!(conversation, a);
            assert_eq!(text.as_str(), "over the socket");
        }
        other => panic!("{other:?}"),
    }

    // Not supported through the daemon yet: the user is told, nothing is sent.
    w.hub
        .dispatch(AppCommand::DeclineGroupInvite { conversation: a });
    let end = Instant::now() + Duration::from_secs(3);
    while Instant::now() < end && w.hub.read(|s| s.notice.is_none()) {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let n = w.hub.read(|s| s.notice.clone()).expect("a notice");
    assert_eq!(n.level, NoticeLevel::Warning);
    assert!(d.commands.try_recv().is_err());

    let _ = w.stop.send(true);
    let _ = w.task.await;
    d.rt.shutdown(Duration::from_secs(1)).await;
}

#[tokio::test]
async fn a_snapshot_too_big_for_one_frame_is_paged_and_still_converges() {
    let path = sock("paged");
    let d = start_daemon(&path).await;
    populate(&d.hub, 40, 20, 1_500);
    let w = open_window(&path);
    assert!(converged(&d.hub, &w.hub).await, "multi-page snapshot");
    assert_eq!(w.hub.read(|s| s.conversations.ordered().len()), 40);
    let _ = w.stop.send(true);
    let _ = w.task.await;
    d.rt.shutdown(Duration::from_secs(1)).await;
}

#[tokio::test]
async fn the_window_reconnects_to_a_restarted_daemon_and_adopts_its_state() {
    let path = sock("restart");
    let d1 = start_daemon(&path).await;
    populate(&d1.hub, 2, 3, 10);
    let w = open_window(&path);
    assert!(converged(&d1.hub, &w.hub).await);

    // The daemon goes away…
    d1.rt.shutdown(Duration::from_secs(1)).await;
    let _ = d1.serve.await;
    let end = Instant::now() + Duration::from_secs(5);
    while Instant::now() < end && w.hub.read(|s| s.notice.is_none()) {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        w.hub.read(|s| s
            .notice
            .as_ref()
            .is_some_and(|n| n.text.contains("Reconnecting"))),
        "the user is told once"
    );

    // …and comes back as a different instance with different content.
    let d2 = start_daemon(&path).await;
    populate(&d2.hub, 5, 2, 10);
    assert!(
        converged(&d2.hub, &w.hub).await,
        "adopts the new instance's state"
    );
    assert_eq!(w.hub.read(|s| s.conversations.ordered().len()), 5);

    let _ = w.stop.send(true);
    let _ = w.task.await;
    d2.rt.shutdown(Duration::from_secs(1)).await;
}

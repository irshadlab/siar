//! End-to-end: two real cores (own identity, database, iroh endpoint) talk
//! through the same `UiHub` + `AppCommand` path the Slint window uses.
//! Only the window is absent (`UiHub::detached`).
//!
//! Needs loopback networking; it is `#[ignore]`d by default so a sandbox
//! without sockets doesn't fail the suite:
//!     cargo test -p siar-desktop -- --ignored e2e --nocapture

use std::time::{Duration, Instant};

use siar_domain::{DeliveryState, MessageText};
use siar_ui_slint::UiHub;
use siar_ui_state::AppCommand;
use tokio::sync::mpsc;

struct Node {
    hub: UiHub,
    tx: mpsc::Sender<AppCommand>,
}

async fn start_node(data_home: &std::path::Path) -> Node {
    // `directories` resolves the data dir from XDG_DATA_HOME at call time,
    // and bootstrap reads it exactly once, so sequential starts get
    // independent identities and databases.
    unsafe { std::env::set_var("XDG_DATA_HOME", data_home) };
    let boot = crate::bootstrap::bootstrap_messaging()
        .await
        .expect("bootstrap");
    let (tx, rx) = mpsc::channel(64);
    let hub = UiHub::detached(tx.clone());
    tokio::spawn(crate::core::run(boot, hub.clone(), rx));
    wait_until("core ready", || hub.read(|s| s.ready)).await;
    Node { hub, tx }
}

async fn wait_until(what: &str, mut cond: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(40);
    while Instant::now() < deadline {
        if cond() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("timed out waiting for: {what}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs loopback networking"]
async fn two_cores_exchange_a_message_and_the_sender_sees_it_delivered() {
    let tmp = std::env::temp_dir().join(format!("siar-e2e-{}", std::process::id()));
    let a = start_node(&tmp.join("a")).await;
    let b = start_node(&tmp.join("b")).await;

    let ticket_a = a.hub.read(|s| s.identity.ticket.clone());
    let ticket_b = b.hub.read(|s| s.identity.ticket.clone());
    assert!(!ticket_a.is_empty() && !ticket_b.is_empty() && ticket_a != ticket_b);

    // Both sides pair (the receiver must know the sender: unknown devices are ignored).
    a.tx.send(AppCommand::PairWithTicket {
        ticket_text: ticket_b,
    })
    .await
    .unwrap();
    b.tx.send(AppCommand::PairWithTicket {
        ticket_text: ticket_a,
    })
    .await
    .unwrap();
    wait_until("A has a selected, sendable conversation", || {
        a.hub
            .read(|s| s.selected.map(|c| s.can_send(c)).unwrap_or(false))
    })
    .await;
    wait_until("B paired", || b.hub.read(|s| s.selected.is_some())).await;
    let conv_a = a.hub.read(|s| s.selected.unwrap());

    // A sends.
    let text = MessageText::parse("hello from A".to_string()).unwrap();
    a.tx.send(AppCommand::SendMessage {
        conversation: conv_a,
        text,
    })
    .await
    .unwrap();

    // B receives it, routed to B's conversation with A, unread because B isn't looking elsewhere.
    wait_until("B received the message", || {
        b.hub.read(|s| {
            s.timelines
                .values()
                .any(|t| t.visible().any(|e| !e.from_me))
        })
    })
    .await;
    let (preview, from_me) = b.hub.read(|s| {
        let c = s
            .conversations
            .ordered()
            .iter()
            .find(|c| c.last_message_preview.is_some())
            .unwrap()
            .clone();
        let e = s
            .timelines
            .values()
            .flat_map(|t| t.visible())
            .next()
            .unwrap()
            .from_me;
        (c.last_message_preview.unwrap(), e)
    });
    assert_eq!(preview, "hello from A");
    assert!(!from_me);

    // The delivery ACK flows back and the poller mirrors it into A's timeline.
    wait_until("A sees Delivered", || {
        a.hub.read(|s| {
            s.timelines
                .get(&conv_a)
                .map(|t| {
                    t.visible()
                        .any(|e| e.from_me && e.delivery_state == DeliveryState::Delivered)
                })
                .unwrap_or(false)
        })
    })
    .await;
}

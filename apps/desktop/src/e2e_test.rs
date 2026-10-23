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
    core: std::sync::Arc<crate::core::Core>,
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
    let core = crate::core::start(boot, hub.clone()).await;
    tokio::spawn(crate::core::command_loop(core.clone(), rx));
    wait_until("core ready", || hub.read(|s| s.ready)).await;
    Node { hub, tx, core }
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

async fn save_contact(from: &Node, to: &Node, name: &str) {
    let (account, device, ticket) = to.hub.read(|s| {
        (
            s.identity.account.unwrap(),
            s.identity.device.unwrap(),
            s.identity.ticket.clone(),
        )
    });
    from.tx
        .send(AppCommand::SaveContact {
            device_id: device,
            account_id: account,
            display_name: name.into(),
            ticket_text: ticket,
            key_package_b64: None,
        })
        .await
        .unwrap();
    wait_until("contact safety computed", || {
        from.hub.read(|s| s.contact_safety.contains_key(&device))
    })
    .await;
}

fn device_of(n: &Node) -> siar_domain::DeviceId {
    n.hub.read(|s| s.identity.device.unwrap())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs loopback networking"]
async fn verification_uses_one_shared_safety_number_and_flags_a_changed_key() {
    use siar_ui_slint::VerificationState;
    let tmp = std::env::temp_dir().join(format!("siar-e2e-verify-{}", std::process::id()));
    let (a, b, c) = (
        start_node(&tmp.join("a")).await,
        start_node(&tmp.join("b")).await,
        start_node(&tmp.join("c")).await,
    );
    let (dev_b, dev_a) = (device_of(&b), device_of(&a));

    save_contact(&a, &b, "B").await;
    save_contact(&b, &a, "A").await;

    // Two independent cores, two real key pairs, ONE number.
    let num_a = a.hub.read(|s| s.contact_safety[&dev_b].number.clone());
    let num_b = b.hub.read(|s| s.contact_safety[&dev_a].number.clone());
    assert_eq!(
        num_a, num_b,
        "both sides must read out the same safety number"
    );
    assert_eq!(num_a.split(' ').count(), 12);
    assert_eq!(
        a.hub.read(|s| s.contact_safety[&dev_b].state),
        VerificationState::Unverified
    );

    // The user compares and confirms.
    a.tx.send(AppCommand::SetContactVerified {
        device_id: dev_b,
        verified: true,
    })
    .await
    .unwrap();
    wait_until("verified", || {
        a.hub
            .read(|s| s.contact_safety[&dev_b].state == VerificationState::Verified)
    })
    .await;
    assert_eq!(
        a.hub
            .read(|s| s.banner.visible_issues(&s.security_events).len()),
        0
    );

    // Same contact, DIFFERENT key (device C's ticket under B's ids): never
    // silently trusted, never downgraded to a plain "unverified".
    let (account_b, ticket_c) = (
        b.hub.read(|s| s.identity.account.unwrap()),
        c.hub.read(|s| s.identity.ticket.clone()),
    );
    a.tx.send(AppCommand::SaveContact {
        device_id: dev_b,
        account_id: account_b,
        display_name: "B".into(),
        ticket_text: ticket_c,
        key_package_b64: None,
    })
    .await
    .unwrap();
    wait_until("key change detected", || {
        a.hub
            .read(|s| s.contact_safety[&dev_b].state == VerificationState::KeyChanged)
    })
    .await;
    assert_ne!(
        a.hub.read(|s| s.contact_safety[&dev_b].number.clone()),
        num_a
    );
    // ...and it raised a critical event that the banner shows.
    let shown = a.hub.read(|s| {
        s.banner
            .visible_issues(&s.security_events)
            .iter()
            .map(|e| e.kind)
            .collect::<Vec<_>>()
    });
    assert_eq!(
        shown,
        vec![siar_ui_state::SecurityEventKind::IdentityChanged]
    );
    // Saving the same changed key again must not spam new events.
    let before = a.hub.read(|s| s.security_events.events().len());
    a.tx.send(AppCommand::SetContactVerified {
        device_id: dev_b,
        verified: false,
    })
    .await
    .unwrap();
    wait_until("unmarked", || {
        a.hub
            .read(|s| s.contact_safety[&dev_b].state == VerificationState::Unverified)
    })
    .await;
    assert_eq!(a.hub.read(|s| s.security_events.events().len()), before);
}

fn text_of(e: &siar_ui_state::TimelineWindow) -> String {
    match &e.content {
        siar_domain::MessageContent::Text(t) => t.as_str().to_string(),
        _ => String::new(),
    }
}

async fn say(from: &Node, conversation: siar_domain::ConversationId, body: &str) {
    let text = MessageText::parse(body.to_string()).unwrap();
    from.tx
        .send(AppCommand::SendMessage { conversation, text })
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs loopback networking"]
async fn history_comes_back_from_encrypted_storage_in_pages_and_skips_other_peers() {
    use siar_messaging::PeerTicket;
    let tmp = std::env::temp_dir().join(format!("siar-e2e-history-{}", std::process::id()));
    let (a, b, c) = (
        start_node(&tmp.join("a")).await,
        start_node(&tmp.join("b")).await,
        start_node(&tmp.join("c")).await,
    );
    let ticket = |n: &Node| n.hub.read(|s| s.identity.ticket.clone());
    let (ta, tb, tc) = (ticket(&a), ticket(&b), ticket(&c));

    // everyone who talks to B is paired both ways (unpaired senders are ignored)
    for (node, other) in [(&a, &tb), (&c, &tb), (&b, &ta), (&b, &tc)] {
        node.tx
            .send(AppCommand::PairWithTicket {
                ticket_text: other.clone(),
            })
            .await
            .unwrap();
    }
    let conv_a_side = crate::peers::conversation_for(&PeerTicket::decode(&tb).unwrap()); // A's chat with B
    let conv_c_side = crate::peers::conversation_for(&PeerTicket::decode(&tb).unwrap());
    let b_chat_with_a = crate::peers::conversation_for(&PeerTicket::decode(&ta).unwrap());
    wait_until("pairings", || {
        a.hub.read(|s| s.can_send(conv_a_side)) && b.hub.read(|s| s.can_send(b_chat_with_a))
    })
    .await;

    // 60 from A (more than one page), 1 from C into B's shared inbound
    // stream, then B's own reply.
    for i in 1..=60 {
        say(&a, conv_a_side, &format!("a{i:02}")).await;
    }
    say(&c, conv_c_side, "from-c").await;
    wait_until("B has everything", || {
        b.hub.read(|s| {
            s.timelines
                .values()
                .map(|t| t.visible().filter(|e| !e.from_me).count())
                .sum::<usize>()
                >= 61
        })
    })
    .await;
    say(&b, b_chat_with_a, "b-reply").await;
    wait_until("reply stored", || {
        b.hub.read(|s| {
            s.timelines
                .get(&b_chat_with_a)
                .is_some_and(|t| t.visible().any(|e| e.from_me))
        })
    })
    .await;

    // Forget everything in memory — this is what a restart does to the UI.
    b.hub.update(|s| {
        s.timelines.clear();
        s.history.clear();
    });

    // Opening the chat brings back the newest page: 50 of the 61 rows that
    // belong to this chat (60 from A + B's reply). C's message must not be there.
    b.tx.send(AppCommand::OpenConversation(b_chat_with_a))
        .await
        .unwrap();
    wait_until("first page", || {
        b.hub
            .read(|s| s.history.get(&b_chat_with_a).is_some_and(|h| h.loaded))
    })
    .await;
    let (n, exhausted) = b.hub.read(|s| {
        (
            s.timelines[&b_chat_with_a].visible().count(),
            s.history[&b_chat_with_a].exhausted,
        )
    });
    assert_eq!(n, 50);
    assert!(!exhausted, "11 older messages remain");
    let preview = b.hub.read(|s| {
        s.conversations
            .ordered()
            .iter()
            .find(|c| c.id == b_chat_with_a)
            .unwrap()
            .last_message_preview
            .clone()
    });
    assert_eq!(preview.as_deref(), Some("You: b-reply"));

    // The rest, on request.
    b.tx.send(AppCommand::LoadEarlierMessages {
        conversation: b_chat_with_a,
    })
    .await
    .unwrap();
    wait_until("older page", || {
        b.hub.read(|s| s.history[&b_chat_with_a].exhausted)
    })
    .await;
    let all = b.hub.read(|s| {
        s.timelines[&b_chat_with_a]
            .visible()
            .cloned()
            .collect::<Vec<_>>()
    });
    assert_eq!(all.len(), 61, "60 from A + B's reply");
    let bodies: Vec<String> = all.iter().map(text_of).collect();
    assert!(
        !bodies.iter().any(|t| t == "from-c"),
        "another peer's message leaked into this chat"
    );
    // order and ownership survive storage
    let from_a: Vec<&String> = bodies.iter().filter(|t| t.starts_with('a')).collect();
    let mut sorted = from_a.clone();
    sorted.sort();
    assert_eq!(from_a, sorted, "A's messages stay in send order");
    assert_eq!(bodies.last().map(String::as_str), Some("b-reply"));
    assert!(all.last().unwrap().from_me);
    assert!(all[..60].iter().all(|e| !e.from_me));
    // sent message state came from storage too, not "Saving…" forever
    let reply_state = all.last().unwrap().delivery_state;
    assert_ne!(reply_state, siar_domain::DeliveryState::Local);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs loopback networking"]
async fn unread_badges_and_read_state_survive_a_restart() {
    use siar_messaging::PeerTicket;
    let tmp = std::env::temp_dir().join(format!("siar-e2e-unread-{}", std::process::id()));
    let (a, b) = (
        start_node(&tmp.join("a")).await,
        start_node(&tmp.join("b")).await,
    );
    let (ta, tb) = (
        a.hub.read(|s| s.identity.ticket.clone()),
        b.hub.read(|s| s.identity.ticket.clone()),
    );
    a.tx.send(AppCommand::PairWithTicket {
        ticket_text: tb.clone(),
    })
    .await
    .unwrap();
    b.tx.send(AppCommand::PairWithTicket {
        ticket_text: ta.clone(),
    })
    .await
    .unwrap();
    let a_chat = crate::peers::conversation_for(&PeerTicket::decode(&tb).unwrap());
    let b_chat = crate::peers::conversation_for(&PeerTicket::decode(&ta).unwrap());
    wait_until("paired", || {
        a.hub.read(|s| s.can_send(a_chat)) && b.hub.read(|s| s.selected == Some(b_chat))
    })
    .await;

    // B is looking at the chat: arriving messages are read, and that sticks.
    for i in 1..=2 {
        say(&a, a_chat, &format!("seen{i}")).await;
    }
    wait_until("B got 2", || {
        b.hub.read(|s| {
            s.timelines
                .get(&b_chat)
                .is_some_and(|t| t.visible().filter(|e| !e.from_me).count() == 2)
        })
    })
    .await;
    assert_eq!(unread(&b, b_chat), 0, "on-screen messages are not unread");

    // B looks away; three more arrive.
    b.hub.update(|s| s.selected = None);
    for i in 1..=3 {
        say(&a, a_chat, &format!("new{i}")).await;
    }
    wait_until("B has 3 unread", || unread(&b, b_chat) == 3).await;

    // "Restart" B's UI: forget memory, then run the start-up restore.
    b.hub.update(|s| {
        s.timelines.clear();
        s.history.clear();
        s.conversations.set_unread(b_chat, 0);
    });
    b.core.preload_history().await;
    assert_eq!(
        unread(&b, b_chat),
        3,
        "the badge comes back from storage + the saved mark"
    );

    // Opening the chat reads it; a second restart shows nothing unread.
    b.tx.send(AppCommand::OpenConversation(b_chat))
        .await
        .unwrap();
    wait_until("read", || unread(&b, b_chat) == 0).await;
    b.hub.update(|s| {
        s.selected = None;
        s.timelines.clear();
        s.history.clear();
    });
    b.core.preload_history().await;
    assert_eq!(unread(&b, b_chat), 0, "read state persisted");
}

fn unread(n: &Node, c: siar_domain::ConversationId) -> u32 {
    n.hub.read(|s| {
        s.conversations
            .ordered()
            .iter()
            .find(|x| x.id == c)
            .map(|x| x.unread_count)
            .unwrap_or(0)
    })
}

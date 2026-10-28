//! End-to-end: two real cores (own identity, database, iroh endpoint) talk
//! through the same `StateHub` + `AppCommand` path the Slint window uses.
//! Only the window is absent (no window behind the `StateHub`).
//!
//! Needs loopback networking; it is `#[ignore]`d by default so a sandbox
//! without sockets doesn't fail the suite:
//!     cargo test -p siar-desktop -- --ignored e2e --nocapture

use std::time::{Duration, Instant};

use siar_domain::{DeliveryState, MessageText};
use std::sync::Arc;

use siar_ui_model::StateHub;
use siar_ui_state::AppCommand;
use tokio::sync::mpsc;

struct Node {
    hub: Arc<StateHub>,
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
    let hub = std::sync::Arc::new(StateHub::new(tx.clone()));
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
    use siar_ui_model::VerificationState;
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs loopback networking"]
async fn groups_and_the_shared_key_package_come_back_after_a_restart() {
    let tmp = std::env::temp_dir().join(format!("siar-e2e-groups-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let first = start_node(&tmp.join("a")).await;
    let founder = first.hub.read(|s| s.identity.account.unwrap());
    let key_package_before = first.hub.read(|s| s.identity.key_package.clone());
    assert!(!key_package_before.is_empty());

    first
        .tx
        .send(AppCommand::CreateGroup { founder })
        .await
        .unwrap();
    wait_until("group created", || {
        first.hub.read(|s| s.groups.ordered().len() == 1)
    })
    .await;
    let conversation = first.hub.read(|s| s.groups.ordered()[0].conversation_id);

    // "Restart": a brand-new core over the same data directory.
    let second = start_node(&tmp.join("a")).await;
    wait_until("group restored into the UI", || {
        second.hub.read(|s| s.groups.get(conversation).is_some())
    })
    .await;
    assert!(
        second.hub.read(|s| s.is_group(conversation)),
        "it is in the chat list as a group"
    );
    assert!(second
        .hub
        .read(|s| s.groups.get(conversation).unwrap().is_admin));
    // the key package handed out before the restart is still the one shown
    assert_eq!(
        second.hub.read(|s| s.identity.key_package.clone()),
        key_package_before
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs loopback networking"]
async fn group_invite_is_one_step_for_the_admin_and_one_tap_for_the_invitee() {
    use base64::Engine;
    let tmp = std::env::temp_dir().join(format!("siar-e2e-invite-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let (a, b) = (
        start_node(&tmp.join("a")).await,
        start_node(&tmp.join("b")).await,
    );
    let ids = |n: &Node| {
        n.hub.read(|s| {
            (
                s.identity.account.unwrap(),
                s.identity.device.unwrap(),
                s.identity.ticket.clone(),
                s.identity.key_package.clone(),
            )
        })
    };
    let ((acct_a, _dev_a, _ticket_a, _), (acct_b, dev_b, ticket_b, kp_b)) = (ids(&a), ids(&b));

    // A needs to know how to reach B (a saved contact, in app terms).
    save_contact(&a, &b, "B").await;

    // Admin: create the group, then add B with B's key package.
    a.tx.send(AppCommand::CreateGroup { founder: acct_a })
        .await
        .unwrap();
    wait_until("group created", || {
        a.hub.read(|s| s.groups.ordered().len() == 1)
    })
    .await;
    let conv = a.hub.read(|s| s.groups.ordered()[0].conversation_id);
    let key_package_bytes = base64::engine::general_purpose::STANDARD
        .decode(kp_b)
        .unwrap();
    a.tx.send(AppCommand::AddGroupMember {
        conversation: conv,
        new_member: acct_b,
        input: siar_ui_state::AddMemberInput {
            peer_account: acct_b,
            peer_device: dev_b,
            peer_ticket_text: ticket_b,
            key_package_bytes,
        },
    })
    .await
    .unwrap();

    // Invitee: the invite arrives already carrying the roster — nothing to paste.
    wait_until("B has an invite", || {
        b.hub.read(|s| !s.pending_invites.pending().is_empty())
    })
    .await;
    assert!(b.hub.read(|s| s.pending_invites.pending()[0].carries_state));
    b.tx.send(AppCommand::AcceptGroupInvite { conversation: conv })
        .await
        .unwrap();
    wait_until("B joined", || b.hub.read(|s| s.groups.get(conv).is_some())).await;
    let (members, is_admin) = b.hub.read(|s| {
        let g = s.groups.get(conv).unwrap();
        (g.member_count, g.is_admin)
    });
    assert_eq!(members, 2);
    assert!(!is_admin, "the invitee is a member, not an admin");
    assert!(
        b.hub.read(|s| s.pending_invites.pending().is_empty()),
        "the invite is consumed"
    );

    // Group messages flow admin → member…
    a.tx.send(AppCommand::SendGroupMessage {
        conversation: conv,
        text: MessageText::parse("hello group".into()).unwrap(),
    })
    .await
    .unwrap();
    wait_until("B received", || {
        b.hub.read(|s| {
            s.timelines.get(&conv).is_some_and(|t| {
                t.visible()
                    .any(|e| !e.from_me && text_of(e) == "hello group")
            })
        })
    })
    .await;

    // …and member → admin, once B can reach A (a saved contact).
    save_contact(&b, &a, "A").await;
    b.tx.send(AppCommand::SendGroupMessage {
        conversation: conv,
        text: MessageText::parse("hi admin".into()).unwrap(),
    })
    .await
    .unwrap();
    wait_until("A received", || {
        a.hub.read(|s| {
            s.timelines
                .get(&conv)
                .is_some_and(|t| t.visible().any(|e| !e.from_me && text_of(e) == "hi admin"))
        })
    })
    .await;
}

fn open_events(n: &Node) -> usize {
    n.hub.read(|s| {
        s.security_events
            .events()
            .iter()
            .filter(|e| !e.resolved)
            .count()
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs loopback networking"]
async fn the_security_log_survives_restarts_and_a_resolved_warning_stays_resolved() {
    use siar_ui_model::VerificationState;
    let tmp = std::env::temp_dir().join(format!("siar-e2e-seclog-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let (a, b, c) = (
        start_node(&tmp.join("a")).await,
        start_node(&tmp.join("b")).await,
        start_node(&tmp.join("c")).await,
    );
    let dev_b = device_of(&b);

    // Verify B, then present a DIFFERENT key under B's ids: a critical warning.
    save_contact(&a, &b, "B").await;
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
    wait_until("warning raised", || open_events(&a) == 1).await;

    // ---- restart #1: the warning is still there, and not doubled ----
    let a2 = start_node(&tmp.join("a")).await;
    wait_until("log restored", || {
        a2.hub.read(|s| !s.security_events.events().is_empty())
    })
    .await;
    wait_until("contacts restored", || {
        a2.hub.read(|s| s.contact_safety.contains_key(&dev_b))
    })
    .await;
    assert_eq!(
        open_events(&a2),
        1,
        "the open warning came back, exactly once"
    );
    assert_eq!(
        a2.hub.read(|s| s.contact_safety[&dev_b].state),
        VerificationState::KeyChanged,
        "and the contact is still shown as key-changed"
    );
    assert_eq!(
        a2.hub
            .read(|s| s.banner.visible_issues(&s.security_events).len()),
        1
    );

    // The user resolves it (what the UI does: update memory, then persist).
    let event_id = a2.hub.read(|s| s.security_events.events()[0].id);
    a2.hub.update(|s| s.security_events.resolve(event_id));
    a2.tx
        .send(AppCommand::ResolveSecurityEvent {
            event_id: event_id.get(),
        })
        .await
        .unwrap();
    // give the core a moment to persist it
    tokio::time::sleep(Duration::from_millis(300)).await;

    // ---- restart #2: resolved stays resolved, and is NOT re-raised even
    // though the contact's key is still different ----
    let a3 = start_node(&tmp.join("a")).await;
    wait_until("log restored again", || {
        a3.hub.read(|s| !s.security_events.events().is_empty())
    })
    .await;
    wait_until("contacts restored again", || {
        a3.hub.read(|s| s.contact_safety.contains_key(&dev_b))
    })
    .await;
    assert_eq!(
        a3.hub.read(|s| s.security_events.events().len()),
        1,
        "no duplicate event"
    );
    assert_eq!(open_events(&a3), 0, "resolved stays resolved");
    assert_eq!(
        a3.hub
            .read(|s| s.banner.visible_issues(&s.security_events).len()),
        0
    );
    assert_eq!(
        a3.hub.read(|s| s.contact_safety[&dev_b].state),
        VerificationState::KeyChanged
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs loopback networking"]
async fn a_restored_backup_is_the_same_working_account() {
    use siar_messaging::PeerTicket;
    use siar_ui_model::VerificationState;
    let tmp = std::env::temp_dir().join(format!("siar-e2e-restore-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let (a, b) = (
        start_node(&tmp.join("a")).await,
        start_node(&tmp.join("b")).await,
    );
    let (acct_a, dev_a, kp_a) = a.hub.read(|s| {
        (
            s.identity.account.unwrap(),
            s.identity.device.unwrap(),
            s.identity.key_package.clone(),
        )
    });
    let dev_b = device_of(&b);
    let (ta, tb) = (
        a.hub.read(|s| s.identity.ticket.clone()),
        b.hub.read(|s| s.identity.ticket.clone()),
    );

    // Build up a real account: a saved + verified contact, history, a group.
    save_contact(&a, &b, "Bea").await;
    save_contact(&b, &a, "Ann").await;
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
    let a_chat = crate::peers::conversation_for(&PeerTicket::decode(&tb).unwrap());
    let b_chat = crate::peers::conversation_for(&PeerTicket::decode(&ta).unwrap());
    wait_until("both can send", || {
        a.hub.read(|s| s.can_send(a_chat)) && b.hub.read(|s| s.can_send(b_chat))
    })
    .await;
    for i in 1..=3 {
        say(&a, a_chat, &format!("keep-{i}")).await;
    }
    wait_until("B got them", || {
        b.hub.read(|s| {
            s.timelines
                .get(&b_chat)
                .is_some_and(|t| t.visible().filter(|e| !e.from_me).count() == 3)
        })
    })
    .await;
    a.tx.send(AppCommand::CreateGroup { founder: acct_a })
        .await
        .unwrap();
    wait_until("group", || a.hub.read(|s| s.groups.ordered().len() == 1)).await;
    let group = a.hub.read(|s| s.groups.ordered()[0].conversation_id);
    tokio::time::sleep(Duration::from_millis(500)).await; // let writes settle

    // Disaster recovery: back up, then restore on a "new machine".
    let archive = tmp.join("account.siarbak");
    const PASS: &str = "an adequately long passphrase";
    let summary = siar_backup::create_backup_with(
        &tmp.join("a").join("siar"),
        PASS,
        &archive,
        siar_backup::KdfParams {
            mem_kib: 8,
            iterations: 1,
            lanes: 1,
        },
    )
    .expect("backup");
    assert!(summary.files >= 4);
    let restored_home = tmp.join("restored");
    siar_backup::restore_backup(&archive, PASS, &restored_home.join("siar")).expect("restore");

    let r = start_node(&restored_home).await;
    // identity: the SAME person and device, not a fresh account
    assert_eq!(r.hub.read(|s| s.identity.account), Some(acct_a));
    assert_eq!(r.hub.read(|s| s.identity.device), Some(dev_a));
    assert_eq!(
        r.hub.read(|s| s.identity.key_package.clone()),
        kp_a,
        "the key package already shared still works"
    );
    // contact + verification mark
    wait_until("contacts back", || {
        r.hub.read(|s| s.contact_safety.contains_key(&dev_b))
    })
    .await;
    assert_eq!(
        r.hub.read(|s| s.contact_safety[&dev_b].state),
        VerificationState::Verified
    );
    // group came back with its MLS state
    wait_until("group back", || {
        r.hub.read(|s| s.groups.get(group).is_some())
    })
    .await;
    // readable history: the messages decrypt with the restored keys
    wait_until("history decrypts", || {
        r.hub.read(|s| {
            s.timelines
                .get(&a_chat)
                .is_some_and(|t| t.visible().filter(|e| e.from_me).count() == 3)
        })
    })
    .await;
    let bodies: Vec<String> = r
        .hub
        .read(|s| s.timelines[&a_chat].visible().map(text_of).collect());
    assert_eq!(bodies, ["keep-1", "keep-2", "keep-3"]);
    // and it can still talk to the peer (same transport identity? at least same keys): send works
    say(&r, a_chat, "after-restore").await;
    wait_until("B receives from the restored account", || {
        b.hub.read(|s| {
            s.timelines.get(&b_chat).is_some_and(|t| {
                t.visible()
                    .any(|e| !e.from_me && text_of(e) == "after-restore")
            })
        })
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs loopback networking"]
async fn a_restarted_device_keeps_its_address_and_its_peer_keeps_accepting_it() {
    use siar_messaging::PeerTicket;
    let tmp = std::env::temp_dir().join(format!("siar-e2e-address-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let (a, b) = (
        start_node(&tmp.join("a")).await,
        start_node(&tmp.join("b")).await,
    );
    let endpoint_of = |n: &Node| {
        PeerTicket::decode(&n.hub.read(|s| s.identity.ticket.clone()))
            .unwrap()
            .endpoint_addr
            .id
    };
    let before = endpoint_of(&a);

    // A is a saved contact of B and vice versa; B already trusts A's address.
    save_contact(&a, &b, "B").await;
    save_contact(&b, &a, "A").await;
    let b_chat_with_a = crate::peers::conversation_for(
        &PeerTicket::decode(&a.hub.read(|s| s.identity.ticket.clone())).unwrap(),
    );

    // A "restarts" (a new core over the same data folder).
    let a2 = start_node(&tmp.join("a")).await;
    assert_eq!(
        endpoint_of(&a2),
        before,
        "the device's address changed across a restart: every saved ticket for it is now stale"
    );

    // …so B, which was never told anything new, still accepts A's messages
    // (routed by sender address) with no re-pairing.
    let a2_chat = a2
        .hub
        .read(|s| s.conversations.ordered().first().map(|c| c.id))
        .expect("B restored as a chat");
    say(&a2, a2_chat, "i am back").await;
    wait_until("B accepts the restarted A", || {
        b.hub.read(|s| {
            s.timelines
                .get(&b_chat_with_a)
                .is_some_and(|t| t.visible().any(|e| !e.from_me && text_of(e) == "i am back"))
        })
    })
    .await;
}

async fn invite(admin: &Node, to: &Node, conv: siar_domain::ConversationId) {
    use base64::Engine;
    let (account, device, ticket, kp) = to.hub.read(|s| {
        (
            s.identity.account.unwrap(),
            s.identity.device.unwrap(),
            s.identity.ticket.clone(),
            s.identity.key_package.clone(),
        )
    });
    admin
        .tx
        .send(AppCommand::AddGroupMember {
            conversation: conv,
            new_member: account,
            input: siar_ui_state::AddMemberInput {
                peer_account: account,
                peer_device: device,
                peer_ticket_text: ticket,
                key_package_bytes: base64::engine::general_purpose::STANDARD
                    .decode(kp)
                    .unwrap(),
            },
        })
        .await
        .unwrap();
}

fn heard(n: &Node, conv: siar_domain::ConversationId, body: &str) -> bool {
    n.hub.read(|s| {
        s.timelines
            .get(&conv)
            .is_some_and(|t| t.visible().any(|e| !e.from_me && text_of(e) == body))
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs loopback networking"]
async fn group_members_reach_each_other_without_contacts_and_after_a_restart() {
    let tmp = std::env::temp_dir().join(format!("siar-e2e-discovery-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let (a, b, c) = (
        start_node(&tmp.join("a")).await,
        start_node(&tmp.join("b")).await,
        start_node(&tmp.join("c")).await,
    );
    let founder = a.hub.read(|s| s.identity.account.unwrap());

    // The admin builds a group. NO contacts are saved anywhere.
    a.tx.send(AppCommand::CreateGroup { founder })
        .await
        .unwrap();
    wait_until("group", || a.hub.read(|s| s.groups.ordered().len() == 1)).await;
    let conv = a.hub.read(|s| s.groups.ordered()[0].conversation_id);

    invite(&a, &b, conv).await;
    wait_until("B invited", || {
        b.hub.read(|s| !s.pending_invites.pending().is_empty())
    })
    .await;
    b.tx.send(AppCommand::AcceptGroupInvite { conversation: conv })
        .await
        .unwrap();
    wait_until("B joined", || b.hub.read(|s| s.groups.get(conv).is_some())).await;

    invite(&a, &c, conv).await;
    // B, an existing member, sees the roster grow without being told by hand.
    wait_until("B's group shows 3 members", || {
        b.hub
            .read(|s| s.groups.get(conv).is_some_and(|g| g.member_count == 3))
    })
    .await;
    wait_until("C invited", || {
        c.hub.read(|s| !s.pending_invites.pending().is_empty())
    })
    .await;
    c.tx.send(AppCommand::AcceptGroupInvite { conversation: conv })
        .await
        .unwrap();
    wait_until("C joined", || c.hub.read(|s| s.groups.get(conv).is_some())).await;
    assert!(
        a.hub.read(|s| s.contacts.ordered().is_empty()),
        "no contact was ever saved"
    );
    assert!(b.hub.read(|s| s.contacts.ordered().is_empty()));

    // B ↔ C never met, yet each reaches the other (and A).
    let say_group = |n: &Node, body: &str| {
        let text = MessageText::parse(body.to_string()).unwrap();
        let tx = n.tx.clone();
        async move {
            tx.send(AppCommand::SendGroupMessage {
                conversation: conv,
                text,
            })
            .await
            .unwrap()
        }
    };
    say_group(&b, "b says hi").await;
    wait_until("A and C hear B", || {
        heard(&a, conv, "b says hi") && heard(&c, conv, "b says hi")
    })
    .await;
    say_group(&c, "c says hi").await;
    wait_until("A and B hear C", || {
        heard(&a, conv, "c says hi") && heard(&b, conv, "c says hi")
    })
    .await;

    // B restarts. What it learned about A and C was saved, and its MLS state
    // reopened, so it can still reach both — with no contacts to rebuild from.
    let b2 = start_node(&tmp.join("b")).await;
    wait_until("B2 group back", || {
        b2.hub.read(|s| s.groups.get(conv).is_some())
    })
    .await;
    say_group(&b2, "b after restart").await;
    wait_until("A and C hear restarted B", || {
        heard(&a, conv, "b after restart") && heard(&c, conv, "b after restart")
    })
    .await;
}

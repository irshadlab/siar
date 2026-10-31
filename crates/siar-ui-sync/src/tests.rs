//! Round-trip, delta, gap and bounds tests for the sync engine.

use siar_domain::{
    AccountId, ConversationId, DeliveryState, DeviceId, MessageContent, MessageId, MessageText,
};
use siar_ui_model::{ContactSafety, HistoryPaging, PowerStatus, Slices, VerificationState};
use siar_ui_state::{
    ConversationKind, ConversationSummary, NetworkState, PeerReachability, SavedContact,
    TimelineWindow,
};

use crate::apply::{apply_delta, apply_snapshot, ClientCursor, Verdict};
use crate::engine::{capture, SyncServer};
use crate::wire::*;

fn msg(seq: u64, text: &str, mine: bool) -> TimelineWindow {
    TimelineWindow {
        message_id: MessageId::new(),
        sequence: seq,
        content: MessageContent::Text(MessageText::parse(text.to_owned()).unwrap()),
        delivery_state: DeliveryState::Sent,
        from_me: mine,
    }
}

struct World {
    s: Slices,
    a: ConversationId,
    b: ConversationId,
    dev: DeviceId,
}

fn world() -> World {
    let mut s = Slices::new();
    let (a, b, dev) = (
        ConversationId::new(),
        ConversationId::new(),
        DeviceId::new(),
    );
    s.ready = true;
    s.identity.ticket = "my-ticket".into();
    s.identity.account = Some(AccountId::new());
    s.conversations.upsert(ConversationSummary {
        id: a,
        display_name: "Alice".into(),
        last_message_preview: Some("hi".into()),
        unread_count: 2,
        kind: ConversationKind::Direct,
    });
    s.conversations.upsert(ConversationSummary {
        id: b,
        display_name: "Team".into(),
        last_message_preview: None,
        unread_count: 0,
        kind: ConversationKind::Group,
    });
    for i in 1..=5 {
        s.timeline_mut(a)
            .push_latest(msg(i, &format!("message {i}"), i % 2 == 0));
    }
    s.history.insert(
        a,
        HistoryPaging {
            loaded: true,
            next_before: Some(1),
            exhausted: false,
        },
    );
    s.sendable.insert(a);
    s.contacts.upsert(SavedContact {
        device_id: dev,
        account_id: AccountId::new(),
        display_name: "Alice".into(),
        ticket_text: "t".into(),
        key_package_b64: None,
    });
    s.contact_safety.insert(
        dev,
        ContactSafety {
            number: "12345 67890".into(),
            state: VerificationState::Verified,
        },
    );
    s.network.set_overall(NetworkState::Online);
    s.network.set_peer(dev, PeerReachability::Connected);
    s.power = Some(PowerStatus {
        headline: "Running at full speed".into(),
        details: vec![],
        waiting: 0,
    });
    World { s, a, b, dev }
}

/// Two `Slices` are equal as far as sync is concerned iff they capture equally.
fn same(x: &Slices, y: &Slices) -> bool {
    capture(x) == capture(y)
}

fn mirror(server: &SyncServer, s: &Slices) -> (Slices, ClientCursor) {
    let mut client = Slices::new();
    let mut cur = ClientCursor::default();
    apply_snapshot(&mut client, &mut cur, &server.snapshot(s)).unwrap();
    (client, cur)
}

#[test]
fn a_snapshot_reproduces_every_synced_section() {
    let w = world();
    let server = SyncServer::new(1);
    let (client, cur) = mirror(&server, &w.s);
    assert!(same(&w.s, &client));
    assert!(cur.has_snapshot());
    assert_eq!(client.timelines.get(&w.a).unwrap().visible().count(), 5);
    assert!(client.sendable.contains(&w.a) && !client.sendable.contains(&w.b));
    assert!(client.is_group(w.b));
    assert_eq!(
        client.contact_safety.get(&w.dev).unwrap().state,
        VerificationState::Verified
    );
}

#[test]
fn it_survives_the_wire_encoding() {
    let w = world();
    let server = SyncServer::new(1);
    let frames: Vec<SyncFrame> = server
        .snapshot(&w.s)
        .iter()
        .map(|f| decode(&encode(f)).unwrap())
        .collect();
    let mut client = Slices::new();
    apply_snapshot(&mut client, &mut ClientCursor::default(), &frames).unwrap();
    assert!(same(&w.s, &client));
}

#[test]
fn only_changed_sections_are_sent() {
    let mut w = world();
    let mut server = SyncServer::new(7);
    let first = server.diff(&w.s);
    assert!(!first.is_empty(), "the first diff announces everything");
    assert!(
        server.diff(&w.s).is_empty(),
        "nothing changed, nothing sent"
    );

    w.s.timeline_mut(w.a).push_latest(msg(6, "new one", false));
    let d = server.diff(&w.s);
    assert_eq!(d.len(), 1);
    let keys: Vec<_> = d[0].updates.iter().map(|u| u.key).collect();
    assert_eq!(
        keys,
        vec![SectionKey::Timeline(w.a)],
        "only that conversation's timeline moved: {keys:?}"
    );
    assert!(d[0].updates[0].version >= 2);
}

#[test]
fn deltas_applied_in_order_converge_to_the_server_state() {
    let mut w = world();
    let mut server = SyncServer::new(7);
    let _ = server.diff(&w.s);
    let (mut client, mut cur) = mirror(&server, &w.s);

    w.s.timeline_mut(w.a).push_latest(msg(6, "later", true));
    w.s.conversations.set_unread(w.a, 0);
    let first_id = w.s.timelines[&w.a].visible().next().unwrap().message_id;
    w.s.timeline_mut(w.a)
        .update_delivery_state(first_id, DeliveryState::Delivered);
    w.s.network.set_overall(NetworkState::Offline);
    for f in server.diff(&w.s) {
        assert_eq!(apply_delta(&mut client, &mut cur, &f), Verdict::Apply);
    }
    assert!(same(&w.s, &client));
}

#[test]
fn window_local_state_is_never_touched_by_sync() {
    let w = world();
    let server = SyncServer::new(1);
    let mut client = Slices::new();
    client.selected = Some(w.a);
    client.conversation_filter = "ali".into();
    client.open_content = true;
    client.composer = Default::default();
    apply_snapshot(
        &mut client,
        &mut ClientCursor::default(),
        &server.snapshot(&w.s),
    )
    .unwrap();
    assert_eq!(client.selected, Some(w.a));
    assert_eq!(client.conversation_filter, "ali");
    assert!(client.open_content);
}

#[test]
fn a_gap_or_a_new_instance_demands_a_resync_and_stale_frames_are_ignored() {
    let mut w = world();
    let mut server = SyncServer::new(7);
    let _ = server.diff(&w.s);
    let (mut client, mut cur) = mirror(&server, &w.s);

    w.s.conversations.set_unread(w.a, 9);
    let d1 = server.diff(&w.s);
    w.s.conversations.set_unread(w.a, 10);
    let d2 = server.diff(&w.s);
    // d1 is lost: d2 arrives out of order.
    assert_eq!(apply_delta(&mut client, &mut cur, &d2[0]), Verdict::Resync);
    // …and a delta from a restarted daemon is never trusted either.
    let mut other = SyncServer::new(8);
    let _ = other.diff(&w.s);
    w.s.conversations.set_unread(w.a, 11);
    assert_eq!(
        apply_delta(&mut client, &mut cur, &other.diff(&w.s)[0]),
        Verdict::Resync
    );
    // Applying d1 then d2 in order is fine; replaying d1 afterwards is stale.
    assert_eq!(apply_delta(&mut client, &mut cur, &d1[0]), Verdict::Apply);
    assert_eq!(apply_delta(&mut client, &mut cur, &d2[0]), Verdict::Apply);
    assert_eq!(apply_delta(&mut client, &mut cur, &d1[0]), Verdict::Stale);
}

#[test]
fn a_snapshot_overlapping_deltas_is_harmless() {
    let mut w = world();
    let mut server = SyncServer::new(7);
    let _ = server.diff(&w.s);
    w.s.timeline_mut(w.a).push_latest(msg(6, "x", true));
    let delta = server.diff(&w.s);
    // The client takes a snapshot *after* the delta was published…
    let (mut client, mut cur) = mirror(&server, &w.s);
    // …so that delta, delivered late, is stale, and applying nothing is correct.
    assert_eq!(
        apply_delta(&mut client, &mut cur, &delta[0]),
        Verdict::Stale
    );
    assert!(same(&w.s, &client));
}

#[test]
fn removed_conversations_disappear_on_the_client() {
    let mut w = world();
    let mut server = SyncServer::new(7);
    let _ = server.diff(&w.s);
    let (mut client, mut cur) = mirror(&server, &w.s);
    w.s.timelines.remove(&w.a);
    w.s.history.remove(&w.a);
    w.s.sendable.remove(&w.a);
    for f in server.diff(&w.s) {
        apply_delta(&mut client, &mut cur, &f);
    }
    assert!(!client.timelines.contains_key(&w.a) && !client.history.contains_key(&w.a));
}

#[test]
fn a_huge_timeline_is_truncated_at_the_old_end_and_says_so() {
    let mut s = Slices::new();
    let c = ConversationId::new();
    let big = "z".repeat(8_000);
    for i in 1..=100 {
        s.timeline_mut(c).push_latest(msg(i, &big, true));
    }
    let server = SyncServer::new(1);
    let frames = server.snapshot(&s);
    let Section::Timeline(t) = frames
        .iter()
        .flat_map(|f| &f.updates)
        .find_map(|u| matches!(u.key, SectionKey::Timeline(_)).then(|| u.section.clone().unwrap()))
        .unwrap()
    else {
        panic!()
    };
    assert!(encode(&Section::Timeline(t.clone())).len() <= MAX_SECTION_BYTES);
    assert!(t.entries.len() < 100 && t.has_more, "truncated and flagged");
    assert_eq!(
        t.entries.last().unwrap().sequence,
        100,
        "the newest messages are the ones kept"
    );
    assert_eq!(
        t.paging.next_before,
        Some(t.entries.first().unwrap().sequence)
    );
    for f in &frames {
        assert!(encode(f).len() < 1_000_000);
    }
}

#[test]
fn many_conversations_split_into_bounded_frames_that_reassemble() {
    let mut s = Slices::new();
    for i in 0..60 {
        let c = ConversationId::new();
        for j in 1..=20 {
            s.timeline_mut(c).push_latest(msg(
                j,
                &format!("{i}-{j} {}", "w".repeat(2_000)),
                j % 2 == 0,
            ));
        }
    }
    let server = SyncServer::new(1);
    let frames = server.snapshot(&s);
    assert!(frames.len() > 1, "one frame could not carry this");
    assert!(frames
        .iter()
        .all(|f| encode(f).len() <= MAX_FRAME_PAYLOAD + 64 * 1024));
    assert!(frames[0].full && frames[1..].iter().all(|f| !f.full));
    let mut client = Slices::new();
    apply_snapshot(&mut client, &mut ClientCursor::default(), &frames).unwrap();
    assert!(same(&s, &client));
}

#[test]
fn malformed_snapshots_are_rejected() {
    let w = world();
    let server = SyncServer::new(1);
    let mut frames = server.snapshot(&w.s);
    frames[0].full = false;
    assert!(apply_snapshot(&mut Slices::new(), &mut ClientCursor::default(), &frames).is_err());
    assert!(apply_snapshot(&mut Slices::new(), &mut ClientCursor::default(), &[]).is_err());
}

#[test]
fn hostile_bytes_decode_to_nothing_without_panicking() {
    let mut x = 0x1234_5678_9abc_def1_u64;
    for len in 0..400usize {
        let mut buf = vec![0u8; len];
        for b in &mut buf {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            *b = x as u8;
        }
        let _ = decode::<SyncFrame>(&buf);
        let _ = decode::<crate::WireCommand>(&buf);
    }
}

#[test]
fn a_mismatched_key_and_section_is_ignored() {
    let mut s = Slices::new();
    let bad = SyncFrame {
        instance_id: 1,
        seq: 1,
        full: true,
        updates: vec![SectionUpdate {
            key: SectionKey::Core,
            version: 1,
            section: Some(Section::Power(None)),
        }],
    };
    apply_snapshot(&mut s, &mut ClientCursor::default(), &[bad]).unwrap();
    assert!(!s.ready);
}

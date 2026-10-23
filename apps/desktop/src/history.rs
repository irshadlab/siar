//! Chat history from storage.
//!
//! Stored 1:1 payloads are encrypted with a static *pairwise* key, so they
//! can be shown again after a restart — but two facts shape the paging:
//!
//! * My messages to a peer are filed under MY conversation id for them,
//!   while every peer's messages to ME are filed under ONE shared inbound
//!   id (each sender derives it from my endpoint). So a chat is the merge
//!   of two stored streams, and the inbound stream also holds other
//!   peers' messages.
//! * The only way to tell those apart is that they fail to decrypt with
//!   this peer's key. So a page is filled by *decrypting* rows until
//!   `want` succeed, not by counting rows.
//!
//! `collect_page` is pure: storage and decryption are injected, which
//! keeps the boundary and tie handling below unit-testable.

use siar_domain::{ConversationId, DeviceId, MessageContent};
use siar_storage::StoredMessage;
use siar_ui_state::TimelineWindow;

/// Rows requested from each stored stream per pass.
pub const RAW_PAGE: usize = 100;
/// Bound on passes, so a chat buried under thousands of other peers'
/// messages can't stall the UI; the page just comes back short and the
/// user can ask for more.
const MAX_PASSES: usize = 20;

#[derive(Debug)]
pub struct Page {
    /// Oldest → newest, ready for `TimelineState::prepend_page`.
    pub entries: Vec<TimelineWindow>,
    /// Pass this as `before` to get the next-older page.
    pub next_before: Option<u64>,
    /// Nothing older exists for this chat.
    pub exhausted: bool,
}

pub fn collect_page(
    streams: &[ConversationId],
    before: Option<u64>,
    want: usize,
    my_device: DeviceId,
    mut fetch: impl FnMut(ConversationId, Option<u64>, usize) -> Vec<StoredMessage>,
    open: impl Fn(&[u8]) -> Option<MessageContent>,
) -> Page {
    let mut cursor = before;
    let mut out: Vec<TimelineWindow> = Vec::new();

    for _ in 0..MAX_PASSES {
        let fetched: Vec<Vec<StoredMessage>> = streams
            .iter()
            .map(|s| fetch(*s, cursor, RAW_PAGE))
            .collect();

        // A stream that returned a full page may have more rows below its
        // last one, so merged rows are only trustworthy down to the HIGHEST
        // of the full streams' minima. A short stream is exhausted: no limit.
        let boundary = fetched
            .iter()
            .filter(|f| f.len() == RAW_PAGE)
            .filter_map(|f| f.last().map(|m| m.sequence))
            .max();

        let mut rows: Vec<StoredMessage> = fetched
            .into_iter()
            .flatten()
            .filter(|m| boundary.is_none_or(|b| m.sequence >= b))
            .collect();
        rows.sort_by(|a, b| b.sequence.cmp(&a.sequence));

        let mut last_seq = None;
        for m in rows {
            // Once the page is full, keep taking rows that share the last
            // row's sequence: the next page starts strictly below it
            // (`seq < before`), so leaving a tie behind would lose it.
            if out.len() >= want && Some(m.sequence) != last_seq {
                break;
            }
            if let Some(content) = open(&m.payload) {
                last_seq = Some(m.sequence);
                out.push(TimelineWindow {
                    message_id: m.message_id,
                    sequence: m.sequence,
                    content,
                    delivery_state: m.delivery_state,
                    from_me: m.sender_device == my_device,
                });
            }
        }

        if out.len() >= want {
            let next = last_seq.expect("a full page has a last row");
            return finish(out, Some(next), &mut fetch, streams);
        }
        match boundary {
            // Every stream was exhausted within this pass.
            None => return reversed(out, cursor, true),
            Some(b) => cursor = Some(b),
        }
    }
    reversed(out, cursor, false)
}

/// After a full page, probe once below it so the UI can hide "load
/// earlier" when there is genuinely nothing left (instead of offering a
/// button that does nothing).
fn finish(
    out: Vec<TimelineWindow>,
    next_before: Option<u64>,
    fetch: &mut impl FnMut(ConversationId, Option<u64>, usize) -> Vec<StoredMessage>,
    streams: &[ConversationId],
) -> Page {
    let more = streams
        .iter()
        .any(|s| !fetch(*s, next_before, 1).is_empty());
    reversed(out, next_before, !more)
}

fn reversed(mut out: Vec<TimelineWindow>, next_before: Option<u64>, exhausted: bool) -> Page {
    out.reverse();
    Page {
        entries: out,
        next_before,
        exhausted,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use siar_domain::{DeliveryState, MessageId, MessageText};

    const ME: u8 = 0;

    /// payload[0] = which peer the row belongs to (the fake "key"),
    /// payload[1..9] = the message number, so `open` can tell them apart.
    fn row(conv: ConversationId, seq: u64, owner: u8, sender: DeviceId) -> StoredMessage {
        let mut payload = vec![owner];
        payload.extend_from_slice(&seq.to_be_bytes());
        StoredMessage {
            message_id: MessageId::new(),
            conversation_id: conv,
            sender_device: sender,
            sequence: seq,
            timestamp_millis: seq,
            delivery_state: DeliveryState::Sent,
            payload,
        }
    }

    struct Db(Vec<StoredMessage>);
    impl Db {
        fn fetch(
            &self,
            conv: ConversationId,
            before: Option<u64>,
            limit: usize,
        ) -> Vec<StoredMessage> {
            let mut v: Vec<_> = self
                .0
                .iter()
                .filter(|m| m.conversation_id == conv && m.sequence < before.unwrap_or(u64::MAX))
                .cloned()
                .collect();
            v.sort_by(|a, b| b.sequence.cmp(&a.sequence));
            v.truncate(limit);
            v
        }
    }

    fn opener(owner: u8) -> impl Fn(&[u8]) -> Option<MessageContent> {
        move |p: &[u8]| {
            (p[0] == owner).then(|| {
                let n = u64::from_be_bytes(p[1..9].try_into().unwrap());
                MessageContent::Text(MessageText::parse(format!("m{n}")).unwrap())
            })
        }
    }

    fn texts(p: &Page) -> Vec<String> {
        p.entries
            .iter()
            .map(|e| match &e.content {
                MessageContent::Text(t) => t.as_str().to_string(),
                _ => unreachable!(),
            })
            .collect()
    }

    #[test]
    fn merges_both_streams_in_order_and_marks_my_messages() {
        let (out_c, in_c) = (ConversationId::new(), ConversationId::new());
        let (me, peer) = (DeviceId::new(), DeviceId::new());
        let db = Db(vec![
            row(out_c, 1, 1, me),
            row(in_c, 2, 1, peer),
            row(out_c, 3, 1, me),
            row(in_c, 4, 1, peer),
        ]);
        let page = collect_page(
            &[out_c, in_c],
            None,
            50,
            me,
            |c, b, l| db.fetch(c, b, l),
            opener(1),
        );
        assert_eq!(texts(&page), ["m1", "m2", "m3", "m4"]);
        assert_eq!(
            page.entries.iter().map(|e| e.from_me).collect::<Vec<_>>(),
            [true, false, true, false]
        );
        assert!(page.exhausted);
    }

    #[test]
    fn other_peers_messages_in_the_shared_inbound_stream_are_skipped() {
        let (out_c, in_c) = (ConversationId::new(), ConversationId::new());
        let (me, peer, other) = (DeviceId::new(), DeviceId::new(), DeviceId::new());
        let db = Db(vec![
            row(in_c, 1, 1, peer),
            row(in_c, 2, 2, other), // someone else's — fails to open
            row(in_c, 3, 1, peer),
            row(in_c, 4, 3, other),
        ]);
        let page = collect_page(
            &[out_c, in_c],
            None,
            50,
            me,
            |c, b, l| db.fetch(c, b, l),
            opener(1),
        );
        assert_eq!(texts(&page), ["m1", "m3"]);
        assert!(page.exhausted);
    }

    #[test]
    fn pages_walk_backwards_without_gaps_or_repeats() {
        let (out_c, in_c) = (ConversationId::new(), ConversationId::new());
        let (me, peer) = (DeviceId::new(), DeviceId::new());
        let mut rows = Vec::new();
        for seq in 1..=25u64 {
            let (c, who) = if seq % 2 == 0 {
                (out_c, me)
            } else {
                (in_c, peer)
            };
            rows.push(row(c, seq, 1, who));
        }
        let db = Db(rows);
        let mut seen = Vec::new();
        let mut before = None;
        let mut pages = 0;
        loop {
            let page = collect_page(
                &[out_c, in_c],
                before,
                10,
                me,
                |c, b, l| db.fetch(c, b, l),
                opener(1),
            );
            seen.splice(0..0, texts(&page));
            pages += 1;
            if page.exhausted {
                break;
            }
            before = page.next_before;
            assert!(pages < 10, "must terminate");
        }
        let expected: Vec<String> = (1..=25).map(|n| format!("m{n}")).collect();
        assert_eq!(seen, expected);
        assert_eq!(pages, 3);
    }

    #[test]
    fn a_page_boundary_inside_one_full_stream_loses_nothing() {
        // Stream A has MORE than RAW_PAGE rows, interleaved with stream B's:
        // merged order is only valid down to A's last fetched row.
        let (a, b) = (ConversationId::new(), ConversationId::new());
        let (me, peer) = (DeviceId::new(), DeviceId::new());
        let mut rows = Vec::new();
        for i in 0..250u64 {
            rows.push(row(a, i * 2, 1, me)); // evens
        }
        for i in 0..40u64 {
            rows.push(row(b, i * 2 + 1, 1, peer)); // sparse odds, low range
        }
        let db = Db(rows);
        let page = collect_page(
            &[a, b],
            None,
            300,
            me,
            |c, bf, l| db.fetch(c, bf, l),
            opener(1),
        );
        let mut seqs: Vec<u64> = page.entries.iter().map(|e| e.sequence).collect();
        let sorted = {
            let mut s = seqs.clone();
            s.sort_unstable();
            s
        };
        assert_eq!(seqs, sorted, "oldest → newest");
        seqs.dedup();
        assert_eq!(seqs.len(), page.entries.len(), "no repeats");
        assert_eq!(
            page.entries.len(),
            290,
            "every row was returned exactly once"
        );
        assert!(page.exhausted);
    }

    #[test]
    fn rows_sharing_the_cut_sequence_are_not_lost_between_pages() {
        let (out_c, in_c) = (ConversationId::new(), ConversationId::new());
        let (me, peer) = (DeviceId::new(), DeviceId::new());
        // three rows at seq 5, wanting only 2 per page
        let db = Db(vec![
            row(out_c, 5, 1, me),
            row(in_c, 5, 1, peer),
            row(out_c, 5, 1, me),
            row(in_c, 9, 1, peer),
            row(in_c, 1, 1, peer),
        ]);
        let first = collect_page(
            &[out_c, in_c],
            None,
            2,
            me,
            |c, b, l| db.fetch(c, b, l),
            opener(1),
        );
        let second = collect_page(
            &[out_c, in_c],
            first.next_before,
            2,
            me,
            |c, b, l| db.fetch(c, b, l),
            opener(1),
        );
        let total = first.entries.len() + second.entries.len();
        assert_eq!(total, 5, "ties at the cut must all be delivered");
    }

    #[test]
    fn empty_storage_is_an_exhausted_empty_page() {
        let page = collect_page(
            &[ConversationId::new()],
            None,
            50,
            DeviceId::new(),
            |_, _, _| vec![],
            |_| None,
        );
        assert!(page.entries.is_empty());
        assert!(page.exhausted);
    }

    #[test]
    fn only_foreign_rows_exhaust_without_hanging() {
        let (out_c, in_c) = (ConversationId::new(), ConversationId::new());
        let (me, other) = (DeviceId::new(), DeviceId::new());
        let rows = (0..300u64).map(|i| row(in_c, i, 9, other)).collect();
        let db = Db(rows);
        let page = collect_page(
            &[out_c, in_c],
            None,
            50,
            me,
            |c, b, l| db.fetch(c, b, l),
            opener(1),
        );
        assert!(page.entries.is_empty());
        assert!(page.exhausted);
        let _ = ME;
    }
}

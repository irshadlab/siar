//! §63: the message-request inbox.
//!
//! A first message from someone we do not know must **not** become a
//! conversation, a notification, a download or a database fan-out. It becomes
//! a tiny, bounded *request* the user can accept, decline or block. Only on
//! accept does the caller fetch the rest and promote the sender.
//!
//! Hard bounds: a short preview per request, a few requests per sender, a
//! global cap on count and bytes, and an expiry. When full, **new** requests
//! are refused; older ones are never evicted to make room, so an attacker
//! cannot push a genuine request out by flooding.

use std::collections::VecDeque;

use crate::gate::PeerKey;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RequestId(pub u64);

#[derive(Debug, Clone, Copy)]
pub struct InboxConfig {
    pub max_total: usize,
    pub max_per_peer: usize,
    pub preview_bytes: usize,
    pub max_total_bytes: usize,
    pub ttl_ms: u64,
}

impl Default for InboxConfig {
    fn default() -> Self {
        Self {
            max_total: 200,
            max_per_peer: 3,
            preview_bytes: 512,
            max_total_bytes: 256 * 1024,
            ttl_ms: 7 * 24 * 3_600_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub id: RequestId,
    pub peer: PeerKey,
    /// Opaque, length-limited bytes from an **untrusted** sender. Render only
    /// after sanitising; never treat as markup or a link target.
    pub preview: Vec<u8>,
    pub truncated: bool,
    pub received_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InboxError {
    #[error("this sender already has the maximum number of pending requests")]
    PeerLimit,
    #[error("the request inbox is full")]
    Full,
}

#[derive(Debug)]
pub struct RequestInbox {
    cfg: InboxConfig,
    items: VecDeque<Request>,
    bytes: usize,
    next_id: u64,
}

impl RequestInbox {
    #[must_use]
    pub fn new(cfg: InboxConfig) -> Self {
        Self {
            cfg,
            items: VecDeque::new(),
            bytes: 0,
            next_id: 1,
        }
    }

    pub fn offer(
        &mut self,
        peer: PeerKey,
        message: &[u8],
        now_ms: u64,
    ) -> Result<RequestId, InboxError> {
        self.sweep(now_ms);
        if self.items.iter().filter(|r| r.peer == peer).count() >= self.cfg.max_per_peer {
            return Err(InboxError::PeerLimit);
        }
        let take = message.len().min(self.cfg.preview_bytes);
        if self.items.len() >= self.cfg.max_total || self.bytes + take > self.cfg.max_total_bytes {
            return Err(InboxError::Full);
        }
        let id = RequestId(self.next_id);
        self.next_id += 1;
        self.bytes += take;
        self.items.push_back(Request {
            id,
            peer,
            preview: message[..take].to_vec(),
            truncated: take < message.len(),
            received_ms: now_ms,
        });
        Ok(id)
    }

    fn remove_at(&mut self, i: usize) -> Request {
        let r = self.items.remove(i).expect("valid index");
        self.bytes -= r.preview.len();
        r
    }

    /// The user accepted: hand the request to the caller to promote.
    pub fn accept(&mut self, id: RequestId) -> Option<Request> {
        let i = self.items.iter().position(|r| r.id == id)?;
        Some(self.remove_at(i))
    }

    pub fn decline(&mut self, id: RequestId) -> bool {
        self.accept(id).is_some()
    }

    /// The user blocked the sender: drop everything they sent. (The caller
    /// also adds them to the gate's blocklist.)
    pub fn block_peer(&mut self, peer: PeerKey) -> usize {
        let before = self.items.len();
        let mut kept = VecDeque::with_capacity(before);
        for r in self.items.drain(..) {
            if r.peer == peer {
                self.bytes -= r.preview.len();
            } else {
                kept.push_back(r);
            }
        }
        self.items = kept;
        before - self.items.len()
    }

    /// Drop expired requests.
    pub fn sweep(&mut self, now_ms: u64) -> usize {
        let ttl = self.cfg.ttl_ms;
        let before = self.items.len();
        let mut kept = VecDeque::with_capacity(before);
        for r in self.items.drain(..) {
            if now_ms.saturating_sub(r.received_ms) >= ttl {
                self.bytes -= r.preview.len();
            } else {
                kept.push_back(r);
            }
        }
        self.items = kept;
        before - self.items.len()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    #[must_use]
    pub const fn total_bytes(&self) -> usize {
        self.bytes
    }

    pub fn pending(&self) -> impl Iterator<Item = &Request> {
        self.items.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pk(n: u8) -> PeerKey {
        PeerKey([n; 16])
    }

    #[test]
    fn previews_are_truncated_and_flagged() {
        let mut i = RequestInbox::new(InboxConfig::default());
        let id = i.offer(pk(1), &vec![b'a'; 100_000], 0).unwrap();
        let r = i.accept(id).unwrap();
        assert_eq!(r.preview.len(), 512);
        assert!(r.truncated);
        assert_eq!(i.total_bytes(), 0);
    }

    #[test]
    fn one_sender_cannot_fill_the_inbox() {
        let mut i = RequestInbox::new(InboxConfig::default());
        for _ in 0..3 {
            i.offer(pk(1), b"hi", 0).unwrap();
        }
        assert_eq!(i.offer(pk(1), b"hi", 0), Err(InboxError::PeerLimit));
        assert!(
            i.offer(pk(2), b"hi", 0).is_ok(),
            "other senders are unaffected"
        );
    }

    #[test]
    fn when_full_new_requests_are_refused_and_old_ones_are_never_evicted() {
        let mut i = RequestInbox::new(InboxConfig {
            max_total: 5,
            ..Default::default()
        });
        let first = i.offer(pk(0), b"genuine", 0).unwrap();
        for n in 1..5 {
            i.offer(pk(n), b"x", 0).unwrap();
        }
        for n in 10..100 {
            assert_eq!(i.offer(pk(n), b"flood", 0), Err(InboxError::Full));
        }
        assert!(
            i.pending().any(|r| r.id == first),
            "the genuine request survived the flood"
        );
    }

    #[test]
    fn the_byte_budget_is_enforced() {
        let mut i = RequestInbox::new(InboxConfig {
            max_total_bytes: 1_000,
            max_total: 1_000,
            max_per_peer: 1_000,
            ..Default::default()
        });
        let mut accepted = 0;
        for n in 0..100u8 {
            if i.offer(pk(n), &[0u8; 400], 0).is_ok() {
                accepted += 1;
            }
        }
        assert!(accepted <= 2 && i.total_bytes() <= 1_000);
    }

    #[test]
    fn blocking_a_sender_removes_all_their_requests_and_returns_their_bytes() {
        let mut i = RequestInbox::new(InboxConfig::default());
        i.offer(pk(1), b"aaa", 0).unwrap();
        i.offer(pk(1), b"bbb", 0).unwrap();
        i.offer(pk(2), b"ccc", 0).unwrap();
        assert_eq!(i.block_peer(pk(1)), 2);
        assert_eq!((i.len(), i.total_bytes()), (1, 3));
    }

    #[test]
    fn requests_expire() {
        let mut i = RequestInbox::new(InboxConfig {
            ttl_ms: 1_000,
            ..Default::default()
        });
        i.offer(pk(1), b"old", 0).unwrap();
        assert_eq!(i.sweep(500), 0);
        i.offer(pk(2), b"new", 1_500).unwrap(); // offer() sweeps first
        assert_eq!(i.len(), 1);
        assert_eq!(i.pending().next().unwrap().peer, pk(2));
    }

    #[test]
    fn decline_and_accept_of_unknown_ids_are_harmless() {
        let mut i = RequestInbox::new(InboxConfig::default());
        assert!(!i.decline(RequestId(99)));
        assert!(i.accept(RequestId(99)).is_none());
    }
}

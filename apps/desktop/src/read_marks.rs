//! Read state that survives a restart.
//!
//! One number per chat: the highest *incoming* sequence the user has
//! seen. Unread is then "incoming messages above that mark", which can be
//! recomputed from storage at start-up — so no per-message flag needs to
//! be stored, and nothing can drift out of sync with the messages.
//!
//! Only incoming sequences are compared with the mark. They all come from
//! the one peer's clock and are strictly increasing per sender (see
//! `next_sequence` in siar-messaging), so no cross-device clock skew
//! enters the comparison.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use siar_domain::ConversationId;
use siar_ui_state::TimelineWindow;

#[derive(Debug, Default)]
pub struct ReadMarks {
    path: Option<PathBuf>,
    marks: HashMap<ConversationId, u64>,
}

impl ReadMarks {
    /// A missing or damaged file means "no marks yet", which `Core` treats
    /// as "everything already read" — a bad file can never produce a wall
    /// of phantom unread badges.
    pub fn load(path: &Path) -> Self {
        let mut marks = HashMap::new();
        if let Ok(text) = std::fs::read_to_string(path) {
            for line in text.lines() {
                let Some((id, seq)) = line.split_once('=') else {
                    continue;
                };
                if let (Ok(id), Ok(seq)) = (
                    id.trim().parse::<ConversationId>(),
                    seq.trim().parse::<u64>(),
                ) {
                    marks.insert(id, seq);
                }
            }
        }
        Self {
            path: Some(path.to_path_buf()),
            marks,
        }
    }

    #[cfg(test)]
    pub fn in_memory() -> Self {
        Self::default()
    }

    pub fn get(&self, conversation: ConversationId) -> Option<u64> {
        self.marks.get(&conversation).copied()
    }

    /// Raises the mark; never lowers it (an out-of-order or replayed call
    /// must not resurrect messages already read). Returns whether it moved.
    pub fn advance(
        &mut self,
        conversation: ConversationId,
        sequence: u64,
    ) -> std::io::Result<bool> {
        if self
            .marks
            .get(&conversation)
            .is_some_and(|&m| m >= sequence)
        {
            return Ok(false);
        }
        self.marks.insert(conversation, sequence);
        self.save()?;
        Ok(true)
    }

    fn save(&self) -> std::io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let mut lines: Vec<String> = self.marks.iter().map(|(c, s)| format!("{c}={s}")).collect();
        lines.sort();
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, lines.join("\n") + "\n")?;
        std::fs::rename(&tmp, path)
    }
}

/// Highest incoming sequence in a timeline, if any.
pub fn newest_incoming<'a>(entries: impl Iterator<Item = &'a TimelineWindow>) -> Option<u64> {
    entries.filter(|e| !e.from_me).map(|e| e.sequence).max()
}

/// Incoming messages strictly above `mark`.
pub fn unread_above<'a>(entries: impl Iterator<Item = &'a TimelineWindow>, mark: u64) -> u32 {
    entries.filter(|e| !e.from_me && e.sequence > mark).count() as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use siar_domain::{DeliveryState, MessageContent, MessageId, MessageText};

    fn entry(seq: u64, from_me: bool) -> TimelineWindow {
        TimelineWindow {
            message_id: MessageId::new(),
            sequence: seq,
            content: MessageContent::Text(MessageText::parse("x".into()).unwrap()),
            delivery_state: DeliveryState::Delivered,
            from_me,
        }
    }

    #[test]
    fn unread_counts_only_incoming_strictly_above_the_mark() {
        let t = [
            entry(1, false),
            entry(2, true),
            entry(3, false),
            entry(4, false),
            entry(5, true),
        ];
        assert_eq!(unread_above(t.iter(), 0), 3);
        assert_eq!(unread_above(t.iter(), 3), 1); // seq 3 itself is read
        assert_eq!(unread_above(t.iter(), 4), 0);
        assert_eq!(unread_above([].iter(), 0), 0);
        // my own messages never count, however new
        assert_eq!(unread_above([entry(99, true)].iter(), 0), 0);
    }

    #[test]
    fn newest_incoming_ignores_my_messages() {
        let t = [entry(1, false), entry(9, true), entry(4, false)];
        assert_eq!(newest_incoming(t.iter()), Some(4));
        assert_eq!(newest_incoming([entry(9, true)].iter()), None);
    }

    #[test]
    fn marks_only_move_forward() {
        let mut m = ReadMarks::in_memory();
        let c = ConversationId::new();
        assert_eq!(m.get(c), None);
        assert!(m.advance(c, 10).unwrap());
        assert!(!m.advance(c, 10).unwrap());
        assert!(!m.advance(c, 3).unwrap(), "never lowered");
        assert_eq!(m.get(c), Some(10));
        assert!(m.advance(c, 11).unwrap());
    }

    #[test]
    fn marks_survive_a_restart_and_garbage_is_ignored() {
        let dir = std::env::temp_dir().join(format!("siar-read-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("read-marks.txt");
        let (a, b) = (ConversationId::new(), ConversationId::new());
        let mut m = ReadMarks::load(&path);
        m.advance(a, 42).unwrap();
        m.advance(b, 7).unwrap();
        let mut text = std::fs::read_to_string(&path).unwrap();
        text.push_str("junk\nnot-a-uuid=5\n");
        std::fs::write(&path, text).unwrap();
        let again = ReadMarks::load(&path);
        assert_eq!((again.get(a), again.get(b)), (Some(42), Some(7)));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

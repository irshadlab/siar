//! View-model state for rendering image attachments inline (closing the
//! gap flagged in `apps/desktop`'s `components.rs`: attachments only
//! ever showed as a `[attachment, N bytes]` placeholder, and incoming
//! 1:1 `Attachment` content wasn't even reaching the timeline).
//!
//! Deliberately keyed by `MessageId` rather than folded into
//! `TimelineWindow` itself: fetching + decoding + downscaling an
//! attachment is a slow, fallible, on-demand operation (`siar-messaging`'s
//! own `fetch_attachment` doc comment: "not called automatically...
//! attachments download on demand"), so it needs its own load-state
//! machine (`NotRequested` -> `Loading` -> `Ready`/`Failed`) separate
//! from the message arriving. Same granular-signal reasoning as every
//! other slice in this crate (plan.md §94) — a component that only
//! renders one message's preview shouldn't re-render when another
//! message's preview finishes loading, though the `HashMap`-per-state
//! granularity that would require is left as a possible future
//! refinement, not attempted here.

use siar_domain::MessageId;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttachmentPreview {
    NotRequested,
    Loading,
    /// JPEG bytes of a downscaled preview (`siar_media_image::
    /// generate_preview`'s output) — stored pre-encoded rather than as
    /// raw RGBA so this crate never has to know how to turn pixels back
    /// into a displayable image; `apps/desktop` base64-encodes this into
    /// a `data:` URI at render time.
    Ready {
        jpeg_bytes: Vec<u8>,
        width: u32,
        height: u32,
    },
    /// Held as a display string, not a typed error — this crate has no
    /// dependency on `siar-media-image`/`siar-messaging` to name their
    /// error types (same infra-free boundary `lib.rs`'s module doc
    /// states for the whole crate), and a UI only ever shows this
    /// verbatim, never matches on it.
    Failed {
        reason: String,
    },
}

#[derive(Debug, Default)]
pub struct AttachmentPreviewState {
    previews: HashMap<MessageId, AttachmentPreview>,
}

impl AttachmentPreviewState {
    pub fn new() -> Self {
        Self::default()
    }

    /// What a component should render right now for this message —
    /// `NotRequested` for any message it's never seen before, so a
    /// missing entry and an explicitly-not-yet-requested one render
    /// identically without the caller needing to handle `Option`.
    pub fn get(&self, message_id: MessageId) -> AttachmentPreview {
        self.previews
            .get(&message_id)
            .cloned()
            .unwrap_or(AttachmentPreview::NotRequested)
    }

    /// Every message that has a preview in any state but `NotRequested`.
    pub fn iter(&self) -> impl Iterator<Item = (MessageId, &AttachmentPreview)> {
        self.previews.iter().map(|(id, p)| (*id, p))
    }

    /// Back to `NotRequested`.
    pub fn forget(&mut self, message_id: MessageId) {
        self.previews.remove(&message_id);
    }

    pub fn set_loading(&mut self, message_id: MessageId) {
        self.previews.insert(message_id, AttachmentPreview::Loading);
    }

    pub fn set_ready(
        &mut self,
        message_id: MessageId,
        jpeg_bytes: Vec<u8>,
        width: u32,
        height: u32,
    ) {
        self.previews.insert(
            message_id,
            AttachmentPreview::Ready {
                jpeg_bytes,
                width,
                height,
            },
        );
    }

    pub fn set_failed(&mut self, message_id: MessageId, reason: String) {
        self.previews
            .insert(message_id, AttachmentPreview::Failed { reason });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unrequested_message_reads_as_not_requested_not_missing() {
        let state = AttachmentPreviewState::new();
        assert_eq!(state.get(MessageId::new()), AttachmentPreview::NotRequested);
    }

    #[test]
    fn loading_then_ready_transitions_are_visible_to_readers() {
        let mut state = AttachmentPreviewState::new();
        let id = MessageId::new();
        state.set_loading(id);
        assert_eq!(state.get(id), AttachmentPreview::Loading);

        state.set_ready(id, vec![1, 2, 3], 160, 90);
        assert_eq!(
            state.get(id),
            AttachmentPreview::Ready {
                jpeg_bytes: vec![1, 2, 3],
                width: 160,
                height: 90
            }
        );
    }

    #[test]
    fn failed_preview_is_distinct_from_loading_and_not_requested() {
        let mut state = AttachmentPreviewState::new();
        let id = MessageId::new();
        state.set_failed(id, "unsupported format".to_string());
        assert_eq!(
            state.get(id),
            AttachmentPreview::Failed {
                reason: "unsupported format".to_string()
            }
        );
    }

    #[test]
    fn each_message_id_tracks_its_own_preview_independently() {
        let mut state = AttachmentPreviewState::new();
        let a = MessageId::new();
        let b = MessageId::new();
        state.set_ready(a, vec![1], 1, 1);
        state.set_loading(b);
        assert_eq!(
            state.get(a),
            AttachmentPreview::Ready {
                jpeg_bytes: vec![1],
                width: 1,
                height: 1
            }
        );
        assert_eq!(state.get(b), AttachmentPreview::Loading);
    }
}

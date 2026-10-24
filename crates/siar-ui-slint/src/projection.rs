//! Slices → Slint models.
//!
//! Runs only on the UI thread. Each list is synced into its long-lived
//! `VecModel` with a keyed in-place diff (`sync_model`), so a new message
//! appends one row instead of rebuilding the list, and scroll position,
//! hover and focus on untouched rows survive every update.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use siar_domain::{MediaType, MessageContent, MessageId};
use siar_ui_state::{AttachmentPreview, ConversationKind, NetworkState, SecurityEventSeverity};
use slint::{
    ComponentHandle, Image, Model, ModelRc, Rgba8Pixel, SharedPixelBuffer, SharedString, VecModel,
};

use crate::hub::UiHub;
use crate::slices::{EventView, NoticeLevel, Slices, VerificationState};
use crate::text;
use crate::ui::{
    AppState, ContactRow, ConversationRow, DeviceRow, EventRow, GroupRow, InviteRow, MainWindow,
    MessageRow, NoticeKind,
};

/// Longest edge, in logical px, an inline preview is drawn at.
const PREVIEW_MAX_EDGE: f32 = 320.0;

struct Models {
    conversations: Rc<VecModel<ConversationRow>>,
    messages: Rc<VecModel<MessageRow>>,
    contacts: Rc<VecModel<ContactRow>>,
    groups: Rc<VecModel<GroupRow>>,
    invites: Rc<VecModel<InviteRow>>,
    events: Rc<VecModel<EventRow>>,
    devices: Rc<VecModel<DeviceRow>>,
}

#[derive(Default)]
struct ImageCache {
    decoded: HashMap<MessageId, (Image, u32, u32)>,
}

thread_local! {
    static MODELS: RefCell<Option<Models>> = const { RefCell::new(None) };
    static IMAGES: RefCell<ImageCache> = RefCell::new(ImageCache::default());
}

/// Binds fresh models to the window's `AppState`. UI thread, once.
pub(crate) fn install(window: &MainWindow) {
    let models = Models {
        conversations: Rc::new(VecModel::default()),
        messages: Rc::new(VecModel::default()),
        contacts: Rc::new(VecModel::default()),
        groups: Rc::new(VecModel::default()),
        invites: Rc::new(VecModel::default()),
        events: Rc::new(VecModel::default()),
        devices: Rc::new(VecModel::default()),
    };
    let app = window.global::<AppState>();
    app.set_conversations(ModelRc::from(models.conversations.clone()));
    app.set_messages(ModelRc::from(models.messages.clone()));
    app.set_contacts(ModelRc::from(models.contacts.clone()));
    app.set_groups(ModelRc::from(models.groups.clone()));
    app.set_invites(ModelRc::from(models.invites.clone()));
    app.set_events(ModelRc::from(models.events.clone()));
    app.set_devices(ModelRc::from(models.devices.clone()));
    MODELS.with(|m| *m.borrow_mut() = Some(models));
}

pub(crate) fn flush(hub: &UiHub) {
    let Some(window) = hub.0.window.upgrade() else {
        return;
    };
    MODELS.with(|models| {
        let models = models.borrow();
        let Some(models) = models.as_ref() else {
            tracing::error!("flush before install — models missing");
            return;
        };
        let mut slices = hub.lock();
        project(&mut slices, models, &window.global::<AppState>());
    });
}

/// In-place diff: updates changed rows, trims or appends the tail.
pub(crate) fn sync_model<T: Clone + PartialEq + 'static>(model: &VecModel<T>, new: Vec<T>) {
    let common = model.row_count().min(new.len());
    for (i, item) in new.iter().take(common).enumerate() {
        if model.row_data(i).as_ref() != Some(item) {
            model.set_row_data(i, item.clone());
        }
    }
    while model.row_count() > new.len() {
        model.remove(model.row_count() - 1);
    }
    for item in new.into_iter().skip(common) {
        model.push(item);
    }
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn project(s: &mut Slices, m: &Models, app: &AppState<'_>) {
    // ---- lifecycle & identity -------------------------------------------
    app.set_ready(s.ready);
    app.set_startup_error(s.startup_error.clone().unwrap_or_default().into());
    app.set_my_ticket(s.identity.ticket.clone().into());
    app.set_my_key_package(s.identity.key_package.clone().into());
    app.set_my_account_id(
        s.identity
            .account
            .map(|a| a.to_string())
            .unwrap_or_default()
            .into(),
    );
    app.set_my_device_id(
        s.identity
            .device
            .map(|d| d.to_string())
            .unwrap_or_default()
            .into(),
    );

    // ---- network ---------------------------------------------------------
    let (kind, label) = match s.network.overall() {
        None => (0, "Connecting…"),
        Some(NetworkState::Online) => (1, "Online"),
        Some(NetworkState::Offline) => (2, "Offline — messages send when you reconnect"),
    };
    app.set_network_kind(kind);
    app.set_network_label(label.into());

    // ---- conversations ----------------------------------------------------
    let q = s.conversation_filter.trim().to_lowercase();
    let rows: Vec<ConversationRow> = s
        .conversations
        .ordered()
        .iter()
        .filter(|c| {
            q.is_empty()
                || c.display_name.to_lowercase().contains(&q)
                || c.last_message_preview
                    .as_deref()
                    .unwrap_or("")
                    .to_lowercase()
                    .contains(&q)
        })
        .map(|c| {
            let id = c.id.to_string();
            ConversationRow {
                hue: text::hue_for(&id),
                selected: s.selected == Some(c.id),
                title: c.display_name.clone().into(),
                preview: c.last_message_preview.clone().unwrap_or_default().into(),
                initial: text::initial(&c.display_name).into(),
                unread: c.unread_count as i32,
                is_group: c.kind == ConversationKind::Group,
                id: id.into(),
            }
        })
        .collect();
    sync_model(&m.conversations, rows);
    let total_unread: u32 = s
        .conversations
        .ordered()
        .iter()
        .map(|c| c.unread_count)
        .sum();
    app.set_total_unread(total_unread as i32);

    let selected = s.selected.and_then(|id| {
        s.conversations
            .ordered()
            .iter()
            .find(|c| c.id == id)
            .map(|c| (id, c))
    });
    match selected {
        Some((id, c)) => {
            app.set_selected_id(id.to_string().into());
            app.set_selected_title(c.display_name.clone().into());
            app.set_selected_is_group(c.kind == ConversationKind::Group);
            app.set_can_send(s.can_send(id));
        }
        None => {
            app.set_selected_id("".into());
            app.set_selected_title("".into());
            app.set_selected_is_group(false);
            app.set_can_send(false);
        }
    }

    // ---- messages (selected conversation only) ---------------------------
    let mut ready_ids = Vec::new();
    let mut msg_rows = Vec::new();
    if let Some((id, _)) = selected {
        if let Some(timeline) = s.timelines.get(&id) {
            let mut prev_from_me: Option<bool> = None;
            for e in timeline.visible() {
                let (status_icon, status_label) = if e.from_me {
                    text::delivery(e.delivery_state)
                } else {
                    (0, String::new())
                };
                let mut row = MessageRow {
                    id: e.message_id.to_string().into(),
                    from_me: e.from_me,
                    group_start: prev_from_me != Some(e.from_me),
                    status_icon,
                    status_label: status_label.into(),
                    ..Default::default()
                };
                match &e.content {
                    MessageContent::Text(t) => {
                        row.kind = 0;
                        row.text = t.as_str().into();
                    }
                    MessageContent::Attachment(r) => {
                        let is_image = matches!(
                            r.media_type,
                            MediaType::ImagePng | MediaType::ImageJpeg | MediaType::ImageWebp
                        );
                        row.attachment_label =
                            text::media_label(r.media_type, r.encrypted_size.bytes()).into();
                        if is_image {
                            row.kind = 1;
                            apply_preview(
                                &mut row,
                                s.attachment_previews.get(e.message_id),
                                e.message_id,
                                &mut ready_ids,
                            );
                        } else {
                            row.kind = 2;
                        }
                    }
                }
                prev_from_me = Some(e.from_me);
                msg_rows.push(row);
            }
        }
    }
    app.set_newest_message_id(msg_rows.last().map(|r| r.id.clone()).unwrap_or_default());
    app.set_has_more_history(
        selected
            .and_then(|(id, _)| s.history.get(&id))
            .is_some_and(|h| h.loaded && !h.exhausted),
    );
    sync_model(&m.messages, msg_rows);
    // Forget decoded images that are no longer on screen.
    IMAGES.with(|c| {
        c.borrow_mut()
            .decoded
            .retain(|id, _| ready_ids.contains(id))
    });

    // ---- contacts -----------------------------------------------------------
    let contact_rows: Vec<ContactRow> = s
        .contacts
        .ordered()
        .iter()
        .map(|c| {
            let id = c.device_id.to_string();
            let name = if c.display_name.trim().is_empty() {
                format!("Contact {}", c.device_id.fmt_short())
            } else {
                c.display_name.clone()
            };
            ContactRow {
                verification: verification_code(
                    s.contact_safety.get(&c.device_id).map(|x| x.state),
                ),
                hue: text::hue_for(&id),
                selected: s.selected_contact == Some(c.device_id),
                initial: text::initial(&name).into(),
                name: name.into(),
                short_id: c.device_id.fmt_short().into(),
                id: id.into(),
            }
        })
        .collect();
    sync_model(&m.contacts, contact_rows);
    match s
        .selected_contact
        .and_then(|d| s.contacts.ordered().iter().find(|c| c.device_id == d))
    {
        Some(c) => {
            app.set_selected_contact_id(c.device_id.to_string().into());
            app.set_selected_contact_name(
                if c.display_name.trim().is_empty() {
                    format!("Contact {}", c.device_id.fmt_short())
                } else {
                    c.display_name.clone()
                }
                .into(),
            );
            app.set_selected_contact_detail(
                format!(
                    "Device {} · account {}",
                    c.device_id.fmt_short(),
                    c.account_id.fmt_short()
                )
                .into(),
            );
            let safety = s.contact_safety.get(&c.device_id);
            app.set_selected_contact_safety(
                safety
                    .map(|x| text::wrap_groups(&x.number, 3))
                    .unwrap_or_default()
                    .into(),
            );
            app.set_selected_contact_verification(verification_code(safety.map(|x| x.state)));
        }
        None => {
            app.set_selected_contact_id("".into());
            app.set_selected_contact_name("".into());
            app.set_selected_contact_detail("".into());
            app.set_selected_contact_safety("".into());
            app.set_selected_contact_verification(0);
        }
    }

    // ---- groups & invites ------------------------------------------------------
    let group_rows: Vec<GroupRow> = s
        .groups
        .ordered()
        .iter()
        .map(|g| GroupRow {
            id: g.conversation_id.to_string().into(),
            label: g.display_label.clone().into(),
            members: format!(
                "{} member{}",
                g.member_count,
                if g.member_count == 1 { "" } else { "s" }
            )
            .into(),
            admin: g.is_admin,
            epoch: g.epoch.to_string().into(),
            selected: s.selected_group == Some(g.conversation_id),
        })
        .collect();
    sync_model(&m.groups, group_rows);
    match s.selected_group.and_then(|id| s.groups.get(id)) {
        Some(g) => {
            app.set_selected_group_id(g.conversation_id.to_string().into());
            app.set_selected_group_label(g.display_label.clone().into());
            app.set_selected_group_admin(g.is_admin);
        }
        None => {
            app.set_selected_group_id("".into());
            app.set_selected_group_label("".into());
            app.set_selected_group_admin(false);
        }
    }
    let invite_rows: Vec<InviteRow> = s
        .pending_invites
        .pending()
        .iter()
        .map(|i| InviteRow {
            id: i.conversation_id.to_string().into(),
            from: i.from_device.fmt_short().into(),
            has_state: i.carries_state,
        })
        .collect();
    sync_model(&m.invites, invite_rows);

    // ---- security -----------------------------------------------------------------
    let all_events = s.security_events.events();
    let event_rows: Vec<EventRow> = all_events
        .iter()
        .rev() // newest first
        .filter(|e| match s.event_view {
            EventView::All => true,
            EventView::Unresolved => !e.resolved,
            EventView::Critical => e.severity == SecurityEventSeverity::Critical,
        })
        .map(|e| {
            let (code, label) = text::severity(e.severity);
            EventRow {
                id: e.id.get().to_string().into(),
                title: text::event_headline(e.kind).into(),
                severity: code,
                severity_label: label.into(),
                resolved: e.resolved,
            }
        })
        .collect();
    app.set_unresolved_events(all_events.iter().filter(|e| !e.resolved).count() as i32);
    // The banner shows unresolved critical events the user has not already
    // dismissed. Dismissal is local only (ui-ux-15 §98): the event stays
    // unresolved and listed under Security.
    let criticals = s.banner.visible_issues(&s.security_events);
    app.set_critical_count(criticals.len() as i32);
    app.set_critical_title(
        criticals
            .first()
            .map(|e| text::event_headline(e.kind))
            .unwrap_or("")
            .into(),
    );
    app.set_critical_id(
        criticals
            .first()
            .map(|e| e.id.get().to_string())
            .unwrap_or_default()
            .into(),
    );
    sync_model(&m.events, event_rows);

    let now = now_millis();
    let mut device_rows = Vec::new();
    let section = |title: &'static str,
                   devices: Vec<&siar_ui_state::DeviceSecurityView>,
                   out: &mut Vec<DeviceRow>| {
        for (i, d) in devices.into_iter().enumerate() {
            let (kind_label, kind_icon) = text::device_kind(d.kind);
            let (status, status_label) = text::trust(d.status);
            out.push(DeviceRow {
                id: d.id.to_string().into(),
                name: siar_ui_state::device_display_name(d).into(),
                kind_label: kind_label.into(),
                kind_icon,
                status,
                status_label: status_label.into(),
                last_active: text::coarse_last_active(now, d.last_active_millis).into(),
                is_this_device: d.current_device,
                flags: d
                    .security_flags
                    .iter()
                    .map(|f| text::flag_text(*f))
                    .collect::<Vec<_>>()
                    .join(" ")
                    .into(),
                section_title: if i == 0 {
                    title.into()
                } else {
                    SharedString::default()
                },
            });
        }
    };
    section(
        "This device",
        s.devices.this_device().into_iter().collect(),
        &mut device_rows,
    );
    section(
        "Trusted devices",
        s.devices.trusted().collect(),
        &mut device_rows,
    );
    section(
        "Pending approval",
        s.devices.pending().collect(),
        &mut device_rows,
    );
    section(
        "Removed / history",
        s.devices.revoked_or_history().collect(),
        &mut device_rows,
    );
    sync_model(&m.devices, device_rows);

    // ---- identity & diagnostics -------------------------------------------------------
    match &s.own_identity {
        Some(own) => {
            app.set_own_fingerprint_short(own.short_fingerprint.clone().into());
            app.set_own_fingerprint_full(own.full_fingerprint.clone().into());
            app.set_own_key_health(own.key_health_message().into());
        }
        None => {
            app.set_own_fingerprint_short("".into());
            app.set_own_fingerprint_full("".into());
            app.set_own_key_health("".into());
        }
    }
    match &s.diagnostics {
        Some(d) => {
            app.set_diag_key_id(d.key_id.clone().into());
            app.set_diag_signature(d.signature_status.clone().into());
            app.set_diag_transport(d.transport_security_details.clone().into());
        }
        None => {
            app.set_diag_key_id("".into());
            app.set_diag_signature("".into());
            app.set_diag_transport("".into());
        }
    }
    let epochs: Vec<String> = s
        .groups
        .ordered()
        .iter()
        .map(|g| format!("{} · epoch {}", g.display_label, g.epoch))
        .collect();
    app.set_diag_epochs(
        if epochs.is_empty() {
            "No groups yet".to_string()
        } else {
            epochs.join("\n")
        }
        .into(),
    );

    // ---- one-shot signals ----------------------------------------------------------
    if let Some(n) = s.notice.take() {
        app.set_notice_kind(match n.level {
            NoticeLevel::Info => NoticeKind::Info,
            NoticeLevel::Success => NoticeKind::Success,
            NoticeLevel::Warning => NoticeKind::Warning,
            NoticeLevel::Error => NoticeKind::Error,
        });
        // Clear first so an identical repeated message re-triggers the toast timer.
        app.set_notice_text("".into());
        app.set_notice_text(n.text.into());
    }
    if std::mem::take(&mut s.open_content) {
        app.set_content_open(true);
    }
}

fn verification_code(state: Option<VerificationState>) -> i32 {
    match state {
        Some(VerificationState::Verified) => 1,
        Some(VerificationState::KeyChanged) => 2,
        _ => 0,
    }
}

fn apply_preview(
    row: &mut MessageRow,
    preview: AttachmentPreview,
    id: MessageId,
    ready_ids: &mut Vec<MessageId>,
) {
    match preview {
        AttachmentPreview::NotRequested => row.preview_state = 0,
        AttachmentPreview::Loading => row.preview_state = 1,
        AttachmentPreview::Failed { reason } => {
            row.preview_state = 3;
            row.preview_error = reason.into();
        }
        AttachmentPreview::Ready { jpeg_bytes, .. } => {
            ready_ids.push(id);
            let cached = IMAGES.with(|c| c.borrow().decoded.get(&id).cloned());
            let decoded = cached.or_else(|| {
                let d = decode_jpeg(&jpeg_bytes)?;
                IMAGES.with(|c| c.borrow_mut().decoded.insert(id, d.clone()));
                Some(d)
            });
            match decoded {
                Some((image, w, h)) => {
                    let scale = (PREVIEW_MAX_EDGE / w.max(h) as f32).min(1.0);
                    row.preview = image;
                    row.preview_width = w as f32 * scale;
                    row.preview_height = h as f32 * scale;
                    row.preview_state = 2;
                }
                None => {
                    row.preview_state = 3;
                    row.preview_error = "the image data was unreadable".into();
                }
            }
        }
    }
}

fn decode_jpeg(bytes: &[u8]) -> Option<(Image, u32, u32)> {
    let img = image::load_from_memory_with_format(bytes, image::ImageFormat::Jpeg).ok()?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    if w == 0 || h == 0 {
        return None;
    }
    let buf = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(rgba.as_raw(), w, h);
    Some((Image::from_rgba8(buf), w, h))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_updates_in_place_appends_and_trims() {
        let model = VecModel::from(vec![1, 2, 3]);
        sync_model(&model, vec![1, 9, 3, 4]);
        assert_eq!(model.iter().collect::<Vec<_>>(), vec![1, 9, 3, 4]);
        sync_model(&model, vec![1]);
        assert_eq!(model.iter().collect::<Vec<_>>(), vec![1]);
        sync_model(&model, vec![]);
        assert_eq!(model.row_count(), 0);
    }

    #[test]
    fn corrupt_jpeg_is_rejected_not_panicking() {
        assert!(decode_jpeg(b"not a jpeg").is_none());
        assert!(decode_jpeg(&[]).is_none());
    }
}

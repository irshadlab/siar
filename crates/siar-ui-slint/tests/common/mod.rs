//! Headless harness: the real UI on Slint's software renderer.
#![allow(dead_code)]

use std::rc::Rc;
use std::sync::{Arc, Mutex};

use siar_domain::{
    AccountId, AttachmentReference, BlobSize, ConversationId, DeliveryState, DeviceId, MediaType,
    MessageContent, MessageId, MessageText,
};
use siar_ui_slint::{ContactSafety, SiarUi, UiHub, UiOptions, UiSettings, VerificationState};
use siar_ui_state::{
    AppCommand, ConversationKind, ConversationSummary, DeviceKind, DeviceSecurityFlag,
    DeviceSecurityView, DeviceTrustState, GroupSummary, NetworkState, PendingInvite, SavedContact,
    SecurityEventKind, TimelineWindow,
};
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};
use slint::{ComponentHandle, PhysicalSize, PlatformError, Rgb8Pixel, SharedPixelBuffer};
use tokio::sync::mpsc;

struct TestPlatform(Rc<MinimalSoftwareWindow>);
impl Platform for TestPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(self.0.clone())
    }
}

pub struct Harness {
    pub ui: SiarUi,
    pub hub: UiHub,
    pub rx: mpsc::Receiver<AppCommand>,
    pub saved_settings: Arc<Mutex<Vec<UiSettings>>>,
    window: Rc<MinimalSoftwareWindow>,
    size: (u32, u32),
}

impl Harness {
    pub fn new(width: u32, height: u32) -> Self {
        Self::with_validator(width, height, |t| {
            if t.starts_with("ticket-") {
                Ok(())
            } else {
                Err("bad ticket".into())
            }
        })
    }

    pub fn with_validator(
        width: u32,
        height: u32,
        validator: impl Fn(&str) -> Result<(), String> + Send + Sync + 'static,
    ) -> Self {
        let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        slint::platform::set_platform(Box::new(TestPlatform(window.clone())))
            .expect("platform already set on this thread");
        window.set_size(PhysicalSize::new(width, height));
        let (tx, rx) = mpsc::channel(64);
        let saved = Arc::new(Mutex::new(Vec::new()));
        let sink = saved.clone();
        let ui = SiarUi::new(
            UiOptions {
                settings: UiSettings::default(),
                validate_ticket: Arc::new(validator),
                on_settings_changed: Arc::new(move |s| sink.lock().unwrap().push(s)),
            },
            tx,
        )
        .expect("window");
        ui.window().show().unwrap();
        let hub = ui.hub();
        Self {
            ui,
            hub,
            rx,
            saved_settings: saved,
            window,
            size: (width, height),
        }
    }

    pub fn app(&self) -> siar_ui_slint::AppState<'_> {
        self.ui.window().global::<siar_ui_slint::AppState>()
    }

    pub fn flush(&self) {
        self.hub.flush_now();
    }

    pub fn commands(&mut self) -> Vec<AppCommand> {
        let mut out = Vec::new();
        while let Ok(c) = self.rx.try_recv() {
            out.push(c);
        }
        out
    }

    /// Renders the current frame to an RGB image, interleaving layout
    /// passes and timer ticks the way a real event loop does.
    pub fn render(&self) -> image::RgbImage {
        let (w, h) = self.size;
        let mut buf = SharedPixelBuffer::<Rgb8Pixel>::new(w, h);
        let draw = |buf: &mut SharedPixelBuffer<Rgb8Pixel>| {
            self.window.request_redraw();
            self.window.draw_if_needed(|r| {
                r.render(buf.make_mut_slice(), w as usize);
            });
        };
        draw(&mut buf);
        for _ in 0..8 {
            std::thread::sleep(std::time::Duration::from_millis(20));
            slint::platform::update_timers_and_animations();
            draw(&mut buf);
        }
        image::RgbImage::from_raw(w, h, buf.as_bytes().to_vec()).unwrap()
    }

    pub fn shot(&self, name: &str) -> image::RgbImage {
        let img = self.render();
        let dir = std::env::var("SIAR_SHOT_DIR").unwrap_or_else(|_| "/tmp/siar-shots".into());
        std::fs::create_dir_all(&dir).unwrap();
        img.save(format!("{dir}/{name}.png")).unwrap();
        img
    }
}

pub fn distinct_colors(img: &image::RgbImage) -> usize {
    let mut set = std::collections::HashSet::new();
    for p in img.pixels() {
        set.insert(p.0);
        if set.len() > 500 {
            break;
        }
    }
    set.len()
}

pub fn text(s: &str) -> MessageContent {
    MessageContent::Text(MessageText::parse(s.to_string()).unwrap())
}

pub fn jpeg(w: u32, h: u32) -> Vec<u8> {
    let img = image::RgbImage::from_fn(w, h, |x, y| {
        image::Rgb([(x * 255 / w) as u8, (y * 255 / h) as u8, 160])
    });
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Jpeg).unwrap();
    out.into_inner()
}

pub struct Sample {
    pub alice: ConversationId,
    pub bob: ConversationId,
    pub team: ConversationId,
    pub alice_contact: DeviceId,
    pub bob_contact: DeviceId,
}

/// A realistic populated state exercising every row type.
pub fn populate(hub: &UiHub) -> Sample {
    let alice = ConversationId::new();
    let bob = ConversationId::new();
    let team = ConversationId::new();
    let device_a = DeviceId::new();
    let device_b = DeviceId::new();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;

    hub.update(|s| {
        s.ready = true;
        s.network.set_overall(NetworkState::Online);
        s.identity.account = Some(AccountId::new());
        s.identity.device = Some(DeviceId::new());
        s.identity.ticket = "ticket-endpoint-AbCdEf0123456789-x25519-ed25519".into();
        s.identity.key_package = "a2V5LXBhY2thZ2UtYnl0ZXM=".into();

        s.ensure_conversation(alice, "Alice Rahman", ConversationKind::Direct);
        s.ensure_conversation(bob, "Bob", ConversationKind::Direct);
        s.ensure_conversation(team, "Group 3f9a21bc", ConversationKind::Group);
        s.sendable.insert(alice);
        s.sendable.insert(bob);

        s.record_incoming(alice, MessageId::new(), 1, text("Hey! Did the relay node come up?"));
        s.record_outgoing(alice, MessageId::new(), text("Yes — it joined over Wi-Fi Direct a minute ago."));
        s.record_incoming(alice, MessageId::new(), 2, text("Great. Sending the floor plan now, it is quite a long message so that it has to wrap onto more than one line inside the bubble."));
        s.record_incoming(alice, MessageId::new(), 3, text("Also the route table."));
        let img_msg = MessageId::new();
        s.record_outgoing(alice, img_msg, MessageContent::Attachment(AttachmentReference {
            blob_hash: [1; 32],
            encrypted_size: BlobSize::parse(183_000).unwrap(),
            media_type: MediaType::ImageJpeg,
            attachment_key: [2; 32],
            thumbnail: None,
        }));
        s.attachment_previews.set_ready(img_msg, jpeg(240, 150), 240, 150);
        let file_msg = MessageId::new();
        s.record_incoming(alice, file_msg, 4, MessageContent::Attachment(AttachmentReference {
            blob_hash: [3; 32],
            encrypted_size: BlobSize::parse(2_400_000).unwrap(),
            media_type: MediaType::AudioOpus,
            attachment_key: [4; 32],
            thumbnail: None,
        }));
        let last = MessageId::new();
        s.record_outgoing(alice, last, text("Received, thanks!"));
        s.timeline_mut(alice).update_delivery_state(last, DeliveryState::CarriedByPeers { copies: 2 });
        s.record_incoming(bob, MessageId::new(), 1, text("lunch?"));
        s.record_incoming(bob, MessageId::new(), 2, text("also: are we still on for Thursday"));
        s.conversations.set_preview(team, "No messages yet".into());

        s.contacts.upsert(SavedContact {
            device_id: device_a,
            account_id: AccountId::new(),
            display_name: "Alice Rahman".into(),
            ticket_text: "ticket-a".into(),
            key_package_b64: None,
        });
        s.contacts.upsert(SavedContact {
            device_id: device_b,
            account_id: AccountId::new(),
            display_name: "Bob".into(),
            ticket_text: "ticket-b".into(),
            key_package_b64: None,
        });
        let number = "12345 67890 12345 67890 12345 67890 12345 67890 12345 67890 12345 67890".to_string();
        s.contact_safety.insert(device_a, ContactSafety { number: number.clone(), state: VerificationState::Verified });
        s.contact_safety.insert(device_b, ContactSafety { number, state: VerificationState::KeyChanged });
        s.own_identity = Some(siar_ui_state::OwnIdentityView {
            account: AccountId::new(),
            short_fingerprint: "a1b2 c3d4 e5f6 0718".into(),
            full_fingerprint: "a1b2 c3d4 e5f6 0718 293a 4b5c 6d7e 8f90 a1b2 c3d4 e5f6 0718 293a 4b5c 6d7e 8f90".into(),
            created_at_millis: now,
            key_health: siar_ui_state::IdentityKeyHealth::Healthy,
        });
        s.diagnostics = Some(siar_ui_state::SecurityDiagnosticDetail {
            key_id: "a1b2 c3d4 e5f6 0718".into(),
            epoch_id: String::new(),
            signature_status: "Messages are signed with this device’s Ed25519 key.".into(),
            transport_security_details: "QUIC with TLS 1.3 (iroh) · endpoint 7fa09c1e".into(),
        });
        s.groups.upsert(GroupSummary {
            conversation_id: team,
            display_label: "Group 3f9a21bc".into(),
            member_count: 3,
            is_admin: true,
            epoch: 2,
        });
        s.pending_invites.add(PendingInvite {
            conversation_id: ConversationId::new(),
            from_device: DeviceId::new(),
            welcome_bytes: vec![1, 2, 3],
            state_input: String::new(),
            carries_state: false,
        });
        s.security_events.push(SecurityEventKind::DeviceLinked, now, None, None, vec![]);
        s.security_events.push(SecurityEventKind::IdentityChanged, now, None, None, vec![]);
        s.security_events.push(SecurityEventKind::KeyRotation, now, None, None, vec![]);
        s.devices.set_devices(vec![
            DeviceSecurityView {
                id: DeviceId::new(), display_name: "Irshad’s desktop".into(),
                kind: DeviceKind::Desktop, status: DeviceTrustState::Trusted,
                added_at_millis: now, last_active_millis: Some(now), current_device: true,
                security_flags: vec![],
            },
            DeviceSecurityView {
                id: DeviceId::new(), display_name: "Pixel 8".into(),
                kind: DeviceKind::AndroidPhone, status: DeviceTrustState::Trusted,
                added_at_millis: now, last_active_millis: Some(now - 7_200_000), current_device: false,
                security_flags: vec![],
            },
            DeviceSecurityView {
                id: DeviceId::new(), display_name: "".into(),
                kind: DeviceKind::Unknown, status: DeviceTrustState::Pending,
                added_at_millis: now, last_active_millis: None, current_device: false,
                security_flags: vec![DeviceSecurityFlag::PendingAuthorization],
            },
        ]);
        s.select(alice);
        s.open_content = false;
    });
    let _ = TimelineWindow {
        message_id: MessageId::new(),
        sequence: 0,
        content: text("x"),
        delivery_state: DeliveryState::Local,
        from_me: false,
    };
    let _ = ConversationSummary {
        id: alice,
        display_name: String::new(),
        last_message_preview: None,
        unread_count: 0,
        kind: ConversationKind::Direct,
    };
    Sample {
        alice,
        bob,
        team,
        alice_contact: device_a,
        bob_contact: device_b,
    }
}

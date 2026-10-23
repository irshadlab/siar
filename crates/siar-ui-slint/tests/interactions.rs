mod common;
use common::*;
use siar_domain::{AccountId, DeviceId};
use siar_ui_slint::{Page, Theme, ThemeChoice, ThemeMode};
use siar_ui_state::AppCommand;
use slint::ComponentHandle;

#[test]
fn sending_dispatches_a_direct_message_and_clears_the_draft() {
    let mut h = Harness::new(1180, 760);
    let s = populate(&h.hub);
    h.flush();
    h.app().set_composer_draft("hello there".into());
    h.app().invoke_send_message("hello there".into());
    let cmds = h.commands();
    assert_eq!(cmds.len(), 1);
    match &cmds[0] {
        AppCommand::SendMessage { conversation, text } => {
            assert_eq!(*conversation, s.alice);
            assert_eq!(text.as_str(), "hello there");
        }
        other => panic!("unexpected {other:?}"),
    }
    assert_eq!(h.app().get_composer_draft(), "");
}

#[test]
fn whitespace_only_drafts_send_nothing() {
    let mut h = Harness::new(1180, 760);
    populate(&h.hub);
    h.app().invoke_send_message("   \n ".into());
    assert!(h.commands().is_empty());
}

#[test]
fn group_conversations_use_the_group_command() {
    let mut h = Harness::new(1180, 760);
    let s = populate(&h.hub);
    h.app().invoke_open_conversation(s.team.to_string().into());
    h.app().invoke_send_message("to the team".into());
    let cmds = h.commands();
    assert!(matches!(cmds[0], AppCommand::OpenConversation(c) if c == s.team));
    assert!(
        matches!(&cmds[1], AppCommand::SendGroupMessage { conversation, .. } if *conversation == s.team)
    );
}

#[test]
fn unpaired_direct_chat_refuses_to_send_and_says_why() {
    let mut h = Harness::new(1180, 760);
    let s = populate(&h.hub);
    h.hub.update(|st| {
        st.sendable.remove(&s.alice);
    });
    h.flush();
    assert!(!h.app().get_can_send());
    h.app().invoke_send_message("anyone there?".into());
    assert!(h.commands().is_empty());
    h.flush();
    assert!(h.app().get_notice_text().contains("Pair with this person"));
}

#[test]
fn opening_a_conversation_selects_it_clears_unread_and_switches_to_chats() {
    let mut h = Harness::new(1180, 760);
    let s = populate(&h.hub);
    h.flush();
    assert_eq!(h.app().get_total_unread(), 2); // Bob's two
    h.app().set_page(Page::Groups);
    h.app().invoke_open_conversation(s.bob.to_string().into());
    h.flush();
    assert_eq!(h.app().get_selected_id(), s.bob.to_string().as_str());
    assert_eq!(h.app().get_total_unread(), 0);
    assert_eq!(h.app().get_page(), Page::Chats);
    assert!(h.app().get_content_open());
    assert!(matches!(h.commands()[0], AppCommand::OpenConversation(c) if c == s.bob));
}

#[test]
fn filtering_conversations_matches_titles_and_previews() {
    let h = Harness::new(1180, 760);
    populate(&h.hub);
    h.flush();
    assert_eq!(h.app().get_conversations().row_count(), 3);
    h.app().invoke_filter_conversations("thursday".into());
    h.flush();
    assert_eq!(h.app().get_conversations().row_count(), 1);
    h.app().invoke_filter_conversations("zzz".into());
    h.flush();
    assert_eq!(h.app().get_conversations().row_count(), 0);
    h.app().invoke_filter_conversations("".into());
    h.flush();
    assert_eq!(h.app().get_conversations().row_count(), 3);
}

use slint::Model;

#[test]
fn bad_ticket_is_rejected_inline_with_no_command() {
    let mut h = Harness::new(1180, 760);
    populate(&h.hub);
    assert!(!h.app().invoke_pair_with_ticket("not-a-ticket".into()));
    assert!(!h.app().invoke_pair_with_ticket("   ".into()));
    assert!(h.commands().is_empty());
    h.flush();
    assert!(h.app().get_notice_text().contains("Paste their ticket")); // latest: empty paste
}

#[test]
fn good_ticket_pairs() {
    let mut h = Harness::new(1180, 760);
    assert!(h.app().invoke_pair_with_ticket("  ticket-abc \n".into()));
    assert!(
        matches!(&h.commands()[0], AppCommand::PairWithTicket { ticket_text } if ticket_text == "ticket-abc")
    );
}

#[test]
fn saving_a_contact_validates_every_field() {
    let mut h = Harness::new(1180, 760);
    let (acct, dev) = (AccountId::new().to_string(), DeviceId::new().to_string());
    assert!(!h.app().invoke_save_contact(
        "".into(),
        acct.clone().into(),
        dev.clone().into(),
        "ticket-1".into()
    ));
    assert!(!h.app().invoke_save_contact(
        "Ann".into(),
        "bogus".into(),
        dev.clone().into(),
        "ticket-1".into()
    ));
    assert!(!h.app().invoke_save_contact(
        "Ann".into(),
        acct.clone().into(),
        "bogus".into(),
        "ticket-1".into()
    ));
    assert!(!h.app().invoke_save_contact(
        "Ann".into(),
        acct.clone().into(),
        dev.clone().into(),
        "nope".into()
    ));
    assert!(h.commands().is_empty());
    assert!(h
        .app()
        .invoke_save_contact("Ann".into(), acct.into(), dev.into(), "ticket-1".into()));
    assert!(
        matches!(&h.commands()[0], AppCommand::SaveContact { display_name, .. } if display_name == "Ann")
    );
}

#[test]
fn only_admins_can_add_members_and_key_package_must_be_base64() {
    let mut h = Harness::new(1180, 760);
    let s = populate(&h.hub);
    h.app().invoke_select_group(s.team.to_string().into());
    let (a, d) = (AccountId::new().to_string(), DeviceId::new().to_string());
    // bad base64
    assert!(!h.app().invoke_add_member(
        a.clone().into(),
        d.clone().into(),
        "ticket-x".into(),
        "***".into()
    ));
    // good, with pasted line breaks
    assert!(h.app().invoke_add_member(
        a.clone().into(),
        d.clone().into(),
        "ticket-x".into(),
        "a2V5\nLXBhY2thZ2U=".into()
    ));
    match &h.commands()[0] {
        AppCommand::AddGroupMember {
            conversation,
            input,
            ..
        } => {
            assert_eq!(*conversation, s.team);
            assert_eq!(input.key_package_bytes, b"key-package");
        }
        other => panic!("unexpected {other:?}"),
    }
    // demote ourselves: now refused
    h.hub.update(|st| {
        st.groups.upsert(siar_ui_state::GroupSummary {
            conversation_id: s.team,
            display_label: "G".into(),
            member_count: 3,
            is_admin: false,
            epoch: 2,
        });
    });
    assert!(!h
        .app()
        .invoke_add_member(a.into(), d.into(), "ticket-x".into(), "a2V5".into()));
    assert!(h.commands().is_empty());
}

#[test]
fn accepting_an_invite_needs_the_group_state() {
    let mut h = Harness::new(1180, 760);
    populate(&h.hub);
    h.flush();
    let invite = h.app().get_invites().row_data(0).unwrap();
    assert!(!h.app().invoke_accept_invite(invite.id.clone(), "  ".into()));
    assert!(h.commands().is_empty());
    assert!(h
        .app()
        .invoke_accept_invite(invite.id.clone(), "state-blob".into()));
    assert!(matches!(
        h.commands()[0],
        AppCommand::AcceptGroupInvite { .. }
    ));
    h.app().invoke_decline_invite(invite.id);
    assert!(matches!(
        h.commands()[0],
        AppCommand::DeclineGroupInvite { .. }
    ));
}

#[test]
fn resolving_the_critical_event_dismisses_the_banner() {
    let h = Harness::new(1180, 760);
    populate(&h.hub);
    h.flush();
    assert_eq!(h.app().get_critical_count(), 1);
    assert_eq!(h.app().get_unresolved_events(), 3);
    h.app().invoke_resolve_event(h.app().get_critical_id());
    h.flush();
    assert_eq!(h.app().get_critical_count(), 0);
    assert_eq!(h.app().get_unresolved_events(), 2);
    // filter tabs
    h.app().invoke_set_event_filter(1);
    h.flush();
    assert_eq!(h.app().get_events().row_count(), 2);
    h.app().invoke_set_event_filter(2);
    h.flush();
    assert_eq!(h.app().get_events().row_count(), 1); // resolved criticals still listed
}

#[test]
fn image_previews_are_requested_exactly_once() {
    let mut h = Harness::new(1180, 760);
    let s = populate(&h.hub);
    // a not-yet-loaded image from Bob
    let msg = siar_domain::MessageId::new();
    h.hub.update(|st| {
        st.record_incoming(
            s.alice,
            msg,
            9,
            siar_domain::MessageContent::Attachment(siar_domain::AttachmentReference {
                blob_hash: [9; 32],
                encrypted_size: siar_domain::BlobSize::parse(5000).unwrap(),
                media_type: siar_domain::MediaType::ImagePng,
                attachment_key: [9; 32],
                thumbnail: None,
            }),
        );
    });
    h.flush();
    h.app().invoke_load_attachment(msg.to_string().into());
    h.app().invoke_load_attachment(msg.to_string().into());
    let cmds = h.commands();
    assert_eq!(cmds.len(), 1, "second request must be a no-op");
    assert!(
        matches!(&cmds[0], AppCommand::LoadAttachmentPreview { message_id, .. } if *message_id == msg)
    );
    // unknown ids are ignored
    h.app().invoke_load_attachment("garbage".into());
    assert!(h.commands().is_empty());
}

#[test]
fn changing_appearance_notifies_the_persistence_hook() {
    let h = Harness::new(1180, 760);
    h.ui.window().global::<Theme>().set_mode(ThemeMode::Dark);
    h.ui.window().global::<Theme>().set_ui_scale(1.15);
    h.app().invoke_settings_changed();
    let saved = h.saved_settings.lock().unwrap();
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].theme, ThemeChoice::Dark);
    assert!((saved[0].scale - 1.15).abs() < 1e-6);
}

#[test]
fn startup_failure_is_shown_not_swallowed() {
    let h = Harness::new(1180, 760);
    h.hub
        .update(|s| s.startup_error = Some("The data folder isn’t writable.".into()));
    h.flush();
    assert_eq!(
        h.app().get_startup_error(),
        "The data folder isn’t writable."
    );
    let img = h.shot("startup-error");
    assert!(distinct_colors(&img) > 5);
}

#[test]
fn a_full_command_queue_tells_the_user_instead_of_dropping_silently() {
    // capacity-1 channel
    let window = slint::platform::software_renderer::MinimalSoftwareWindow::new(
        slint::platform::software_renderer::RepaintBufferType::NewBuffer,
    );
    struct P(std::rc::Rc<slint::platform::software_renderer::MinimalSoftwareWindow>);
    impl slint::platform::Platform for P {
        fn create_window_adapter(
            &self,
        ) -> Result<std::rc::Rc<dyn slint::platform::WindowAdapter>, slint::PlatformError> {
            Ok(self.0.clone())
        }
    }
    slint::platform::set_platform(Box::new(P(window))).unwrap();
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    let ui = siar_ui_slint::SiarUi::new(siar_ui_slint::UiOptions::permissive(), tx).unwrap();
    let hub = ui.hub();
    assert!(hub.dispatch(AppCommand::DeclineGroupInvite {
        conversation: siar_domain::ConversationId::new()
    }));
    assert!(!hub.dispatch(AppCommand::DeclineGroupInvite {
        conversation: siar_domain::ConversationId::new()
    }));
    hub.flush_now();
    assert!(ui
        .window()
        .global::<siar_ui_slint::AppState>()
        .get_notice_text()
        .contains("busy"));
}

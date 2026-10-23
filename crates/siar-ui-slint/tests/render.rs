mod common;
use common::*;
use siar_ui_slint::{Density, Page, Theme, ThemeMode};
use slint::ComponentHandle;

fn assert_drawn(img: &image::RgbImage) {
    assert!(distinct_colors(img) > 12, "frame looks blank");
}

#[test]
fn chat_light_and_dark() {
    let h = Harness::new(1180, 760);
    populate(&h.hub);
    h.ui.window().global::<Theme>().set_mode(ThemeMode::Light);
    h.flush();
    assert_drawn(&h.shot("chat-light"));
    h.ui.window().global::<Theme>().set_mode(ThemeMode::Dark);
    assert_drawn(&h.shot("chat-dark"));
    h.ui.window()
        .global::<Theme>()
        .set_mode(ThemeMode::HighContrast);
    assert_drawn(&h.shot("chat-high-contrast"));
}

#[test]
fn every_page_renders() {
    let h = Harness::new(1180, 760);
    populate(&h.hub);
    h.ui.window().global::<Theme>().set_mode(ThemeMode::Light);
    h.flush();
    for (page, name) in [
        (Page::Contacts, "contacts"),
        (Page::Groups, "groups"),
        (Page::Security, "security"),
        (Page::Connect, "connect"),
        (Page::Settings, "settings"),
    ] {
        h.app().set_page(page);
        assert_drawn(&h.shot(name));
    }
    h.app().set_page(Page::Security);
    h.app()
        .set_security_tab(siar_ui_slint::SecurityTab::Devices);
    assert_drawn(&h.shot("security-devices"));
}

#[test]
fn verification_and_identity_screens_render() {
    let h = Harness::new(1180, 900);
    let s = populate(&h.hub);
    h.ui.window().global::<Theme>().set_mode(ThemeMode::Light);
    h.flush();
    h.app().set_page(Page::Contacts);
    h.app()
        .invoke_select_contact(s.bob_contact.to_string().into());
    h.flush();
    assert_drawn(&h.shot("contact-key-changed"));
    h.app()
        .invoke_select_contact(s.alice_contact.to_string().into());
    h.flush();
    assert_drawn(&h.shot("contact-verified"));
    h.app().set_page(Page::Settings);
    assert_drawn(&h.shot("settings-identity"));
}

#[test]
fn compact_window_shows_one_pane_at_a_time() {
    let h = Harness::new(560, 700);
    populate(&h.hub);
    h.ui.window().global::<Theme>().set_mode(ThemeMode::Light);
    h.flush();
    h.app().set_content_open(false);
    assert_drawn(&h.shot("compact-list"));
    h.app().set_content_open(true);
    assert_drawn(&h.shot("compact-chat"));
}

#[test]
fn compact_density_and_large_scale() {
    let h = Harness::new(1180, 760);
    populate(&h.hub);
    h.ui.window().global::<Theme>().set_mode(ThemeMode::Light);
    h.ui.window()
        .global::<Theme>()
        .set_density(Density::Compact);
    h.flush();
    assert_drawn(&h.shot("chat-compact-density"));
    h.ui.window()
        .global::<Theme>()
        .set_density(Density::Comfortable);
    h.ui.window().global::<Theme>().set_ui_scale(1.3);
    assert_drawn(&h.shot("chat-scale-130"));
}

#[test]
fn empty_and_offline_states_render() {
    let h = Harness::new(1180, 760);
    h.ui.window().global::<Theme>().set_mode(ThemeMode::Light);
    h.hub
        .update(|s| s.network.set_overall(siar_ui_state::NetworkState::Offline));
    h.flush();
    assert_drawn(&h.shot("empty-offline"));
}

#[test]
fn load_earlier_row_renders_above_the_oldest_message() {
    let h = Harness::new(1180, 760);
    let s = populate(&h.hub);
    h.ui.window().global::<Theme>().set_mode(ThemeMode::Light);
    // A short chat (Bob, two messages) so the row at the top is on screen.
    h.hub.update(|st| {
        st.select(s.bob);
        st.history.insert(
            s.bob,
            siar_ui_slint::HistoryPaging {
                loaded: true,
                next_before: Some(3),
                exhausted: false,
            },
        );
    });
    h.flush();
    assert_drawn(&h.shot("chat-load-earlier"));
}

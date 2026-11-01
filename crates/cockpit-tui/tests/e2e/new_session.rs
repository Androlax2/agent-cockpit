//! M1 guide, section 2: the `N` form starts sessions in place and in a new clone, Enter
//! shows a session's agent in the viewer, and one started from the CLI appears by itself.

use crate::harness::{Cockpit, FIRST_MESSAGE, PROJECT, session_ids};

/// Opens the form and waits for the daemon's agents in it.
fn open_form(cockpit: &Cockpit) {
    cockpit.press(&["N"]);
    cockpit.wait_for("Agent: claude");
}

/// The one session the list shows once it shows one.
fn first_card(cockpit: &Cockpit) -> String {
    let screen = cockpit.wait_until("a card in the list", |screen| {
        !session_ids(screen).is_empty()
    });
    session_ids(&screen).remove(0)
}

#[test]
fn the_form_starts_a_session_in_place_with_its_first_message() {
    let cockpit = Cockpit::start();
    open_form(&cockpit);
    cockpit.press(&["Tab", "Tab"]);
    cockpit.type_text(FIRST_MESSAGE);
    cockpit.press(&["Enter"]);
    let session = first_card(&cockpit);
    let view = cockpit.wait_for_session(&session, "runs", |view| {
        view.state == cockpit_protocol::SessionStateView::Running
    });
    assert_eq!(view.working_dir, cockpit.project_root().to_str().unwrap());
    cockpit.wait_for(&format!("{session}  claude  running"));
    cockpit.press(&["Enter"]);
    cockpit.wait_for(&format!("reply: {FIRST_MESSAGE}"));
}

#[test]
fn enter_shows_the_selected_agent_in_the_viewer() {
    let cockpit = Cockpit::start();
    let session = cockpit.session_from_cli();
    cockpit.wait_for(&session);
    cockpit.press(&["Enter"]);
    cockpit.wait_for(&format!("reply: {FIRST_MESSAGE}"));
}

#[test]
fn the_form_starts_a_session_in_a_new_clone_on_a_branch() {
    let cockpit = Cockpit::start();
    open_form(&cockpit);
    cockpit.press(&["Tab", "Tab", "Tab", "Right", "Tab"]);
    cockpit.wait_for("Branch:");
    cockpit.type_text("accept-clone");
    cockpit.press(&["Enter"]);
    let session = first_card(&cockpit);
    let view = cockpit.wait_for_session(&session, "runs", |view| {
        view.state == cockpit_protocol::SessionStateView::Running
    });
    assert!(
        view.working_dir
            .ends_with(&format!(".parallel/{PROJECT}/accept-clone")),
        "{view:?}"
    );
    assert!(view.holds_checkout, "{view:?}");
}

#[test]
fn a_session_started_from_the_cli_appears_by_itself() {
    let cockpit = Cockpit::start();
    let session = cockpit.session_from_cli();
    let screen = cockpit.wait_for(&session);
    assert_eq!(session_ids(&screen), vec![session]);
}

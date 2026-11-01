//! M1 guide, section 2: the `N` form starts sessions in place and in a new clone, Enter
//! shows a session's agent in the viewer, and one started from the CLI appears by itself.

use cockpit_protocol::SessionStateView;

use crate::harness::{Cockpit, FIRST_MESSAGE, PROJECT, session_ids, viewer_rows};

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
        view.state == SessionStateView::Running
    });
    assert_eq!(view.working_dir, cockpit.project_root().to_str().unwrap());
    cockpit.wait_for(&format!("{session}  claude  running"));
    cockpit.press(&["Enter"]);
    cockpit.wait_for(&format!("reply: {FIRST_MESSAGE}"));
}

#[test]
fn enter_shows_the_selected_agent_in_the_viewer() {
    let cockpit = Cockpit::start();
    cockpit.shown_session();
}

/// #338: the window shrinks from the size it started with to the viewer's as the viewer
/// attaches, and tmux rewraps a line wider than the viewer into two rows. The agent printed
/// it before anyone viewed the session; the viewer must still show its screen row for row.
#[test]
fn the_viewer_shows_every_line_after_one_wider_than_itself() {
    let cockpit = Cockpit::start();
    let session = cockpit.session_from_cli();
    cockpit.wait_for(&session);
    cockpit.agent_does(&session, "!printf 'w%.0s' $(seq 200); echo ' end'");
    let wider_than_the_viewer = "w".repeat(121);
    cockpit.wait_for_agent_screen(&session, "the wide line, before any viewer", |screen| {
        screen
            .iter()
            .any(|row| row.contains(&wider_than_the_viewer))
            && screen.last().is_some_and(|row| row.starts_with("fake>"))
    });
    cockpit.press(&["Enter"]);
    cockpit.wait_until("the agent's screen, row for row, in the viewer", |screen| {
        viewer_rows(screen) == cockpit.agent_screen(&session)
    });
}

/// The viewer sizes the agent's window before it attaches (#338); the window still follows
/// the viewer when the owner resizes the terminal.
#[test]
fn the_shown_agent_follows_the_terminal_when_it_is_resized() {
    let cockpit = Cockpit::start();
    let session = cockpit.shown_session();
    let shown_size = cockpit.agent_window_size(&session);

    cockpit.resize_terminal(140, 40);

    cockpit.wait_until("the agent's window follows the viewer", |_| {
        let size = cockpit.agent_window_size(&session);
        size != shown_size && size == cockpit.viewer_size()
    });
}

#[test]
fn the_form_starts_a_session_in_a_new_clone_on_a_branch() {
    let cockpit = Cockpit::start();
    open_form(&cockpit);
    // One key at a time, each once the form shows the one before took (#402).
    for (key, field) in [
        ("Tab", "> Agent: claude"),
        ("Tab", "> First message:"),
        ("Tab", "> Where: in place"),
        ("Right", "> Where: new clone"),
        ("Tab", "> Branch:"),
    ] {
        cockpit.press(&[key]);
        cockpit.wait_for(field);
    }
    cockpit.type_text("accept-clone");
    cockpit.press(&["Enter"]);
    let session = first_card(&cockpit);
    let view = cockpit.wait_for_session(&session, "runs", |view| {
        view.state == SessionStateView::Running
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

//! Typing into a pane on a real private tmux server (#385): what the pane's program reads is
//! what was sent, character for character.

mod common;

use std::fs;

use cockpit_tmux::{Error, Keystroke, PaneId};
use common::{TestServer, eventually};

/// A pane whose program writes every line it reads, as bytes, to `typed`.
fn reading_pane(fixture: &TestServer) -> (PaneId, std::path::PathBuf) {
    let typed = fixture.home.path().join("typed");
    let (_, pane) = fixture.start_shell("agent-1", &format!("cat > '{}'", typed.display()));
    (pane, typed)
}

fn read(path: &std::path::Path) -> String {
    fs::read_to_string(path).unwrap_or_default()
}

#[test]
fn text_reaches_the_pane_literally_and_enter_ends_the_line() {
    let fixture = TestServer::new();
    let (pane, typed) = reading_pane(&fixture);
    let lines = [
        "fix the login; then test;",
        "-n Enter C-x",
        r"ends with a backslash \;",
        ";",
        "été — ü",
    ];

    for line in lines {
        fixture
            .server
            .send_keys(&pane, &[Keystroke::Text(line), Keystroke::Enter])
            .expect("typed");
    }

    let expected: String = lines.iter().map(|line| format!("{line}\n")).collect();
    eventually("every line was read", || read(&typed) == expected);
}

#[test]
fn control_and_escape_send_their_bytes() {
    let fixture = TestServer::new();
    let (pane, typed) = reading_pane(&fixture);

    fixture
        .server
        .send_keys(
            &pane,
            &[
                Keystroke::Text("a"),
                Keystroke::Control('x'),
                Keystroke::Escape,
                Keystroke::Enter,
            ],
        )
        .expect("typed");

    eventually("the line was read", || read(&typed) == "a\u{18}\u{1b}\n");
}

#[test]
fn a_control_chord_of_no_lower_case_letter_is_refused_and_nothing_is_typed() {
    let fixture = TestServer::new();
    let (pane, typed) = reading_pane(&fixture);

    let refused = fixture
        .server
        .send_keys(&pane, &[Keystroke::Text("before"), Keystroke::Control('X')]);

    assert!(
        matches!(refused, Err(Error::InvalidArgument { .. })),
        "{refused:?}"
    );
    fixture
        .server
        .send_keys(&pane, &[Keystroke::Text("after"), Keystroke::Enter])
        .expect("typed");
    eventually("the next line was read", || read(&typed) == "after\n");
}

#[test]
fn typing_into_a_pane_that_is_gone_fails() {
    let fixture = TestServer::new();
    let (pane, _) = reading_pane(&fixture);
    fixture.server.close_pane(&pane).expect("closed");

    let refused = fixture
        .server
        .send_keys(&pane, &[Keystroke::Text("lost"), Keystroke::Enter]);

    assert!(
        matches!(refused, Err(Error::CommandFailed { .. })),
        "{refused:?}"
    );
}

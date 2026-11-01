//! M1 guide, section 3: restart, sleep, wake, exit and resume keep the conversation, and the
//! TUI rides out a daemon killed with `kill -9` while every session keeps running.

use cockpit_protocol::{LaunchKindView, SessionStateView, SessionView};

use crate::harness::{Cockpit, FIRST_MESSAGE};

/// A session started from the CLI, selected and shown in the viewer, its agent at its
/// prompt; returns its id.
fn shown_session(cockpit: &Cockpit) -> String {
    let session = cockpit.session_from_cli();
    cockpit.wait_for(&session);
    cockpit.press(&["Enter"]);
    cockpit.wait_for(&format!("reply: {FIRST_MESSAGE}"));
    session
}

fn launched_by(kind: LaunchKindView) -> impl Fn(&SessionView) -> bool {
    move |view| {
        view.state == SessionStateView::Running
            && view
                .launch
                .as_ref()
                .is_some_and(|launch| launch.kind == kind)
    }
}

/// The agent remembers what came before: it names the conversation's first message.
fn assert_same_conversation(cockpit: &Cockpit) {
    cockpit.tell_agent("What did you reply before?");
    cockpit.wait_for(&format!(
        "reply: What did you reply before? (first message: {FIRST_MESSAGE})"
    ));
}

#[test]
fn restart_after_y_keeps_the_conversation() {
    let cockpit = Cockpit::start();
    let session = shown_session(&cockpit);
    cockpit.click(crate::harness::ManagerPane::List);
    cockpit.press(&["r"]);
    cockpit.wait_for(&format!("restart {session}? y to confirm"));
    cockpit.press(&["y"]);
    cockpit.wait_for_session(&session, "restarted", launched_by(LaunchKindView::Restart));
    cockpit.press(&["Enter"]);
    cockpit.wait_for("resumed conversation 1 messages");
    assert_same_conversation(&cockpit);
}

#[test]
fn sleep_marks_the_card_asleep_and_closes_its_terminal() {
    let cockpit = Cockpit::start();
    let session = shown_session(&cockpit);
    cockpit.cockpit(&["session", "sleep", &session]);
    let screen = cockpit.wait_for(&format!("{session}  claude  asleep"));
    let screen = if screen.contains("fake>") {
        cockpit.wait_for_gone("fake>")
    } else {
        screen
    };
    assert!(!screen.contains(FIRST_MESSAGE), "{screen}");
}

#[test]
fn enter_wakes_an_asleep_session_in_its_conversation() {
    let cockpit = Cockpit::start();
    let session = shown_session(&cockpit);
    cockpit.cockpit(&["session", "sleep", &session]);
    cockpit.wait_for(&format!("{session}  claude  asleep"));
    cockpit.click(crate::harness::ManagerPane::List);
    cockpit.press(&["Enter"]);
    cockpit.wait_for_session(&session, "woken", launched_by(LaunchKindView::Wake));
    cockpit.wait_for("resumed conversation 1 messages");
    assert_same_conversation(&cockpit);
}

#[test]
fn an_ended_session_leaves_the_list_and_resume_brings_it_back() {
    let cockpit = Cockpit::start();
    let session = shown_session(&cockpit);
    cockpit.cockpit(&["session", "end", &session]);
    cockpit.wait_for_gone(&session);
    cockpit.cockpit(&["session", "resume", &session]);
    cockpit.wait_for(&format!("{session}  claude  running"));
    cockpit.click(crate::harness::ManagerPane::List);
    cockpit.press(&["Enter"]);
    cockpit.wait_for("resumed conversation 1 messages");
    assert_same_conversation(&cockpit);
}

#[test]
fn the_tui_says_the_daemon_is_gone_and_reconnects_when_it_returns() {
    let mut cockpit = Cockpit::start();
    let session = shown_session(&cockpit);
    cockpit.kill_daemon();
    cockpit.wait_for("change feed:");
    cockpit.start_daemon();
    cockpit.wait_for_gone("change feed:");
    cockpit.wait_for(&format!("{session}  claude  running"));
}

#[test]
fn sessions_keep_their_agent_across_a_daemon_killed_with_kill_9() {
    let mut cockpit = Cockpit::start();
    let session = shown_session(&cockpit);
    let before = cockpit.wait_for_session(&session, "runs", launched_by(LaunchKindView::Start));
    cockpit.kill_daemon();
    cockpit.start_daemon();
    let after = cockpit.wait_for_session(&session, "runs again", |view| {
        view.state == SessionStateView::Running
    });
    assert_eq!(after.launch, before.launch, "the agent was not relaunched");
    assert_eq!(cockpit.launches(&session).len(), 1);
    assert_same_conversation(&cockpit);
}

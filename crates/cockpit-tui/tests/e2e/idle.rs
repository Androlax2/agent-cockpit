//! M1 idle guide (`docs/acceptance/m1-idle.md`): a session whose agent waits at its prompt
//! for the whole idle window falls asleep by itself, and Enter wakes it in its conversation.
//! The tests' daemon shortens the window to seconds; the product's 30-minute floor stays.

use cockpit_protocol::{LaunchKindView, SessionStateView};

use crate::harness::{Cockpit, DaemonKind, FIRST_MESSAGE};

const SHORT_IDLE: DaemonKind = DaemonKind::ShortIdle {
    idle_ms: 2_000,
    tick_ms: 200,
};

#[test]
fn an_idle_session_falls_asleep_by_itself() {
    let cockpit = Cockpit::start_with(SHORT_IDLE);
    let session = cockpit.session_from_cli();
    cockpit.wait_for(&format!("{session}  claude  running"));
    cockpit.wait_for_session(&session, "asleep", |view| {
        view.state == SessionStateView::Asleep
    });
    cockpit.wait_for(&format!("{session}  claude  asleep"));
}

#[test]
fn enter_wakes_a_session_that_fell_asleep_in_its_conversation() {
    let cockpit = Cockpit::start_with(SHORT_IDLE);
    let session = cockpit.session_from_cli();
    cockpit.wait_for(&format!("{session}  claude  asleep"));
    cockpit.press(&["Enter"]);
    cockpit.wait_for_session(&session, "woken", |view| {
        view.launch
            .as_ref()
            .is_some_and(|launch| launch.kind == LaunchKindView::Wake)
    });
    cockpit.wait_for("resumed conversation 1 messages");
    cockpit.tell_agent("What did you reply before?");
    cockpit.wait_for(&format!(
        "reply: What did you reply before? (first message: {FIRST_MESSAGE})"
    ));
}

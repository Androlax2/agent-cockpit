//! M1 idle guide (`docs/acceptance/m1-idle.md`): a session whose agent waits at its prompt
//! for the whole idle window falls asleep by itself, and Enter wakes it in its conversation.
//! The tests' daemon shortens the window to seconds; the product's 30-minute floor stays.
//! Report times are whole seconds, so a 3-second window sleeps 2 to 3 seconds after the
//! agent's report: the tests wait on the sleep, never on the moments before it.

use cockpit_protocol::{LaunchKindView, SessionStateView};

use crate::harness::{Cockpit, DaemonKind};

const SHORT_IDLE: DaemonKind = DaemonKind::ShortIdle {
    idle_seconds: 3,
    tick_ms: 200,
};

#[test]
fn an_idle_session_falls_asleep_by_itself() {
    let cockpit = Cockpit::start_with(SHORT_IDLE);
    let session = cockpit.session_from_cli();
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
    // The fake agent counts the messages of the conversation it resumed by its id. Telling
    // it more would race the next idle window, which starts with its report.
    cockpit.wait_for("resumed conversation 1 messages");
}

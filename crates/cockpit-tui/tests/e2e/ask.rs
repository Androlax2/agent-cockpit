//! M1 guide, section 4: an agent's `ask run` and `ask guide` reach the owner in the TUI's
//! "Waiting on you" pane and the answers reach the agent.

use crate::harness::{Cockpit, FIRST_MESSAGE, ManagerPane};

/// The pane's grace hint, as far as the list column shows it on one line.
const GRACE_HINT: &str = "Read it first:";
const RUN_KEYS: &str = "y approve   x decline   m more";
/// The command of the guide's section 4, with a token the run's output must not show.
const HOST_COMMAND: &str = r#"!cockpit ask run --reason "acceptance check" 'printf "hello from the host\nGITHUB_TOKEN=ghp_abcdefghijklmnopqrstuvwxyz0123456789\n"'"#;

/// A session from the CLI shown in the viewer, its agent at its prompt; returns its id.
fn shown_session(cockpit: &Cockpit) -> String {
    let session = cockpit.session_from_cli();
    cockpit.wait_for(&session);
    cockpit.press(&["Enter"]);
    cockpit.wait_for(&format!("reply: {FIRST_MESSAGE}"));
    session
}

/// The agent asks to run `command` (with its `!`); waits for the pane to show it past its
/// grace and gives the keys to the list, as the owner's click does (#317).
fn asked(cockpit: &Cockpit, command: &str) {
    cockpit.tell_agent(command);
    cockpit.wait_for(RUN_KEYS);
    cockpit.click(ManagerPane::List);
}

#[test]
fn a_key_pressed_while_the_pane_asks_to_read_first_does_nothing() {
    let cockpit = Cockpit::start();
    shown_session(&cockpit);
    cockpit.tell_agent(HOST_COMMAND);
    cockpit.wait_for(GRACE_HINT);
    cockpit.click(ManagerPane::List);
    cockpit.press(&["y"]);
    let screen = cockpit.wait_for(RUN_KEYS);
    assert!(screen.contains("Run request 1"), "{screen}");
    assert!(!screen.contains("exit 0"), "{screen}");
    assert!(!screen.contains("approve it again"), "{screen}");
}

#[test]
fn y_runs_the_command_and_the_agent_gets_its_output_redacted() {
    let cockpit = Cockpit::start();
    shown_session(&cockpit);
    asked(&cockpit, HOST_COMMAND);
    let screen = cockpit.wait_for("Reason: acceptance check");
    assert!(screen.contains("Directory:"), "{screen}");
    cockpit.press(&["y"]);
    let screen = cockpit.wait_for("exit 0");
    assert!(
        screen.contains("\nhello from the host") || screen.contains("││hello from the host"),
        "{screen}"
    );
    assert!(screen.contains("GITHUB_TOKEN=[redacted]"), "{screen}");
    cockpit.wait_for_gone("Waiting on you");
}

#[test]
fn x_declines_and_the_agent_gets_exit_125() {
    let cockpit = Cockpit::start();
    shown_session(&cockpit);
    asked(
        &cockpit,
        r#"!cockpit ask run --reason "decline check" 'echo should not run'"#,
    );
    cockpit.press(&["x"]);
    let screen = cockpit.wait_for("exit 125");
    assert!(!screen.contains("││should not run"), "{screen}");
}

#[test]
fn a_guide_shows_one_step_at_a_time_and_the_agent_gets_the_answer() {
    let cockpit = Cockpit::start();
    shown_session(&cockpit);
    cockpit.tell_agent(
        r"!printf '# Pane guide\n## One\n1. Look at the sky\n   ask: What colour is it?\n2. Press y\n' | cockpit ask guide",
    );
    cockpit.wait_for("y answer   s stuck   x close");
    cockpit.click(ManagerPane::List);
    let screen = cockpit.wait_for("Step 1 of 2: Look at the sky");
    assert!(!screen.contains("Step 2 of 2"), "{screen}");
    cockpit.press(&["y"]);
    cockpit.wait_for("Answer: _");
    cockpit.type_text("blue");
    cockpit.press(&["Enter"]);
    cockpit.wait_for("Step 2 of 2: Press y");
    cockpit.wait_for("y done   s stuck   x close");
    cockpit.press(&["y"]);
    cockpit.wait_for("exit 0");
    let screen = cockpit.wait_for_gone("Waiting on you");
    assert!(screen.contains("blue"), "{screen}");
}

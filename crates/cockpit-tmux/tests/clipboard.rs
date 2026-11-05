//! Copy out of a session (#848): text copied in an agent's pane reaches the desktop terminal
//! through the viewer's nested client, and no pane reads a paste buffer back.
//!
//! The desktop terminal is a second private tmux server: its pane runs the owner's client of
//! Cockpit's server, as kitty would, and it keeps every OSC 52 it receives as a paste buffer.

mod common;

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

use cockpit_tmux::PaneId;
use common::{TestServer, eventually, wait_for};

/// Kitty's `TERM` is `xterm-kitty`; tmux gives the clipboard feature to every `xterm*`.
const DESKTOP_CONFIG: &str = "\
set-option -s set-clipboard on
set-option -g default-terminal xterm-256color
";

struct Desktop {
    socket: PathBuf,
}

impl Desktop {
    /// The owner's terminal, attached to the manager session `cockpit`.
    fn attached_to_cockpit(fixture: &TestServer) -> Desktop {
        let config = fixture.home.path().join("desktop.conf");
        fs::write(&config, DESKTOP_CONFIG).expect("the desktop config is writable");
        let desktop = Desktop {
            socket: fixture.home.path().join("desktop.sock"),
        };
        let owner_client = format!(
            "env -u TMUX tmux -S '{}' attach-session -t =cockpit",
            fixture.socket_path().display()
        );
        let started = desktop.run(&[
            "-f".as_ref(),
            config.as_os_str(),
            "-S".as_ref(),
            desktop.socket.as_os_str(),
            "new-session".as_ref(),
            "-d".as_ref(),
            "-x".as_ref(),
            "120".as_ref(),
            "-y".as_ref(),
            "40".as_ref(),
            owner_client.as_ref(),
        ]);
        assert!(started.status.success(), "{started:?}");
        desktop
    }

    fn run(&self, arguments: &[&std::ffi::OsStr]) -> Output {
        Command::new("tmux")
            .args(arguments)
            .env_remove("TMUX")
            .output()
            .expect("tmux runs")
    }

    /// What the terminal last received with OSC 52, if anything.
    fn clipboard(&self) -> Option<String> {
        let shown = self.run(&[
            "-S".as_ref(),
            self.socket.as_os_str(),
            "show-buffer".as_ref(),
        ]);
        shown
            .status
            .success()
            .then(|| String::from_utf8_lossy(&shown.stdout).into_owned())
    }
}

impl Drop for Desktop {
    fn drop(&mut self) {
        let _ = self.run(&[
            "-S".as_ref(),
            self.socket.as_os_str(),
            "kill-server".as_ref(),
        ]);
    }
}

/// The manager session `cockpit` with the agent shown in its viewer, and the owner's terminal
/// attached to it. Returns once both clients of Cockpit's server can write the clipboard.
fn agent_shown_on_the_desktop(fixture: &TestServer, agent_script: &str) -> (Desktop, PaneId) {
    let (_, viewer) = fixture.start_shell("cockpit", "sleep 60");
    let (agent, agent_pane) = fixture.start_shell("agent-1", agent_script);
    fixture
        .server
        .show_in_viewer(&viewer, &agent)
        .expect("respawns the viewer");
    let desktop = Desktop::attached_to_cockpit(fixture);
    eventually("both clients have the clipboard feature", || {
        clients_with_clipboard(fixture) == ["agent-1", "cockpit"]
    });
    (desktop, agent_pane)
}

/// The sessions whose clients tmux sends OSC 52 to, sorted.
fn clients_with_clipboard(fixture: &TestServer) -> Vec<String> {
    let listed = fixture.tmux(&[
        "list-clients",
        "-F",
        "#{session_name} #{client_termfeatures}",
    ]);
    let mut sessions: Vec<String> = listed
        .lines()
        .filter_map(|line| line.split_once(' '))
        .filter(|(_, features)| features.split(',').any(|feature| feature == "clipboard"))
        .map(|(session, _)| session.to_string())
        .collect();
    sessions.sort();
    sessions
}

#[test]
fn text_an_agent_copies_with_osc_52_reaches_the_desktop_terminal() {
    let fixture = TestServer::new();
    let copy = "printf '\\033]52;c;%s\\007' \"$(printf agent-copied | base64)\"";
    let (desktop, _) =
        agent_shown_on_the_desktop(&fixture, &format!("{}; {copy}; sleep 60", wait_for("copy")));

    fixture.release("copy");

    eventually("the desktop clipboard holds the agent's text", || {
        desktop.clipboard().as_deref() == Some("agent-copied")
    });
}

#[test]
fn a_line_copied_in_copy_mode_reaches_the_desktop_terminal() {
    let fixture = TestServer::new();
    let (desktop, agent_pane) =
        agent_shown_on_the_desktop(&fixture, "echo owner-selected; sleep 60");
    eventually("the agent printed its line", || {
        fixture
            .tmux(&["capture-pane", "-p", "-t", agent_pane.as_str()])
            .contains("owner-selected")
    });

    fixture.tmux(&["copy-mode", "-t", agent_pane.as_str()]);
    fixture.tmux(&[
        "send-keys",
        "-t",
        agent_pane.as_str(),
        "-X",
        "history-top",
        ";",
        "send-keys",
        "-t",
        agent_pane.as_str(),
        "-X",
        "select-line",
        ";",
        "send-keys",
        "-t",
        agent_pane.as_str(),
        "-X",
        "copy-selection-and-cancel",
    ]);

    eventually("the desktop clipboard holds the selected line", || {
        desktop.clipboard().as_deref() == Some("owner-selected")
    });
}

#[test]
fn a_pane_asking_for_the_clipboard_gets_no_answer() {
    let fixture = TestServer::new();
    // Raw, unechoed input: whatever tmux answers lands in `answer`, waiting up to a second.
    let ask = "stty -icanon -echo min 0 time 10; printf '\\033]52;c;?\\007'; \
               dd bs=4096 count=1 of=answer.part 2>/dev/null; mv answer.part answer";
    fixture.start_shell("agent-1", &format!("{}; {ask}; sleep 60", wait_for("ask")));
    fixture.tmux(&["set-buffer", "owner-copied-elsewhere"]);
    let answer = fixture.home.path().join("answer");

    fixture.release("ask");

    eventually("the pane stopped waiting for an answer", || answer.exists());

    let answer = fs::read(&answer).expect("readable");
    assert_eq!(String::from_utf8_lossy(&answer), "");
}

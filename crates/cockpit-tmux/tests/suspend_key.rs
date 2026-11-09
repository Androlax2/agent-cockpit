//! Ctrl+Z and an agent's pane (#1271). A harness that reads Ctrl+Z suspends itself, and under
//! Cockpit's server nothing can resume it, so the key typed by any client never reaches a
//! pane marked as an agent's; it reaches every other pane, and keys Cockpit types still
//! arrive.
//!
//! The owner's terminal is a second private tmux server whose pane runs the owner's client
//! of Cockpit's server, as kitty would.

mod common;

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

use cockpit_tmux::{Keystroke, PaneId, Viewing};
use common::{TestServer, eventually};

struct Desktop {
    socket: PathBuf,
}

impl Desktop {
    /// The owner's terminal, attached to the manager session `cockpit`.
    fn attached_to_cockpit(fixture: &TestServer) -> Desktop {
        let desktop = Desktop {
            socket: fixture.home.path().join("desktop.sock"),
        };
        let owner_client = format!(
            "env -u TMUX tmux -S '{}' attach-session -t =cockpit",
            fixture.socket_path().display()
        );
        let started = desktop.run(&[
            "-f",
            "/dev/null",
            "new-session",
            "-d",
            "-s",
            "desk",
            "-x",
            "120",
            "-y",
            "40",
            &owner_client,
        ]);
        assert!(started.status.success(), "{started:?}");
        desktop
    }

    fn run(&self, arguments: &[&str]) -> Output {
        Command::new("tmux")
            .arg("-S")
            .arg(&self.socket)
            .args(arguments)
            .env_remove("TMUX")
            .output()
            .expect("tmux runs")
    }

    /// Types `keys` (tmux key names) into the owner's client, as the owner would.
    fn type_keys(&self, keys: &[&str]) {
        let mut arguments = vec!["send-keys", "-t", "=desk:"];
        arguments.extend(keys);
        let typed = self.run(&arguments);
        assert!(typed.status.success(), "{typed:?}");
    }
}

impl Drop for Desktop {
    fn drop(&mut self) {
        let _ = self.run(&["kill-server"]);
    }
}

/// A shell line that writes the first byte the pane's program reads, raw, to `name`.
fn first_byte_to(name: &str) -> String {
    format!("stty raw -echo; dd bs=1 count=1 of={name}.part 2>/dev/null; mv {name}.part {name}")
}

fn read(fixture: &TestServer, name: &str) -> Option<String> {
    fs::read_to_string(fixture.home.path().join(name)).ok()
}

/// The agent shown in the viewer of `cockpit`, its pane marked as an agent's when
/// `is_marked`, and the owner's terminal attached. Returns the terminal, the viewer and the
/// agent's pane.
fn agent_in_the_viewer(fixture: &TestServer, is_marked: bool) -> (Desktop, PaneId, PaneId) {
    let (_, viewer) = fixture.start_shell("cockpit", "sleep 60");
    let (agent, agent_pane) = fixture.start_shell(
        "agent-1",
        &format!("{}; sleep 60", first_byte_to("agent-read")),
    );
    if is_marked {
        fixture
            .server
            .drop_suspend_key(&agent_pane)
            .expect("marks the pane");
    }
    fixture
        .server
        .show_in_viewer(&viewer, &agent, Viewing::ReadWrite)
        .expect("respawns the viewer");
    let desktop = Desktop::attached_to_cockpit(fixture);
    eventually("the owner and the viewer are attached", || {
        fixture
            .tmux(&["list-clients", "-F", "#{session_name}"])
            .lines()
            .count()
            == 2
    });
    (desktop, viewer, agent_pane)
}

#[test]
fn ctrl_z_typed_in_the_viewer_never_reaches_an_agent() {
    let fixture = TestServer::new();
    let (desktop, _, _) = agent_in_the_viewer(&fixture, true);

    desktop.type_keys(&["C-z", "x"]);

    eventually("the agent read a key", || {
        read(&fixture, "agent-read").is_some()
    });
    assert_eq!(read(&fixture, "agent-read").as_deref(), Some("x"));
}

#[test]
fn ctrl_z_reaches_a_pane_not_marked_as_an_agents() {
    let fixture = TestServer::new();
    let (desktop, _, _) = agent_in_the_viewer(&fixture, false);

    desktop.type_keys(&["C-z", "x"]);

    eventually("the program read a key", || {
        read(&fixture, "agent-read").is_some()
    });
    assert_eq!(read(&fixture, "agent-read").as_deref(), Some("\u{1a}"));
}

#[test]
fn ctrl_z_cockpit_types_into_an_agent_still_arrives() {
    let fixture = TestServer::new();
    let (_desktop, _, agent_pane) = agent_in_the_viewer(&fixture, true);

    fixture
        .server
        .send_keys(&agent_pane, &[Keystroke::Control('z')])
        .expect("typed");

    eventually("the agent read the key", || {
        read(&fixture, "agent-read").as_deref() == Some("\u{1a}")
    });
}

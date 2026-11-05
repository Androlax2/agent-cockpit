//! The manager's phone layout (#855) on a real private tmux server: a read-only viewer drops
//! what is typed into it, and the column alone, stacked above the viewer or beside it.

mod common;

use std::process::{Child, Command, Stdio};
use std::time::Duration;

use cockpit_tmux::{Keystroke, PaneId, Split, Viewing};
use common::{TestServer, eventually, shell};

fn screen(fixture: &TestServer, pane: &PaneId) -> String {
    let screen = fixture.server.capture(pane, 0).expect("the pane exists");
    String::from_utf8_lossy(&screen).into_owned()
}

/// A manager-like session, its column and the viewer beside it, and an agent session whose
/// terminal echoes what reaches it.
fn manager_and_agent(fixture: &TestServer) -> (PaneId, PaneId, PaneId) {
    let (_, column) = fixture.start_shell("manager", "sleep 60");
    let viewer = fixture
        .server
        .split_pane(
            &column,
            Split::SideBySide { percent: 70 },
            &shell("sleep 60"),
        )
        .expect("splits");
    let (_, agent) = fixture.start_shell("agent-1", "echo agent-ready; cat");
    eventually("the agent runs", || {
        screen(fixture, &agent).contains("agent-ready")
    });
    (column, viewer, agent)
}

fn attached(fixture: &TestServer) -> bool {
    fixture
        .tmux(&["list-clients", "-F", "#{session_name}"])
        .lines()
        .any(|session| session == "agent-1")
}

/// The owner's terminal on the manager: a control-mode client, attached while it lives.
struct OwnerClient {
    process: Child,
    name: String,
}

impl OwnerClient {
    fn attach(fixture: &TestServer) -> OwnerClient {
        let process = Command::new("tmux")
            .arg("-S")
            .arg(fixture.socket_path())
            .args(["-C", "attach-session", "-t", "=manager"])
            .env_remove("TMUX")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .expect("tmux runs");
        let mut name = String::new();
        eventually("the owner's client is attached to the manager", || {
            name = fixture
                .tmux(&["list-clients", "-t", "=manager", "-F", "#{client_name}"])
                .trim()
                .to_owned();
            !name.is_empty()
        });
        OwnerClient { process, name }
    }

    /// Types `text` and Enter into `pane` as this client, the way the owner's keys reach it.
    fn type_into(&self, fixture: &TestServer, pane: &PaneId, text: &str) {
        let pane = pane.as_str();
        let client = self.name.as_str();
        fixture.tmux(&["send-keys", "-c", client, "-t", pane, "-l", text]);
        fixture.tmux(&["send-keys", "-c", client, "-t", pane, "Enter"]);
    }
}

impl Drop for OwnerClient {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

#[test]
fn keys_typed_into_a_read_write_viewer_reach_the_session() {
    let fixture = TestServer::new();
    let (_, viewer, agent) = manager_and_agent(&fixture);
    let name = cockpit_tmux::SessionName::new("agent-1").expect("valid");

    fixture
        .server
        .show_in_viewer(&viewer, &name, Viewing::ReadWrite)
        .expect("respawns the viewer");
    eventually("the viewer's client is attached", || attached(&fixture));
    let owner = OwnerClient::attach(&fixture);
    owner.type_into(&fixture, &viewer, "typed-through-the-viewer");

    eventually("the agent's terminal got the keys", || {
        screen(&fixture, &agent).contains("typed-through-the-viewer")
    });
}

#[test]
fn keys_typed_into_a_read_only_viewer_reach_no_session() {
    let fixture = TestServer::new();
    let (_, viewer, agent) = manager_and_agent(&fixture);
    let name = cockpit_tmux::SessionName::new("agent-1").expect("valid");

    fixture
        .server
        .show_in_viewer(&viewer, &name, Viewing::ReadOnly)
        .expect("respawns the viewer");
    eventually("the viewer's client is attached", || attached(&fixture));
    eventually("the viewer shows the agent's screen", || {
        screen(&fixture, &viewer).contains("agent-ready")
    });
    let owner = OwnerClient::attach(&fixture);
    owner.type_into(&fixture, &viewer, "typed-through-the-viewer");
    // The read-write test above sees the same keys arrive well within this.
    std::thread::sleep(Duration::from_secs(1));

    assert!(
        !screen(&fixture, &agent).contains("typed-through-the-viewer"),
        "{}",
        screen(&fixture, &agent)
    );
}

#[test]
fn a_read_only_viewer_leaves_send_keys_to_the_session_working() {
    let fixture = TestServer::new();
    let (_, viewer, agent) = manager_and_agent(&fixture);
    let name = cockpit_tmux::SessionName::new("agent-1").expect("valid");
    fixture
        .server
        .show_in_viewer(&viewer, &name, Viewing::ReadOnly)
        .expect("respawns the viewer");
    eventually("the viewer's client is attached", || attached(&fixture));

    fixture
        .server
        .send_keys(
            &agent,
            &[Keystroke::Text("typed-by-the-daemon"), Keystroke::Enter],
        )
        .expect("types");

    eventually("the agent's terminal got the keys", || {
        screen(&fixture, &agent).contains("typed-by-the-daemon")
    });
}

#[test]
fn showing_read_write_again_turns_the_viewer_s_input_back_on() {
    let fixture = TestServer::new();
    let (_, viewer, agent) = manager_and_agent(&fixture);
    let name = cockpit_tmux::SessionName::new("agent-1").expect("valid");
    fixture
        .server
        .show_in_viewer(&viewer, &name, Viewing::ReadOnly)
        .expect("respawns the viewer");

    fixture
        .server
        .show_in_viewer(&viewer, &name, Viewing::ReadWrite)
        .expect("respawns the viewer");
    eventually("the viewer's client is attached", || attached(&fixture));
    let owner = OwnerClient::attach(&fixture);
    owner.type_into(&fixture, &viewer, "typed-after-read-only");

    eventually("the agent's terminal got the keys", || {
        screen(&fixture, &agent).contains("typed-after-read-only")
    });
}

#[test]
fn stacking_puts_the_first_pane_on_top_across_the_whole_width_and_rows_high() {
    let fixture = TestServer::new();
    let (column, viewer, _) = manager_and_agent(&fixture);

    fixture.server.stack_panes(&column, 3).expect("stacks");

    let geometry = fixture.server.pane_geometry(&column).expect("reads");
    assert_eq!(geometry.pane_height, 3);
    assert_eq!(geometry.pane_width, geometry.window_width);
    let viewer = fixture.server.pane_geometry(&viewer).expect("reads");
    assert_eq!(viewer.pane_width, viewer.window_width);
    assert_eq!(viewer.pane_height, viewer.window_height - 4);
}

#[test]
fn zooming_shows_the_pane_alone_and_gives_it_the_keys() {
    let fixture = TestServer::new();
    let (column, viewer, _) = manager_and_agent(&fixture);
    fixture.server.select_pane(&viewer).expect("selects");

    fixture.server.toggle_zoom(&column).expect("zooms");

    let geometry = fixture.server.pane_geometry(&column).expect("reads");
    assert!(geometry.is_window_zoomed);
    assert!(geometry.is_active);
    assert_eq!(
        (geometry.pane_width, geometry.pane_height),
        (geometry.window_width, geometry.window_height)
    );
}

#[test]
fn side_by_side_gives_the_first_pane_its_share_of_the_width_from_a_zoomed_stack() {
    let fixture = TestServer::new();
    let (column, _, _) = manager_and_agent(&fixture);
    fixture.server.stack_panes(&column, 3).expect("stacks");
    fixture.server.toggle_zoom(&column).expect("zooms");

    fixture
        .server
        .place_side_by_side(&column, 30)
        .expect("places them");

    let geometry = fixture.server.pane_geometry(&column).expect("reads");
    assert!(!geometry.is_window_zoomed);
    assert_eq!(geometry.pane_height, geometry.window_height);
    assert_eq!(geometry.pane_width, geometry.window_width * 30 / 100);
}

//! Sessions, panes, the viewer and the approval pane on a real private tmux server.

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;

use cockpit_tmux::{Error, PaneId, PaneState, SessionLaunch, SessionName};
use common::{TestServer, eventually, shell};

fn screen(fixture: &TestServer, pane: &PaneId) -> String {
    let screen = fixture.server.capture(pane, 0).expect("the pane exists");
    String::from_utf8_lossy(&screen).into_owned()
}

fn screen_contains(fixture: &TestServer, pane: &PaneId, text: &str) -> bool {
    screen(fixture, pane).contains(text)
}

#[test]
fn a_session_runs_its_command_in_its_directory_with_its_environment() {
    let fixture = TestServer::new();
    let name = SessionName::new("agent-1").expect("valid");
    let pane = fixture
        .server
        .new_session(&SessionLaunch {
            name,
            working_dir: fixture.home.path().to_path_buf(),
            command: shell("echo \"in $(pwd) as $COCKPIT_SESSION\"; sleep 30"),
            environment: vec![("COCKPIT_SESSION".to_string(), "s-41".into())],
            columns: 100,
            rows: 30,
        })
        .expect("starts");

    let expected = format!("in {} as s-41", fixture.home.path().display());
    eventually("the command printed", || {
        screen_contains(&fixture, &pane, &expected)
    });
}

#[test]
fn a_command_path_with_shell_characters_or_equals_signs_runs_unparsed() {
    let fixture = TestServer::new();
    let directory = fixture.home.path().join("dir with 'quote' $HOME a=b");
    fs::create_dir(&directory).expect("creatable");
    let program = directory.join("agent;true");
    fs::write(&program, "#!/bin/sh\necho program-ran\nsleep 30\n").expect("writable");
    fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).expect("chmod");

    let pane = fixture
        .server
        .new_session(&SessionLaunch {
            name: SessionName::new("agent-1").expect("valid"),
            working_dir: fixture.home.path().to_path_buf(),
            command: vec![program.into()],
            environment: Vec::new(),
            columns: 100,
            rows: 30,
        })
        .expect("starts");

    eventually("the program printed", || {
        screen_contains(&fixture, &pane, "program-ran")
    });
}

#[test]
fn sessions_lists_every_session_and_nothing_when_no_server_runs() {
    let fixture = TestServer::new();
    assert_eq!(
        fixture.server.sessions().expect("no server is no error"),
        Vec::new()
    );

    fixture.start_shell("agent-2", "sleep 30");
    fixture.start_shell("agent-1", "sleep 30");

    let names: Vec<String> = fixture
        .server
        .sessions()
        .expect("lists")
        .iter()
        .map(|name| name.to_string())
        .collect();
    assert_eq!(names, ["agent-1", "agent-2"]);
}

#[test]
fn a_session_name_never_reaches_another_session_by_prefix() {
    let fixture = TestServer::new();
    fixture.start_shell("agent-12", "sleep 30");

    let error = fixture
        .server
        .panes(&SessionName::new("agent-1").expect("valid"))
        .expect_err("agent-1 does not exist");

    assert!(matches!(error, Error::CommandFailed { .. }), "{error:?}");
}

#[test]
fn a_taken_session_name_is_refused() {
    let fixture = TestServer::new();
    fixture.start_shell("agent-1", "sleep 30");

    let error = fixture
        .server
        .new_session(&SessionLaunch {
            name: SessionName::new("agent-1").expect("valid"),
            working_dir: fixture.home.path().to_path_buf(),
            command: shell("sleep 30"),
            environment: Vec::new(),
            columns: 100,
            rows: 30,
        })
        .expect_err("the name is taken");

    assert!(matches!(error, Error::CommandFailed { .. }), "{error:?}");
}

#[test]
fn an_exited_pane_stays_with_its_exit_status_until_closed() {
    let fixture = TestServer::new();
    let (session, pane) = fixture.start_shell("agent-1", "echo last words; exit 7");

    eventually("the pane exited", || {
        fixture.server.panes(&session).expect("lists")[0].state
            == PaneState::Exited { status: Some(7) }
    });
    assert_eq!(fixture.server.panes(&session).expect("lists")[0].pid, None);
    // tmux moves the screen of a pane that died into its history.
    let screen = fixture
        .server
        .capture(&pane, 100)
        .expect("the dead pane remains");
    assert!(String::from_utf8_lossy(&screen).contains("last words"));

    fixture.server.close_session(&session).expect("closes");
    assert_eq!(fixture.server.sessions().expect("lists"), Vec::new());
}

#[test]
fn a_pane_reports_the_process_tmux_started_in_it() {
    let fixture = TestServer::new();
    let (session, _) = fixture.start_shell("agent-1", "echo $$ > pid; sleep 30");

    let pid_file = fixture.home.path().join("pid");
    eventually("the shell wrote its pid", || {
        fs::read_to_string(&pid_file).is_ok_and(|text| text.ends_with('\n'))
    });
    let shell_pid: u32 = fs::read_to_string(&pid_file)
        .expect("written")
        .trim()
        .parse()
        .expect("a pid");
    let panes = fixture.server.panes(&session).expect("lists");
    assert_eq!(panes.len(), 1);
    assert_eq!(panes[0].pid, Some(shell_pid));
    assert_eq!(panes[0].state, PaneState::Running);
}

#[test]
fn the_viewer_pane_attaches_to_an_agent_session_nested() {
    let fixture = TestServer::new();
    let (_, viewer) = fixture.start_shell("cockpit", "sleep 60");
    let (agent, _) = fixture.start_shell("agent-1", "echo agent-screen; sleep 60");

    fixture
        .server
        .show_in_viewer(&viewer, &agent)
        .expect("respawns the viewer");

    eventually("a client is attached to the agent session", || {
        fixture
            .tmux(&["list-clients", "-F", "#{session_name}"])
            .lines()
            .any(|session| session == "agent-1")
    });
    eventually("the viewer shows the agent's screen", || {
        screen_contains(&fixture, &viewer, "agent-screen")
    });
}

#[test]
fn the_approval_pane_opens_focused_below_the_viewer() {
    let fixture = TestServer::new();
    let (_, viewer) = fixture.start_shell("cockpit", "sleep 60");

    let approval = fixture
        .server
        .split_approval_pane(&viewer, &shell("echo approve?; sleep 60"))
        .expect("splits");

    let layout = fixture.tmux(&[
        "list-panes",
        "-t",
        "=cockpit:",
        "-F",
        "#{pane_id} #{pane_top} #{pane_left} #{pane_active}",
    ]);
    let rows: Vec<Vec<&str>> = layout
        .lines()
        .map(|line| line.split(' ').collect())
        .collect();
    let viewer_row = rows
        .iter()
        .find(|row| row[0] == viewer.as_str())
        .expect("viewer");
    let approval_row = rows
        .iter()
        .find(|row| row[0] == approval.as_str())
        .expect("approval");
    let top = |row: &Vec<&str>| row[1].parse::<u32>().expect("a number");
    assert!(top(approval_row) > top(viewer_row), "{layout}");
    assert_eq!(approval_row[2], viewer_row[2], "{layout}");
    assert_eq!(
        approval_row[3], "1",
        "the approval pane has focus: {layout}"
    );
    eventually("the approval command runs", || {
        screen_contains(&fixture, &approval, "approve?")
    });

    fixture.server.close_pane(&approval).expect("closes");
    assert!(
        !fixture
            .tmux(&["list-panes", "-a", "-F", "#{pane_id}"])
            .contains(approval.as_str())
    );
}

#[test]
fn selecting_a_pane_makes_it_the_one_keys_go_to() {
    let fixture = TestServer::new();
    let (_, left) = fixture.start_shell("cockpit", "sleep 60");
    fixture
        .server
        .split_pane(
            &left,
            cockpit_tmux::Split::SideBySide { percent: 70 },
            &shell("sleep 60"),
        )
        .expect("splits");

    fixture.server.select_pane(&left).expect("selects");

    let active = fixture.tmux(&["display-message", "-p", "-t", "=cockpit:", "#{pane_id}"]);
    assert_eq!(active.trim(), left.as_str());
}

#[test]
fn the_server_starts_with_cockpits_own_config() {
    let fixture = TestServer::new();
    fixture.start_shell("agent-1", "sleep 30");

    let history_limit = fixture.tmux(&["show-options", "-gv", "history-limit"]);
    let default_shell = fixture.tmux(&["show-options", "-gv", "default-shell"]);

    assert_eq!(history_limit.trim(), "50000");
    assert_eq!(default_shell.trim(), "/bin/sh");
}

#[test]
fn a_session_cockpit_did_not_name_is_left_out_of_the_list() {
    let fixture = TestServer::new();
    fixture.start_shell("agent-1", "sleep 30");
    fixture.tmux(&["new-session", "-d", "-s", "my project", "sleep 30"]);

    let names = fixture.server.sessions().expect("lists");

    assert_eq!(names, vec![SessionName::new("agent-1").expect("valid")]);
}

/// A shell line that reports the pane's tmux variables and whether a plain `tmux` reaches
/// a server. Without `TMUX`, tmux falls back to its default socket; `TMUX_TMPDIR` keeps that
/// one in the temporary directory too, never the user's server.
fn tmux_probe(fixture: &TestServer) -> String {
    let elsewhere = fixture.home.path().join("elsewhere");
    fs::create_dir_all(&elsewhere).expect("creatable");
    format!(
        "echo \"TMUX=${{TMUX-unset}} TMUX_PANE=${{TMUX_PANE-unset}}\"; \
         TMUX_TMPDIR='{}' tmux list-sessions; echo \"listed=$?\"",
        elsewhere.display()
    )
}

fn assert_pane_cannot_reach_the_server(fixture: &TestServer, pane: &PaneId) {
    let mut screen = String::new();
    eventually("the probe finished", || {
        screen = self::screen(fixture, pane);
        // Not "listed=": a hand-opened pane also shows the typed `listed=$?`.
        screen.contains("listed=0") || screen.contains("listed=1")
    });
    assert!(screen.contains("TMUX=unset TMUX_PANE=unset"), "{screen}");
    assert!(screen.contains("listed=1"), "{screen}");
    assert!(
        screen.contains("no server running") || screen.contains("error connecting"),
        "{screen}"
    );
}

#[test]
fn an_agent_pane_cannot_list_the_servers_sessions() {
    let fixture = TestServer::new();
    let script = format!("{}; sleep 30", tmux_probe(&fixture));

    let (_, pane) = fixture.start_shell("agent-1", &script);

    assert_pane_cannot_reach_the_server(&fixture, &pane);
}

#[test]
fn an_approval_pane_cannot_list_the_servers_sessions() {
    let fixture = TestServer::new();
    let (_, viewer) = fixture.start_shell("cockpit", "sleep 60");
    let script = format!("{}; sleep 30", tmux_probe(&fixture));

    let approval = fixture
        .server
        .split_approval_pane(&viewer, &shell(&script))
        .expect("splits");

    assert_pane_cannot_reach_the_server(&fixture, &approval);
}

#[test]
fn a_pane_opened_by_hand_cannot_list_the_servers_sessions() {
    let fixture = TestServer::new();
    fixture.start_shell("agent-1", "sleep 60");
    let opened = fixture.tmux(&[
        "new-window",
        "-d",
        "-t",
        "=agent-1:",
        "-P",
        "-F",
        "#{pane_id}",
    ]);
    let pane = PaneId::parse(opened.trim()).expect("a pane id");

    fixture.tmux(&[
        "send-keys",
        "-t",
        pane.as_str(),
        &tmux_probe(&fixture),
        "Enter",
    ]);

    assert_pane_cannot_reach_the_server(&fixture, &pane);
}

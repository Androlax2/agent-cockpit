//! Closing a server's last session and opening the next one at once, on real private tmux
//! servers: the close waits until the emptied server has exited (issue #197).

mod common;

use cockpit_tmux::SessionName;
use common::{TestServer, shell};

const PARALLEL_SERVERS: usize = 16;
const RESTARTS_PER_SERVER: usize = 10;

#[test]
fn restarting_the_only_session_never_reaches_an_exiting_server() {
    std::thread::scope(|scope| {
        for _ in 0..PARALLEL_SERVERS {
            scope.spawn(|| {
                let fixture = TestServer::new();
                let (name, _) = fixture.start_shell("only", "sleep 30");
                for restart in 0..RESTARTS_PER_SERVER {
                    fixture.server.close_session(&name).expect("closes");
                    fixture
                        .server
                        .new_session(&launch(&fixture, &name))
                        .unwrap_or_else(|error| panic!("restart {restart} failed: {error}"));
                }
            });
        }
    });
}

#[test]
fn closing_one_of_two_sessions_leaves_the_server_running() {
    let fixture = TestServer::new();
    let (closed, _) = fixture.start_shell("closed", "sleep 30");
    fixture.start_shell("kept", "sleep 30");
    let before = fixture.server.server_pid().expect("asks");

    fixture.server.close_session(&closed).expect("closes");

    assert_eq!(fixture.server.server_pid().expect("asks"), before);
    assert_eq!(
        fixture.server.sessions().expect("lists"),
        [SessionName::new("kept").expect("valid")]
    );
}

#[test]
fn closing_the_last_session_returns_once_the_server_has_exited() {
    let fixture = TestServer::new();
    let (name, _) = fixture.start_shell("only", "sleep 30");

    fixture.server.close_session(&name).expect("closes");

    assert_eq!(fixture.server.server_pid().expect("asks"), None);
}

fn launch(fixture: &TestServer, name: &SessionName) -> cockpit_tmux::SessionLaunch {
    cockpit_tmux::SessionLaunch {
        name: name.clone(),
        working_dir: fixture.home.path().to_path_buf(),
        command: shell("sleep 30"),
        environment: Vec::new(),
        columns: 100,
        rows: 30,
    }
}

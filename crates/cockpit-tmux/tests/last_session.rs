//! Closing a server's last session and opening the next one at once, on real private tmux
//! servers: the close waits until the emptied server has exited (issue #197), and another
//! caller's `new-session` that reaches the exiting server tries again once it is gone (#202).

mod common;

use cockpit_tmux::SessionName;
use common::TestServer;

const PARALLEL_SERVERS: usize = 16;
const RESTARTS_PER_SERVER: usize = 10;

#[test]
fn restarting_the_only_session_never_reaches_an_exiting_server() {
    std::thread::scope(|scope| {
        for _ in 0..PARALLEL_SERVERS {
            scope.spawn(|| {
                let fixture = TestServer::new();
                let (name, _) = fixture.start_shell("only", "sleep 30");
                for _ in 0..RESTARTS_PER_SERVER {
                    fixture.server.close_session(&name).expect("closes");
                    fixture.start_shell("only", "sleep 30");
                }
            });
        }
    });
}

#[test]
fn a_session_opened_while_another_caller_empties_the_server_always_starts() {
    std::thread::scope(|scope| {
        for _ in 0..PARALLEL_SERVERS {
            scope.spawn(|| {
                let fixture = TestServer::new();
                let other_caller = fixture.reopen();
                let (only, _) = fixture.start_shell("only", "sleep 30");
                for _ in 0..RESTARTS_PER_SERVER {
                    std::thread::scope(|round| {
                        round.spawn(|| {
                            fixture.server.close_session(&only).expect("closes");
                            fixture.start_shell("only", "sleep 30");
                        });
                        round.spawn(|| {
                            let (second, _) =
                                fixture.start_shell_through(&other_caller, "second", "sleep 30");
                            other_caller.close_session(&second).expect("closes");
                        });
                    });
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

//! `cockpit session` driven as a process against a scripted daemon socket. The crate graph
//! keeps frontends from building the daemon, so the lifecycle itself is proven in
//! `cockpit-daemon/tests/sessions.rs`; these prove each verb's request and its output.

use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::Output;

mod scripted;

use cockpit_protocol::{
    Command, ErrorCode, LaunchKindView, LaunchView, Outcome, ProtocolError, Reply, SessionFailure,
    SessionStateView, SessionView, StopTargetView,
};
use scripted::{scripted_daemon, text};

struct Home {
    dir: tempfile::TempDir,
}

impl Home {
    fn new() -> Home {
        Home {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn runtime_dir(&self) -> PathBuf {
        self.dir.path().join("run")
    }

    fn daemon_socket(&self) -> PathBuf {
        let path = self.runtime_dir().join("cockpit/daemon.sock");
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path.parent().unwrap())
            .unwrap();
        path
    }

    /// Runs `cockpit` with `arguments` from `cwd`, off the test's runtime threads.
    async fn cockpit(&self, cwd: &Path, arguments: &[&str]) -> Output {
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_cockpit"));
        command
            .current_dir(cwd)
            .args(arguments)
            .env_clear()
            .env("HOME", self.dir.path())
            .env("XDG_RUNTIME_DIR", self.runtime_dir());
        tokio::task::spawn_blocking(move || command.output().unwrap())
            .await
            .unwrap()
    }
}

fn view(id: &str, state: SessionStateView) -> SessionView {
    SessionView {
        id: id.to_owned(),
        agent: "claude".to_owned(),
        checkout: 1,
        holds_checkout: false,
        working_dir: "/p/app".to_owned(),
        conversation: Some("c-1".to_owned()),
        design_link: None,
        state,
        reason: None,
        revision: 1,
        created_at: 1,
        changed_at: 1,
        launch: Some(LaunchView {
            number: 1,
            kind: LaunchKindView::Start,
            launched_at: 1,
        }),
        changed_start_files: Vec::new(),
        restart_pending: false,
        finished: false,
    }
}

#[tokio::test]
async fn session_new_sends_an_absolute_dir_with_its_options_and_prints_the_session() {
    let home = Home::new();
    let daemon = scripted_daemon(
        &home.daemon_socket(),
        Outcome::Ok(Reply::Session {
            session: Box::new(view("s0123456789ab", SessionStateView::Running)),
        }),
    );
    let cwd = home.dir.path().canonicalize().unwrap();

    let output = home
        .cockpit(
            &cwd,
            &[
                "session",
                "new",
                "app",
                "--agent",
                "claude",
                "--prompt",
                "fix it",
                "--clone",
                "feature/x",
            ],
        )
        .await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        text(&output.stdout),
        "s0123456789ab\trunning\tclaude\tstart\tc-1\t/p/app\t-\t-\t-\n"
    );
    assert_eq!(
        daemon.await.unwrap(),
        Command::SessionNew {
            dir: cwd.join("app").display().to_string(),
            agent: Some("claude".to_owned()),
            prompt: Some("fix it".to_owned()),
            clone: Some("feature/x".to_owned()),
        }
    );
}

#[tokio::test]
async fn session_list_ended_asks_for_ended_sessions_and_prints_one_line_each() {
    let home = Home::new();
    let mut ended = view("s000000000002", SessionStateView::Ended);
    ended.reason = Some("its agent exited".to_owned());
    let daemon = scripted_daemon(
        &home.daemon_socket(),
        Outcome::Ok(Reply::Sessions {
            sessions: vec![
                view(
                    "s000000000001",
                    SessionStateView::Stopping {
                        to: StopTargetView::Asleep,
                    },
                ),
                ended,
            ],
        }),
    );

    let output = home
        .cockpit(home.dir.path(), &["session", "list", "--ended"])
        .await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        text(&output.stdout),
        "s000000000001\tstopping:asleep\tclaude\tstart\tc-1\t/p/app\t-\t-\t-\n\
         s000000000002\tended\tclaude\tstart\tc-1\t/p/app\tits agent exited\t-\t-\n"
    );
    assert_eq!(daemon.await.unwrap(), Command::SessionList { ended: true });
}

#[tokio::test]
async fn each_one_session_verb_sends_its_command() {
    let session = "s0123456789ab".to_owned();
    let cases: [(&[&str], Command); 6] = [
        (
            &["restart", "s0123456789ab"],
            Command::SessionRestart {
                session: session.clone(),
            },
        ),
        (
            &["sleep", "s0123456789ab"],
            Command::SessionSleep {
                session: session.clone(),
            },
        ),
        (
            &["wake", "s0123456789ab"],
            Command::SessionWake {
                session: session.clone(),
            },
        ),
        (
            &["resume", "s0123456789ab"],
            Command::SessionResume {
                session: session.clone(),
            },
        ),
        (
            &["end", "s0123456789ab"],
            Command::SessionEnd {
                session: session.clone(),
            },
        ),
        (
            &["link", "s0123456789ab", "https://example.com/canvas"],
            Command::SessionLink {
                session: session.clone(),
                url: "https://example.com/canvas".to_owned(),
            },
        ),
    ];
    for (verb, expected) in cases {
        let home = Home::new();
        let daemon = scripted_daemon(
            &home.daemon_socket(),
            Outcome::Ok(Reply::Session {
                session: Box::new(view(&session, SessionStateView::Running)),
            }),
        );
        let arguments: Vec<&str> = ["session"].iter().chain(verb).copied().collect();

        let output = home.cockpit(home.dir.path(), &arguments).await;

        assert!(
            output.status.success(),
            "{verb:?}: {}",
            text(&output.stderr)
        );
        assert_eq!(daemon.await.unwrap(), expected, "{verb:?}");
    }
}

#[tokio::test]
async fn restart_all_exits_1_naming_each_session_it_could_not_restart() {
    let home = Home::new();
    let daemon = scripted_daemon(
        &home.daemon_socket(),
        Outcome::Ok(Reply::RestartedAll {
            restarted: vec![view("s000000000001", SessionStateView::Running)],
            deferred: Vec::new(),
            failed: vec![SessionFailure {
                session: "s000000000002".to_owned(),
                error: ProtocolError::new(ErrorCode::Conflict, "it cannot resume"),
            }],
        }),
    );

    let output = home
        .cockpit(home.dir.path(), &["session", "restart", "--all"])
        .await;

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        text(&output.stdout),
        "s000000000001\trunning\tclaude\tstart\tc-1\t/p/app\t-\t-\t-\n"
    );
    assert_eq!(
        text(&output.stderr),
        "cockpit: s000000000002 not restarted: conflict: it cannot resume\n"
    );
    assert_eq!(daemon.await.unwrap(), Command::SessionRestartAll);
}

#[tokio::test]
async fn a_refused_session_verb_exits_1_with_the_daemons_reason() {
    let home = Home::new();
    let _daemon = scripted_daemon(
        &home.daemon_socket(),
        Outcome::Error(ProtocolError::new(
            ErrorCode::Conflict,
            "session s0123456789ab is running; wake needs asleep",
        )),
    );

    let output = home
        .cockpit(home.dir.path(), &["session", "wake", "s0123456789ab"])
        .await;

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        text(&output.stderr),
        "cockpit: conflict: session s0123456789ab is running; wake needs asleep\n"
    );
}

#[tokio::test]
async fn malformed_session_arguments_exit_2_before_any_connection() {
    let home = Home::new();
    let cases: [&[&str]; 6] = [
        &["session"],
        &["session", "new"],
        &["session", "new", "app", "--agent"],
        &["session", "new", "app", "--clone", "a", "--clone", "b"],
        &["session", "new", "app", "--model", "x"],
        &["session", "link", "s0123456789ab"],
    ];
    for arguments in cases {
        let output = home.cockpit(home.dir.path(), arguments).await;

        assert_eq!(output.status.code(), Some(2), "{arguments:?}");
        assert!(
            text(&output.stderr).contains("usage:"),
            "{arguments:?}: {}",
            text(&output.stderr)
        );
    }
}

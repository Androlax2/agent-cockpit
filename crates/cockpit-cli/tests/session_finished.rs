//! #157 (decided in #201): `cockpit session list` marks finished sessions, and
//! `cockpit session end --finished` ends them together, driven as a process against a
//! scripted daemon socket. Which sessions are finished is the daemon's rule, proven in
//! `cockpit-daemon/tests/session_robustness.rs`.

use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;
use std::process::Output;

mod scripted;

use cockpit_protocol::{
    Command, ErrorCode, Outcome, ProtocolError, Reply, SessionFailure, SessionStateView,
    SessionView,
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

    async fn cockpit(&self, arguments: &[&str]) -> Output {
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_cockpit"));
        command
            .current_dir(self.dir.path())
            .args(arguments)
            .env_clear()
            .env("HOME", self.dir.path())
            .env("XDG_RUNTIME_DIR", self.runtime_dir());
        tokio::task::spawn_blocking(move || command.output().unwrap())
            .await
            .unwrap()
    }
}

fn view(id: &str, state: SessionStateView, finished: bool) -> SessionView {
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
        launch: None,
        changed_start_files: Vec::new(),
        restart_pending: false,
        finished,
    }
}

#[tokio::test]
async fn session_list_marks_a_finished_session_in_its_work_column() {
    let home = Home::new();
    let _daemon = scripted_daemon(
        &home.daemon_socket(),
        Outcome::Ok(Reply::Sessions {
            sessions: vec![
                view("s000000000001", SessionStateView::Running, true),
                view("s000000000002", SessionStateView::Running, false),
            ],
        }),
    );

    let output = home.cockpit(&["session", "list"]).await;

    assert_eq!(
        text(&output.stdout),
        "s000000000001\trunning\tclaude\t-\tc-1\t/p/app\t-\t-\tfinished\n\
         s000000000002\trunning\tclaude\t-\tc-1\t/p/app\t-\t-\t-\n"
    );
}

#[tokio::test]
async fn end_finished_sends_one_request_and_prints_each_ended_session() {
    let home = Home::new();
    let daemon = scripted_daemon(
        &home.daemon_socket(),
        Outcome::Ok(Reply::EndedFinished {
            ended: vec![view("s000000000001", SessionStateView::Ended, false)],
            failed: Vec::new(),
        }),
    );

    let output = home.cockpit(&["session", "end", "--finished"]).await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        text(&output.stdout),
        "s000000000001\tended\tclaude\t-\tc-1\t/p/app\t-\t-\t-\n"
    );
    assert_eq!(
        daemon.await.unwrap(),
        Command::SessionEndFinished { only: None }
    );
}

#[tokio::test]
async fn end_finished_exits_1_naming_each_session_it_could_not_end() {
    let home = Home::new();
    let _daemon = scripted_daemon(
        &home.daemon_socket(),
        Outcome::Ok(Reply::EndedFinished {
            ended: Vec::new(),
            failed: vec![SessionFailure {
                session: "s000000000002".to_owned(),
                error: ProtocolError::new(ErrorCode::Conflict, "its agent did not stop"),
            }],
        }),
    );

    let output = home.cockpit(&["session", "end", "--finished"]).await;

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        text(&output.stderr),
        "cockpit: s000000000002 not ended: conflict: its agent did not stop\n"
    );
}

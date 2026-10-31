//! `cockpit provider list` and `cockpit session show` (#35) driven as a process against a
//! scripted daemon socket: each verb's request and its output.

use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;
use std::process::Output;

mod scripted;

use cockpit_protocol::{
    Command, ErrorCode, LaunchKindView, LaunchView, Outcome, ProtocolError, ProviderView, Reply,
    SessionStateView, SessionView,
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

    fn daemon_socket(&self) -> PathBuf {
        let path = self.dir.path().join("run/cockpit/daemon.sock");
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
            .args(arguments)
            .env_clear()
            .env("HOME", self.dir.path())
            .env("XDG_RUNTIME_DIR", self.dir.path().join("run"));
        tokio::task::spawn_blocking(move || command.output().unwrap())
            .await
            .unwrap()
    }
}

#[tokio::test]
async fn provider_list_prints_each_provider_and_whether_it_resumes() {
    let home = Home::new();
    let daemon = scripted_daemon(
        &home.daemon_socket(),
        Outcome::Ok(Reply::Providers {
            providers: vec![
                ProviderView {
                    name: "claude".to_owned(),
                    resume: true,
                },
                ProviderView {
                    name: "plain".to_owned(),
                    resume: false,
                },
            ],
        }),
    );

    let output = home.cockpit(&["provider", "list"]).await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(text(&output.stdout), "claude\tresume\nplain\tno-resume\n");
    assert_eq!(daemon.await.unwrap(), Command::ProviderList);
}

#[tokio::test]
async fn session_show_asks_the_daemon_to_show_the_session_and_prints_it() {
    let home = Home::new();
    let daemon = scripted_daemon(
        &home.daemon_socket(),
        Outcome::Ok(Reply::Session {
            session: Box::new(SessionView {
                id: "s0123456789ab".to_owned(),
                agent: "claude".to_owned(),
                checkout: 1,
                holds_checkout: false,
                working_dir: "/p/app".to_owned(),
                conversation: Some("c-1".to_owned()),
                design_link: None,
                state: SessionStateView::Running,
                reason: None,
                revision: 2,
                created_at: 1,
                changed_at: 2,
                launch: Some(LaunchView {
                    number: 2,
                    kind: LaunchKindView::Wake,
                    launched_at: 2,
                }),
                changed_start_files: Vec::new(),
                restart_pending: false,
            }),
        }),
    );

    let output = home.cockpit(&["session", "show", "s0123456789ab"]).await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        text(&output.stdout),
        "s0123456789ab\trunning\tclaude\twake\tc-1\t/p/app\t-\n"
    );
    assert_eq!(
        daemon.await.unwrap(),
        Command::SessionShow {
            session: "s0123456789ab".to_owned()
        }
    );
}

#[tokio::test]
async fn session_show_without_a_manager_exits_1_with_the_daemons_fix() {
    let home = Home::new();
    let _daemon = scripted_daemon(
        &home.daemon_socket(),
        Outcome::Error(ProtocolError::new(
            ErrorCode::NotFound,
            "no manager session is open: start cockpit-tui",
        )),
    );

    let output = home.cockpit(&["session", "show", "s0123456789ab"]).await;

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        text(&output.stderr),
        "cockpit: not-found: no manager session is open: start cockpit-tui\n"
    );
}

#[tokio::test]
async fn provider_with_anything_but_list_is_a_usage_error() {
    let home = Home::new();

    let output = home.cockpit(&["provider", "add", "claude"]).await;

    assert_eq!(output.status.code(), Some(2));
    assert!(text(&output.stderr).starts_with("cockpit: unknown provider arguments\nusage:"));
}

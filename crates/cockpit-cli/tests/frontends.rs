//! `cockpit provider list`, `cockpit session show` and `cockpit tui` (#35) driven as a process against a
//! scripted daemon socket: each verb's request and its output.

use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
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
        let path = self.dir.path().join("run/cockpit-dev/daemon.sock");
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path.parent().unwrap())
            .unwrap();
        path
    }

    async fn cockpit(&self, arguments: &[&str]) -> Output {
        self.run(PathBuf::from(env!("CARGO_BIN_EXE_cockpit")), arguments)
            .await
    }

    /// A copy of `cockpit` in its own directory, with `cockpit-tui` beside it when asked:
    /// `cockpit tui` looks for the TUI next to itself.
    fn installed_cockpit(&self, with_tui: bool) -> PathBuf {
        let bin = self.dir.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let cockpit = bin.join("cockpit");
        std::fs::copy(env!("CARGO_BIN_EXE_cockpit"), &cockpit).unwrap();
        if with_tui {
            std::fs::write(bin.join("cockpit-tui"), "").unwrap();
        }
        cockpit
    }

    async fn run(&self, program: PathBuf, arguments: &[&str]) -> Output {
        let mut command = std::process::Command::new(program);
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
                    events: true,
                },
                ProviderView {
                    name: "plain".to_owned(),
                    resume: false,
                    events: false,
                },
            ],
        }),
    );

    let output = home.cockpit(&["provider", "list"]).await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        text(&output.stdout),
        "claude\tresume\tevents\nplain\tno-resume\tno-events\n"
    );
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
                finished: false,
                coordinator_grants: Vec::new(),
                inbound_message: None,
            }),
        }),
    );

    let output = home.cockpit(&["session", "show", "s0123456789ab"]).await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        text(&output.stdout),
        "s0123456789ab\trunning\tclaude\twake\tc-1\t/p/app\t-\t-\t-\n"
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
            "no manager session is open: run `cockpit tui`",
        )),
    );

    let output = home.cockpit(&["session", "show", "s0123456789ab"]).await;

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        text(&output.stderr),
        "cockpit: not-found: no manager session is open: run `cockpit tui`\n"
    );
}

#[tokio::test]
async fn provider_with_anything_but_list_is_a_usage_error() {
    let home = Home::new();

    let output = home.cockpit(&["provider", "add", "claude"]).await;

    assert_eq!(output.status.code(), Some(2));
    assert!(text(&output.stderr).starts_with("cockpit: unknown provider arguments\nusage:"));
}

#[tokio::test]
async fn tui_opens_the_manager_with_the_tui_beside_cockpit_and_attaches_to_it() {
    let home = Home::new();
    let socket = home.daemon_socket();
    let cockpit = home.installed_cockpit(true);
    let daemon = scripted_daemon(
        &socket,
        Outcome::Ok(Reply::Manager {
            attach: vec!["/bin/echo".to_owned(), "attached".to_owned()],
            created: true,
        }),
    );

    let output = home.run(cockpit.clone(), &["tui"]).await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(text(&output.stdout), "attached\n");
    let tui = path_text(&cockpit.with_file_name("cockpit-tui"));
    assert_eq!(
        daemon.await.unwrap(),
        Command::ManagerOpen {
            dir: path_text(&std::env::current_dir().unwrap()),
            column: vec![
                tui.clone(),
                "--column".to_owned(),
                "--socket".to_owned(),
                path_text(&socket),
                "--profile".to_owned(),
                "dev".to_owned(),
                "--state-dir".to_owned(),
                path_text(&home.dir.path().join(".local/state/cockpit-dev")),
            ],
            viewer: vec![tui, "--viewer".to_owned()],
        }
    );
}

#[tokio::test]
async fn tui_without_cockpit_tui_beside_it_exits_1_naming_the_missing_file() {
    let home = Home::new();
    let cockpit = home.installed_cockpit(false);

    let output = home.run(cockpit.clone(), &["tui"]).await;

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        text(&output.stderr),
        format!(
            "cockpit: {} is missing: install cockpit-tui beside cockpit\n",
            cockpit.with_file_name("cockpit-tui").display()
        )
    );
}

fn path_text(path: &Path) -> String {
    path.to_str().unwrap().to_owned()
}

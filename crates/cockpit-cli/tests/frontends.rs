//! `cockpit provider list`, `cockpit session show`, `cockpit tui` (#35) and `torlek notify`
//! (#62) driven as a process against a scripted daemon socket: each verb's request and its
//! output.

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
        let path = self.dir.path().join("run/torlek-dev/daemon.sock");
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path.parent().unwrap())
            .unwrap();
        path
    }

    async fn cockpit(&self, arguments: &[&str]) -> Output {
        self.run(PathBuf::from(env!("CARGO_BIN_EXE_torlek")), arguments)
            .await
    }

    /// A copy of `cockpit` in its own directory, with `cockpit-tui` beside it when asked:
    /// `cockpit tui` looks for the TUI next to itself.
    fn installed_cockpit(&self, with_tui: bool) -> PathBuf {
        let bin = self.dir.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let cockpit = bin.join("torlek");
        std::fs::copy(env!("CARGO_BIN_EXE_torlek"), &cockpit).unwrap();
        if with_tui {
            std::fs::write(bin.join("torlek-tui"), "").unwrap();
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
                exit: None,
                changed_start_files: Vec::new(),
                restart_pending: false,
                finished: false,
                coordinator_grants: Vec::new(),
                inbound_message: None,
                sign_in_needed: None,
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
            "no manager session is open: run `torlek tui`",
        )),
    );

    let output = home.cockpit(&["session", "show", "s0123456789ab"]).await;

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        text(&output.stderr),
        "torlek: not-found: no manager session is open: run `torlek tui`\n"
    );
}

#[tokio::test]
async fn provider_with_anything_but_list_is_a_usage_error() {
    let home = Home::new();

    let output = home.cockpit(&["provider", "add", "claude"]).await;

    assert_eq!(output.status.code(), Some(2));
    assert!(text(&output.stderr).starts_with("torlek: unknown provider arguments\nusage:"));
}

#[tokio::test]
async fn session_paste_image_asks_the_daemon_and_prints_nothing() {
    let home = Home::new();
    let daemon = scripted_daemon(
        &home.daemon_socket(),
        Outcome::Ok(Reply::ClipboardImage {
            session: "s0123456789ab".to_owned(),
            path: Some("/run/s0123456789ab/clipboard-1.png".to_owned()),
        }),
    );

    let output = home.cockpit(&["session", "paste-image"]).await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(text(&output.stdout), "");
    assert_eq!(daemon.await.unwrap(), Command::SessionPasteImage);
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
    let tui = path_text(&cockpit.with_file_name("torlek-tui"));
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
                path_text(&home.dir.path().join(".local/state/torlek-dev")),
            ],
            viewer: vec![tui, "--viewer".to_owned()],
            image_paste: vec![
                path_text(&cockpit),
                "--profile".to_owned(),
                "dev".to_owned(),
                "session".to_owned(),
                "paste-image".to_owned(),
            ],
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
            "torlek: {} is missing: install torlek-tui beside torlek\n",
            cockpit.with_file_name("torlek-tui").display()
        )
    );
}

fn path_text(path: &Path) -> String {
    path.to_str().unwrap().to_owned()
}

#[tokio::test]
async fn tui_version_executes_the_installed_sibling_without_a_daemon() {
    use std::os::unix::fs::PermissionsExt;

    let home = Home::new();
    let cockpit = home.installed_cockpit(true);
    let tui = cockpit.with_file_name("torlek-tui");
    std::fs::write(&tui, "#!/bin/sh\n[ \"$#\" = 1 ] && [ \"$1\" = --version ] || exit 2\necho sibling-version\nexit 7\n").unwrap();
    std::fs::set_permissions(&tui, std::fs::Permissions::from_mode(0o755)).unwrap();
    let output = home.run(cockpit, &["tui", "--version"]).await;
    assert_eq!(output.status.code(), Some(7));
    assert_eq!(text(&output.stdout), "sibling-version\n");
}

#[tokio::test]
async fn tui_version_requires_the_sibling_binary() {
    let home = Home::new();
    let cockpit = home.installed_cockpit(false);
    let output = home.run(cockpit, &["tui", "--version"]).await;
    assert_eq!(output.status.code(), Some(1));
    assert!(text(&output.stderr).contains("install torlek-tui beside torlek"));
}

/// Installs a `torlek-notify` beside `cockpit` that prints each argument on its own line.
fn install_echoing_notifier(cockpit: &Path) {
    use std::os::unix::fs::PermissionsExt;

    let notify = cockpit.with_file_name("torlek-notify");
    std::fs::write(&notify, "#!/bin/sh\nprintf '%s\\n' \"$@\"\n").unwrap();
    std::fs::set_permissions(&notify, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[tokio::test]
async fn notify_runs_the_notifier_beside_cockpit_with_the_socket_state_and_settings() {
    let home = Home::new();
    let socket = home.daemon_socket();
    let cockpit = home.installed_cockpit(false);
    install_echoing_notifier(&cockpit);
    let config = home.dir.path().join(".config/torlek-dev/config.toml");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(
        &config,
        "[notify.desktop]\nfinished = false\n[notify.sound]\nfailure = false\nhold = false\n\
         question = false\nquota = false\nfinished = false\n",
    )
    .unwrap();

    let output = home.run(cockpit, &["notify"]).await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        text(&output.stdout),
        format!(
            "--socket\n{}\n--state-dir\n{}\n--desktop\nfailure,hold,question,quota\n--sound\nnone\n",
            socket.display(),
            home.dir.path().join(".local/state/torlek-dev").display()
        )
    );
}

#[tokio::test]
async fn notify_refuses_an_invalid_notify_setting_before_starting_the_notifier() {
    let home = Home::new();
    home.daemon_socket();
    let cockpit = home.installed_cockpit(false);
    install_echoing_notifier(&cockpit);
    let config = home.dir.path().join(".config/torlek-dev/config.toml");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(&config, "[notify.sound]\nlogin = false\n").unwrap();

    let output = home.run(cockpit, &["notify"]).await;

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(text(&output.stdout), "");
    assert!(
        text(&output.stderr).contains("login"),
        "{}",
        text(&output.stderr)
    );
}

#[tokio::test]
async fn notify_version_executes_the_installed_sibling_without_a_daemon() {
    let home = Home::new();
    let cockpit = home.installed_cockpit(false);
    install_echoing_notifier(&cockpit);

    let output = home.run(cockpit, &["notify", "--version"]).await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(text(&output.stdout), "--version\n");
}

#[tokio::test]
async fn notify_without_the_notifier_beside_cockpit_exits_1_naming_the_missing_file() {
    let home = Home::new();
    home.daemon_socket();
    let cockpit = home.installed_cockpit(false);

    let output = home.run(cockpit.clone(), &["notify"]).await;

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        text(&output.stderr),
        format!(
            "torlek: {} is missing: install torlek-notify beside torlek\n",
            cockpit.with_file_name("torlek-notify").display()
        )
    );
}

#[tokio::test]
async fn notify_refuses_an_extra_argument() {
    let home = Home::new();
    let cockpit = home.installed_cockpit(false);
    install_echoing_notifier(&cockpit);

    let output = home.run(cockpit, &["notify", "--loud"]).await;

    assert_eq!(output.status.code(), Some(2));
    assert!(
        text(&output.stderr).contains("unknown argument --loud"),
        "{}",
        text(&output.stderr)
    );
}

//! Issue #35 at the daemon's socket: the provider list the new-session form offers, the
//! manager tmux session (the TUI's column and the viewer beside it) and showing a session in
//! the viewer, on a private tmux server with a fake provider.
//!
//! The daemon runs in this process because the fake provider is a test type the binary does
//! not link (as in `sessions.rs`, whose fixture this file does not share so the two can
//! change apart; both use `cockpit_provider_api::fake`).
#![cfg(target_os = "linux")]

mod candidate;
mod common;

use std::fs::Permissions;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cockpit_client::{ClientError, Connection};
use cockpit_daemon::{Daemon, Instance, Limits, Options, Routes};
use cockpit_protocol::{
    Command, ErrorCode, LaunchKindView, ManagerPaneView, ProviderView, Reply, SessionStateView,
    SessionView,
};
use cockpit_provider_api::fake::FakeProvider;
use cockpit_provider_api::{Provider, ProviderName};
use common::GENEROUS;

struct Home {
    dir: tempfile::TempDir,
}

impl Home {
    fn new() -> Home {
        Home {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn path(&self) -> PathBuf {
        self.dir.path().canonicalize().unwrap()
    }

    async fn start(&self) -> Daemon {
        self.start_with_tmux(PathBuf::from("tmux")).await
    }

    /// Starts the daemon with `tmux` as its tmux binary.
    async fn start_with_tmux(&self, tmux: PathBuf) -> Daemon {
        self.start_wired(|wiring| wiring.tmux.tmux = tmux).await
    }

    /// Starts the daemon reading the clipboard with [`Home::fake_clipboard`] holding
    /// `clipboard`, the owner's desktop session imported as `desktop`.
    async fn start_with_clipboard(&self, clipboard: Clipboard, desktop: &[(&str, &str)]) -> Daemon {
        let reader = self.fake_clipboard(clipboard);
        let desktop = desktop
            .iter()
            .map(|(name, value)| (name.into(), value.into()))
            .collect();
        self.start_wired(|wiring| {
            wiring.clipboard_reader = reader;
            wiring.desktop_environment = desktop;
        })
        .await
    }

    async fn start_wired(&self, wire: impl FnOnce(&mut cockpit_daemon::Wiring)) -> Daemon {
        let log = self.path().join("agents.log");
        let providers: Vec<Arc<dyn Provider>> = vec![
            Arc::new(FakeProvider::new(
                ProviderName::new("fake").unwrap(),
                log.clone(),
            )),
            Arc::new(
                FakeProvider::new(ProviderName::new("plain").unwrap(), log.clone())
                    .without_resume(),
            ),
            Arc::new(
                FakeProvider::new(ProviderName::new("keys").unwrap(), log)
                    .running(self.first_key_recorder()),
            ),
        ];
        let mut wiring = common::wiring_in(&self.path(), providers);
        wire(&mut wiring);
        wiring.sessions.stop = cockpit_processes::Escalation {
            grace: Duration::from_secs(2),
            kill_wait: Duration::from_secs(5),
        };
        let options = Options {
            runtime_dir: self.path().join("run/cockpit"),
            state_dir: self.path().join("state/cockpit"),
            limits: Limits::default(),
        };
        Instance::acquire(options)
            .unwrap()
            .start(wiring, |capabilities| {
                Arc::new(Routes::new(
                    capabilities,
                    cockpit_config::Environment {
                        home: Some(self.path().join("home").into()),
                        ..cockpit_config::Environment::default()
                    },
                ))
            })
            .await
            .expect("the daemon starts")
    }

    fn tmux(&self, arguments: &[&str]) -> String {
        let output = Process::new("tmux")
            .arg("-S")
            .arg(self.path().join("tmux.sock"))
            .args(arguments)
            .env_remove("TMUX")
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    /// A Git project registered through the daemon, returning its root.
    async fn project(&self, client: &Connection) -> PathBuf {
        let root = self.path().join("projects/app");
        std::fs::create_dir_all(&root).unwrap();
        git(&root, &["init", "--quiet", "--initial-branch=main"]);
        std::fs::write(root.join("README"), "fixture\n").unwrap();
        git(&root, &["add", "."]);
        git(&root, &["commit", "--quiet", "-m", "initial"]);
        request(
            client,
            Command::ProjectAdd {
                path: root.display().to_string(),
                name: None,
            },
        )
        .await
        .unwrap();
        root
    }

    /// The owner's file, as `torlek` reads it for the daemon's default profile in this home.
    fn write_owner_config(&self, contents: &str) {
        let config_dir = self.path().join("home/.config/torlek");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(config_dir.join("config.toml"), contents).unwrap();
    }

    /// The command the manager's column pane was started with.
    fn column_command(&self) -> String {
        self.tmux(&[
            "list-panes",
            "-t",
            "=torlek:",
            "-F",
            "#{pane_left} #{pane_start_command}",
        ])
        .lines()
        .find(|pane| pane.starts_with("0 "))
        .expect("the manager has a column pane")
        .to_owned()
    }

    /// An agent program that records the first byte its terminal sends it in
    /// [`Home::first_key`], as typed: no echo, no line editing.
    fn first_key_recorder(&self) -> PathBuf {
        let recorder = self.path().join("record-first-key");
        std::fs::write(
            &recorder,
            format!(
                "#!/bin/sh\nstty raw -echo\ndd bs=1 count=1 of='{}' 2>/dev/null\n",
                self.path().join("first-key").display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&recorder, Permissions::from_mode(0o755)).unwrap();
        recorder
    }

    /// The first byte a `keys` agent got, once it has one.
    fn first_key(&self) -> Option<Vec<u8>> {
        std::fs::read(self.path().join("first-key"))
            .ok()
            .filter(|key| !key.is_empty())
    }

    /// A `wl-paste` holding `clipboard`, as wl-clipboard's answers: types listed one a line,
    /// the PNG's bytes, `Nothing is copied` for an empty clipboard. It answers only on the
    /// Wayland display `wayland-test`, as the real one finds no clipboard without one.
    fn fake_clipboard(&self, clipboard: Clipboard) -> PathBuf {
        let png = self.path().join("clipboard.png");
        let (types, bytes): (&str, &[u8]) = match clipboard {
            Clipboard::Empty => ("", b""),
            Clipboard::Text => ("text/plain;charset=utf-8\nUTF8_STRING", b""),
            Clipboard::Image(bytes) => ("image/png\ntext/html", bytes),
        };
        std::fs::write(&png, bytes).unwrap();
        let reader = self.path().join("wl-paste");
        std::fs::write(
            &reader,
            format!(
                "#!/bin/sh\n\
                 [ \"$WAYLAND_DISPLAY\" = wayland-test ] || {{ echo 'Failed to connect to a Wayland server' >&2; exit 1; }}\n\
                 [ -n '{types}' ] || {{ echo 'Nothing is copied' >&2; exit 1; }}\n\
                 case \"$*\" in\n\
                 --list-types) printf '%s\\n' '{types}' ;;\n\
                 '--no-newline --type image/png') cat '{png}' ;;\n\
                 *) echo \"unexpected arguments: $*\" >&2; exit 2 ;;\n\
                 esac\n",
                png = png.display(),
            ),
        )
        .unwrap();
        std::fs::set_permissions(&reader, Permissions::from_mode(0o755)).unwrap();
        reader
    }

    async fn open_manager(&self, client: &Connection) -> Result<Reply, ClientError> {
        request(
            client,
            Command::ManagerOpen {
                dir: self.path().display().to_string(),
                // A shell, so the words the daemon appends to the column (`--bell on`) are
                // positional arguments it ignores, as the real column's flags are its own.
                column: vec![
                    "/bin/sh".to_owned(),
                    "-c".to_owned(),
                    "sleep 600".to_owned(),
                    "column".to_owned(),
                ],
                viewer: vec!["/bin/sleep".to_owned(), "601".to_owned()],
                image_paste: Vec::new(),
            },
        )
        .await
    }
}

/// What the fake desktop clipboard holds.
#[derive(Clone, Copy)]
enum Clipboard {
    Empty,
    Text,
    Image(&'static [u8]),
}

/// The smallest PNG: its signature and a header chunk are what the daemon checks.
const PNG: &[u8] =
    b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x01\0\0\0\x01\x08\x06\0\0\0\x1f\x15\xc4\x89";

impl Drop for Home {
    fn drop(&mut self) {
        let _ = self.tmux(&["kill-server"]);
    }
}

fn git(dir: &Path, arguments: &[&str]) {
    let output = Process::new("git")
        .current_dir(dir)
        .args(["-c", "user.name=Fixture", "-c", "user.email=f@example.com"])
        .args(arguments)
        .output()
        .unwrap();
    assert!(output.status.success(), "git {arguments:?}");
}

async fn request(client: &Connection, command: Command) -> Result<Reply, ClientError> {
    client.request(command, Duration::from_secs(60)).await
}

async fn session(client: &Connection, command: Command) -> SessionView {
    match request(client, command.clone()).await {
        Ok(Reply::Session { session }) => *session,
        other => panic!("{command:?}: {other:?}"),
    }
}

fn refusal(result: Result<Reply, ClientError>) -> cockpit_protocol::ProtocolError {
    match result {
        Err(ClientError::Daemon(error)) => error,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + GENEROUS;
    while !check() {
        assert!(Instant::now() < deadline, "timed out waiting until {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_provider_list_names_each_wired_provider_and_whether_it_resumes() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();

    let reply = request(&client, Command::ProviderList).await.unwrap();

    assert_eq!(
        reply,
        Reply::Providers {
            providers: vec![
                ProviderView {
                    name: "fake".to_owned(),
                    resume: true,
                    events: false
                },
                ProviderView {
                    name: "plain".to_owned(),
                    resume: false,
                    events: false
                },
            ]
        }
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_daemon_opens_its_tmux_server_once_for_the_sessions_and_the_manager() {
    let home = Home::new();
    // Opening the server checks the version (`tmux -V`), then writes the config; nothing
    // else writes it. One check that found no config, and a config after, is one open.
    let checks = home.path().join("version-checks.log");
    let config = home.path().join("tmux.conf");
    let wrapper = home.path().join("tmux-wrapper");
    // The wrapper finds the config and its log beside itself, so no path is quoted into it.
    std::fs::write(
        &wrapper,
        "#!/bin/sh\n\
         here=$(dirname \"$0\")\n\
         if [ \"$1\" = -V ]; then\n\
         if [ -e \"$here/tmux.conf\" ]; then echo config-written >> \"$here/version-checks.log\"; \
         else echo no-config >> \"$here/version-checks.log\"; fi\n\
         fi\n\
         exec tmux \"$@\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&wrapper, Permissions::from_mode(0o755)).unwrap();
    let daemon = home.start_with_tmux(wrapper).await;
    let client = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();
    let root = home.project(&client).await;

    home.open_manager(&client).await.unwrap();
    session(
        &client,
        Command::SessionNew {
            dir: root.display().to_string(),
            agent: Some("fake".to_owned()),
            prompt: None,
            clone: None,
            model: None,
            effort: None,
        },
    )
    .await;

    assert_eq!(std::fs::read_to_string(&checks).unwrap(), "no-config\n");
    assert!(config.exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn without_tmux_the_manager_and_sessions_are_refused_naming_the_same_cause_and_fix() {
    let home = Home::new();
    let missing_tmux = home.path().join("no-tmux-here");
    let daemon = home.start_with_tmux(missing_tmux.clone()).await;
    let client = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();
    let root = home.project(&client).await;

    let manager = refusal(home.open_manager(&client).await);
    let sessions = refusal(
        request(
            &client,
            Command::SessionNew {
                dir: root.display().to_string(),
                agent: Some("fake".to_owned()),
                prompt: None,
                clone: None,
                model: None,
                effort: None,
            },
        )
        .await,
    );

    assert_eq!(
        (manager.code, sessions.code),
        (ErrorCode::Refused, ErrorCode::Refused)
    );
    let reason = format!("`{}` could not be run", missing_tmux.display());
    for message in [&manager.message, &sessions.message] {
        assert!(message.contains(&reason), "{message}");
        assert!(message.contains("restart the daemon"), "{message}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_manager_opens_a_column_and_a_viewer_once_and_answers_how_to_attach() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();

    let first = home.open_manager(&client).await.unwrap();
    let second = home.open_manager(&client).await.unwrap();

    let socket = home.path().join("tmux.sock").display().to_string();
    let attach: Vec<String> = ["tmux", "-S", &socket, "attach-session", "-t", "=torlek:"]
        .map(str::to_owned)
        .to_vec();
    assert_eq!(
        (first, second),
        (
            Reply::Manager {
                attach: attach.clone(),
                created: true
            },
            Reply::Manager {
                attach,
                created: false
            }
        )
    );
    let panes = home.tmux(&[
        "list-panes",
        "-t",
        "=torlek:",
        "-F",
        "#{pane_left} #{pane_start_command}",
    ]);
    let commands: Vec<&str> = panes.lines().collect();
    assert_eq!(commands.len(), 2, "{panes}");
    assert!(
        commands[0].starts_with("0 ") && commands[0].contains("600"),
        "{panes}"
    );
    assert!(
        !commands[1].starts_with("0 ") && commands[1].contains("601"),
        "{panes}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_column_is_told_to_ring_the_attention_bell_by_default() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();

    home.open_manager(&client).await.unwrap();

    let column = home.column_command();
    assert!(column.contains("--bell on"), "{column}");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_column_is_told_to_keep_quiet_when_the_owner_turned_the_bell_off() {
    let home = Home::new();
    home.write_owner_config("[tui]\nattention_bell = false\n");
    let daemon = home.start().await;
    let client = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();

    home.open_manager(&client).await.unwrap();

    let column = home.column_command();
    assert!(column.contains("--bell off"), "{column}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bell_change_reaches_the_column_the_next_time_the_manager_is_built() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();
    home.open_manager(&client).await.unwrap();
    home.write_owner_config("[tui]\nattention_bell = false\n");

    home.open_manager(&client).await.unwrap();
    let kept = home.column_command();
    home.tmux(&["kill-session", "-t", "=torlek:"]);
    home.open_manager(&client).await.unwrap();
    let rebuilt = home.column_command();

    assert!(kept.contains("--bell on"), "{kept}");
    assert!(rebuilt.contains("--bell off"), "{rebuilt}");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_invalid_owner_file_refuses_to_build_the_manager_naming_the_fix() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();
    home.write_owner_config("[tui]\nattention_bell = \"loud\"\n");

    let refused = refusal(home.open_manager(&client).await);

    assert_eq!(refused.code, ErrorCode::Refused);
    assert!(refused.message.contains("attention_bell"), "{refused:?}");
    assert!(refused.message.contains("torlek tui"), "{refused:?}");
}

/// The index of the pane an attached client's keys go to: the window's active pane. The
/// column is pane 0, the viewer pane 1.
fn focus(pane: ManagerPaneView) -> Command {
    Command::ManagerFocus { pane }
}

fn focused_pane(home: &Home) -> String {
    home.tmux(&["display-message", "-p", "-t", "=torlek:", "#{pane_index}"])
        .trim()
        .to_owned()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_manager_answers_with_its_column_focused() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();

    home.open_manager(&client).await.unwrap();

    assert_eq!(focused_pane(&home), "0");
}

#[tokio::test(flavor = "multi_thread")]
async fn reopening_the_manager_keeps_the_pane_its_owner_focused() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();
    home.open_manager(&client).await.unwrap();
    home.tmux(&["select-pane", "-t", "=torlek:.1"]);

    home.open_manager(&client).await.unwrap();

    assert_eq!(focused_pane(&home), "1");
}

#[tokio::test(flavor = "multi_thread")]
async fn manager_focus_gives_the_column_the_keys_the_viewer_had() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();
    home.open_manager(&client).await.unwrap();
    home.tmux(&["select-pane", "-t", "=torlek:.1"]);

    let reply = request(&client, focus(ManagerPaneView::Column))
        .await
        .unwrap();

    assert_eq!(
        (reply, focused_pane(&home)),
        (
            Reply::ManagerFocused {
                took_from_viewer: true
            },
            "0".to_owned()
        )
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn manager_focus_on_the_column_that_has_the_keys_says_the_viewer_had_none() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();
    home.open_manager(&client).await.unwrap();

    let reply = request(&client, focus(ManagerPaneView::Column))
        .await
        .unwrap();

    assert_eq!(
        reply,
        Reply::ManagerFocused {
            took_from_viewer: false
        }
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn manager_focus_gives_the_viewer_its_keys_back() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();
    home.open_manager(&client).await.unwrap();

    request(&client, focus(ManagerPaneView::Viewer))
        .await
        .unwrap();

    assert_eq!(focused_pane(&home), "1");
}

#[tokio::test(flavor = "multi_thread")]
async fn manager_focus_before_the_manager_is_open_is_refused_naming_the_fix() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();

    let error = refusal(request(&client, focus(ManagerPaneView::Column)).await);

    assert_eq!(error.code, ErrorCode::NotFound);
    assert!(
        error.message.contains("run `torlek tui`"),
        "{}",
        error.message
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn showing_an_asleep_session_wakes_it_and_attaches_the_viewer_to_it() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();
    let root = home.project(&client).await;
    home.open_manager(&client).await.unwrap();
    let started = session(
        &client,
        Command::SessionNew {
            dir: root.display().to_string(),
            agent: Some("fake".to_owned()),
            prompt: None,
            clone: None,
            model: None,
            effort: None,
        },
    )
    .await;
    session(
        &client,
        Command::SessionSleep {
            session: started.id.clone(),
        },
    )
    .await;

    let shown = session(
        &client,
        Command::SessionShow {
            session: started.id.clone(),
        },
    )
    .await;

    assert_eq!(shown.state, SessionStateView::Running);
    assert_eq!(shown.launch.unwrap().kind, LaunchKindView::Wake);
    eventually("a client in the viewer is attached to the session", || {
        home.tmux(&["list-clients", "-F", "#{session_name}"])
            .lines()
            .any(|attached| attached == started.id)
    });
}

#[tokio::test(flavor = "multi_thread")]
async fn showing_a_session_before_the_manager_is_open_is_refused_naming_the_fix() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();
    let root = home.project(&client).await;
    let started = session(
        &client,
        Command::SessionNew {
            dir: root.display().to_string(),
            agent: Some("fake".to_owned()),
            prompt: None,
            clone: None,
            model: None,
            effort: None,
        },
    )
    .await;

    let error = refusal(
        request(
            &client,
            Command::SessionShow {
                session: started.id,
            },
        )
        .await,
    );

    assert_eq!(error.code, ErrorCode::NotFound);
    assert!(
        error.message.contains("run `torlek tui`"),
        "{}",
        error.message
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn showing_a_running_session_whose_tmux_session_is_gone_names_the_missing_terminal() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();
    let root = home.project(&client).await;
    home.open_manager(&client).await.unwrap();
    let started = session(
        &client,
        Command::SessionNew {
            dir: root.display().to_string(),
            agent: Some("fake".to_owned()),
            prompt: None,
            clone: None,
            model: None,
            effort: None,
        },
    )
    .await;
    home.tmux(&["kill-session", "-t", &format!("={}", started.id)]);

    let error = refusal(
        request(
            &client,
            Command::SessionShow {
                session: started.id.clone(),
            },
        )
        .await,
    );

    assert_eq!(error.code, ErrorCode::Conflict, "{}", error.message);
    assert!(
        error.message.contains(&started.id) && error.message.contains("terminal is gone"),
        "{}",
        error.message
    );
    let ended = session(
        &client,
        Command::SessionEnd {
            session: started.id.clone(),
        },
    )
    .await;
    assert_eq!(ended.state, SessionStateView::Ended);
    let resumed = session(
        &client,
        Command::SessionResume {
            session: started.id,
        },
    )
    .await;
    assert_eq!(resumed.state, SessionStateView::Running);
}

#[tokio::test(flavor = "multi_thread")]
async fn showing_a_running_session_after_the_whole_tmux_server_is_gone_is_not_internal() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();
    let root = home.project(&client).await;
    home.open_manager(&client).await.unwrap();
    let started = session(
        &client,
        Command::SessionNew {
            dir: root.display().to_string(),
            agent: Some("fake".to_owned()),
            prompt: None,
            clone: None,
            model: None,
            effort: None,
        },
    )
    .await;
    home.tmux(&["kill-server"]);

    let error = refusal(
        request(
            &client,
            Command::SessionShow {
                session: started.id,
            },
        )
        .await,
    );

    assert_eq!(error.code, ErrorCode::NotFound, "{}", error.message);
    assert!(
        error.message.contains("run `torlek tui`"),
        "{}",
        error.message
    );
}

/// A session started and ended by its owner: it has no terminal any more.
async fn ended_session(client: &Connection, root: &Path) -> SessionView {
    let started = session(
        client,
        Command::SessionNew {
            dir: root.display().to_string(),
            agent: Some("fake".to_owned()),
            prompt: None,
            clone: None,
            model: None,
            effort: None,
        },
    )
    .await;
    session(
        client,
        Command::SessionEnd {
            session: started.id,
        },
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn showing_an_ended_session_describes_it_without_a_manager() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();
    let root = home.project(&client).await;
    let ended = ended_session(&client, &root).await;

    let shown = session(
        &client,
        Command::SessionShow {
            session: ended.id.clone(),
        },
    )
    .await;

    assert_eq!(shown.state, SessionStateView::Ended);
    assert_eq!(shown.reason.as_deref(), Some("ended by the owner"));
}

#[tokio::test(flavor = "multi_thread")]
async fn showing_an_ended_session_leaves_the_viewer_unattached() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();
    let root = home.project(&client).await;
    home.open_manager(&client).await.unwrap();
    let ended = ended_session(&client, &root).await;

    let shown = request(
        &client,
        Command::SessionShow {
            session: ended.id.clone(),
        },
    )
    .await;

    assert!(matches!(shown, Ok(Reply::Session { .. })), "{shown:?}");
    // The viewer's client attaches asynchronously: keep looking for a while.
    let deadline = std::time::Instant::now() + common::SETTLE;
    while std::time::Instant::now() < deadline {
        assert!(
            !home
                .tmux(&["list-clients", "-F", "#{session_name}"])
                .lines()
                .any(|attached| attached == ended.id)
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_may_neither_open_nor_focus_the_manager_nor_list_providers() {
    let home = Home::new();
    let daemon = home.start().await;
    let agent = Connection::open(daemon.agent_socket_path(), GENEROUS)
        .await
        .unwrap();

    let codes = [
        refusal(home.open_manager(&agent).await).code,
        refusal(request(&agent, focus(ManagerPaneView::Column)).await).code,
        refusal(request(&agent, Command::ProviderList).await).code,
    ];

    assert_eq!(
        codes,
        [
            ErrorCode::NotAuthorized,
            ErrorCode::NotAuthorized,
            ErrorCode::NotAuthorized
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn showing_an_asleep_session_without_a_manager_leaves_it_asleep() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();
    let root = home.project(&client).await;
    let started = session(
        &client,
        Command::SessionNew {
            dir: root.display().to_string(),
            agent: Some("fake".to_owned()),
            prompt: None,
            clone: None,
            model: None,
            effort: None,
        },
    )
    .await;
    session(
        &client,
        Command::SessionSleep {
            session: started.id.clone(),
        },
    )
    .await;

    let error = refusal(
        request(
            &client,
            Command::SessionShow {
                session: started.id.clone(),
            },
        )
        .await,
    );

    assert_eq!(error.code, ErrorCode::NotFound);
    let listed = match request(&client, Command::SessionList { ended: false }).await {
        Ok(Reply::Sessions { sessions }) => sessions,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        listed
            .iter()
            .find(|listed| listed.id == started.id)
            .map(|listed| listed.state.clone()),
        Some(SessionStateView::Asleep)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_manager_that_lost_its_viewer_is_built_again_on_the_next_open() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();
    home.open_manager(&client).await.unwrap();
    home.tmux(&["kill-pane", "-t", "=torlek:.1"]);

    let reopened = home.open_manager(&client).await.unwrap();

    assert!(
        matches!(reopened, Reply::Manager { created: true, .. }),
        "{reopened:?}"
    );
    let panes = home.tmux(&["list-panes", "-t", "=torlek:", "-F", "#{pane_id}"]);
    assert_eq!(panes.lines().count(), 2, "{panes}");
}

// The phone layout (#855): `manager-fit` arranges the manager for its window's width, the
// viewer shows a session read-only on the phone, and `session-type` types into it.

fn fit(session_open: bool) -> Command {
    Command::ManagerFit { session_open }
}

async fn fitted(client: &Connection, session_open: bool) -> bool {
    match request(client, fit(session_open)).await {
        Ok(Reply::ManagerFitted { phone }) => phone,
        other => panic!("manager-fit: {other:?}"),
    }
}

/// The owner's terminal, `columns` wide, is the manager window's size.
fn resize_manager(home: &Home, columns: u16) {
    home.tmux(&[
        "resize-window",
        "-t",
        "=torlek:",
        "-x",
        &columns.to_string(),
        "-y",
        "30",
    ]);
}

/// The column's `width height zoomed active`.
fn column_geometry(home: &Home) -> String {
    home.tmux(&[
        "display-message",
        "-p",
        "-t",
        "=torlek:.0",
        "#{pane_width} #{pane_height} #{window_zoomed_flag} #{pane_active}",
    ])
    .trim()
    .to_owned()
}

fn is_attached(home: &Home, session: &str) -> bool {
    home.tmux(&["list-clients", "-F", "#{session_name}"])
        .lines()
        .any(|attached| attached == session)
}

/// Whether tmux drops what is typed into the viewer: its input is off.
fn is_viewer_read_only(home: &Home) -> bool {
    home.tmux(&[
        "display-message",
        "-p",
        "-t",
        "=torlek:.1",
        "#{pane_input_off}",
    ])
    .trim()
        == "1"
}

fn agent_screen(home: &Home, session: &str) -> String {
    home.tmux(&["capture-pane", "-p", "-t", &format!("={session}:")])
}

/// A running session shown in the viewer, the manager `columns` wide and fitted first.
async fn shown_session(home: &Home, client: &Connection, columns: u16) -> SessionView {
    shown_agent(home, client, columns, "fake").await
}

/// [`shown_session`] of the provider `agent`.
async fn shown_agent(home: &Home, client: &Connection, columns: u16, agent: &str) -> SessionView {
    let root = home.project(client).await;
    home.open_manager(client).await.unwrap();
    resize_manager(home, columns);
    fitted(client, false).await;
    let started = session(
        client,
        Command::SessionNew {
            dir: root.display().to_string(),
            agent: Some(agent.to_owned()),
            prompt: None,
            clone: None,
            model: None,
            effort: None,
        },
    )
    .await;
    session(
        client,
        Command::SessionShow {
            session: started.id.clone(),
        },
    )
    .await;
    eventually("the viewer's client is attached to the session", || {
        is_attached(home, &started.id)
    });
    started
}

#[tokio::test(flavor = "multi_thread")]
async fn a_desktop_wide_manager_keeps_its_layout_on_fit() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();
    home.open_manager(&client).await.unwrap();
    let before = column_geometry(&home);

    let phone = fitted(&client, false).await;

    assert!(!phone);
    assert_eq!(column_geometry(&home), before);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_manager_narrower_than_sixty_columns_shows_the_column_alone_with_the_keys() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();
    home.open_manager(&client).await.unwrap();
    home.tmux(&["select-pane", "-t", "=torlek:.1"]);
    resize_manager(&home, 45);

    let phone = fitted(&client, false).await;

    assert!(phone);
    assert_eq!(column_geometry(&home), "45 30 1 1");
}

#[tokio::test(flavor = "multi_thread")]
async fn on_the_phone_a_session_is_shown_read_only_below_a_bar_of_the_column() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();
    shown_session(&home, &client, 45).await;

    let phone = fitted(&client, true).await;

    assert!(phone);
    assert_eq!(column_geometry(&home), "45 3 0 1");
    assert!(is_viewer_read_only(&home));
}

/// The owner's terminal on the manager, a control-mode client attached while it lives, typing
/// into the viewer the way a tap and a key on the phone would.
struct OwnerTerminal {
    process: std::process::Child,
    name: String,
}

impl OwnerTerminal {
    fn attach(home: &Home) -> OwnerTerminal {
        let process = Process::new("tmux")
            .arg("-S")
            .arg(home.path().join("tmux.sock"))
            .args(["-C", "attach-session", "-t", "=torlek"])
            .env_remove("TMUX")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let mut name = String::new();
        eventually("the owner's terminal is attached to the manager", || {
            name = home
                .tmux(&["list-clients", "-t", "=torlek", "-F", "#{client_name}"])
                .trim()
                .to_owned();
            !name.is_empty()
        });
        OwnerTerminal { process, name }
    }

    fn type_into_viewer(&self, home: &Home, text: &str) {
        for keys in [["-l", text], ["Enter", ""]] {
            let mut arguments = vec!["send-keys", "-c", &self.name, "-t", "=torlek:.1"];
            arguments.extend(keys.into_iter().filter(|key| !key.is_empty()));
            home.tmux(&arguments);
        }
    }
}

impl Drop for OwnerTerminal {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn on_the_phone_keys_typed_into_the_viewer_reach_no_pane_and_session_type_does() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();
    let started = shown_session(&home, &client, 45).await;
    fitted(&client, true).await;
    let owner = OwnerTerminal::attach(&home);
    // The owner taps the session: tmux gives the viewer the keys, as a click would.
    home.tmux(&["select-pane", "-t", "=torlek:.1"]);

    owner.type_into_viewer(&home, "stray-keystrokes");
    request(
        &client,
        Command::SessionType {
            session: started.id.clone(),
            text: "typed-after-the-switch".to_owned(),
        },
    )
    .await
    .unwrap();

    eventually("the typed line reaches the agent's terminal", || {
        agent_screen(&home, &started.id).contains("typed-after-the-switch")
    });
    let screen = agent_screen(&home, &started.id);
    assert!(!screen.contains("stray-keystrokes"), "{screen}");
}

#[tokio::test(flavor = "multi_thread")]
async fn on_the_phone_a_session_takes_the_size_of_the_viewer_below_the_bar_as_it_is_shown() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();

    let started = shown_session(&home, &client, 45).await;

    assert_eq!(column_geometry(&home), "45 3 0 1");
    let window = home.tmux(&[
        "display-message",
        "-p",
        "-t",
        &format!("={}:", started.id),
        "#{window_width} #{window_height}",
    ]);
    assert_eq!(window.trim(), "45 26");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_desktop_fit_undoes_a_phone_layout_the_daemon_does_not_remember() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();
    home.open_manager(&client).await.unwrap();
    resize_manager(&home, 200);
    // What a daemon that stopped on the phone leaves in tmux, which outlives it.
    home.tmux(&["resize-pane", "-Z", "-t", "=torlek:.0"]);
    home.tmux(&["select-pane", "-d", "-t", "=torlek:.1"]);

    let phone = fitted(&client, false).await;

    assert!(!phone);
    assert_eq!(column_geometry(&home), "60 30 0 1");
    assert!(!is_viewer_read_only(&home));
}

#[tokio::test(flavor = "multi_thread")]
async fn on_the_desktop_a_session_is_shown_read_write() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();

    shown_session(&home, &client, 200).await;

    assert!(!is_viewer_read_only(&home));
}

#[tokio::test(flavor = "multi_thread")]
async fn widening_the_phone_manager_puts_the_desktop_layout_back_and_the_viewer_read_write() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();
    let started = shown_session(&home, &client, 45).await;
    fitted(&client, true).await;
    resize_manager(&home, 200);

    let phone = fitted(&client, true).await;

    assert!(!phone);
    assert_eq!(column_geometry(&home), "60 30 0 1");
    assert!(!is_viewer_read_only(&home));
    eventually(
        "the viewer's client is attached to the session again",
        || is_attached(&home, &started.id),
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn session_type_types_the_line_into_the_session_on_screen() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();
    let started = shown_session(&home, &client, 45).await;
    fitted(&client, true).await;

    let typed = request(
        &client,
        Command::SessionType {
            session: started.id.clone(),
            text: "typed-from-the-phone".to_owned(),
        },
    )
    .await
    .unwrap();

    assert_eq!(
        typed,
        Reply::SessionAction {
            session: started.id.clone(),
            held: Vec::new(),
        }
    );
    eventually("the line reaches the agent's terminal", || {
        agent_screen(&home, &started.id).contains("typed-from-the-phone")
    });
}

#[tokio::test(flavor = "multi_thread")]
async fn session_type_into_a_session_not_on_screen_is_refused() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();
    let root = home.project(&client).await;
    home.open_manager(&client).await.unwrap();
    let started = session(
        &client,
        Command::SessionNew {
            dir: root.display().to_string(),
            agent: Some("fake".to_owned()),
            prompt: None,
            clone: None,
            model: None,
            effort: None,
        },
    )
    .await;

    let error = refusal(
        request(
            &client,
            Command::SessionType {
                session: started.id.clone(),
                text: "never typed".to_owned(),
            },
        )
        .await,
    );

    assert_eq!(error.code, ErrorCode::Conflict);
    assert!(error.message.contains("open it"), "{}", error.message);
    assert!(!agent_screen(&home, &started.id).contains("never typed"));
}

const DESKTOP: &[(&str, &str)] = &[("WAYLAND_DISPLAY", "wayland-test")];
/// The manager's width as built: the desktop layout, the viewer taking keys.
const DESKTOP_COLUMNS: u16 = 200;

async fn paste_image(client: &Connection) -> Result<Reply, ClientError> {
    request(client, Command::SessionPasteImage).await
}

/// The image path a `session-paste-image` answered for `session`.
fn pasted_path(reply: Reply, session: &SessionView) -> PathBuf {
    match reply {
        Reply::ClipboardImage {
            session: pasted,
            path: Some(path),
        } if pasted == session.id => PathBuf::from(path),
        other => panic!(
            "expected an image pasted into {}, got {other:?}",
            session.id
        ),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pasted_image_is_the_clipboards_png_saved_privately_in_the_sessions_temp_folder() {
    let home = Home::new();
    let daemon = home
        .start_with_clipboard(Clipboard::Image(PNG), DESKTOP)
        .await;
    let client = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();
    let started = shown_session(&home, &client, DESKTOP_COLUMNS).await;

    let path = pasted_path(paste_image(&client).await.unwrap(), &started);

    assert_eq!(std::fs::read(&path).unwrap(), PNG);
    let mode = std::fs::metadata(&path).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600, "{path:?}");
    let folder = path.parent().and_then(Path::file_name).unwrap();
    assert_eq!(folder, started.id.as_str(), "{path:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pasted_images_path_reaches_the_session_on_screen() {
    let home = Home::new();
    let daemon = home
        .start_with_clipboard(Clipboard::Image(PNG), DESKTOP)
        .await;
    let client = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();
    let started = shown_session(&home, &client, DESKTOP_COLUMNS).await;

    let path = pasted_path(paste_image(&client).await.unwrap(), &started);

    let path = path.display().to_string();
    eventually("the image's path reaches the agent's terminal", || {
        agent_screen(&home, &started.id)
            .replace('\n', "")
            .contains(&path)
    });
}

#[tokio::test(flavor = "multi_thread")]
async fn without_an_image_on_the_clipboard_the_key_goes_on_to_the_session_on_screen() {
    for clipboard in [Clipboard::Empty, Clipboard::Text] {
        let home = Home::new();
        let daemon = home.start_with_clipboard(clipboard, DESKTOP).await;
        let client = Connection::open(daemon.socket_path(), GENEROUS)
            .await
            .unwrap();
        let started = shown_agent(&home, &client, DESKTOP_COLUMNS, "keys").await;

        let reply = paste_image(&client).await.unwrap();

        assert_eq!(
            reply,
            Reply::ClipboardImage {
                session: started.id.clone(),
                path: None,
            }
        );
        eventually("Ctrl-V reaches the agent", || {
            home.first_key() == Some(vec![0x16])
        });
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn pasting_an_image_without_the_owners_wayland_display_is_refused_naming_the_import() {
    let home = Home::new();
    let daemon = home.start_with_clipboard(Clipboard::Image(PNG), &[]).await;
    let client = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();
    shown_session(&home, &client, DESKTOP_COLUMNS).await;

    let error = refusal(paste_image(&client).await);

    assert_eq!(error.code, ErrorCode::Refused, "{error:?}");
    assert!(
        error.message.contains("torlek desktop-session import"),
        "{}",
        error.message
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_clipboard_image_that_is_not_a_png_is_refused_and_nothing_is_pasted() {
    let home = Home::new();
    let daemon = home
        .start_with_clipboard(Clipboard::Image(b"GIF89a-not-a-png"), DESKTOP)
        .await;
    let client = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();
    let started = shown_session(&home, &client, DESKTOP_COLUMNS).await;

    let error = refusal(paste_image(&client).await);

    assert_eq!(error.code, ErrorCode::Refused, "{error:?}");
    assert!(error.message.contains("not a PNG"), "{}", error.message);
    assert!(!agent_screen(&home, &started.id).contains(".png"));
}

#[tokio::test(flavor = "multi_thread")]
async fn pasting_an_image_with_no_session_on_screen_is_refused() {
    let home = Home::new();
    let daemon = home
        .start_with_clipboard(Clipboard::Image(PNG), DESKTOP)
        .await;
    let client = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();
    home.open_manager(&client).await.unwrap();

    let error = refusal(paste_image(&client).await);

    assert_eq!(error.code, ErrorCode::Conflict, "{error:?}");
    assert!(error.message.contains("open"), "{}", error.message);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_manager_opened_with_an_image_paste_command_binds_ctrl_v_to_it() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();

    request(
        &client,
        Command::ManagerOpen {
            dir: home.path().display().to_string(),
            column: vec![
                "/bin/sh".to_owned(),
                "-c".to_owned(),
                "sleep 600".to_owned(),
                "column".to_owned(),
            ],
            viewer: vec!["/bin/sleep".to_owned(), "601".to_owned()],
            image_paste: vec!["/bin/true".to_owned(), "paste-image".to_owned()],
        },
    )
    .await
    .unwrap();

    let binding = home.tmux(&["list-keys", "-T", "root", "C-v"]);
    assert!(binding.contains("run-shell"), "{binding}");
    let script = std::fs::read_to_string(home.path().join("image-paste")).unwrap();
    assert!(script.contains("'/bin/true' 'paste-image'"), "{script}");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_may_neither_fit_the_manager_nor_type_nor_paste_into_a_session() {
    let home = Home::new();
    let daemon = home.start().await;
    let agent = Connection::open(daemon.agent_socket_path(), GENEROUS)
        .await
        .unwrap();

    for command in [
        fit(false),
        Command::SessionType {
            session: "s000000000001".to_owned(),
            text: "x".to_owned(),
        },
        Command::SessionPasteImage,
    ] {
        let error = refusal(request(&agent, command).await);
        assert_eq!(error.code, ErrorCode::NotAuthorized, "{error:?}");
    }
}

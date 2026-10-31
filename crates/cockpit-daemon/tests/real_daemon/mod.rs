//! The `cockpit-daemon` binary in a temporary home, for fixtures that kill the real process
//! (#36). Claude Code on its `PATH` is a shell script that logs each launch and then acts as
//! the directory it runs in says (see [`FAKE_CLAUDE`]); tmux is the real one, on a private
//! server under `TMUX_TMPDIR` inside the home, never the user's.
#![allow(
    dead_code,
    reason = "each test file uses a different part of the fixture"
)]

use std::io::{BufRead, BufReader};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command as Process, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use cockpit_client::Connection;
use cockpit_protocol::{Command, Reply, SessionStateView, SessionView};

/// Long enough never to fire in a passing test, short enough to fail a hung one.
pub const GENEROUS: Duration = Duration::from_secs(10);
/// How long a test waits after a start for a launch the daemon should not make; the fake
/// agent logs its launch within milliseconds of being started.
pub const SETTLE: Duration = Duration::from_secs(1);
/// A start reconciles what the killed daemon left before it is ready (report 03), and
/// stopping an agent that ignores `SIGTERM` takes the stop's grace and kill wait (5 s each);
/// a session's stop or launch under way is waited for as long.
const SETTLED_WITHIN: Duration = Duration::from_secs(30);

/// A project of this name runs an agent that writes to its terminal without pause.
pub const FLOOD_PROJECT: &str = "flood";
/// A project of this name runs an agent that ignores `SIGTERM` and never ends on its own.
pub const STALL_PROJECT: &str = "stall";

/// The agent acts as the last component of its working directory says ([`FLOOD_PROJECT`],
/// [`STALL_PROJECT`]); anything else sleeps. `--version` answers the version the adapter's
/// conformance tests ran on.
const FAKE_CLAUDE: &str = r#"#!/bin/sh
if [ "$1" = --version ]; then echo '2.1.316 (Claude Code)'; exit 0; fi
printf '%s pid=%s\n' "$COCKPIT_SESSION" "$$" >> '@LOG@'
case "${PWD##*/}" in
  @FLOOD@) exec yes 'cockpit latency fixture: one build log line after another, without pause' ;;
  @STALL@) trap '' TERM; while :; do sleep 1; done ;;
  *) exec sleep 3600 ;;
esac
"#;

pub struct Home {
    /// The temporary directory, canonical: the daemon compares paths it is sent with it.
    root: PathBuf,
    _dir: tempfile::TempDir,
}

/// One launch of the fake agent, as it logged it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub session: String,
    pub pid: i32,
}

impl Home {
    pub fn new() -> Home {
        let dir = tempfile::tempdir().expect("a temporary home");
        let home = Home {
            root: dir.path().canonicalize().unwrap(),
            _dir: dir,
        };
        for dir in ["run", "tmux", "bin"] {
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(home.path().join(dir))
                .expect("the home's directories are creatable");
        }
        let claude = home.path().join("bin/claude");
        let log = home.launch_log();
        std::fs::write(
            &claude,
            FAKE_CLAUDE
                .replace("@LOG@", log.to_str().expect("a UTF-8 path"))
                .replace("@FLOOD@", FLOOD_PROJECT)
                .replace("@STALL@", STALL_PROJECT),
        )
        .unwrap();
        std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755)).unwrap();
        home
    }

    pub fn path(&self) -> PathBuf {
        self.root.clone()
    }

    fn launch_log(&self) -> PathBuf {
        self.path().join("launches.log")
    }

    pub fn socket_path(&self) -> PathBuf {
        self.path()
            .join("run/cockpit")
            .join(cockpit_config::DAEMON_SOCKET)
    }

    /// Every launch the fake agent logged, oldest first.
    pub fn launches(&self) -> Vec<Launch> {
        std::fs::read_to_string(self.launch_log())
            .unwrap_or_default()
            .lines()
            .map(|line| {
                let (session, pid) = line
                    .split_once(" pid=")
                    .unwrap_or_else(|| panic!("a launch line: {line:?}"));
                Launch {
                    session: session.to_owned(),
                    pid: pid.parse().unwrap(),
                }
            })
            .collect()
    }

    pub fn launches_of(&self, session: &str) -> Vec<Launch> {
        self.launches()
            .into_iter()
            .filter(|launch| launch.session == session)
            .collect()
    }

    /// Waits until `session`'s agent logged `count` launches and returns them.
    pub async fn wait_for_launches(&self, session: &str, count: usize) -> Vec<Launch> {
        let deadline = Instant::now() + GENEROUS;
        loop {
            let launches = self.launches_of(session);
            if launches.len() >= count {
                return launches;
            }
            assert!(
                Instant::now() < deadline,
                "session {session} logged {launches:?}, not {count} launches"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Every daemon's standard error, appended in the order they started.
    fn stderr_log(&self) -> PathBuf {
        self.path().join("daemon.stderr")
    }

    /// Starts the daemon binary and waits for its `ready` line. Its standard output is read
    /// to the end and its standard error goes to a file, so neither pipe fills and blocks it.
    pub fn start(&self) -> Daemon {
        let path = std::env::var_os("PATH").unwrap_or_default();
        let mut search = vec![self.path().join("bin")];
        search.extend(std::env::split_paths(&path));
        let mut child = Process::new(env!("CARGO_BIN_EXE_cockpit-daemon"))
            .env_clear()
            .env("HOME", self.path())
            .env("XDG_RUNTIME_DIR", self.path().join("run"))
            .env("XDG_STATE_HOME", self.path().join("state"))
            .env("XDG_CONFIG_HOME", self.path().join("config"))
            .env("TMUX_TMPDIR", self.path().join("tmux"))
            .env("PATH", std::env::join_paths(search).unwrap())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(self.stderr_log())
                    .expect("the daemon's standard error file opens"),
            )
            .spawn()
            .expect("the daemon binary runs");
        let stdout = child.stdout.take().expect("stdout is piped");
        let (lines, ready) = mpsc::channel();
        std::thread::spawn(move || {
            let mut stdout = BufReader::new(stdout).lines();
            let _ = lines.send(stdout.next().and_then(Result::ok).unwrap_or_default());
            stdout.for_each(drop);
        });
        let line = ready.recv_timeout(SETTLED_WITHIN).unwrap_or_default();
        let daemon = Daemon { child };
        if line.trim() != format!("ready {}", self.socket_path().display()) {
            drop(daemon);
            let stderr = std::fs::read_to_string(self.stderr_log()).unwrap_or_default();
            panic!("the daemon did not report ready: stdout {line:?}, stderr {stderr:?}");
        }
        daemon
    }

    pub async fn connect(&self) -> Connection {
        Connection::open(&self.socket_path(), GENEROUS)
            .await
            .expect("the daemon accepts the handshake")
    }

    /// A Git repository at `projects/<name>` registered through the daemon; returns its
    /// root and the project's name.
    pub async fn project(&self, client: &Connection, name: &str) -> (PathBuf, String) {
        let root = self.path().join("projects").join(name);
        std::fs::create_dir_all(&root).unwrap();
        git(&root, &["init", "--quiet", "--initial-branch=main"]);
        std::fs::write(root.join("README"), "fixture\n").unwrap();
        git(&root, &["add", "."]);
        git(&root, &["commit", "--quiet", "-m", "initial"]);
        let added = client
            .request(
                Command::ProjectAdd {
                    path: root.display().to_string(),
                    name: None,
                },
                GENEROUS,
            )
            .await;
        match added {
            Ok(Reply::ProjectAdded { project, .. }) => (root, project.name),
            other => panic!("project-add {name}: {other:?}"),
        }
    }
}

impl Drop for Home {
    /// The agents live in the private tmux server, which outlives every daemon.
    fn drop(&mut self) {
        let _ = Process::new("tmux")
            .env_remove("TMUX")
            .env("TMUX_TMPDIR", self.path().join("tmux"))
            .args(["-L", "cockpit", "kill-server"])
            .output();
    }
}

pub struct Daemon {
    child: Child,
}

impl Daemon {
    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// `kill -9`: no cleanup runs, as after a crash. The agents keep running in tmux.
    pub fn kill(mut self) {
        self.child.kill().expect("the daemon is killable");
        self.child.wait().expect("the daemon is reaped");
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn git(dir: &Path, arguments: &[&str]) {
    let output = Process::new("git")
        .current_dir(dir)
        .args(["-c", "user.name=Fixture", "-c", "user.email=f@example.com"])
        .args(arguments)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {arguments:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Starts a session with the fake Claude Code in `dir`, as the owner would.
pub fn session_new_command(dir: &Path) -> Command {
    Command::SessionNew {
        dir: dir.display().to_string(),
        agent: Some("claude".to_owned()),
        prompt: None,
        clone: None,
    }
}

pub async fn new_session(client: &Connection, dir: &Path) -> SessionView {
    let answer = client
        .request(session_new_command(dir), Duration::from_secs(60))
        .await;
    match answer {
        Ok(Reply::Session { session }) => *session,
        other => panic!("session-new in {}: {other:?}", dir.display()),
    }
}

pub async fn listed(client: &Connection, id: &str) -> SessionView {
    let answer = client
        .request(Command::SessionList { ended: true }, GENEROUS)
        .await;
    let Ok(Reply::Sessions { sessions }) = answer else {
        panic!("session-list answered {answer:?}");
    };
    sessions
        .into_iter()
        .find(|session| session.id == id)
        .unwrap_or_else(|| panic!("session {id} is listed"))
}

/// Waits until session `id` is in a state `wanted` accepts.
pub async fn wait_for_state(
    client: &Connection,
    id: &str,
    wanted: impl Fn(&SessionStateView) -> bool,
) -> SessionView {
    let deadline = Instant::now() + SETTLED_WITHIN;
    loop {
        let session = listed(client, id).await;
        if wanted(&session.state) {
            return session;
        }
        assert!(
            Instant::now() < deadline,
            "session {id} stayed {:?}",
            session.state
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Waits until session `id` runs: any stop or launch under way has finished.
pub async fn wait_for_running(client: &Connection, id: &str) -> SessionView {
    wait_for_state(client, id, |state| *state == SessionStateView::Running).await
}

/// Whether `pid` runs and is not a zombie.
pub fn is_alive(pid: i32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| !stat.contains(") Z "))
}

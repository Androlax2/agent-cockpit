//! The harness: a real daemon on a scratch home, the real `cockpit tui` attached in a pane of
//! a test-owned tmux server (the owner's terminal), and a fake Claude Code the daemon
//! launches. Tests press keys into that terminal and read its screen as plain text.
//!
//! Nothing here sleeps to wait. A screen wait blocks on the terminal's own output: a
//! control-mode client of the test's tmux server announces every write the TUI (or an agent
//! in the viewer) makes, and the screen is read again after each one. A state wait blocks
//! on the daemon's change feed. Both give up after [`GENEROUS`], showing the last screen.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::OnceLock;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use cockpit_client::Subscription;
use cockpit_protocol::{Change, EntityKind, FeedPosition, SessionView};

/// Long enough never to fire in a passing run on a loaded CI runner; a hung step fails with
/// its screen instead of hanging the suite.
const GENEROUS: Duration = Duration::from_secs(30);
/// The owner's terminal: wide enough for the list and the viewer side by side.
pub const COLUMNS: u16 = 160;
const ROWS: u16 = 45;
/// The project every test starts with, as `tools/acceptance/run.sh` registers it.
pub const PROJECT: &str = "demo";
/// The first message the form and `session new` send; the fake agent answers it.
pub const FIRST_MESSAGE: &str = "Say hello in one line, then wait.";

/// The fake Claude Code. It answers `--version` with the version the adapter's conformance
/// tests ran on, logs each launch, keeps each conversation's messages in a file of its own
/// (so a resumed conversation shows what came before), reports through the agent channel the
/// way the hook bridge does, and emits OSC 7501 (#293 reads it; nothing asserts it yet).
/// At its `fake>` prompt a line starting with `!` runs as a shell command, the way Claude
/// runs a tool; any other line is a message it answers.
const FAKE_CLAUDE: &str = r#"#!/bin/sh
if [ "$1" = --version ]; then echo '2.1.316 (Claude Code)'; exit 0; fi
conversation= mode= message=
while [ $# -gt 0 ]; do
  case $1 in
    --session-id) conversation=$2; mode=new; shift 2 ;;
    --resume) conversation=$2; mode=resume; shift 2 ;;
    --settings|--effort) shift 2 ;;
    --) message=$2; break ;;
    *) shift ;;
  esac
done
transcript='@CONVERSATIONS@'/$conversation
printf '%s pid=%s %s\n' "$COCKPIT_SESSION" "$$" "$mode" >> '@LAUNCHES@'
# A report before the daemon recorded this launch's process tree is refused (#279): retry
# that refusal only; any other is final and logged for the failure report. Not printed:
# a refusal that wraps cuts the viewer's first screen short.
report() {
  tries=0
  until refusal=$(cockpit agent report "$@" 2>&1 >/dev/null); do
    tries=$((tries + 1))
    case $refusal in
      *"no process tree yet"*) [ "$tries" -lt 100 ] && { sleep 0.1; continue; } ;;
    esac
    echo "report $* refused: $refusal" >> '@REFUSALS@'
    return 1
  done
}
status() { printf '\033]7501;state=%s:app=fake-agent\033\\' "$1"; }
answer() {
  echo "$1" >> "$transcript"
  echo "reply: $1 (first message: $(head -n 1 "$transcript"))"
}
if [ "$mode" = resume ]; then echo "resumed conversation $(wc -l < "$transcript" | tr -d ' ') messages"; fi
if [ -n "$message" ]; then answer "$message"; fi
status idle
report --state waiting
while :; do
  printf 'fake> '
  IFS= read -r line || exit 0
  status working
  report --state working --prompt
  case $line in
    !*) sh -c "${line#!}"; echo "exit $?" ;;
    *) answer "$line" ;;
  esac
  status idle
  report --state waiting
done
"#;

/// The test terminal's tmux configuration: no status bar, and a pane that outlives
/// `cockpit tui` so a failure still shows its last screen. The size is pinned on the one
/// window once it exists (`resize-window`): a global `window-size manual` made tmux 3.6's
/// server exit while spawning it, and without any, macOS's tmux shrank it.
const OUTER_CONFIG: &str = "\
set-option -g status off
set-option -g default-terminal screen
set-option -s escape-time 0
set-option -g remain-on-exit on
";

/// The programs under test, all of the profile this test was built with.
struct Programs {
    cockpit: PathBuf,
    daemon: PathBuf,
    e2e_daemon: PathBuf,
    /// Where `cockpit`, `cockpit-tui` and the daemon are installed side by side.
    dir: PathBuf,
}

/// `cockpit-tui` is this crate's; the others belong to crates the TUI may not depend on
/// (rule `tui -> client/protocol/diagnostics only`), so Cargo built none of them for this
/// test: build them here, a no-op once they are up to date.
fn programs() -> &'static Programs {
    static PROGRAMS: OnceLock<Programs> = OnceLock::new();
    PROGRAMS.get_or_init(|| {
        let tui = Path::new(env!("CARGO_BIN_EXE_cockpit-tui"));
        let dir = tui.parent().expect("a profile directory").to_path_buf();
        let profile = match dir.file_name().and_then(|name| name.to_str()) {
            Some("debug") => "dev",
            Some(name) => name,
            None => panic!("unexpected binary path {}", tui.display()),
        };
        let built = Command::new(env!("CARGO"))
            .args(["build", "--locked", "--quiet", "--profile", profile])
            .args(["--package", "cockpit-cli", "--bin", "cockpit"])
            .args(["--package", "cockpit-daemon", "--bin", "cockpit-daemon"])
            .args(["--example", "cockpit-e2e-daemon"])
            .args([
                "--package",
                "cockpit-claude",
                "--bin",
                "cockpit-claude-hook",
            ])
            .arg("--manifest-path")
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.toml"))
            .output()
            .expect("cargo runs");
        assert!(
            built.status.success(),
            "building the programs under test failed: {}",
            String::from_utf8_lossy(&built.stderr)
        );
        Programs {
            cockpit: dir.join("cockpit"),
            daemon: dir.join("cockpit-daemon"),
            e2e_daemon: dir.join("examples/cockpit-e2e-daemon"),
            dir,
        }
    })
}

/// Which daemon a test runs.
#[derive(Clone, Copy, Debug)]
pub enum DaemonKind {
    /// The product's `cockpit-daemon`.
    Product,
    /// The tests' daemon: an idle session sleeps after `idle_seconds`, checked every
    /// `tick_ms`.
    ShortIdle { idle_seconds: u64, tick_ms: u64 },
}

/// A scratch home with a daemon, the TUI attached in the test's terminal, and the fake agent.
pub struct Cockpit {
    root: PathBuf,
    kind: DaemonKind,
    daemon: Option<Child>,
    socket: PathBuf,
    terminal: Option<Terminal>,
    _dir: tempfile::TempDir,
}

impl Cockpit {
    /// The product daemon on a fresh home with project `demo`, and the TUI attached.
    pub fn start() -> Cockpit {
        Cockpit::start_with(DaemonKind::Product)
    }

    pub fn start_with(kind: DaemonKind) -> Cockpit {
        // Under /tmp: the daemon's and tmux's sockets live in the home, and macOS's own
        // temporary directory is too long a path for them.
        let dir = tempfile::Builder::new()
            .prefix("cockpit-e2e-")
            .tempdir_in("/tmp")
            .expect("a scratch home");
        let root = dir.path().canonicalize().expect("a canonical scratch home");
        for private in ["run", "tmux", "config", "state", "cache"] {
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(root.join(private))
                .expect("the home's directories are creatable");
        }
        for plain in ["bin", "conversations", "projects"] {
            std::fs::create_dir(root.join(plain)).unwrap();
        }
        // The owner's one opt-out of the sandbox (#61): these tests drive the TUI, not the
        // sandbox, and must not depend on whether the host has Greywall (CI does not; a
        // developer's would sandbox the fake agent for real).
        std::fs::create_dir(root.join("config/cockpit")).unwrap();
        std::fs::write(
            root.join("config/cockpit/config.toml"),
            "[agents]\nsandbox = \"off\"\n",
        )
        .unwrap();
        let utf8 = |path: PathBuf| path.to_str().expect("a UTF-8 path").to_owned();
        let fake = root.join("bin/claude");
        std::fs::write(
            &fake,
            FAKE_CLAUDE
                .replace("@CONVERSATIONS@", &utf8(root.join("conversations")))
                .replace("@LAUNCHES@", &utf8(root.join("launches.log")))
                .replace("@REFUSALS@", &utf8(root.join("refusals.log"))),
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut cockpit = Cockpit {
            socket: PathBuf::new(),
            root,
            kind,
            daemon: None,
            terminal: None,
            _dir: dir,
        };
        cockpit.start_daemon();
        cockpit.add_project();
        cockpit.attach();
        cockpit
    }

    pub fn project_root(&self) -> PathBuf {
        self.root.join("projects").join(PROJECT)
    }

    /// The scratch layout every program here runs with, and nothing of the caller's.
    fn environment(&self) -> Vec<(&'static str, std::ffi::OsString)> {
        let mut path = vec![self.root.join("bin"), programs().dir.clone()];
        path.extend(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        ));
        vec![
            ("HOME", self.root.clone().into()),
            ("PATH", std::env::join_paths(path).unwrap()),
            ("XDG_RUNTIME_DIR", self.root.join("run").into()),
            ("XDG_STATE_HOME", self.root.join("state").into()),
            ("XDG_CONFIG_HOME", self.root.join("config").into()),
            ("XDG_CACHE_HOME", self.root.join("cache").into()),
            ("TMUX_TMPDIR", self.root.join("tmux").into()),
            // tmux draws the TUI's borders only in a UTF-8 locale.
            ("LANG", UTF8_LOCALE.into()),
        ]
    }

    fn command(&self, program: &Path) -> Command {
        let mut command = Command::new(program);
        command
            .env_clear()
            .envs(self.environment())
            .current_dir(self.root.join("projects"));
        command
    }

    /// Starts the daemon and waits for its `ready` line. Its standard error goes to a file,
    /// shown when a wait fails.
    pub fn start_daemon(&mut self) {
        assert!(self.daemon.is_none(), "the daemon already runs");
        let mut command = match self.kind {
            DaemonKind::Product => self.command(&programs().daemon),
            DaemonKind::ShortIdle {
                idle_seconds,
                tick_ms,
            } => {
                let mut command = self.command(&programs().e2e_daemon);
                command.args([
                    "--idle-sleep-after-s",
                    &idle_seconds.to_string(),
                    "--session-tick-ms",
                    &tick_ms.to_string(),
                ]);
                command
            }
        };
        let mut child = command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(self.root.join("daemon.stderr"))
                    .unwrap(),
            )
            .spawn()
            .expect("the daemon runs");
        let stdout = child.stdout.take().expect("stdout is piped");
        let (lines, ready) = mpsc::channel();
        std::thread::spawn(move || {
            let mut stdout = BufReader::new(stdout).lines();
            let _ = lines.send(stdout.next().and_then(Result::ok).unwrap_or_default());
            stdout.for_each(drop);
        });
        let line = ready.recv_timeout(GENEROUS).unwrap_or_default();
        self.daemon = Some(child);
        match line.strip_prefix("ready ") {
            Some(socket) => self.socket = PathBuf::from(socket.trim()),
            None => panic!(
                "the daemon did not report ready: stdout {line:?}, stderr {}",
                self.failure_context()
            ),
        }
    }

    /// `kill -9`: no cleanup runs, as after a crash. Its sessions keep running in tmux.
    pub fn kill_daemon(&mut self) {
        let mut daemon = self.daemon.take().expect("the daemon runs");
        daemon.kill().expect("the daemon is killable");
        daemon.wait().expect("the daemon is reaped");
    }

    /// What a failed wait shows after the screen: the daemon's stderr, the end of its log and
    /// of the TUI's (which keys it took), and the fake agent's launches.
    fn failure_context(&self) -> String {
        let read = |path: &Path| {
            String::from_utf8_lossy(&std::fs::read(path).unwrap_or_default()).into_owned()
        };
        let mut report = read(&self.root.join("daemon.stderr"));
        for process in ["daemon", "tui"] {
            let logs = self.root.join("state/cockpit/logs").join(process);
            for entry in std::fs::read_dir(&logs).into_iter().flatten().flatten() {
                let text = read(&entry.path());
                let lines: Vec<&str> = text.lines().collect();
                let tail = &lines[lines.len().saturating_sub(30)..];
                report += &format!("\n--- {} ---\n{}", entry.path().display(), tail.join("\n"));
            }
        }
        report
            + "\n--- launches ---\n"
            + &read(&self.root.join("launches.log"))
            + "\n--- the agents' refused reports ---\n"
            + &read(&self.root.join("refusals.log"))
            + &self.agent_panes()
    }

    /// Each window of the daemon's tmux with its size and its pane's history: what the
    /// agents printed, whether or not the viewer shows it. Never panics, as it reports one.
    fn agent_panes(&self) -> String {
        let private_tmux = |arguments: &[&str]| {
            self.command(Path::new("tmux"))
                .args(["-L", "cockpit"])
                .args(arguments)
                .output()
                .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
                .unwrap_or_else(|error| format!("tmux did not run: {error}\n"))
        };
        let windows = private_tmux(&[
            "list-windows",
            "-a",
            "-F",
            "#{session_name}:#{window_index} #{window_width}x#{window_height}",
        ]);
        let mut report = format!("\n--- the daemon's windows ---\n{windows}");
        for window in windows.lines() {
            let target = window.split(' ').next().unwrap_or_default();
            let history = private_tmux(&["capture-pane", "-p", "-S", "-60", "-t", target]);
            report += &format!("--- {target} ---\n{}\n", history.trim_end());
        }
        report
    }

    /// Runs `cockpit <arguments>` from the projects directory and returns its stdout,
    /// failing the test unless it succeeds.
    pub fn cockpit(&self, arguments: &[&str]) -> String {
        let output = self
            .command(&programs().cockpit)
            .args(arguments)
            .stdin(Stdio::null())
            .output()
            .expect("cockpit runs");
        assert!(
            output.status.success(),
            "cockpit {arguments:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    /// `cockpit session new demo --prompt FIRST_MESSAGE`: a session started from the CLI;
    /// returns its id.
    pub fn session_from_cli(&self) -> String {
        let line = self.cockpit(&["session", "new", PROJECT, "--prompt", FIRST_MESSAGE]);
        line.split('\t')
            .next()
            .filter(|id| id.starts_with('s'))
            .unwrap_or_else(|| panic!("a session id first: {line:?}"))
            .to_owned()
    }

    fn add_project(&self) {
        let root = self.project_root();
        std::fs::create_dir_all(&root).unwrap();
        git(&root, &["init", "--quiet", "--initial-branch=main"]);
        std::fs::write(root.join("README.md"), "# demo\n").unwrap();
        git(&root, &["add", "README.md"]);
        git(&root, &["commit", "--quiet", "-m", "Scratch project"]);
        self.cockpit(&["project", "add", root.to_str().unwrap(), "--name", PROJECT]);
    }

    /// `cockpit tui` in the test's terminal, as the owner runs it, then waits for the list.
    fn attach(&mut self) {
        let socket = self.root.join("terminal.sock");
        let config = self.root.join("terminal.conf");
        std::fs::write(&config, OUTER_CONFIG).unwrap();
        let tmux = |arguments: &[&str]| {
            let mut command = self.outer_tmux(&socket, &config);
            command.args(arguments);
            command
        };
        let (columns, rows) = (COLUMNS.to_string(), ROWS.to_string());
        // `cockpit tui` refuses to nest inside the terminal's own tmux, as it would for an
        // owner: their terminal is not a tmux pane.
        let new_session = [
            "new-session",
            "-d",
            "-s",
            "owner",
            "-x",
            &columns,
            "-y",
            &rows,
            "--",
            "env",
            "-u",
            "TMUX",
            "-u",
            "TMUX_PANE",
            programs().cockpit.to_str().unwrap(),
            "tui",
        ];
        let started = tmux(&new_session).output().expect("tmux runs");
        if !started.status.success() {
            panic!(
                "the test terminal did not start: {}{}",
                String::from_utf8_lossy(&started.stderr),
                self.terminal_start_log(&config)
            );
        }
        let resized = tmux(&[
            "resize-window",
            "-t",
            "=owner:",
            "-x",
            &columns,
            "-y",
            &rows,
        ])
        .output()
        .expect("tmux runs");
        assert!(
            resized.status.success(),
            "the test terminal kept its size: {}",
            String::from_utf8_lossy(&resized.stderr)
        );
        let mut watcher = tmux(&["-C", "attach-session", "-t", "=owner", "-f", "ignore-size"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("a control-mode client starts");
        let written = announced_writes(watcher.stdout.take().expect("stdout is piped"));
        self.terminal = Some(Terminal {
            socket,
            config,
            _watcher_input: watcher.stdin.take(),
            watcher,
            written,
        });
        self.wait_for(PROJECT);
    }

    fn terminal(&self) -> &Terminal {
        self.terminal.as_ref().expect("the TUI is attached")
    }

    /// The test terminal's screen: what the owner sees, as plain text, trailing spaces cut.
    pub fn screen(&self) -> String {
        let terminal = self.terminal();
        let captured = self
            .terminal_tmux(terminal, &["capture-pane", "-p", "-t", "=owner:"])
            .unwrap_or_else(|problem| panic!("cannot read the screen: {problem}"));
        captured
            .lines()
            .map(str::trim_end)
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// For a terminal that did not start: the tmux version, and tmux's own logs of the same
    /// start retried once on a socket of its own, running `true` instead of a second TUI.
    /// Never panics: the original failure stays the message.
    fn terminal_start_log(&self, config: &Path) -> String {
        let logs = self.root.join("terminal-start-logs");
        if let Err(error) = std::fs::create_dir_all(&logs) {
            return format!("\n(no tmux logs: {error})");
        }
        let version = self.command(Path::new("tmux")).arg("-V").output();
        let retried = self
            .outer_tmux(&logs.join("retry.sock"), config)
            .args(["-vv", "new-session", "-d", "-s", "retry", "--", "true"])
            .current_dir(&logs)
            .output();
        let mut report = format!(
            "\n--- tmux -V ---\n{}\n--- retried with -vv ---\n{retried:?}\n",
            version.map_or_else(
                |error| error.to_string(),
                |output| String::from_utf8_lossy(&output.stdout).into_owned()
            ),
        );
        for entry in std::fs::read_dir(&logs).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|extension| extension == "log") {
                // tmux logs the pane's bytes as they came.
                let text =
                    String::from_utf8_lossy(&std::fs::read(&path).unwrap_or_default()).into_owned();
                let lines: Vec<&str> = text.lines().collect();
                let tail = &lines[lines.len().saturating_sub(60)..];
                report += &format!("--- {} ---\n{}\n", path.display(), tail.join("\n"));
            }
        }
        let _ = self
            .outer_tmux(&logs.join("retry.sock"), config)
            .arg("kill-server")
            .output();
        report
    }

    /// tmux on the owner's terminal server.
    fn outer_tmux(&self, socket: &Path, config: &Path) -> Command {
        let mut command = self.command(Path::new("tmux"));
        command.arg("-S").arg(socket).arg("-f").arg(config);
        command
    }

    fn terminal_tmux(&self, terminal: &Terminal, arguments: &[&str]) -> Result<String, String> {
        let output = self
            .outer_tmux(&terminal.socket, &terminal.config)
            .args(arguments)
            .output()
            .map_err(|error| error.to_string())?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).into_owned());
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    /// Gives the owner's terminal `rows` rows, its width kept, as the owner's resize does.
    pub fn resize_rows(&self, rows: u16) {
        let terminal = self.terminal();
        let (columns, rows) = (COLUMNS.to_string(), rows.to_string());
        self.terminal_tmux(
            terminal,
            &[
                "resize-window",
                "-t",
                "=owner:",
                "-x",
                &columns,
                "-y",
                &rows,
            ],
        )
        .unwrap_or_else(|problem| panic!("cannot resize the terminal: {problem}"));
    }

    /// Presses keys in the owner's terminal: tmux key names (`Enter`, `Tab`, `Right`, `y`).
    pub fn press(&self, keys: &[&str]) {
        let terminal = self.terminal();
        let mut arguments = vec!["send-keys", "-t", "=owner:"];
        arguments.extend_from_slice(keys);
        self.terminal_tmux(terminal, &arguments)
            .unwrap_or_else(|problem| panic!("cannot press {keys:?}: {problem}"));
    }

    /// Resizes the owner's terminal to `columns` wide and presses `key` in the same tmux
    /// command, so the TUI's resize and the key reach it together.
    pub fn resize_and_press(&self, columns: u16, key: &str) {
        let terminal = self.terminal();
        let columns = columns.to_string();
        let rows = ROWS.to_string();
        let arguments = [
            "resize-window",
            "-t",
            "=owner:",
            "-x",
            &columns,
            "-y",
            &rows,
            ";",
            "send-keys",
            "-t",
            "=owner:",
            key,
        ];
        self.terminal_tmux(terminal, &arguments)
            .unwrap_or_else(|problem| panic!("cannot resize and press {key:?}: {problem}"));
    }

    /// Types `text` in the owner's terminal, every character a key.
    pub fn type_text(&self, text: &str) {
        let terminal = self.terminal();
        self.terminal_tmux(terminal, &["send-keys", "-t", "=owner:", "-l", "--", text])
            .unwrap_or_else(|problem| panic!("cannot type {text:?}: {problem}"));
    }

    /// Waits until the screen shows `text`; returns that screen.
    pub fn wait_for(&self, text: &str) -> String {
        self.wait_until(&format!("the screen shows {text:?}"), |screen| {
            screen.contains(text)
        })
    }

    /// Waits until the screen no longer shows `text`; returns that screen.
    pub fn wait_for_gone(&self, text: &str) -> String {
        self.wait_until(&format!("the screen no longer shows {text:?}"), |screen| {
            !screen.contains(text)
        })
    }

    /// Waits until `pane` has the keys. tmux redraws the manager's pane borders when the
    /// active pane changes, which is a write to the terminal that wakes the wait.
    pub fn wait_for_focus(&self, pane: ManagerPane) {
        self.wait_until(&format!("the {pane:?} has the keys"), |_| {
            self.focused() == pane
        });
    }

    /// Waits until `condition` holds for the screen, reading it again after each write to
    /// the terminal; returns that screen.
    pub fn wait_until(&self, what: &str, condition: impl Fn(&str) -> bool) -> String {
        let terminal = self.terminal();
        let deadline = Instant::now() + GENEROUS;
        loop {
            // Writes announced before this read are in it; any after it wake the wait.
            while terminal.written.try_recv().is_ok() {}
            let screen = self.screen();
            if condition(&screen) {
                return screen;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            match terminal.written.recv_timeout(left) {
                Ok(()) => {}
                Err(RecvTimeoutError::Timeout) => panic!(
                    "waited {GENEROUS:?} for: {what}\n--- screen ---\n{screen}\n--- daemon \
                     stderr ---\n{}",
                    self.failure_context()
                ),
                Err(RecvTimeoutError::Disconnected) => {
                    panic!("the test terminal closed while waiting for: {what}\n{screen}")
                }
            }
        }
    }

    /// Waits until the screen, redacted, is `tests/e2e/snapshots/<name>.txt`: a fixture
    /// reviewed in pull requests. Waiting rather than comparing once, because a capture can
    /// land in the middle of a redraw. `COCKPIT_E2E_UPDATE=1` records the screen instead,
    /// once no write came for half a second.
    pub fn snapshot(&self, name: &str) {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/e2e/snapshots")
            .join(format!("{name}.txt"));
        let shown = |screen: &str| format!("{}\n", redacted(screen, &self.root).trim_end());
        if std::env::var_os("COCKPIT_E2E_UPDATE").is_some() {
            let terminal = self.terminal();
            while terminal
                .written
                .recv_timeout(Duration::from_millis(500))
                .is_ok()
            {}
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, shown(&self.screen())).unwrap();
            return;
        }
        let expected = std::fs::read_to_string(&path).unwrap_or_else(|error| {
            panic!(
                "{}: {error}; run with COCKPIT_E2E_UPDATE=1 to record it",
                path.display()
            )
        });
        self.wait_until(
            &format!("the screen matches {} (COCKPIT_E2E_UPDATE=1 records it)\n--- expected ---\n{expected}", path.display()),
            |screen| shown(screen) == expected,
        );
    }

    /// Gives the keys to the list or the viewer, as the owner's click does: selects that
    /// pane of the manager session on the daemon's tmux server.
    pub fn click(&self, pane: ManagerPane) {
        let panes = self.private_tmux(&["list-panes", "-t", "=cockpit:", "-F", "#{pane_id}"]);
        let ids: Vec<&str> = panes.lines().collect();
        let id = ids
            .get(pane as usize)
            .unwrap_or_else(|| panic!("the manager has no {pane:?} pane: {panes:?}"));
        self.private_tmux(&["select-pane", "-t", id]);
    }

    /// Which manager pane has the keys.
    pub fn focused(&self) -> ManagerPane {
        // By position in tmux's order, as `click` selects them.
        let panes = self.private_tmux(&["list-panes", "-t", "=cockpit:", "-F", "#{pane_active}"]);
        match panes.lines().position(|active| active == "1") {
            Some(0) => ManagerPane::List,
            Some(_) => ManagerPane::Viewer,
            None => panic!("no manager pane is active: {panes:?}"),
        }
    }

    /// A tmux command on the daemon's private server (`-L cockpit` under the scratch home).
    fn private_tmux(&self, arguments: &[&str]) -> String {
        let output = self
            .command(Path::new("tmux"))
            .args(["-L", "cockpit"])
            .args(arguments)
            .output()
            .expect("tmux runs");
        assert!(
            output.status.success(),
            "tmux {arguments:?} on the daemon's server: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    /// A session started from the CLI, selected and shown in the viewer, its agent at its
    /// prompt; returns its id.
    pub fn shown_session(&self) -> String {
        let session = self.session_from_cli();
        self.wait_for(&session);
        self.press(&["Enter"]);
        self.wait_for(&format!("reply: {FIRST_MESSAGE}"));
        session
    }

    /// Sends a line to the agent shown in the viewer, as the owner types into it.
    pub fn tell_agent(&self, line: &str) {
        self.click(ManagerPane::Viewer);
        self.type_text(line);
        self.press(&["Enter"]);
    }

    /// Types `line` at the prompt of `session`'s agent without the viewer, as if the agent
    /// decided to run it on its own: for a session the owner is not looking at.
    pub fn agent_does(&self, session: &str, line: &str) {
        let target = format!("={session}:");
        self.private_tmux(&["send-keys", "-t", &target, "-l", "--", line]);
        self.private_tmux(&["send-keys", "-t", &target, "Enter"]);
    }

    /// Waits until `condition` holds for `session`'s screen as its agent's pane holds it,
    /// reading it again after each write a `read-only,ignore-size` control-mode client of
    /// that session announces; returns that screen. The client leaves before this returns,
    /// and it never sized the window.
    pub fn wait_for_agent_screen(
        &self,
        session: &str,
        what: &str,
        condition: impl Fn(&[String]) -> bool,
    ) -> Vec<String> {
        let target = format!("={session}:");
        let mut watcher = self
            .command(Path::new("tmux"))
            .args(["-L", "cockpit", "-C", "attach-session", "-t", &target])
            .args(["-f", "read-only,ignore-size"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("a control-mode client starts");
        let written = announced_writes(watcher.stdout.take().expect("stdout is piped"));
        let deadline = Instant::now() + GENEROUS;
        let outcome = loop {
            while written.try_recv().is_ok() {}
            let screen = self.agent_screen(session);
            if condition(&screen) {
                break Ok(screen);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if let Err(error) = written.recv_timeout(left) {
                break Err((error, screen));
            }
        };
        let _ = watcher.kill();
        let _ = watcher.wait();
        outcome.unwrap_or_else(|(error, screen)| {
            panic!(
                "{error:?} waiting for {session}'s screen: {what}\n{screen:#?}\n{}",
                self.failure_context()
            )
        })
    }

    /// `session`'s window size on the daemon's tmux, as `<columns>x<rows>`.
    pub fn agent_window_size(&self, session: &str) -> String {
        self.private_tmux(&[
            "display-message",
            "-p",
            "-t",
            &format!("={session}:"),
            "#{window_width}x#{window_height}",
        ])
        .trim_end()
        .to_owned()
    }

    /// The manager's viewer pane size, as `<columns>x<rows>`.
    pub fn viewer_size(&self) -> String {
        let sizes = self.private_tmux(&[
            "list-panes",
            "-t",
            "=cockpit:",
            "-F",
            "#{pane_width}x#{pane_height}",
        ]);
        sizes
            .lines()
            .nth(ManagerPane::Viewer as usize)
            .unwrap_or_else(|| panic!("the manager has no viewer: {sizes:?}"))
            .to_owned()
    }

    /// Resizes the owner's terminal, as dragging its window does.
    pub fn resize_terminal(&self, columns: u16, rows: u16) {
        let terminal = self.terminal();
        let (columns, rows) = (columns.to_string(), rows.to_string());
        self.terminal_tmux(
            terminal,
            &[
                "resize-window",
                "-t",
                "=owner:",
                "-x",
                &columns,
                "-y",
                &rows,
            ],
        )
        .unwrap_or_else(|problem| panic!("cannot resize the terminal: {problem}"));
    }

    /// `session`'s screen as its agent's own pane holds it, trailing spaces and empty rows
    /// cut: what the viewer must show.
    pub fn agent_screen(&self, session: &str) -> Vec<String> {
        trimmed_rows(
            self.private_tmux(&["capture-pane", "-p", "-t", &format!("={session}:")])
                .lines(),
        )
    }

    /// How many times the fake agent of `session` was launched, as its log counts them.
    pub fn launch_count(&self, session: &str) -> usize {
        let prefix = format!("{session} pid=");
        std::fs::read_to_string(self.root.join("launches.log"))
            .unwrap_or_default()
            .lines()
            .filter(|line| line.starts_with(&prefix))
            .count()
    }

    /// Follows the daemon's change feed from a snapshot until `condition` holds for the
    /// sessions it carries (ended ones included); returns them by id.
    fn wait_for_sessions(
        &self,
        what: &str,
        condition: impl Fn(&BTreeMap<String, SessionView>) -> bool,
    ) -> BTreeMap<String, SessionView> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let mut sessions = BTreeMap::new();
        let follow = async {
            let mut subscription = Subscription::new(self.socket.clone(), None, GENEROUS);
            loop {
                let pending = subscription
                    .next_batch()
                    .await
                    .map_err(|error| error.to_string())?;
                let batch = pending.batch();
                if batch.follows == FeedPosition::Snapshot {
                    sessions.clear();
                }
                for event in &batch.changes {
                    if event.entity.kind != EntityKind::Session {
                        continue;
                    }
                    match &event.change {
                        Change::Upsert { state } => {
                            let session: SessionView =
                                serde_json::from_value(serde_json::Value::Object(state.clone()))
                                    .map_err(|error| error.to_string())?;
                            sessions.insert(session.id.clone(), session);
                        }
                        Change::Tombstone => {
                            sessions.remove(&event.entity.id);
                        }
                    }
                }
                pending.applied();
                if condition(&sessions) {
                    return Ok::<_, String>(());
                }
            }
        };
        let followed = runtime.block_on(async { tokio::time::timeout(GENEROUS, follow).await });
        match followed {
            Ok(Ok(())) => sessions,
            Ok(Err(problem)) => panic!("the change feed failed waiting for {what}: {problem}"),
            Err(_) => panic!(
                "waited {GENEROUS:?} on the change feed for: {what}\nsessions: {sessions:#?}\n\
                 --- screen ---\n{}",
                self.screen()
            ),
        }
    }

    /// The session's view once `condition` holds for it on the change feed.
    pub fn wait_for_session(
        &self,
        session: &str,
        what: &str,
        condition: impl Fn(&SessionView) -> bool,
    ) -> SessionView {
        self.wait_for_sessions(&format!("{session} {what}"), |sessions| {
            sessions.get(session).is_some_and(&condition)
        })
        .remove(session)
        .expect("the condition held for it")
    }
}

impl Drop for Cockpit {
    fn drop(&mut self) {
        if let Some(terminal) = self.terminal.as_mut() {
            let _ = terminal.watcher.kill();
            let _ = terminal.watcher.wait();
        }
        if let Some(terminal) = &self.terminal {
            let _ = self.terminal_tmux(terminal, &["kill-server"]);
        }
        if let Some(mut daemon) = self.daemon.take() {
            let _ = daemon.kill();
            let _ = daemon.wait();
        }
        // The agents run in the daemon's tmux server, which outlives the daemon by design.
        let _ = self
            .command(Path::new("tmux"))
            .args(["-L", "cockpit", "kill-server"])
            .output();
    }
}

/// The owner's terminal: a test-owned tmux server running `cockpit tui` in its one pane,
/// and a control-mode client that announces each write to it.
struct Terminal {
    socket: PathBuf,
    config: PathBuf,
    watcher: Child,
    /// Held open: a control-mode client exits when its input ends.
    _watcher_input: Option<ChildStdin>,
    written: Receiver<()>,
}

/// One message per `%output` line a control-mode client prints, until it exits.
fn announced_writes(stdout: std::process::ChildStdout) -> Receiver<()> {
    let (writes, written) = mpsc::channel();
    std::thread::spawn(move || {
        // Bytes, not lines of text: tmux passes the pane's bytes through unchecked.
        for line in BufReader::new(stdout).split(b'\n') {
            let Ok(line) = line else { return };
            if line.starts_with(b"%output") && writes.send(()).is_err() {
                return;
            }
        }
    });
    written
}

/// The manager session's panes, in tmux's order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManagerPane {
    List = 0,
    Viewer = 1,
}

#[cfg(target_os = "macos")]
const UTF8_LOCALE: &str = "en_US.UTF-8";
#[cfg(not(target_os = "macos"))]
const UTF8_LOCALE: &str = "C.UTF-8";

/// `rows` with trailing spaces cut and the empty rows at the end dropped.
pub fn trimmed_rows<'a>(rows: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut rows: Vec<String> = rows.map(|row| row.trim_end().to_owned()).collect();
    while rows.last().is_some_and(String::is_empty) {
        rows.pop();
    }
    rows
}

/// Session ids (`s` and 12 hex digits) in the order `text` shows them, each once.
pub fn session_ids(text: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for word in text.split(|character: char| !character.is_ascii_alphanumeric()) {
        let is_id = word.len() == 13
            && word.starts_with('s')
            && word[1..]
                .chars()
                .all(|character| matches!(character, '0'..='9' | 'a'..='f'));
        if is_id && !found.iter().any(|seen| seen == word) {
            found.push(word.to_owned());
        }
    }
    found
}

/// The screen with what changes from run to run replaced: session ids become `s<1>`,
/// `s<2>`… in the order they first appear, the scratch home `<home>`.
fn redacted(screen: &str, home: &Path) -> String {
    let mut text = screen.replace(home.to_str().unwrap(), "<home>");
    for (index, id) in session_ids(screen).iter().enumerate() {
        let placeholder = format!("s<{}>", index + 1);
        text = text.replace(id, &format!("{placeholder:<width$}", width = id.len()));
    }
    text
}

/// The machine's Git with a fixed identity and none of the developer's configuration
/// (signing, hooks, templates), failing the test when it fails.
fn git(dir: &Path, arguments: &[&str]) {
    let output = Command::new("git")
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["-c", "user.name=e2e", "-c", "user.email=e2e@localhost"])
        .args(arguments)
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {arguments:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

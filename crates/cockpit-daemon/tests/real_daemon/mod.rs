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

/// The profile directory of a daemon this test build runs without `--profile`: a build
/// outside a release bundle runs profile `dev` (#196).
pub const BUILD_PROFILE_DIR: &str = "cockpit-dev";

/// Every profile directory a test here runs a daemon under, whose tmux servers the home
/// stops when dropped.
const PROFILE_DIRS: [&str; 3] = ["cockpit", BUILD_PROFILE_DIR, "cockpit-next"];

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
/// A project of this name, or a name it begins, runs an agent that reports once through the `cockpit` built beside
/// the daemon, as the Claude hook bridge does, as soon as it starts, appending its output to
/// [`Home::report_log`].
pub const REPORT_PROJECT: &str = "report";
/// The label the [`REPORT_PROJECT`] agent reports.
pub const REPORTED_LABEL: &str = "reported from the agent";
/// A project of this name runs an agent that writes `DISPLAY`, `XAUTHORITY`,
/// `WAYLAND_DISPLAY` and `DBUS_SESSION_BUS_ADDRESS` as it was started with to [`Home::environment_log`],
/// `<name>=unset` for each one it lacks.
pub const ENVIRONMENT_PROJECT: &str = "environment";
/// A project of this name runs an agent that, as soon as it starts, asks the owner to run
/// [`ASKED_COMMAND`] through that `cockpit ask run`, waits for the answer and appends
/// `run=<exit code>` to [`Home::ask_log`].
pub const ASK_RUN_PROJECT: &str = "ask-run";
/// A project of this name runs an agent that hands the owner [`ASKED_GUIDE`] through that
/// `cockpit ask guide`, waits for the answer and appends `guide=<exit code>` to
/// [`Home::ask_log`].
pub const ASK_GUIDE_PROJECT: &str = "ask-guide";
/// A project of this name runs an agent that has nothing but what Cockpit launched it with
/// (#49): it runs `cockpit brief`, `cockpit docs` and the `workflow`, `checks` and
/// `approvals` topics, checks the skill under `$COCKPIT_INSTRUCTIONS` is readable, and writes
/// everything it read to [`Home::onboard_log`].
pub const ONBOARD_PROJECT: &str = "onboard";
/// A project of this name runs an agent that runs, in order, each `<n>.sh` the test drops
/// in its `.agent` directory (from 0), writing what it printed and `exit=<code>` to `<n>.out`
/// (#410): a coordinator the test drives through the real `cockpit`.
pub const COMMANDED_PROJECT: &str = "commanded";
/// A project of this name runs an agent that reports [`ASKED_QUESTION`] while it waits at its
/// prompt, then appends each line typed into its terminal to `inbox.log` in its directory.
pub const QUESTION_PROJECT: &str = "question";
/// What the [`QUESTION_PROJECT`] agent asks.
pub const ASKED_QUESTION: &str = "Which branch should I base the fix on?";
/// What the [`ASK_RUN_PROJECT`] agent asks to run.
pub const ASKED_COMMAND: &str = "echo asked from the agent";
/// The guide the [`ASK_GUIDE_PROJECT`] agent hands the owner, one step.
pub const ASKED_GUIDE: &str = "# Asked from the agent\nLook at the panel\n";

/// The agent acts as the last component of its working directory says ([`FLOOD_PROJECT`],
/// [`STALL_PROJECT`], [`REPORT_PROJECT`], [`ENVIRONMENT_PROJECT`], [`ASK_RUN_PROJECT`],
/// [`ASK_GUIDE_PROJECT`], [`ONBOARD_PROJECT`]); anything else sleeps. `--version` answers
/// the version the adapter's conformance tests ran on.
const FAKE_CLAUDE: &str = r#"#!/bin/sh
if [ "$1" = --version ]; then echo '2.1.316 (Claude Code)'; exit 0; fi
printf '%s pid=%s\n' "$COCKPIT_SESSION" "$$" >> '@LOG@'
case "${PWD##*/}" in
  @FLOOD@) exec yes 'cockpit latency fixture: one build log line after another, without pause' ;;
  @STALL@) trap '' TERM; while :; do sleep 1; done ;;
  @REPORT@*) '@COCKPIT@' agent report --state working --label '@LABEL@' >> '@REPORT_LOG@' 2>&1
    exec sleep 3600 ;;
  @ASK_RUN@) '@COCKPIT@' ask run --reason 'proves the agent socket' '@ASKED_COMMAND@' \
      2>> '@ASK_LOG@'
    printf 'run=%s\n' "$?" >> '@ASK_LOG@'
    exec sleep 3600 ;;
  @ASK_GUIDE@) printf '@ASKED_GUIDE@' | '@COCKPIT@' ask guide 2>> '@ASK_LOG@'
    printf 'guide=%s\n' "$?" >> '@ASK_LOG@'
    exec sleep 3600 ;;
  @ENVIRONMENT@) printf 'DISPLAY=%s\nXAUTHORITY=%s\nWAYLAND_DISPLAY=%s\nDBUS_SESSION_BUS_ADDRESS=%s\n' \
      "${DISPLAY-unset}" "${XAUTHORITY-unset}" "${WAYLAND_DISPLAY-unset}" \
      "${DBUS_SESSION_BUS_ADDRESS-unset}" \
      > '@ENVIRONMENT_LOG@.partial' && mv '@ENVIRONMENT_LOG@.partial' '@ENVIRONMENT_LOG@'
    exec sleep 3600 ;;
  @COMMANDED@) n=0
    while :; do
      if [ -f ".agent/$n.sh" ]; then
        sh ".agent/$n.sh" > ".agent/$n.partial" 2>&1
        printf 'exit=%s\n' "$?" >> ".agent/$n.partial"
        mv ".agent/$n.partial" ".agent/$n.out"
        n=$((n + 1))
      fi
      sleep 0.1
    done ;;
  @QUESTION@) '@COCKPIT@' agent report --state waiting --needs-input '@ASKED_QUESTION_TEXT@' \
      >> '@REPORT_LOG@' 2>&1
    while IFS= read -r line; do printf '%s\n' "$line" >> inbox.log; done
    exec sleep 3600 ;;
  @ONBOARD@) { '@COCKPIT@' brief; '@COCKPIT@' docs; for topic in workflow checks approvals; do
      '@COCKPIT@' docs "$topic"; done
    test -r "$COCKPIT_INSTRUCTIONS/cockpit/SKILL.md" && echo 'skill readable on disk'
    } > '@ONBOARD_LOG@.partial' 2>&1; mv '@ONBOARD_LOG@.partial' '@ONBOARD_LOG@'
    exec sleep 3600 ;;
  *) exec sleep 3600 ;;
esac
"#;

/// A stand-in Greywall (#61): `--version` answers as the pinned build on a host with every
/// capability; a launch appends its arguments to `@LOG@`, one line, then runs the agent
/// after `--` unsandboxed, so the agents above behave as they do without it.
const FAKE_GREYWALL: &str = r#"#!/bin/sh
if [ "$1" = --version ]; then
  printf 'greywall - stand-in\n  Version: 0.3.7+dotfiles\n  Commit:  @COMMIT@\n'
  printf 'Checking system capabilities:\n'
  for capability in bubblewrap landlock seccomp 'network isolation' 'D-Bus isolated'; do
    printf '  \342\234\223 %s\n' "$capability"
  done
  exit 0
fi
printf '%s\n' "$*" >> '@LOG@'
while [ "$#" -gt 0 ] && [ "$1" != -- ]; do shift; done
shift
exec "$@"
"#;

/// A stand-in greyproxy: only `--version`, as the pinned build.
const FAKE_GREYPROXY: &str = r#"#!/bin/sh
printf 'greyproxy 0.4.5 (stand-in)\n  commit: @COMMIT@\n'
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
        let utf8 = |path: &Path| path.to_str().expect("a UTF-8 path").to_owned();
        std::fs::write(
            &claude,
            FAKE_CLAUDE
                .replace("@LOG@", &utf8(&home.launch_log()))
                .replace("@FLOOD@", FLOOD_PROJECT)
                .replace("@STALL@", STALL_PROJECT)
                .replace("@REPORT@", REPORT_PROJECT)
                .replace("@COCKPIT@", &utf8(&cockpit_program()))
                .replace("@LABEL@", REPORTED_LABEL)
                .replace("@REPORT_LOG@", &utf8(&home.report_log()))
                .replace("@ENVIRONMENT@", ENVIRONMENT_PROJECT)
                .replace("@ENVIRONMENT_LOG@", &utf8(&home.environment_log()))
                .replace("@ASK_RUN@", ASK_RUN_PROJECT)
                .replace("@ASK_GUIDE@", ASK_GUIDE_PROJECT)
                .replace("@ASKED_COMMAND@", ASKED_COMMAND)
                .replace("@ASKED_GUIDE@", ASKED_GUIDE)
                .replace("@ASK_LOG@", &utf8(&home.ask_log()))
                .replace("@COMMANDED@", COMMANDED_PROJECT)
                .replace("@QUESTION@", QUESTION_PROJECT)
                .replace("@ASKED_QUESTION_TEXT@", ASKED_QUESTION)
                .replace("@ONBOARD@", ONBOARD_PROJECT)
                .replace("@ONBOARD_LOG@", &utf8(&home.onboard_log())),
        )
        .unwrap();
        std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755)).unwrap();
        for (name, script) in [
            (
                "greywall",
                FAKE_GREYWALL
                    .replace("@COMMIT@", cockpit_sandbox::PINNED_GREYWALL_COMMIT)
                    .replace("@LOG@", &utf8(&home.sandbox_log())),
            ),
            (
                "greyproxy",
                FAKE_GREYPROXY.replace("@COMMIT@", cockpit_sandbox::PINNED_GREYPROXY_COMMIT),
            ),
            // Answering only `--version`, as the adapters' conformance versions, so a real
            // `codex` or `opencode` on the host never answers (#411, #415).
            ("codex", "#!/bin/sh\necho 'codex-cli 0.161.0'\n".to_owned()),
            ("opencode", "#!/bin/sh\necho '1.19.5'\n".to_owned()),
        ] {
            let program = home.path().join("bin").join(name);
            std::fs::write(&program, script).unwrap();
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        home
    }

    /// The arguments of each launch the stand-in Greywall ran, one line each.
    pub fn sandbox_log(&self) -> PathBuf {
        self.path().join("sandbox.log")
    }

    pub fn path(&self) -> PathBuf {
        self.root.clone()
    }

    fn launch_log(&self) -> PathBuf {
        self.path().join("launches.log")
    }

    /// What the [`REPORT_PROJECT`] agent's `cockpit agent report` printed.
    pub fn report_log(&self) -> PathBuf {
        self.path().join("reports.log")
    }

    /// What the [`ASK_RUN_PROJECT`] and [`ASK_GUIDE_PROJECT`] agents' `cockpit ask` printed,
    /// and the exit code each one ended with.
    pub fn ask_log(&self) -> PathBuf {
        self.path().join("asks.log")
    }

    /// What the [`ONBOARD_PROJECT`] agent read, written whole once it is done.
    pub fn onboard_log(&self) -> PathBuf {
        self.path().join("onboard.log")
    }

    /// What the [`ENVIRONMENT_PROJECT`] agent wrote, once it wrote all of it.
    pub fn environment_log(&self) -> PathBuf {
        self.path().join("environment.log")
    }

    pub fn socket_path(&self) -> PathBuf {
        self.socket_path_of(BUILD_PROFILE_DIR)
    }

    /// The host-only socket of the daemon of the profile whose directory is `profile_dir`.
    pub fn socket_path_of(&self, profile_dir: &str) -> PathBuf {
        self.path()
            .join("run")
            .join(profile_dir)
            .join(cockpit_config::DAEMON_SOCKET)
    }

    pub fn agent_socket_path(&self) -> PathBuf {
        self.path()
            .join("run")
            .join(BUILD_PROFILE_DIR)
            .join(cockpit_config::AGENT_SOCKET_DIR)
            .join(cockpit_config::AGENT_SOCKET)
    }

    /// The build profile's state directory.
    pub fn state_dir(&self) -> PathBuf {
        self.path().join("state").join(BUILD_PROFILE_DIR)
    }

    /// Writes the owner's `config.toml`, which the next daemon start reads.
    pub fn write_owner_config(&self, contents: &str) {
        let dir = self.path().join("config").join(BUILD_PROFILE_DIR);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.toml"), contents).unwrap();
    }

    /// Runs the real [`cockpit_program`] as the owner in this home, with `extra` as its only
    /// other environment, off the test's runtime threads.
    pub async fn cockpit(
        &self,
        arguments: &[&str],
        extra: &[(&str, &str)],
    ) -> std::process::Output {
        let mut command = Process::new(cockpit_program());
        command
            .args(arguments)
            .env_clear()
            .env("HOME", self.path())
            .env("XDG_RUNTIME_DIR", self.path().join("run"))
            .envs(extra.iter().copied());
        tokio::task::spawn_blocking(move || command.output().expect("cockpit runs"))
            .await
            .unwrap()
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
        self.start_with(&[])
    }

    /// [`Home::start`], with `extra` added to the daemon's own environment.
    pub fn start_with(&self, extra: &[(&str, &str)]) -> Daemon {
        let path = std::env::var_os("PATH").unwrap_or_default();
        let mut search = vec![self.path().join("bin")];
        search.extend(std::env::split_paths(&path));
        self.spawn(search, extra, None)
    }

    /// [`Home::start`] with `--profile profile`; it is ready on that profile's socket.
    pub fn start_profile(&self, profile: &str) -> Daemon {
        let path = std::env::var_os("PATH").unwrap_or_default();
        let mut search = vec![self.path().join("bin")];
        search.extend(std::env::split_paths(&path));
        self.spawn(search, &[], Some(profile))
    }

    /// [`Home::start`] on a host without Greywall: neither stand-in, and no folder of the
    /// caller's `PATH` that holds a real one.
    pub fn start_without_greywall(&self) -> Daemon {
        for program in ["greywall", "greyproxy"] {
            std::fs::remove_file(self.path().join("bin").join(program)).unwrap();
        }
        let path = std::env::var_os("PATH").unwrap_or_default();
        let mut search = vec![self.path().join("bin")];
        search.extend(
            std::env::split_paths(&path)
                .filter(|dir| !dir.join("greywall").exists() && !dir.join("greyproxy").exists()),
        );
        self.spawn(search, &[], None)
    }

    fn spawn(&self, search: Vec<PathBuf>, extra: &[(&str, &str)], profile: Option<&str>) -> Daemon {
        let profile_dir = match profile {
            None => BUILD_PROFILE_DIR.to_owned(),
            Some(profile) => cockpit_config::Profile::parse(profile)
                .expect("a profile name")
                .qualified_name(),
        };
        let mut child = Process::new(env!("CARGO_BIN_EXE_cockpit-daemon"))
            .args(
                profile
                    .map(|profile| ["--profile", profile])
                    .into_iter()
                    .flatten(),
            )
            .env_clear()
            .env("HOME", self.path())
            .env("XDG_RUNTIME_DIR", self.path().join("run"))
            .env("XDG_STATE_HOME", self.path().join("state"))
            .env("XDG_CONFIG_HOME", self.path().join("config"))
            .env("TMUX_TMPDIR", self.path().join("tmux"))
            .env("PATH", std::env::join_paths(search).unwrap())
            .envs(extra.iter().copied())
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
        if line.trim() != format!("ready {}", self.socket_path_of(&profile_dir).display()) {
            drop(daemon);
            let stderr = std::fs::read_to_string(self.stderr_log()).unwrap_or_default();
            panic!("the daemon did not report ready: stdout {line:?}, stderr {stderr:?}");
        }
        daemon
    }

    pub async fn connect(&self) -> Connection {
        self.connect_to(BUILD_PROFILE_DIR).await
    }

    /// Connects to the daemon of the profile whose directory is `profile_dir`.
    pub async fn connect_to(&self, profile_dir: &str) -> Connection {
        Connection::open(&self.socket_path_of(profile_dir), GENEROUS)
            .await
            .expect("the daemon accepts the handshake")
    }

    /// Starts a session of the [`ENVIRONMENT_PROJECT`] agent and returns what it wrote.
    pub async fn environment_of_a_launched_agent(&self) -> String {
        let client = self.connect().await;
        let (root, _) = self.project(&client, ENVIRONMENT_PROJECT).await;
        new_session(&client, &root).await;
        let deadline = Instant::now() + GENEROUS;
        loop {
            if let Ok(written) = std::fs::read_to_string(self.environment_log()) {
                return written;
            }
            assert!(Instant::now() < deadline, "the agent wrote no environment");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
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

impl Home {
    /// Hands the [`COMMANDED_PROJECT`] agent working in `root` its next shell line; returns
    /// its number, which [`Home::agent_output`] takes.
    pub fn command_agent(&self, root: &Path, line: &str) -> usize {
        let dir = root.join(".agent");
        std::fs::create_dir_all(&dir).unwrap();
        let number = (0..)
            .find(|number| !dir.join(format!("{number}.sh")).exists())
            .unwrap();
        let staged = dir.join(format!("{number}.staged"));
        std::fs::write(&staged, format!("{line}\n")).unwrap();
        std::fs::rename(&staged, dir.join(format!("{number}.sh"))).unwrap();
        number
    }

    /// What the [`COMMANDED_PROJECT`] agent's line `number` printed, ending `exit=<code>`.
    pub async fn agent_output(&self, root: &Path, number: usize) -> String {
        let output = root.join(".agent").join(format!("{number}.out"));
        let deadline = Instant::now() + SETTLED_WITHIN;
        loop {
            if let Ok(printed) = std::fs::read_to_string(&output) {
                return printed;
            }
            assert!(
                Instant::now() < deadline,
                "the agent in {} did not finish line {number}",
                root.display()
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

impl Drop for Home {
    /// The agents live in the private tmux server, which outlives every daemon.
    fn drop(&mut self) {
        for profile_dir in PROFILE_DIRS {
            let _ = Process::new("tmux")
                .env_remove("TMUX")
                .env("TMUX_TMPDIR", self.path().join("tmux"))
                .args(["-L", profile_dir, "kill-server"])
                .output();
        }
    }
}

pub struct Daemon {
    child: Child,
}

/// The `cockpit` program beside the daemon's, where the daemon looks for it too. Cargo
/// builds it there for the workspace run (`make crates-test`), not for a run of this
/// package alone.
pub fn cockpit_program() -> PathBuf {
    Path::new(env!("CARGO_BIN_EXE_cockpit-daemon")).with_file_name("cockpit")
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

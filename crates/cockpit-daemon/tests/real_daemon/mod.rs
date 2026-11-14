//! The `cockpit-daemon` binary in a temporary home, for fixtures that kill the real process
//! (#36). Claude Code on its `PATH` is a shell script that logs each launch and then acts as
//! the directory it runs in says (see [`FAKE_CLAUDE`]); tmux is the real one, on a private
//! server under `TMUX_TMPDIR` inside the home, never the user's.
#![allow(
    dead_code,
    reason = "each test file uses a different part of the fixture"
)]

use crate::candidate;

use std::io::{BufRead, BufReader};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command as Process, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use cockpit_client::Connection;
use cockpit_protocol::{CheckReceiptView, Command, Reply, SessionStateView, SessionView};

/// The profile directory of a daemon this test build runs without `--profile`: a build
/// outside a release bundle runs profile `dev` (#196).
pub const BUILD_PROFILE_DIR: &str = "torlek-dev";

/// Every profile directory a test here runs a daemon under, whose tmux servers the home
/// stops when dropped.
const PROFILE_DIRS: [&str; 3] = ["torlek", BUILD_PROFILE_DIR, "torlek-next"];

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
/// `approvals` topics, checks the skill under `$TORLEK_INSTRUCTIONS` is readable, and writes
/// everything it read to [`Home::onboard_log`].
pub const ONBOARD_PROJECT: &str = "onboard";
/// A project of this name runs an agent that runs, in order, each `<n>.sh` the test drops
/// in its `.agent` directory (from 0), writing what it printed and `exit=<code>` to `<n>.out`
/// (#410): a coordinator the test drives through the real `cockpit`. It first writes the
/// words it was launched with to `.agent/launch.arguments` ([`Home::launch_arguments`]).
pub const COMMANDED_PROJECT: &str = "commanded";
/// A project of this name runs an agent that reports [`ASKED_QUESTION`] while it waits at its
/// prompt, then, as the Claude hook bridge's inbox waiter does, collects each message other
/// sessions send it with `cockpit agent inbox` and appends it to `inbox.log` in its directory
/// (#703).
pub const QUESTION_PROJECT: &str = "question";
/// What the [`QUESTION_PROJECT`] agent asks.
pub const ASKED_QUESTION: &str = "Which branch should I base the fix on?";
/// What the [`ASK_RUN_PROJECT`] agent asks to run.
pub const ASKED_COMMAND: &str = "echo asked from the agent";
/// The guide the [`ASK_GUIDE_PROJECT`] agent hands the owner, one step.
pub const ASKED_GUIDE: &str = "# Asked from the agent\nLook at the panel\n";

/// An effort level the fake `claude` lists in its `--help` that no Torlek release has had.
pub const NEW_EFFORT_LEVEL: &str = "ludicrous";

/// The agent acts as the last component of its working directory says ([`FLOOD_PROJECT`],
/// [`STALL_PROJECT`], [`REPORT_PROJECT`], [`ENVIRONMENT_PROJECT`], [`ASK_RUN_PROJECT`],
/// [`ASK_GUIDE_PROJECT`], [`COMMANDED_PROJECT`], [`QUESTION_PROJECT`], [`ONBOARD_PROJECT`]);
/// anything else sleeps. `--version` answers
/// the version the adapter's conformance tests ran on, and `--help` lists `--effort` levels
/// with one ([`NEW_EFFORT_LEVEL`]) no Torlek release has known (#743).
const FAKE_CLAUDE: &str = r#"#!/bin/sh
if [ "$1" = --version ]; then echo '2.1.316 (Claude Code)'; exit 0; fi
if [ "$1" = --help ]; then
  printf '  --effort <level>   Effort level for the current session\n                     (low, medium, high, @NEW_EFFORT@)\n  --env  x\n'
  exit 0
fi
if [ "$*" = 'auth status' ]; then exit 2; fi
printf '%s pid=%s\n' "$TORLEK_SESSION" "$$" >> '@LOG@'
printf '%s %s\n' "$TORLEK_SESSION" "$*" >> '@LOG@.arguments'
printf '%s %s %s %s\n' "$TORLEK_SESSION" "${TMPDIR-unset}" "${CLAUDE_CODE_TMPDIR-unset}" "${DOCKER_CONFIG-unset}" \
  >> '@TEMP_LOG@'
printf '%s %s %s\n' "$TORLEK_SESSION" "${GH_TOKEN-unset}" "${GITHUB_TOKEN-unset}" \
  >> '@GITHUB_LOG@'
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
  @COMMANDED@) mkdir -p .agent && printf '%s\0' "$@" > .agent/launch.partial \
      && mv .agent/launch.partial .agent/launch.arguments
    n=0
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
    while :; do '@COCKPIT@' agent inbox >> inbox.log 2>> inbox.err || sleep 1; done ;;
  @ONBOARD@) { '@COCKPIT@' brief; '@COCKPIT@' docs; for topic in workflow checks approvals; do
      '@COCKPIT@' docs "$topic"; done
    test -r "$TORLEK_INSTRUCTIONS/torlek/SKILL.md" && echo 'skill readable on disk'
    } > '@ONBOARD_LOG@.partial' 2>&1; mv '@ONBOARD_LOG@.partial' '@ONBOARD_LOG@'
    exec sleep 3600 ;;
  *) exec sleep 3600 ;;
esac
"#;

pub struct Home {
    /// The temporary directory, canonical: the daemon compares paths it is sent with it.
    root: PathBuf,
    /// Whether the harness writes the owner's config, `confined_runs = "off"` among it.
    has_owner_config: bool,
    _dir: tempfile::TempDir,
}

/// One launch of the fake agent, as it logged it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub session: String,
    pub pid: i32,
}

impl Home {
    /// A home whose `config/` folder holds nothing, for tests that assert on the folders the
    /// daemon and its profiles create. The daemon then reads the host's free space against the
    /// product default floor (#838), so such a test must not depend on the guard's outcome.
    pub fn without_owner_config() -> Home {
        let mut home = Home::new();
        std::fs::remove_dir_all(home.path().join("config")).unwrap();
        home.has_owner_config = false;
        home
    }

    pub fn new() -> Home {
        let dir = tempfile::tempdir().expect("a temporary home");
        let home = Home {
            root: dir.path().canonicalize().unwrap(),
            has_owner_config: true,
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
                .replace("@TEMP_LOG@", &utf8(&home.temp_log()))
                .replace("@GITHUB_LOG@", &utf8(&home.github_log()))
                .replace("@NEW_EFFORT@", NEW_EFFORT_LEVEL)
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
            // Answering only `--version`, as the adapters' conformance versions, so a real
            // `codex` or `opencode` on the host never answers (#411, #415).
            ("codex", "#!/bin/sh\necho 'codex-cli 0.166.0'\n"),
            ("opencode", "#!/bin/sh\necho '1.19.5'\n"),
        ] {
            let program = home.path().join("bin").join(name);
            std::fs::write(&program, script).unwrap();
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        home.write_owner_config("");
        home
    }

    /// Each launched agent's session, `TMPDIR` and `CLAUDE_CODE_TMPDIR`, one line each.
    pub fn temp_log(&self) -> PathBuf {
        self.path().join("temp.log")
    }

    /// Each launched agent's session, `GH_TOKEN` and `GITHUB_TOKEN`, one line each (#754).
    pub fn github_log(&self) -> PathBuf {
        self.path().join("github.log")
    }

    /// Writes project `project`'s GitHub token file with `mode`, where the owner keeps it
    /// (#754); its folder is owner-only.
    pub fn write_github_token(&self, project: &str, contents: &str, mode: u32) {
        self.write_token_file(cockpit_config::GITHUB_TOKENS_DIR, project, contents, mode);
    }

    /// Writes project `project`'s GitHub API token file, the one the session proxy adds
    /// (#832), as [`Home::write_github_token`] writes the other.
    pub fn write_github_api_token(&self, project: &str, contents: &str, mode: u32) {
        self.write_token_file(
            cockpit_config::GITHUB_API_TOKENS_DIR,
            project,
            contents,
            mode,
        );
    }

    /// Writes provider `provider`'s credential for sandboxed sessions, in the file named
    /// after `variable` (#968).
    pub fn write_provider_credential(
        &self,
        provider: &str,
        variable: &str,
        contents: &str,
        mode: u32,
    ) {
        let folder = format!("{}/{provider}", cockpit_config::PROVIDER_CREDENTIALS_DIR);
        self.write_token_file(&folder, variable, contents, mode);
    }

    /// [`Home::write_provider_credential`] for `account`, in its own folder (#59).
    pub fn write_account_credential(
        &self,
        provider: &str,
        account: &str,
        variable: &str,
        contents: &str,
        mode: u32,
    ) {
        let folder = format!(
            "{}/{provider}/{account}",
            cockpit_config::PROVIDER_CREDENTIALS_DIR
        );
        self.write_token_file(&folder, variable, contents, mode);
    }

    fn write_token_file(&self, folder: &str, project: &str, contents: &str, mode: u32) {
        let dir = self
            .path()
            .join("config")
            .join(BUILD_PROFILE_DIR)
            .join(folder);
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)
            .unwrap();
        let file = dir.join(project);
        std::fs::write(&file, contents).unwrap();
        std::fs::set_permissions(file, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    /// Every daemon's standard error so far.
    pub fn stderr(&self) -> String {
        std::fs::read_to_string(self.stderr_log()).unwrap_or_default()
    }

    /// Session `id`'s temporary folder, its agent's `TMPDIR` (#766).
    pub fn session_temp_dir(&self, id: &str) -> PathBuf {
        self.state_dir().join("tmp").join(id)
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

    /// The folder `project`'s engine proxy shares with the daemon (#390), in the daemon's mount
    /// sources; the daemon makes it when it starts that proxy.
    pub fn engine_proxy_folder(&self, project: &str) -> PathBuf {
        self.path()
            .join("run")
            .join(BUILD_PROFILE_DIR)
            .join("mounts")
            .join(format!("engine-{project}"))
    }

    /// The directory of session `id`, which its launch creates.
    pub fn session_dir(&self, id: &str) -> PathBuf {
        self.path()
            .join("run")
            .join(BUILD_PROFILE_DIR)
            .join("sessions")
            .join(id)
    }

    /// `DBUS_SESSION_BUS_ADDRESS` as systemd sets it for the user running the tests (#435).
    pub fn owner_bus(&self) -> String {
        use std::os::unix::fs::MetadataExt;
        let uid = std::fs::metadata(self.path()).unwrap().uid();
        format!("unix:path=/run/user/{uid}/bus")
    }

    /// The daemon's tmux server of `profile_dir`, named by path: `-L` falls back to tmux's
    /// default socket directory when this home's is gone (#845).
    pub fn tmux_socket_of(&self, profile_dir: &str) -> PathBuf {
        use std::os::unix::fs::MetadataExt;
        let tmux_tmpdir = self.path().join("tmux");
        let uid = std::fs::metadata(self.path()).expect("the home").uid();
        tmux_tmpdir.join(format!("tmux-{uid}")).join(profile_dir)
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

    /// The daemon's log once it holds `needle`, or as it is after [`GENEROUS`].
    pub async fn log_once_it_contains(&self, needle: &str) -> String {
        let file = self.state_dir().join("logs/daemon/cockpit.log");
        let deadline = std::time::Instant::now() + GENEROUS;
        loop {
            let log = std::fs::read_to_string(&file).unwrap_or_default();
            if log.contains(needle) || std::time::Instant::now() > deadline {
                return log;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Writes the owner's `config.toml`, which the next daemon start reads. The low-disk guard
    /// is off in it (#838), so a test's result never depends on the host's free space; a test
    /// of the guard sets `low_disk_floor_mib` in `contents` itself.
    pub fn write_owner_config(&self, contents: &str) {
        let guard_is_set = contents.contains("low_disk_floor_mib");
        assert!(
            guard_is_set || !contents.contains("[storage]"),
            "a [storage] table must also set low_disk_floor_mib: the harness appends its own"
        );
        let contents = if guard_is_set {
            contents.to_owned()
        } else {
            format!("{contents}\n[storage]\nlow_disk_floor_mib = 0\n")
        };
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
        candidate::configure(&mut command);
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

    /// The words each of `session`'s launches gave `claude`, space-joined, oldest first.
    pub fn arguments_of(&self, session: &str) -> Vec<String> {
        let mut log = self.launch_log().into_os_string();
        log.push(".arguments");
        std::fs::read_to_string(log)
            .unwrap_or_default()
            .lines()
            .filter_map(|line| line.strip_prefix(&format!("{session} ")))
            .map(str::to_owned)
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

    /// [`Home::start`] with `first` ahead of everything on the daemon's `PATH`, and its
    /// agents' with it: the owner's toolchain folders (#859).
    pub fn start_with_path_first(&self, first: &[PathBuf]) -> Daemon {
        let path = std::env::var_os("PATH").unwrap_or_default();
        let mut search = first.to_vec();
        search.push(self.path().join("bin"));
        search.extend(std::env::split_paths(&path));
        self.spawn(search, &[], None)
    }

    /// [`Home::start`] on a host without bubblewrap (#829): each folder of the host's `PATH`
    /// that holds `bwrap` is replaced by one of links to everything else it holds, so the
    /// daemon finds its other programs but no sandbox.
    pub fn start_without_bubblewrap(&self) -> Daemon {
        let path = std::env::var_os("PATH").unwrap_or_default();
        let mut search = vec![self.path().join("bin")];
        for (index, folder) in std::env::split_paths(&path).enumerate() {
            if !folder.join("bwrap").exists() {
                search.push(folder);
                continue;
            }
            let without = self.path().join(format!("path-without-bwrap-{index}"));
            std::fs::create_dir_all(&without).unwrap();
            for entry in std::fs::read_dir(&folder).unwrap().flatten() {
                if entry.file_name() != "bwrap" {
                    std::os::unix::fs::symlink(entry.path(), without.join(entry.file_name()))
                        .unwrap();
                }
            }
            search.push(without);
        }
        self.spawn(search, &[], None)
    }

    /// [`Home::start`] with `--profile profile`; it is ready on that profile's socket.
    pub fn start_profile(&self, profile: &str) -> Daemon {
        let path = std::env::var_os("PATH").unwrap_or_default();
        let mut search = vec![self.path().join("bin")];
        search.extend(std::env::split_paths(&path));
        self.spawn(search, &[], Some(profile))
    }

    fn spawn(&self, search: Vec<PathBuf>, extra: &[(&str, &str)], profile: Option<&str>) -> Daemon {
        let profile_dir = match profile {
            None => BUILD_PROFILE_DIR.to_owned(),
            Some(profile) => cockpit_config::Profile::parse(profile)
                .expect("a profile name")
                .qualified_name(),
        };
        // A home without the owner's config keeps none: its tests assert the layout the
        // daemon creates, and run no approved command that would need the opt-out.
        if self.has_owner_config {
            candidate::unconfine_runs(
                &self
                    .path()
                    .join("config")
                    .join(&profile_dir)
                    .join("config.toml"),
            );
        }
        let mut command = Process::new(candidate::program("torlek-daemon"));
        command
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
            );
        candidate::configure(&mut command);
        let mut child = command.spawn().expect("the daemon binary runs");
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
        candidate::assert_running_candidate(daemon.child.id());
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
        self.project_at(client, name, None).await
    }

    /// A Git repository at `projects/<relative>` registered through the daemon as `name`
    /// (the directory's name when `None`); returns its root and the project's name. The fake
    /// agent acts as the root's last component says, so a second project can run the same
    /// agent under another name, for example `worker/commanded` (#713).
    pub async fn project_at(
        &self,
        client: &Connection,
        relative: &str,
        name: Option<&str>,
    ) -> (PathBuf, String) {
        let root = self.path().join("projects").join(relative);
        std::fs::create_dir_all(&root).unwrap();
        git(&root, &["init", "--quiet", "--initial-branch=main"]);
        std::fs::write(root.join("README"), "fixture\n").unwrap();
        git(&root, &["add", "."]);
        git(&root, &["commit", "--quiet", "-m", "initial"]);
        let added = client
            .request(
                Command::ProjectAdd {
                    path: root.display().to_string(),
                    name: name.map(str::to_owned),
                },
                GENEROUS,
            )
            .await;
        match added {
            Ok(Reply::ProjectAdded { project, .. }) => (root, project.name),
            other => panic!("project-add {relative}: {other:?}"),
        }
    }
}

impl Home {
    /// Runs `project`'s `check` as the owner and waits for its receipt.
    pub async fn run_check(&self, project: &str, check: &str) -> CheckReceiptView {
        let client = self.connect().await;
        let run = match client
            .request(
                Command::CheckRun {
                    project: project.to_owned(),
                    check: check.to_owned(),
                    checkout: None,
                },
                GENEROUS,
            )
            .await
        {
            Ok(Reply::CheckRunStarted { run }) => run,
            other => panic!("expected a started run, got {other:?}"),
        };
        let deadline = Instant::now() + GENEROUS * 3;
        loop {
            let reply = client
                .request(
                    Command::CheckReceipt {
                        project: project.to_owned(),
                        run: run.clone(),
                    },
                    GENEROUS,
                )
                .await;
            match reply {
                Ok(Reply::CheckReceipt {
                    receipt: Some(receipt),
                }) => return *receipt,
                Ok(Reply::CheckReceipt { receipt: None }) if Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                other => panic!(
                    "expected the receipt of {run}, got {other:?}; {}",
                    self.stderr()
                ),
            }
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

    /// The words the [`COMMANDED_PROJECT`] agent working in `root` was launched with.
    pub async fn launch_arguments(&self, root: &Path) -> Vec<String> {
        let written = root.join(".agent").join("launch.arguments");
        let deadline = Instant::now() + SETTLED_WITHIN;
        loop {
            if let Ok(words) = std::fs::read_to_string(&written) {
                return words.split_terminator('\0').map(str::to_owned).collect();
            }
            assert!(
                Instant::now() < deadline,
                "the agent in {} wrote no launch arguments",
                root.display()
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Starts a context in the [`COMMANDED_PROJECT`] agent working in `root` as Claude Code
    /// does (#419): runs the `SessionStart` hook its launch's `--settings` subscribes, with a
    /// new conversation's payload on its input, in the agent. The `asyncRewake` inbox waiter
    /// Claude Code leaves running is not run. Returns what the hook printed, ending
    /// `exit=<code>`.
    pub async fn start_claude_context(&self, root: &Path) -> String {
        self.start_claude_context_from(root, "startup").await
    }

    /// As [`Home::start_claude_context`], for a context started from `source`: `startup`,
    /// `resume`, `clear` or `compact` (#781).
    pub async fn start_claude_context_from(&self, root: &Path, source: &str) -> String {
        let arguments = self.launch_arguments(root).await;
        let settings: serde_json::Value =
            serde_json::from_str(launch_option(&arguments, "--settings")).unwrap();
        let hooks: Vec<&str> = settings["hooks"]["SessionStart"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|matcher| matcher["hooks"].as_array().into_iter().flatten())
            .filter(|hook| hook["asyncRewake"] != true)
            .filter_map(|hook| hook["command"].as_str())
            .collect();
        let [hook] = hooks.as_slice() else {
            panic!("one SessionStart hook to wait for, not {hooks:?}");
        };
        let payload = serde_json::json!({
            "session_id": launch_option(&arguments, "--session-id"),
            "transcript_path": root.join(".agent/transcript.jsonl"),
            "cwd": root,
            "hook_event_name": "SessionStart",
            "source": source,
        });
        std::fs::write(root.join(".agent/session-start.json"), payload.to_string()).unwrap();
        // Claude Code reads the hook's standard output alone.
        let number = self.command_agent(
            root,
            &format!("{hook} < .agent/session-start.json 2> .agent/session-start.err"),
        );
        self.agent_output(root, number).await
    }

    /// Asks the MCP server the [`COMMANDED_PROJECT`] agent's launch registers with
    /// `--mcp-config` for tool `tool`, from the agent, as Claude Code would (#419). Returns
    /// what the server printed, one answer per line, ending `exit=<code>`.
    pub async fn call_claude_mcp_tool(&self, root: &Path, tool: &str) -> String {
        let arguments = self.launch_arguments(root).await;
        let config: serde_json::Value =
            serde_json::from_str(launch_option(&arguments, "--mcp-config")).unwrap();
        let servers = config["mcpServers"].as_object().unwrap();
        let [(_, server)] = servers.iter().collect::<Vec<_>>()[..] else {
            panic!("one MCP server, not {servers:?}");
        };
        // As Claude Code starts it: its own `env` on top of the agent's, then the command.
        let variables = server["env"]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(name, value)| format!("{name}={}", value.as_str().unwrap()));
        let words: Vec<String> = variables
            .chain([server["command"].as_str().unwrap().to_owned()])
            .chain(
                server["args"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|argument| argument.as_str().unwrap().to_owned()),
            )
            .map(|word| shell_quoted(&word))
            .collect();
        let messages = [
            serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {"protocolVersion": "2025-06-18", "capabilities": {},
                           "clientInfo": {"name": "fake-claude", "version": "1"}}}),
            serde_json::json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
            serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
                "params": {"name": tool, "arguments": {}}}),
        ];
        let input: String = messages
            .iter()
            .map(|message| format!("{message}\n"))
            .collect();
        std::fs::write(root.join(".agent/mcp.in"), input).unwrap();
        let line = format!("env {} < .agent/mcp.in 2> .agent/mcp.err", words.join(" "));
        let number = self.command_agent(root, &line);
        self.agent_output(root, number).await
    }

    /// What the [`COMMANDED_PROJECT`] agent's line `number` printed, ending `exit=<code>`.
    pub async fn agent_output(&self, root: &Path, number: usize) -> String {
        self.agent_output_within(root, number, SETTLED_WITHIN).await
    }

    /// [`Home::agent_output`] for a line that may take up to `within`, a container's pull.
    pub async fn agent_output_within(
        &self,
        root: &Path,
        number: usize,
        within: Duration,
    ) -> String {
        let output = root.join(".agent").join(format!("{number}.out"));
        let deadline = Instant::now() + within;
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
            let socket = self.tmux_socket_of(profile_dir);
            if socket.exists() {
                let _ = Process::new("tmux")
                    .env_remove("TMUX")
                    .arg("-S")
                    .arg(socket)
                    .arg("kill-server")
                    .output();
            }
        }
    }
}

pub struct Daemon {
    child: Child,
}

/// The `additionalContext` of the `SessionStart` answer [`Home::start_claude_context`]
/// printed, after checking the hook succeeded.
pub fn added_context(printed: &str) -> String {
    let answer = printed
        .strip_suffix("exit=0\n")
        .unwrap_or_else(|| panic!("the hook failed: {printed}"));
    let answer: serde_json::Value = serde_json::from_str(answer)
        .unwrap_or_else(|error| panic!("{error}: the hook answered {answer}"));
    assert_eq!(
        answer["hookSpecificOutput"]["hookEventName"], "SessionStart",
        "{answer}"
    );
    answer["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap_or_else(|| panic!("no context added in {answer}"))
        .to_owned()
}

/// The text of the `tools/call` result [`Home::call_claude_mcp_tool`] printed, after checking
/// the server initialized and the call succeeded.
pub fn mcp_tool_text(printed: &str) -> String {
    let answers = printed
        .strip_suffix("exit=0\n")
        .unwrap_or_else(|| panic!("the MCP server failed: {printed}"));
    let answers: Vec<serde_json::Value> = answers
        .lines()
        .map(|line| {
            serde_json::from_str(line)
                .unwrap_or_else(|error| panic!("{error}: the MCP server printed {line}"))
        })
        .collect();
    let [initialized, called] = answers.as_slice() else {
        panic!("two answers, not {answers:?}");
    };
    assert!(
        initialized["result"]["serverInfo"].is_object(),
        "{initialized}"
    );
    assert_eq!(called["result"]["isError"], false, "{called}");
    called["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// `word` as one shell word.
fn shell_quoted(word: &str) -> String {
    format!("'{}'", word.replace('\'', "'\\''"))
}

/// The word after `flag` in a launch's `arguments`.
pub fn launch_option<'a>(arguments: &'a [String], flag: &str) -> &'a str {
    arguments
        .iter()
        .position(|argument| argument == flag)
        .and_then(|at| arguments.get(at + 1))
        .unwrap_or_else(|| panic!("no {flag} in {arguments:?}"))
}

/// The `cockpit` program beside the daemon's, where the daemon looks for it too. Cargo
/// builds it there for the workspace run (`make crates-test`), not for a run of this
/// package alone.
pub fn cockpit_program() -> PathBuf {
    candidate::program("torlek")
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
        model: None,
        effort: None,
        compaction_threshold: None,
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

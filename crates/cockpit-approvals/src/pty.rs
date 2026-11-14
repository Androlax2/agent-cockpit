//! Running an approved command on the host in a pseudo-terminal (report 17): programs see a
//! real terminal, so `sudo` and other prompts work, and the owner can type into it. Git runs
//! with hooks off and its program-running settings reset. What comes back to the agent is
//! the end of the output, without terminal controls, secrets redacted. A secret the owner
//! typed for the run is redacted as the output is read, before the daemon keeps any of it
//! (#1386, `typed_secret.rs`).
//!
//! Each run is a `cockpit-processes` terminal execution (#163): its own session in a
//! systemd scope or process group, led by `cockpit_processes::run_session_leader`, so it can
//! be terminated whole and a restart can tell what became of it. A confined run (#1387) is
//! a systemd user service instead, with no network, its project's checkout the one folder it
//! writes and the owner's secrets hidden; it gets no sudo, which needs the root route that
//! confined runs do not have yet.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{ErrorKind, Read, Write};
use std::os::fd::OwnedFd;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use cockpit_processes::{
    Confinement, EndReason, Escalation, ExecutionId, LaunchSpec, Phase, Supervisor, Terminal,
    Termination,
};
use cockpit_turso::Database;
use regex::Regex;
use rustix::fs::{Mode, OFlags};
use rustix::io::FdFlags;
use rustix::pty::OpenptFlags;
use rustix::termios::{LocalModes, Winsize};
use tokio::sync::oneshot;

use crate::egress::{self, EgressProxy, RunEgress};
use crate::error::invalid;
use crate::redact::{redact_secrets, redact_secrets_after_cut, strip_terminal_controls};
use crate::runs::{Approved, RunId, RunRequest, RunResult, start_run};
use crate::sudo::{SudoAskpass, SudoAsks};
use crate::sudo_unasked::{self, SudoSightings};
use crate::typed_secret::{TypedSecrets, wipe};
use crate::{Error, Refusal};

/// After the command exits, how long output still in the terminal may take to arrive. A
/// background process that keeps the terminal open is not waited for.
const DRAIN_AFTER_EXIT: Duration = Duration::from_secs(2);
/// How long a run's prompt-like output must have been silent to count as waiting for input.
const PROMPT_IDLE: Duration = Duration::from_millis(1500);
/// Prompt punctuation, short textual choices, or a field's parenthesized default.
static PROMPT_END: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"(?i)(?:[:?>]",
        r"|\[[a-z?][a-z?,/ ]{0,63}\]",
        r"|\([a-z?][a-z?,/ \[\]]{0,63}\)",
        r"|:\s*\([a-z0-9_][a-z0-9 ._-]{0,63}\))\s*\z",
    ))
    .expect("the pattern compiles")
});
/// A carriage return or a move to the first column starts a new drawn frame.
static LINE_RESTARTS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\r|\x1b\[[01]?G").expect("the pattern compiles"));
const SYNTAX_CHECK_TIMEOUT: Duration = Duration::from_secs(10);
/// How the owner's stop ends a run (#1301): a prompt ends at `SIGTERM`; whatever ignores it
/// gets `SIGKILL` soon after, so the owner's request is not held long.
const OWNER_STOP: Escalation = Escalation {
    grace: Duration::from_secs(3),
    kill_wait: Duration::from_secs(5),
};
/// The programs whose `-n` reads commands without running them (POSIX `set -n`, fish's
/// `--no-execute`), by the name of the file a shell path resolves to. Elsewhere `-n` may
/// mean something else: nushell's is `--no-config-file`, so it would run the command.
const PARSE_ONLY_SHELLS: &[&str] = &["sh", "bash", "dash", "zsh", "ksh", "ksh93", "mksh", "fish"];

/// What the check before the owner is asked found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Syntax {
    /// The shell parsed the command without running it.
    Parsed,
    /// The shell has no parse-only mode known here, so it was not given the command.
    NotChecked,
}

/// The program `shell` resolves to, when it is known to parse without running under `-n`:
/// judged by the file its symlinks resolve to, so `/bin/sh` is whichever of dash or bash it
/// is, and a link named `bash` to nushell is nushell. The check starts this resolved file,
/// so a link retargeted after the judgement cannot swap in another program.
fn parse_only_program(shell: &Path) -> std::io::Result<Option<PathBuf>> {
    let program = std::fs::canonicalize(shell)?;
    let is_parse_only = program
        .file_name()
        .and_then(OsStr::to_str)
        .is_some_and(|name| PARSE_ONLY_SHELLS.contains(&name));
    Ok(is_parse_only.then_some(program))
}

/// How runs are started and bounded.
#[derive(Debug, Clone)]
pub struct RunSettings {
    /// The shell that runs each command as `<shell> -c <command>` and, when it is known to
    /// parse without running under `-n`, checks it first with `<shell> -n -c <command>`.
    pub shell: PathBuf,
    /// The session leader each run starts under (`cockpit_processes::Terminal`).
    pub leader: PathBuf,
    pub leader_arguments: Vec<OsString>,
    /// The run's whole environment, besides Git's: nothing else is inherited. Variables whose
    /// name starts with `GIT_` are dropped, so none can point Git elsewhere, and so are the
    /// session's variables.
    pub environment: Vec<(OsString, OsString)>,
    /// The end of the output kept; what came before is counted, not kept.
    pub kept_bytes: usize,
    /// Lines of output the agent's answer quotes; the rest are counted.
    pub answer_lines: usize,
    pub rows: u16,
    pub columns: u16,
    /// How `sudo` in a run asks the owner (#1374); `None` leaves sudo to the terminal. A
    /// confined run never gets it.
    pub sudo: Option<SudoAskpass>,
    pub confinement: RunConfinement,
}

/// Whether runs are confined (#1387), and what a confined run cannot see.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunConfinement {
    /// Each run in its own systemd user service: no network, a `/tmp` of its own, every
    /// folder read-only but its project's checkout, `hidden` unreadable and `emptied` shown
    /// empty but for the `reachable` folders in them. A host that cannot start one refuses
    /// the run.
    Required {
        hidden: Vec<PathBuf>,
        emptied: Vec<PathBuf>,
        reachable: Vec<PathBuf>,
        /// The proxy a run whose card names hosts reaches them through; `None` refuses such
        /// a run.
        egress: Option<EgressProxy>,
    },
    /// The owner's one named opt-out, `agents.confined_runs = "off"`: runs have the owner's
    /// rights.
    Off,
}

impl RunConfinement {
    /// The confined folder `path` is in, if any: a run cannot start there.
    fn hiding(&self, path: &Path) -> Option<&Path> {
        let RunConfinement::Required {
            hidden, emptied, ..
        } = self
        else {
            return None;
        };
        hidden
            .iter()
            .chain(emptied)
            .map(PathBuf::as_path)
            .find(|folder| path.starts_with(folder))
    }

    /// What a confined run's `output` says its limits refused; nothing for an unconfined run.
    fn limit_hits_in(&self, output: &[u8]) -> Vec<String> {
        let RunConfinement::Required {
            hidden, emptied, ..
        } = self
        else {
            return Vec::new();
        };
        let text = strip_terminal_controls(&String::from_utf8_lossy(output));
        let unseen: Vec<PathBuf> = hidden.iter().chain(emptied).cloned().collect();
        crate::limit_hits(&text, &unseen)
            .into_iter()
            .map(|hit| redact_secrets(&hit).0)
            .collect()
    }
}

impl RunSettings {
    pub fn new(shell: PathBuf, leader: PathBuf, leader_arguments: Vec<OsString>) -> RunSettings {
        RunSettings {
            shell,
            leader,
            leader_arguments,
            environment: Vec::new(),
            kept_bytes: ANSWER_MAX_BYTES,
            answer_lines: 120,
            rows: 40,
            columns: 120,
            sudo: None,
            confinement: RunConfinement::Required {
                hidden: Vec::new(),
                emptied: Vec::new(),
                reachable: Vec::new(),
                egress: None,
            },
        }
    }

    /// The environment, with `git_config` passed as `GIT_CONFIG_*` so every Git in the run
    /// applies it over the repository's own configuration.
    pub fn run_environment(&self, git_config: &[String]) -> Vec<(OsString, OsString)> {
        self.environment_with(git_config, self.sudo.as_ref())
    }

    fn environment_with(
        &self,
        git_config: &[String],
        sudo: Option<&SudoAskpass>,
    ) -> Vec<(OsString, OsString)> {
        let mut environment: Vec<(OsString, OsString)> = self
            .environment
            .iter()
            .filter(|(name, _)| !is_withheld_variable(name))
            .cloned()
            .collect();
        environment.push((
            "GIT_CONFIG_COUNT".into(),
            git_config.len().to_string().into(),
        ));
        for (index, setting) in git_config.iter().enumerate() {
            let (key, value) = setting.split_once('=').unwrap_or((setting, ""));
            environment.push((format!("GIT_CONFIG_KEY_{index}").into(), key.into()));
            environment.push((format!("GIT_CONFIG_VALUE_{index}").into(), value.into()));
        }
        if let Some(sudo) = sudo {
            sudo.apply(&mut environment);
        }
        environment
    }

    /// The askpass a run's sudo asks through: none in a confined run.
    fn sudo(&self) -> Option<&SudoAskpass> {
        match self.confinement {
            RunConfinement::Required { .. } => None,
            RunConfinement::Off => self.sudo.as_ref(),
        }
    }

    /// A run's environment: a confined run's `TMPDIR` is its own `/tmp`, and one with a
    /// proxy sends its clients through it.
    fn launch_environment(
        &self,
        git_config: &[String],
        has_proxy: bool,
    ) -> Vec<(OsString, OsString)> {
        let mut environment = self.environment_with(git_config, self.sudo());
        if let RunConfinement::Required { .. } = self.confinement {
            environment.retain(|(name, _)| name != "TMPDIR");
            environment.push(("TMPDIR".into(), "/tmp".into()));
        }
        if has_proxy {
            environment.retain(|(name, _)| !egress::is_proxy_variable(name));
            environment.extend(egress::proxy_variables());
        }
        environment
    }
}

/// What a run produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ran {
    pub result: RunResult,
    /// A process the command left in the background still held the terminal when the
    /// command exited; what it printed afterwards is not in the result.
    pub terminal_held_open: bool,
    /// The run's execution when a process the command left behind still ran in it once the
    /// command exited (see [`Runner::wait_until_ended`]).
    pub leftover: Option<ExecutionId>,
}

/// A run's output as it may reach the agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    /// The last lines of the kept output, for the agent's answer.
    pub shown: String,
    /// Lines of the kept output left out of `shown`.
    pub omitted_lines: u64,
    /// Bytes from the start of the output that were not kept at all.
    pub truncated_bytes: u64,
    /// How many secret-looking values were replaced by `[redacted]`.
    pub redacted: u64,
}

/// Why a run has no result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoResult {
    /// Nothing ran.
    NotStarted(String),
    /// The command started, but its end was not observed: it may have done anything.
    Unobserved(String),
}

/// Starts approved runs and keeps the ones in progress, for input and for the queue.
#[derive(Debug, Clone)]
pub struct Runner {
    pub(crate) settings: Arc<RunSettings>,
    pub(crate) supervisor: Supervisor,
    live: Arc<Mutex<HashMap<RunId, Arc<LiveRun>>>>,
}

#[derive(Debug)]
pub(crate) struct LiveRun {
    execution: ExecutionId,
    /// The terminal session the command leads, once launched: its process group.
    session: OnceLock<i32>,
    input: Mutex<File>,
    output: Mutex<Kept>,
    is_stopped_by_owner: AtomicBool,
    sudo_asks: Mutex<SudoAsks>,
    sudo_sightings: Mutex<SudoSightings>,
}

impl LiveRun {
    /// Fingerprints `secret`, typed for run `id`, so its output redacts it (#1386), then wipes
    /// it. Refused when the run's key cannot be drawn: the secret must not be handed on
    /// unprotected.
    pub(crate) fn protect_typed_secret(
        &self,
        id: RunId,
        mut secret: Vec<u8>,
    ) -> Result<(), Refusal> {
        let added = self
            .output
            .lock()
            .expect("the output lock is never poisoned")
            .typed
            .add(&mut secret);
        crate::typed_secret::wipe_vec(&mut secret);
        added.map_err(|error| Refusal::SecretUnprotected {
            request: id.0,
            why: format!("/dev/urandom: {error}"),
        })
    }

    pub(crate) fn session(&self) -> Option<i32> {
        self.session.get().copied()
    }

    pub(crate) fn sudo_asks(&self) -> MutexGuard<'_, SudoAsks> {
        self.sudo_asks
            .lock()
            .expect("the sudo asks lock is never poisoned")
    }

    pub(crate) fn sudo_sightings(&self) -> MutexGuard<'_, SudoSightings> {
        self.sudo_sightings
            .lock()
            .expect("the sudo sightings lock is never poisoned")
    }
}

#[derive(Debug)]
struct Kept {
    /// The output as read, typed secrets redacted.
    bytes: Vec<u8>,
    /// Bytes appended to `bytes` so far, including those trimmed from its start.
    total: u64,
    last_output: Instant,
    /// The fingerprints of what the owner typed, and the bytes held back until it is known
    /// whether they begin one.
    typed: TypedSecrets,
    /// The run's result was taken and its fingerprints dropped: what a process still holding
    /// the terminal prints now is read and discarded, never kept unscrubbed.
    is_finished: bool,
}

impl Kept {
    fn append(&mut self, chunk: &[u8]) {
        if self.is_finished {
            return;
        }
        let before = self.bytes.len();
        self.typed.scrub_into(chunk, &mut self.bytes);
        self.total += (self.bytes.len() - before) as u64;
    }

    /// Appends the bytes held back: the output ended, or went quiet long enough that what it
    /// shows last must be seen (a prompt).
    fn release_held(&mut self) {
        let before = self.bytes.len();
        self.typed.release(&mut self.bytes);
        self.total += (self.bytes.len() - before) as u64;
    }

    fn release_held_if_quiet(&mut self, now: Instant) {
        if now.duration_since(self.last_output) >= PROMPT_IDLE {
            self.release_held();
        }
    }
}

impl Runner {
    /// Runs launch through `supervisor`, the daemon's one owner of executions.
    pub fn new(settings: RunSettings, supervisor: Supervisor) -> Runner {
        Runner {
            settings: Arc::new(settings),
            supervisor,
            live: Arc::default(),
        }
    }

    /// Whether the shell can parse `command`, asked before the owner is: a command that would
    /// only fail to parse after approval interrupts the owner for nothing. A shell not known
    /// to parse without running is never given the command before approval. The outer error
    /// is a check that could not be made.
    pub async fn check_syntax(&self, command: &str) -> std::io::Result<Result<Syntax, Refusal>> {
        let Some(program) = parse_only_program(&self.settings.shell)? else {
            return Ok(Ok(Syntax::NotChecked));
        };
        let mut check = tokio::process::Command::new(program);
        // The name it is started by: `sh` linked to bash keeps bash in POSIX mode.
        check
            .arg0(&self.settings.shell)
            .arg("-n")
            .arg("-c")
            .arg(command)
            .env_clear()
            .envs(self.settings.run_environment(&[]))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let output = tokio::time::timeout(SYNTAX_CHECK_TIMEOUT, check.output())
            .await
            .map_err(|_| std::io::Error::new(ErrorKind::TimedOut, "the syntax check hung"))??;
        if output.status.success() {
            return Ok(Ok(Syntax::Parsed));
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        Ok(Err(invalid(
            "command",
            format!(
                "{} cannot parse it: {}; put the commands in a script file and ask to run it \
                 with its interpreter",
                self.settings.shell.display(),
                stderr.trim()
            ),
        )))
    }

    /// What `run` may do once approved, one line each, as its card shows the owner.
    pub fn card_limits(&self, run: &RunRequest) -> Vec<String> {
        let RunConfinement::Required {
            hidden, emptied, ..
        } = &self.settings.confinement
        else {
            return vec![
                "UNCONFINED: runs with your rights (`agents.confined_runs = \"off\"`)".to_owned(),
            ];
        };
        let writes: Vec<String> = run
            .project
            .iter()
            .map(|project| project.root.display().to_string())
            .chain(
                run.reach
                    .writes
                    .iter()
                    .map(|write| write.display().to_string()),
            )
            .chain(["a /tmp of its own".to_owned()])
            .collect();
        let unseen: Vec<String> = hidden
            .iter()
            .chain(emptied)
            .map(|folder| folder.display().to_string())
            .collect();
        let network = if run.reach.hosts.is_empty() {
            "no network".to_owned()
        } else {
            format!("reaches only {}", run.reach.hosts.join(", "))
        };
        [
            Some(format!("writes only {}", listed(&writes))),
            (!unseen.is_empty()).then(|| format!("cannot see {}", unseen.join(", "))),
            Some(network),
            Some(
                "no sudo: it needs the root route, which confined runs do not have yet".to_owned(),
            ),
        ]
        .into_iter()
        .flatten()
        .collect()
    }

    /// The shell, when [`Runner::check_syntax`] does not give it commands, so they reach the
    /// owner unchecked. A shell that cannot be resolved is named too: it checks nothing.
    pub fn unchecked_shell(&self) -> Option<&Path> {
        let shell = &self.settings.shell;
        match parse_only_program(shell) {
            Ok(Some(_)) => None,
            Ok(None) | Err(_) => Some(shell.as_path()),
        }
    }

    /// Marks `run` running with a newly reserved execution and runs it to its end, every Git
    /// in it applying `git_config` (`key=value` settings) over the repository's own
    /// configuration.
    pub async fn run(
        &self,
        database: &Database,
        run: &Approved,
        git_config: &[String],
    ) -> Result<Ran, NoResult> {
        if run.start.is_some() {
            return Err(NoResult::NotStarted(
                "a start request is carried out by the daemon, not run as a command".to_owned(),
            ));
        }
        match std::fs::canonicalize(&run.cwd) {
            Ok(resolved) if resolved == run.cwd => {}
            Ok(resolved) => {
                return Err(NoResult::NotStarted(format!(
                    "{} now resolves to {}, not the approved directory",
                    run.cwd.display(),
                    resolved.display()
                )));
            }
            Err(error) => {
                return Err(NoResult::NotStarted(format!(
                    "{}: {error}",
                    run.cwd.display()
                )));
            }
        }
        for path in std::iter::once(&run.cwd).chain(&run.reach.writes) {
            if let Some(folder) = self.settings.confinement.hiding(path) {
                return Err(NoResult::NotStarted(format!(
                    "{} is in {}, which confined runs cannot see",
                    path.display(),
                    folder.display()
                )));
            }
        }
        let not_started =
            |what: &str, error: std::io::Error| NoResult::NotStarted(format!("{what}: {error}"));
        if let Some(sudo) = self.settings.sudo() {
            sudo.write_launchers().map_err(|error| {
                not_started(
                    &format!(
                        "cannot write the run's sudo launchers in {}",
                        sudo.launchers.display()
                    ),
                    error,
                )
            })?;
        }
        let egress = match &self.settings.confinement {
            RunConfinement::Required { egress, .. } if !run.reach.hosts.is_empty() => {
                let proxy = egress.as_ref().ok_or_else(|| {
                    NoResult::NotStarted(
                        "its card names hosts, and this daemon has no proxy to reach them \
                         through (`torlek-sandbox`)"
                            .to_owned(),
                    )
                })?;
                Some(proxy.start(run).await.map_err(NoResult::NotStarted)?)
            }
            RunConfinement::Required { .. } | RunConfinement::Off => None,
        };
        let (master, terminal) = open_terminal(self.settings.rows, self.settings.columns)
            .map_err(|error| not_started("cannot open a terminal for the command", error))?;
        let reader = master
            .try_clone()
            .map_err(|error| not_started("cannot read the terminal", error))?;
        let execution = self.mark_running(database, run).await.map_err(|error| {
            NoResult::NotStarted(format!("the run could not be marked running: {error}"))
        })?;
        let live = Arc::new(LiveRun {
            execution,
            session: OnceLock::new(),
            input: Mutex::new(master),
            output: Mutex::new(Kept {
                bytes: Vec::new(),
                total: 0,
                last_output: Instant::now(),
                typed: TypedSecrets::default(),
                is_finished: false,
            }),
            is_stopped_by_owner: AtomicBool::new(false),
            sudo_asks: Mutex::default(),
            sudo_sightings: Mutex::default(),
        });
        self.live
            .lock()
            .expect("the live runs lock is never poisoned")
            .insert(run.id, Arc::clone(&live));
        let drained = drain(reader, Arc::clone(&live), self.settings.kept_bytes);
        let exited = self
            .launch(database, run, &live, terminal, git_config, egress.as_ref())
            .await;
        let refused = match egress {
            Some(egress) => egress.end().await,
            None => Ok(Vec::new()),
        };
        // Read as the exit is observed: a stop sent after it did not end the command.
        let is_stopped_by_owner = live.is_stopped_by_owner.load(Ordering::SeqCst);
        let terminal_held_open = tokio::time::timeout(DRAIN_AFTER_EXIT, drained)
            .await
            .is_err();
        self.live
            .lock()
            .expect("the live runs lock is never poisoned")
            .remove(&run.id);
        live.sudo_asks().close();
        let exit_code = match exited {
            Ok(exit_code) => exit_code,
            Err(NoResult::NotStarted(reason)) => {
                return Err(NoResult::NotStarted(with_terminal_output(reason, &live)));
            }
            Err(unobserved) => return Err(unobserved),
        };
        let is_left_running = match self.supervisor.observe(database, execution).await {
            Ok(observed) => observed.phase.is_occupying(),
            // Not observed ended, so it stays charged and is watched.
            Err(_) => true,
        };
        let mut kept = live
            .output
            .lock()
            .expect("the output lock is never poisoned");
        kept.release_held();
        let typed_secret_redacted = kept.typed.redacted();
        // The run ended: its key and fingerprints go with it.
        kept.typed = TypedSecrets::default();
        kept.is_finished = true;
        let tail = &kept.bytes[kept.bytes.len().saturating_sub(self.settings.kept_bytes)..];
        let limit_hits = crate::limit_hits::merged(
            self.settings.confinement.limit_hits_in(tail),
            match refused {
                Ok(refused) => refused
                    .iter()
                    .map(|destination| crate::limit_hits::refused_by_proxy(destination))
                    .collect(),
                Err(error) => vec![format!(
                    "its proxy could not say which hosts it refused: {error}"
                )],
            },
        );
        Ok(Ran {
            result: RunResult {
                exit_code,
                output: present(tail, kept.total, self.settings.answer_lines),
                output_retired: false,
                stopped_by_owner: is_stopped_by_owner,
                typed_secret_redacted,
                sudo_ran_unasked: {
                    let mut sightings = live.sudo_sightings();
                    sightings.end();
                    sightings.ran_unasked()
                },
                limit_hits,
            },
            terminal_held_open,
            leftover: is_left_running.then_some(execution),
        })
    }

    /// Reserves the run's execution and marks the run running with it, in one transaction.
    async fn mark_running(
        &self,
        database: &Database,
        run: &Approved,
    ) -> Result<ExecutionId, Error> {
        let write = database.write().await?;
        let execution = match self.settings.confinement {
            RunConfinement::Required { .. } => {
                self.supervisor
                    .reserve_confined(&write, database.epoch())
                    .await?
            }
            RunConfinement::Off => self.supervisor.reserve(&write, database.epoch()).await?,
        };
        start_run(&write, run, execution).await?;
        crate::runs::attribute_run_output(&write, run, execution, database.epoch()).await?;
        write.commit().await?;
        Ok(execution)
    }

    /// Launches the reserved execution in `terminal` and answers the command's exit code.
    async fn launch(
        &self,
        database: &Database,
        run: &Approved,
        live: &LiveRun,
        terminal: OwnedFd,
        git_config: &[String],
        egress: Option<&RunEgress>,
    ) -> Result<i32, NoResult> {
        let execution = live.execution;
        let (leader, leader_arguments) = match egress {
            Some(egress) => {
                egress.handing_over(&self.settings.leader, &self.settings.leader_arguments)
            }
            None => (
                self.settings.leader.clone(),
                self.settings.leader_arguments.clone(),
            ),
        };
        let spec = LaunchSpec {
            program: self.settings.shell.clone(),
            arguments: vec!["-c".into(), run.command.clone().into()],
            working_dir: run.cwd.clone(),
            environment: self
                .settings
                .launch_environment(git_config, egress.is_some()),
            terminal: Some(Terminal {
                device: terminal,
                leader,
                leader_arguments,
            }),
        };
        let launched = match &self.settings.confinement {
            RunConfinement::Required {
                hidden,
                emptied,
                reachable,
                ..
            } => {
                let confinement = Confinement {
                    writable: run
                        .project_root
                        .iter()
                        .chain(&run.reach.writes)
                        .cloned()
                        .collect(),
                    hidden: hidden.clone(),
                    emptied: emptied.clone(),
                    reachable: reachable
                        .iter()
                        .cloned()
                        .chain(egress.map(RunEgress::handoff_folder))
                        .collect(),
                };
                self.supervisor
                    .launch_confined(database, execution, &spec, &confinement)
                    .await
            }
            RunConfinement::Off => {
                self.supervisor
                    .launch_reserved(database, execution, &spec)
                    .await
            }
        };
        // The terminal must end with the run's processes for the reader to see the end of
        // the output: this copy goes as soon as the run has its own.
        drop(spec);
        let launched = match launched {
            Ok(launched) => launched,
            Err(error) => return Err(self.launch_failed(database, execution, &error).await),
        };
        if let Some(session) = launched.execution.process_group {
            let _ = live.session.set(session);
        }
        let watching = async {
            let leader = live
                .session()
                .and_then(|session| u32::try_from(session).ok());
            match (self.settings.sudo(), leader) {
                (Some(askpass), Some(leader)) => sudo_unasked::watch(askpass, live, leader).await,
                _ => std::future::pending().await,
            }
        };
        let exited = tokio::select! {
            exited = launched.exit => exited,
            never = watching => match never {},
        };
        match exited {
            Ok(Ok(status)) => Ok(status
                .code()
                .unwrap_or_else(|| 128 + status.signal().unwrap_or(0))),
            Ok(Err(error)) => Err(NoResult::Unobserved(format!(
                "the command's exit was not observed: {error}"
            ))),
            Err(_) => Err(NoResult::Unobserved(
                "the command's exit was not observed: its launcher was not reaped".to_owned(),
            )),
        }
    }

    /// A failed launch is `not-run` only when its execution is recorded never started;
    /// otherwise the program may have run.
    async fn launch_failed(
        &self,
        database: &Database,
        execution: ExecutionId,
        error: &cockpit_processes::Error,
    ) -> NoResult {
        match cockpit_processes::execution(database, execution).await {
            Ok(observed) if observed.phase == Phase::Ended(EndReason::NeverStarted) => {
                NoResult::NotStarted(format!(
                    "cannot start {}: {error}",
                    self.settings.leader.display()
                ))
            }
            _ => NoResult::Unobserved(format!("the command may have started: {error}")),
        }
    }

    pub(crate) fn live_run(&self, id: RunId) -> Option<Arc<LiveRun>> {
        self.live
            .lock()
            .expect("the live runs lock is never poisoned")
            .get(&id)
            .cloned()
    }

    /// How many sudo processes in the running run `id` ended having maybe reused an unlock
    /// (#1374): 0 when none, or when the run is not running here.
    pub fn sudo_ran_unasked(&self, id: RunId) -> u64 {
        self.live_run(id)
            .map_or(0, |live| live.sudo_sightings().ran_unasked())
    }

    /// How many times a secret the owner typed for the running run `id` was redacted from its
    /// output so far (#1386): 0 when none, or when the run is not running here.
    pub fn typed_secret_redacted(&self, id: RunId) -> u64 {
        self.live_run(id).map_or(0, |live| {
            live.output
                .lock()
                .expect("the output lock is never poisoned")
                .typed
                .redacted()
        })
    }

    pub(crate) fn live_runs(&self) -> Vec<(RunId, Arc<LiveRun>)> {
        self.live
            .lock()
            .expect("the live runs lock is never poisoned")
            .iter()
            .map(|(id, live)| (*id, Arc::clone(live)))
            .collect()
    }

    /// Types `text` and Enter into the running command, only while its terminal does not echo
    /// what is typed: an echo lands in the output the agent reads and the daemon stores, and
    /// the text may be a password (#1160). `is_shown` is the owner's opt-out for a visible
    /// answer, typed whatever the echo. Hidden text is a secret: it is fingerprinted first, so
    /// that the run printing it anyway shows `[redacted]` (#1386).
    pub fn send_input(&self, id: RunId, text: &str, is_shown: bool) -> Result<(), Refusal> {
        let live = self
            .live
            .lock()
            .expect("the live runs lock is never poisoned")
            .get(&id)
            .cloned()
            .ok_or(Refusal::NotRunning)?;
        let mut input = live.input.lock().expect("the input lock is never poisoned");
        if !is_shown {
            if let Some(why) = input_would_show(&input) {
                return Err(Refusal::InputWouldShow { request: id.0, why });
            }
            live.protect_typed_secret(id, text.as_bytes().to_vec())?;
        }
        input
            .write_all(text.as_bytes())
            .and_then(|()| input.write_all(b"\r"))
            .map_err(|_| Refusal::NotRunning)
    }

    /// The owner's stop of a running command (#1301): `SIGTERM`, then `SIGKILL`, to its whole
    /// execution, so a helper it waits on (`git-remote-https` under `git push`) ends with it.
    /// Answers how the termination went; once its processes were observed gone,
    /// [`Runner::run`] answers the run's result `stopped_by_owner`. Only a run this daemon
    /// started has a handle to stop: one that outlived a restart is refused.
    pub async fn stop(&self, database: &Database, id: RunId) -> Result<Termination, Error> {
        let live = self
            .live
            .lock()
            .expect("the live runs lock is never poisoned")
            .get(&id)
            .cloned()
            .ok_or_else(|| Refusal::NotStoppable {
                why: "the command did not start under this daemon: it outlived a daemon \
                      restart, or it ended"
                    .to_owned(),
            })?;
        // Set before the signal, so the run never reads its end without the owner's stop.
        live.is_stopped_by_owner.store(true, Ordering::SeqCst);
        let terminated = self
            .supervisor
            .terminate(database, live.execution, OWNER_STOP)
            .await;
        let has_signalled = match &terminated {
            Ok(Termination::Confirmed { after }) => after.is_some(),
            Ok(Termination::Unconfirmed { diagnostics }) => diagnostics.term.is_some(),
            Err(_) => false,
        };
        if !has_signalled {
            live.is_stopped_by_owner.store(false, Ordering::SeqCst);
        }
        Ok(terminated?)
    }

    /// The runs silent a while with prompt-like output ending mid-line, and those whose sudo
    /// asks the owner (#1374). Read at one instant, so a queue sorted by it stays consistent.
    pub fn waiting_for_input(&self) -> Vec<RunId> {
        let now = Instant::now();
        self.live
            .lock()
            .expect("the live runs lock is never poisoned")
            .iter()
            .filter(|(_, live)| {
                if live.sudo_asks().is_waiting() {
                    return true;
                }
                let mut kept = live
                    .output
                    .lock()
                    .expect("the output lock is never poisoned");
                kept.release_held_if_quiet(now);
                is_at_prompt(&kept, now)
            })
            .map(|(id, _)| *id)
            .collect()
    }

    /// What `id` asks while it waits for input (#1010): the line a terminal shows after its
    /// output's last newline. `None` unless the run waits at a prompt now, when the line is
    /// blank, or when its start was dropped to keep the output bounded: a secret cut there
    /// would no longer match a redaction pattern.
    pub fn prompt(&self, id: RunId) -> Option<String> {
        let live = self
            .live
            .lock()
            .expect("the live runs lock is never poisoned")
            .get(&id)
            .cloned()?;
        // Copied out, so the reader is not held up while the line is redacted.
        let line = {
            let mut kept = live
                .output
                .lock()
                .expect("the output lock is never poisoned");
            let now = Instant::now();
            kept.release_held_if_quiet(now);
            if !is_at_prompt(&kept, now) {
                return None;
            }
            let read = &kept.bytes[kept.bytes.len().saturating_sub(ANSWER_MAX_BYTES)..];
            match read.iter().rposition(|byte| *byte == b'\n') {
                Some(newline) => read[newline + 1..].to_vec(),
                None if read.len() as u64 == kept.total => read.to_vec(),
                None => return None,
            }
        };
        prompt_line(&line)
    }
}

/// An unfinished line idle for 1.5 s with prompt punctuation, choices or a field default.
/// Silence alone also matches progress bars and spinners (#1262); unusual prompts can still
/// be answered through CLI input when their shape is not recognized.
fn is_at_prompt(kept: &Kept, now: Instant) -> bool {
    if kept.total == 0 || now.duration_since(kept.last_output) < PROMPT_IDLE {
        return false;
    }
    let line = kept
        .bytes
        .rsplit(|byte| *byte == b'\n')
        .next()
        .unwrap_or_default();
    drawn_line(line).is_some_and(|line| PROMPT_END.is_match(&line))
}

/// The most characters of a prompt shown; a longer line shows its end.
pub const PROMPT_MAX_CHARS: usize = 200;

/// What a terminal shows of `line`, output after the last newline: the last nonblank redraw,
/// without terminal controls, redacted, at most
/// [`PROMPT_MAX_CHARS`] from its end.
pub(crate) fn prompt_line(line: &[u8]) -> Option<String> {
    let drawn = drawn_line(line)?;
    let (redacted, _) = redact_secrets(&drawn);
    let length = redacted.chars().count();
    if length <= PROMPT_MAX_CHARS {
        return Some(redacted);
    }
    let end: String = redacted.chars().skip(length - PROMPT_MAX_CHARS).collect();
    Some(format!("…{end}"))
}

fn drawn_line(line: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(line);
    LINE_RESTARTS
        .split(&text)
        .map(strip_terminal_controls)
        .filter(|segment| !segment.trim().is_empty())
        .last()
}

/// The most output bytes an answer quotes; what came before is counted in the omission line.
pub const ANSWER_MAX_BYTES: usize = 16 * 1024;

/// The terminal lines quoted with a failed launch: enough for the launcher's error.
const LAUNCH_FAILURE_LINES: usize = 5;

/// `reason`, followed by what the terminal showed: a launcher that failed wrote why there.
fn with_terminal_output(reason: String, live: &LiveRun) -> String {
    let mut kept = live
        .output
        .lock()
        .expect("the output lock is never poisoned");
    kept.release_held();
    let shown = present(&kept.bytes, kept.total, LAUNCH_FAILURE_LINES).shown;
    if shown.is_empty() {
        reason
    } else {
        format!("{reason}; the terminal showed: {shown}")
    }
}

/// How the terminal may show what is typed, or `None` when it hides it as a password prompt
/// does: echo off and lines read whole. A program reading keys one at a time (readline, most
/// REPLs) may echo each one itself; sudo's `pwfeedback` prompt reads keys so too, refused on
/// purpose (#1372): its `*` per key cannot be told from an echo before the key is typed. Read
/// on the master side, which on Linux reads the terminal
/// side's settings. A program could change them between this read and the write that follows;
/// a password prompt turns echo on only after it read its line.
fn input_would_show(master: &File) -> Option<String> {
    let settings = match rustix::termios::tcgetattr(master) {
        Ok(settings) => settings,
        Err(error) => return Some(format!("settings could not be read ({error})")),
    };
    if settings.local_modes.contains(LocalModes::ECHO) {
        return Some("has echo on".to_owned());
    }
    if !settings.local_modes.contains(LocalModes::ICANON) {
        return Some(
            "reads keys one at a time, as sudo's pwfeedback prompt does, and a program may echo \
             each key itself"
                .to_owned(),
        );
    }
    None
}

/// A new pseudo-terminal: the master side for the daemon, the terminal side for the run.
fn open_terminal(rows: u16, columns: u16) -> std::io::Result<(File, OwnedFd)> {
    let master = rustix::pty::openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY)?;
    // `openpt` cannot set close-on-exec everywhere; the run must not inherit the master.
    rustix::io::fcntl_setfd(&master, FdFlags::CLOEXEC)?;
    rustix::pty::grantpt(&master)?;
    rustix::pty::unlockpt(&master)?;
    let name = rustix::pty::ptsname(&master, Vec::new())?;
    let terminal = rustix::fs::open(
        name.as_c_str(),
        OFlags::RDWR | OFlags::NOCTTY | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    rustix::termios::tcsetwinsize(
        &master,
        Winsize {
            ws_row: rows,
            ws_col: columns,
            ws_xpixel: 0,
            ws_ypixel: 0,
        },
    )?;
    Ok((File::from(master), terminal))
}

/// Reads the terminal until every process closed it, keeping the last `kept_bytes`. The
/// returned receiver fires at that end.
fn drain(mut reader: File, live: Arc<LiveRun>, kept_bytes: usize) -> oneshot::Receiver<()> {
    let (done, drained) = oneshot::channel();
    std::thread::spawn(move || {
        let mut buffer = [0u8; 8192];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => {
                    let mut kept = live
                        .output
                        .lock()
                        .expect("the output lock is never poisoned");
                    kept.append(&buffer[..read]);
                    // The raw read may hold a typed secret: only its redacted copy stays.
                    wipe(&mut buffer[..read]);
                    kept.last_output = Instant::now();
                    // Trimmed only at twice the bound, so each byte is moved about once.
                    if kept.bytes.len() > 2 * kept_bytes {
                        let excess = kept.bytes.len() - kept_bytes;
                        kept.bytes.drain(..excess);
                    }
                }
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                // EIO: every process closed the terminal.
                Err(_) => break,
            }
        }
        live.output
            .lock()
            .expect("the output lock is never poisoned")
            .release_held();
        let _ = done.send(());
    });
    drained
}

/// The kept output as the agent may read it: from the first whole line, without terminal
/// controls, redacted, its last `answer_lines` lines quoted. Only the last `ANSWER_MAX_BYTES`
/// of `kept` are read, whatever the caller kept; the rest is counted in `truncated_bytes`.
pub fn present(kept: &[u8], total: u64, answer_lines: usize) -> Output {
    let kept = &kept[kept.len().saturating_sub(ANSWER_MAX_BYTES)..];
    let mut truncated_bytes = total.saturating_sub(kept.len() as u64);
    let mut kept = kept;
    if truncated_bytes > 0
        && let Some(newline) = kept.iter().position(|byte| *byte == b'\n')
    {
        truncated_bytes += newline as u64 + 1;
        kept = &kept[newline + 1..];
    }
    let text = strip_terminal_controls(&String::from_utf8_lossy(kept));
    let (whole, redacted) = if truncated_bytes > 0 {
        redact_secrets_after_cut(&text)
    } else {
        redact_secrets(&text)
    };
    let lines: Vec<&str> = whole.lines().collect();
    let first_shown = lines.len().saturating_sub(answer_lines);
    Output {
        shown: lines[first_shown..].join("\n"),
        omitted_lines: first_shown as u64,
        truncated_bytes,
        redacted: redacted as u64,
    }
}

/// Whether `name` is one of the variables a run never inherits from the daemon. An approved
/// run is the owner's: no session variable may make `torlek` in it act as a session (#729).
fn is_withheld_variable(name: &OsStr) -> bool {
    name.as_encoded_bytes().starts_with(b"GIT_")
        || [
            "TORLEK_LAUNCH_TOKEN",
            "TORLEK_SESSION",
            "TORLEK_AGENT_SOCKET",
        ]
        .iter()
        .any(|withheld| name == *withheld)
}

/// `items` as prose: `a`, `a and b`, `a, b and c`.
fn listed(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [only] => only.clone(),
        [first @ .., last] => format!("{} and {last}", first.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_choices_and_defaults_are_prompts_but_progress_frames_are_not() {
        let now = Instant::now();
        for (line, expected) in [
            ("Password: ", true),
            ("Continue? [y/N] ", true),
            ("Ok to proceed? (y) ", true),
            ("Trust this plugin? [y,n,d,?] ", true),
            ("Continue? [Y/n/q] ", true),
            ("package name: (review) ", true),
            ("package name: (review-web) ", true),
            ("entry point: (index.js) ", true),
            ("version: (1.0.0) ", true),
            ("Continue (yes/no/[fingerprint])? ", true),
            ("Continue (yes/no/[fingerprint]) ", true),
            ("[====>   ]", false),
            ("[####  ]", false),
            ("[        ]", false),
            ("[74/192]", false),
            ("(74/192)", false),
            ("Progress: (74/192)", false),
            ("Progress: (38%)", false),
            ("Downloading: ([====>   ])", false),
            ("38% 74/192", false),
            ("⠋ Working", false),
            ("|", false),
            ("abc123 Waiting", false),
        ] {
            let kept = Kept {
                bytes: line.as_bytes().to_vec(),
                total: line.len() as u64,
                last_output: now - Duration::from_secs(2),
                typed: TypedSecrets::default(),
                is_finished: false,
            };
            assert_eq!(is_at_prompt(&kept, now), expected, "{line:?}");
        }
    }

    #[test]
    fn column_resets_use_only_the_new_frame_for_prompt_redaction() {
        for line in [
            b"token=\rprivate-value ok? ".as_slice(),
            b"token=\x1b[1Gprivate-value ok? ".as_slice(),
        ] {
            // A cursor reset discards the previous frame, including its secret label.
            assert_eq!(prompt_line(line).as_deref(), Some("private-value ok? "));
        }
    }
}

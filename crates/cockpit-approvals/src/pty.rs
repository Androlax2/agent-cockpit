//! Running an approved command on the host in a pseudo-terminal (report 17): programs see a
//! real terminal, so `sudo` and other prompts work, and the owner can type into it. Git runs
//! with hooks off and its program-running settings reset. What comes back to the agent is
//! the end of the output, without terminal controls, secrets redacted.
//!
//! Each run is a `cockpit-processes` terminal execution (#163): its own session in a
//! systemd scope or process group, led by `cockpit_processes::run_session_leader`, so it can
//! be terminated whole and a restart can tell what became of it.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{ErrorKind, Read, Write};
use std::os::fd::OwnedFd;
use std::os::unix::process::ExitStatusExt;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cockpit_processes::{EndReason, ExecutionId, LaunchSpec, Phase, Supervisor, Terminal};
use cockpit_turso::Database;
use rustix::fs::{Mode, OFlags};
use rustix::io::FdFlags;
use rustix::pty::OpenptFlags;
use rustix::termios::Winsize;
use tokio::sync::oneshot;

use crate::error::invalid;
use crate::redact::{redact_secrets, redact_secrets_after_cut, strip_terminal_controls};
use crate::runs::{Approved, RunId, RunResult, start_run};
use crate::{Error, Refusal};

/// After the command exits, how long output still in the terminal may take to arrive. A
/// background process that keeps the terminal open is not waited for.
const DRAIN_AFTER_EXIT: Duration = Duration::from_secs(2);
/// How long a run must have been silent, its output ending mid-line, to count as waiting for
/// input (a prompt).
const PROMPT_IDLE: Duration = Duration::from_millis(1500);
const SYNTAX_CHECK_TIMEOUT: Duration = Duration::from_secs(10);

/// How runs are started and bounded.
#[derive(Debug, Clone)]
pub struct RunSettings {
    /// The shell that runs each command as `<shell> -c <command>` and checks it first with
    /// `<shell> -n -c <command>`.
    pub shell: PathBuf,
    /// The session leader each run starts under (`cockpit_processes::Terminal`).
    pub leader: PathBuf,
    pub leader_arguments: Vec<OsString>,
    /// The run's whole environment, besides Git's: nothing else is inherited. Variables whose
    /// name starts with `GIT_` are dropped, so none can point Git elsewhere, and so is the
    /// launch token.
    pub environment: Vec<(OsString, OsString)>,
    /// The end of the output kept; what came before is counted, not kept.
    pub kept_bytes: usize,
    /// Lines of output the agent's answer quotes; the rest are counted.
    pub answer_lines: usize,
    pub rows: u16,
    pub columns: u16,
}

impl RunSettings {
    pub fn new(shell: PathBuf, leader: PathBuf, leader_arguments: Vec<OsString>) -> RunSettings {
        RunSettings {
            shell,
            leader,
            leader_arguments,
            environment: Vec::new(),
            kept_bytes: 1024 * 1024,
            answer_lines: 120,
            rows: 40,
            columns: 120,
        }
    }

    /// The environment, with `git_config` passed as `GIT_CONFIG_*` so every Git in the run
    /// applies it over the repository's own configuration.
    pub fn run_environment(&self, git_config: &[String]) -> Vec<(OsString, OsString)> {
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
    settings: Arc<RunSettings>,
    pub(crate) supervisor: Supervisor,
    live: Arc<Mutex<HashMap<RunId, Arc<LiveRun>>>>,
}

#[derive(Debug)]
struct LiveRun {
    input: Mutex<File>,
    output: Mutex<Kept>,
}

#[derive(Debug)]
struct Kept {
    bytes: Vec<u8>,
    total: u64,
    last_output: Instant,
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
    /// only fail to parse after approval interrupts the owner for nothing. The outer error is
    /// a check that could not be made.
    pub async fn check_syntax(&self, command: &str) -> std::io::Result<Result<(), Refusal>> {
        let mut check = tokio::process::Command::new(&self.settings.shell);
        check
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
            return Ok(Ok(()));
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
        let not_started =
            |what: &str, error: std::io::Error| NoResult::NotStarted(format!("{what}: {error}"));
        let (master, terminal) = open_terminal(self.settings.rows, self.settings.columns)
            .map_err(|error| not_started("cannot open a terminal for the command", error))?;
        let reader = master
            .try_clone()
            .map_err(|error| not_started("cannot read the terminal", error))?;
        let execution = self.mark_running(database, run).await.map_err(|error| {
            NoResult::NotStarted(format!("the run could not be marked running: {error}"))
        })?;
        let live = Arc::new(LiveRun {
            input: Mutex::new(master),
            output: Mutex::new(Kept {
                bytes: Vec::new(),
                total: 0,
                last_output: Instant::now(),
            }),
        });
        self.live
            .lock()
            .expect("the live runs lock is never poisoned")
            .insert(run.id, Arc::clone(&live));
        let drained = drain(reader, Arc::clone(&live), self.settings.kept_bytes);
        let exited = self
            .launch(database, run, execution, terminal, git_config)
            .await;
        let terminal_held_open = tokio::time::timeout(DRAIN_AFTER_EXIT, drained)
            .await
            .is_err();
        self.live
            .lock()
            .expect("the live runs lock is never poisoned")
            .remove(&run.id);
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
        let kept = live
            .output
            .lock()
            .expect("the output lock is never poisoned");
        let tail = &kept.bytes[kept.bytes.len().saturating_sub(self.settings.kept_bytes)..];
        Ok(Ran {
            result: RunResult {
                exit_code,
                output: present(tail, kept.total, self.settings.answer_lines),
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
        let execution = self.supervisor.reserve(&write, database.epoch()).await?;
        start_run(&write, run, execution).await?;
        write.commit().await?;
        Ok(execution)
    }

    /// Launches the reserved execution in `terminal` and answers the command's exit code.
    async fn launch(
        &self,
        database: &Database,
        run: &Approved,
        execution: ExecutionId,
        terminal: OwnedFd,
        git_config: &[String],
    ) -> Result<i32, NoResult> {
        let spec = LaunchSpec {
            program: self.settings.shell.clone(),
            arguments: vec!["-c".into(), run.command.clone().into()],
            working_dir: run.cwd.clone(),
            environment: self.settings.run_environment(git_config),
            terminal: Some(Terminal {
                device: terminal,
                leader: self.settings.leader.clone(),
                leader_arguments: self.settings.leader_arguments.clone(),
            }),
        };
        let launched = self
            .supervisor
            .launch_reserved(database, execution, &spec)
            .await;
        // The terminal must end with the run's processes for the reader to see the end of
        // the output: this copy goes as soon as the run has its own.
        drop(spec);
        let launched = match launched {
            Ok(launched) => launched,
            Err(error) => return Err(self.launch_failed(database, execution, &error).await),
        };
        match launched.exit.await {
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

    /// Types `text` and Enter into the running command.
    pub fn send_input(&self, id: RunId, text: &str) -> Result<(), Refusal> {
        let live = self
            .live
            .lock()
            .expect("the live runs lock is never poisoned")
            .get(&id)
            .cloned()
            .ok_or(Refusal::NotRunning)?;
        let mut input = live.input.lock().expect("the input lock is never poisoned");
        input
            .write_all(text.as_bytes())
            .and_then(|()| input.write_all(b"\r"))
            .map_err(|_| Refusal::NotRunning)
    }

    /// The runs silent a while with their output ending mid-line: prompts. Read at one
    /// instant, so a queue sorted by it stays consistent.
    pub fn waiting_for_input(&self) -> Vec<RunId> {
        let now = Instant::now();
        self.live
            .lock()
            .expect("the live runs lock is never poisoned")
            .iter()
            .filter(|(_, live)| {
                let kept = live
                    .output
                    .lock()
                    .expect("the output lock is never poisoned");
                kept.total > 0
                    && kept.bytes.last() != Some(&b'\n')
                    && now.duration_since(kept.last_output) >= PROMPT_IDLE
            })
            .map(|(id, _)| *id)
            .collect()
    }
}

/// The terminal lines quoted with a failed launch: enough for the launcher's error.
const LAUNCH_FAILURE_LINES: usize = 5;

/// `reason`, followed by what the terminal showed: a launcher that failed wrote why there.
fn with_terminal_output(reason: String, live: &LiveRun) -> String {
    let kept = live
        .output
        .lock()
        .expect("the output lock is never poisoned");
    let shown = present(&kept.bytes, kept.total, LAUNCH_FAILURE_LINES).shown;
    if shown.is_empty() {
        reason
    } else {
        format!("{reason}; the terminal showed: {shown}")
    }
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
                    kept.bytes.extend_from_slice(&buffer[..read]);
                    kept.total += read as u64;
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
        let _ = done.send(());
    });
    drained
}

/// The kept output as the agent may read it: from the first whole line, without terminal
/// controls, redacted, its last `answer_lines` lines quoted.
pub fn present(kept: &[u8], total: u64, answer_lines: usize) -> Output {
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

/// Whether `name` is one of the variables a run never inherits from the daemon.
fn is_withheld_variable(name: &OsStr) -> bool {
    name.as_encoded_bytes().starts_with(b"GIT_") || name == "COCKPIT_LAUNCH_TOKEN"
}

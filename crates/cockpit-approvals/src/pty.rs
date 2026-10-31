//! Running an approved command on the host in a pseudo-terminal (report 17): programs see a
//! real terminal, so `sudo` and other prompts work, and the owner can type into it. Git runs
//! with hooks off and its program-running settings reset. What comes back to the agent is
//! the end of the output, without terminal controls, secrets redacted.
//!
//! A terminal becomes a process's controlling terminal only when a session leader takes it.
//! Making the child one between `fork` and `exec` would need `unsafe`, so a leader program
//! does it in its own process: [`run_leader`] calls `setsid`, takes its standard input (the
//! terminal) as controlling terminal, then execs the shell.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{ErrorKind, Read, Write};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{ExitCode, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rustix::fs::{Mode, OFlags};
use rustix::io::FdFlags;
use rustix::pty::OpenptFlags;
use rustix::termios::Winsize;
use tokio::sync::oneshot;

use crate::Refusal;
use crate::error::invalid;
use crate::redact::{redact_secrets, strip_terminal_controls};
use crate::runs::{Approved, RunId, RunResult};

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
    /// The program [`run_leader`] runs in: `<leader> <leader_arguments…> <shell> -c <command>`.
    pub leader: PathBuf,
    pub leader_arguments: Vec<OsString>,
    /// The run's whole environment, besides Git's: nothing else is inherited. Variables whose
    /// name starts with `GIT_` are dropped, so none can point Git elsewhere, and so is the
    /// launch token.
    pub environment: Vec<(OsString, OsString)>,
    /// `key=value` Git settings every Git in the run applies over the repository's own
    /// configuration (`cockpit-projects`' safe settings), passed as `GIT_CONFIG_*`.
    pub git_config: Vec<String>,
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
            git_config: Vec::new(),
            kept_bytes: 1024 * 1024,
            answer_lines: 120,
            rows: 40,
            columns: 120,
        }
    }

    fn run_environment(&self) -> Vec<(OsString, OsString)> {
        let mut environment: Vec<(OsString, OsString)> = self
            .environment
            .iter()
            .filter(|(name, _)| !is_withheld_variable(name))
            .cloned()
            .collect();
        environment.push((
            "GIT_CONFIG_COUNT".into(),
            self.git_config.len().to_string().into(),
        ));
        for (index, setting) in self.git_config.iter().enumerate() {
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
    pub exit_code: i32,
    pub output: Output,
    /// A process the command left in the background still held the terminal when the
    /// command exited; what it printed afterwards is not in `output`.
    pub terminal_held_open: bool,
}

/// A run's output as it may reach the agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    /// The last lines of the kept output, for the agent's answer.
    pub shown: String,
    pub omitted_lines: u64,
    pub truncated_bytes: u64,
    pub redacted: u64,
}

impl Ran {
    pub fn result(&self) -> RunResult {
        RunResult {
            exit_code: self.exit_code,
            output: self.output.shown.clone(),
            omitted_lines: self.output.omitted_lines,
            truncated_bytes: self.output.truncated_bytes,
            redacted: self.output.redacted,
        }
    }
}

/// Starts approved runs and keeps the ones in progress, for input and for the queue.
#[derive(Debug, Clone)]
pub struct Runner {
    settings: Arc<RunSettings>,
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
    pub fn new(settings: RunSettings) -> Runner {
        Runner {
            settings: Arc::new(settings),
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
            .envs(self.settings.run_environment())
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

    /// Runs `run` to its end. `Err` says why it did not start; then nothing ran.
    pub async fn run(&self, run: &Approved) -> Result<Ran, String> {
        match std::fs::canonicalize(&run.cwd) {
            Ok(resolved) if resolved == run.cwd => {}
            Ok(resolved) => {
                return Err(format!(
                    "{} now resolves to {}, not the approved directory",
                    run.cwd.display(),
                    resolved.display()
                ));
            }
            Err(error) => return Err(format!("{}: {error}", run.cwd.display())),
        }
        let (master, terminal) = open_terminal(self.settings.rows, self.settings.columns)
            .map_err(|error| format!("cannot open a terminal for the command: {error}"))?;
        let mut child = self
            .spawn(run, terminal)
            .map_err(|error| format!("cannot start {}: {error}", self.settings.leader.display()))?;
        let reader = master
            .try_clone()
            .map_err(|error| format!("cannot read the terminal: {error}"))?;
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
        let status = child.wait().await;
        let terminal_held_open = tokio::time::timeout(DRAIN_AFTER_EXIT, drained)
            .await
            .is_err();
        self.live
            .lock()
            .expect("the live runs lock is never poisoned")
            .remove(&run.id);
        let exit_code = match status {
            Ok(status) => status
                .code()
                .unwrap_or_else(|| 128 + status.signal().unwrap_or(0)),
            Err(error) => return Err(format!("the command's exit was not observed: {error}")),
        };
        let kept = live
            .output
            .lock()
            .expect("the output lock is never poisoned");
        Ok(Ran {
            exit_code,
            output: present(&kept.bytes, kept.total, self.settings.answer_lines),
            terminal_held_open,
        })
    }

    fn spawn(
        &self,
        run: &Approved,
        terminal: rustix::fd::OwnedFd,
    ) -> std::io::Result<tokio::process::Child> {
        let mut command = tokio::process::Command::new(&self.settings.leader);
        command
            .args(&self.settings.leader_arguments)
            .arg(&self.settings.shell)
            .arg("-c")
            .arg(&run.command)
            .env_clear()
            .envs(self.settings.run_environment())
            .current_dir(&run.cwd)
            .stdin(Stdio::from(terminal.try_clone()?))
            .stdout(Stdio::from(terminal.try_clone()?))
            .stderr(Stdio::from(terminal));
        // `command` drops its copies of the terminal when it goes out of scope: the terminal
        // must end with the run's processes for the reader to see the end of the output.
        command.spawn()
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

    /// Whether the run has been silent a while with its output ending mid-line: a prompt.
    pub fn is_waiting_for_input(&self, id: RunId) -> bool {
        let Some(live) = self
            .live
            .lock()
            .expect("the live runs lock is never poisoned")
            .get(&id)
            .cloned()
        else {
            return false;
        };
        let kept = live
            .output
            .lock()
            .expect("the output lock is never poisoned");
        kept.total > 0
            && kept.bytes.last() != Some(&b'\n')
            && kept.last_output.elapsed() >= PROMPT_IDLE
    }
}

/// A new pseudo-terminal: the master side for the daemon, the terminal side for the run.
fn open_terminal(rows: u16, columns: u16) -> std::io::Result<(File, rustix::fd::OwnedFd)> {
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
                    if kept.bytes.len() > kept_bytes {
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
    let (whole, redacted) = redact_secrets(&text);
    let lines: Vec<&str> = whole.lines().collect();
    let first_shown = lines.len().saturating_sub(answer_lines);
    Output {
        shown: lines[first_shown..].join("\n"),
        omitted_lines: first_shown as u64,
        truncated_bytes,
        redacted: redacted as u64,
    }
}

/// The leader program's work: `arguments` is `<program> <arguments…>`. It becomes a session
/// leader, takes its standard input (the run's terminal) as controlling terminal and execs
/// the program, which keeps its pid, session and terminal. Returns only on failure.
pub fn run_leader(arguments: &[OsString]) -> ExitCode {
    let Some((program, program_arguments)) = arguments.split_first() else {
        eprintln!("cockpit: the run leader needs a program to run");
        return ExitCode::from(2);
    };
    if let Err(error) = rustix::process::setsid() {
        eprintln!("cockpit: cannot start a session for the command: {error}");
        return ExitCode::from(127);
    }
    if let Err(error) = rustix::process::ioctl_tiocsctty(std::io::stdin()) {
        eprintln!("cockpit: cannot take the terminal for the command: {error}");
        return ExitCode::from(127);
    }
    let error = std::process::Command::new(program)
        .args(program_arguments)
        .exec();
    eprintln!(
        "cockpit: cannot run {}: {error}",
        Path::new(program).display()
    );
    ExitCode::from(127)
}

/// Whether `name` is one of the variables a run never inherits from the daemon.
fn is_withheld_variable(name: &OsStr) -> bool {
    name.as_encoded_bytes().starts_with(b"GIT_") || name == "COCKPIT_LAUNCH_TOKEN"
}

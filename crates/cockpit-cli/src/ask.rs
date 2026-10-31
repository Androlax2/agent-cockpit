//! `cockpit ask run` and `cockpit ask guide`: an agent asks the owner, on the agent socket,
//! to run a command on the host or to do steps by hand, and waits for the answer (report
//! 17). The daemon identifies the sender; nothing here names a session.
//!
//! Exit codes keep `ask-to-run`'s meaning: a run's own exit code once it ran, 125 declined
//! (or the guide closed), 124 still pending (`--wait <id>` goes on waiting), 126 the daemon
//! cannot be reached (with the fix), 3 the owner is stuck on a guide step, 1 anything else.

use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use cockpit_client::{ClientError, Connection};
use cockpit_protocol::{Command, GuideStateView, GuideView, Reply, RunRequestView, RunStateView};

const PENDING: u8 = 124;
const DECLINED: u8 = 125;
const UNREACHABLE: u8 = 126;
const STUCK: u8 = 3;

/// Overrides how long a call waits before answering 124; whole seconds.
const WAIT_VARIABLE: &str = "COCKPIT_ASK_WAIT_SECONDS";
/// Under the ten minutes an agent's tool call usually may take.
const DEFAULT_WAIT: Duration = Duration::from_secs(540);
const POLL_EVERY: Duration = Duration::from_millis(500);
const DEADLINE: Duration = Duration::from_secs(10);

/// `cockpit ask run [--cwd DIR] [--no-wait] --reason TEXT COMMAND | --wait ID | --check`.
/// With `--no-wait` the agent files the request and goes on with other work: it exits 124
/// at once, naming the `--wait ID` that collects the answer (#156).
pub(crate) fn run(arguments: &[OsString]) -> Result<ExitCode, String> {
    let words = utf8(arguments)?;
    match words.as_slice() {
        ["--check"] => Ok(check()),
        ["--wait", id] => {
            let id = request_id(id)?;
            Ok(Agent::connect().map_or_else(|code| code, |agent| agent.wait_for_run(id)))
        }
        _ => {
            let asked = parse_ask_run(&words)?;
            Ok(Agent::connect().map_or_else(|code| code, |agent| agent.ask_run(asked)))
        }
    }
}

/// `cockpit ask guide [--cwd DIR] | --wait ID`, the guide on standard input.
pub(crate) fn guide(arguments: &[OsString]) -> Result<ExitCode, String> {
    let words = utf8(arguments)?;
    let cwd = match words.as_slice() {
        ["--wait", id] => {
            let id = request_id(id)?;
            return Ok(Agent::connect().map_or_else(|code| code, |agent| agent.wait_for_guide(id)));
        }
        [] => absolute(".")?,
        ["--cwd", dir] => absolute(dir)?,
        _ => return Err("unknown ask guide arguments".to_owned()),
    };
    let mut guide = String::new();
    std::io::stdin()
        .read_to_string(&mut guide)
        .map_err(|error| format!("cannot read the guide from standard input: {error}"))?;
    Ok(Agent::connect().map_or_else(|code| code, |agent| agent.ask_guide(guide, cwd)))
}

pub(crate) fn utf8(arguments: &[OsString]) -> Result<Vec<&str>, String> {
    arguments
        .iter()
        .map(|word| {
            word.to_str()
                .ok_or_else(|| format!("{}: not UTF-8", word.to_string_lossy()))
        })
        .collect()
}

fn request_id(text: &str) -> Result<i64, String> {
    text.parse()
        .map_err(|_| format!("{text} is not a request id"))
}

/// The daemon resolves nothing relative to its own directory.
fn absolute(dir: &str) -> Result<String, String> {
    let path = std::path::absolute(dir).map_err(|error| format!("{dir}: {error}"))?;
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("{}: not UTF-8", path.display()))
}

/// What `ask run` files: the command, the directory it runs in and why; and whether the
/// call waits for the answer.
struct RunAsked {
    command: String,
    cwd: String,
    reason: String,
    waits: bool,
}

fn parse_ask_run(words: &[&str]) -> Result<RunAsked, String> {
    let mut cwd = None;
    let mut reason = None;
    let mut command = None;
    let mut waits = true;
    let mut words = words.iter();
    while let Some(word) = words.next() {
        let slot = match *word {
            "--no-wait" if waits => {
                waits = false;
                continue;
            }
            "--no-wait" => return Err("--no-wait given twice".to_owned()),
            "--cwd" => &mut cwd,
            "--reason" => &mut reason,
            _ if command.is_none() => {
                command = Some((*word).to_owned());
                continue;
            }
            _ => return Err("give the command as one quoted argument".to_owned()),
        };
        let value = words
            .next()
            .ok_or_else(|| format!("{word} needs a value"))?;
        if slot.replace((*value).to_owned()).is_some() {
            return Err(format!("{word} given twice"));
        }
    }
    let reason = reason.ok_or("--reason is required: what the command does and why")?;
    let command = command.ok_or("name the command to run")?;
    let cwd = absolute(cwd.as_deref().unwrap_or("."))?;
    Ok(RunAsked {
        command,
        cwd,
        reason,
        waits,
    })
}

/// `ask-to-run --check`: whether an ask from here would reach the owner now.
fn check() -> ExitCode {
    let agent = match Agent::connect() {
        Ok(agent) => agent,
        Err(code) => return code,
    };
    match agent.request(Command::AskCheck {
        launch_token: crate::agent::launch_token(),
    }) {
        Ok(Reply::AskReady { session }) => {
            outln!("ready: asks reach the owner for session {session}");
            ExitCode::SUCCESS
        }
        other => agent.failed(other),
    }
}

/// A connection to the agent socket, waited on with blocking sleeps between polls.
struct Agent {
    socket: PathBuf,
    runtime: tokio::runtime::Runtime,
    connection: Connection,
}

impl Agent {
    /// Exit 126 with the fix when the daemon cannot be reached.
    fn connect() -> Result<Agent, ExitCode> {
        let socket = crate::agent::agent_socket()?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a single-threaded runtime starts");
        match runtime.block_on(Connection::open(&socket, DEADLINE)) {
            Ok(connection) => Ok(Agent {
                socket,
                runtime,
                connection,
            }),
            Err(error) => Err(unreachable(&socket, &error)),
        }
    }

    fn request(&self, command: Command) -> Result<Reply, ClientError> {
        self.runtime
            .block_on(self.connection.request(command, DEADLINE))
    }

    fn failed(&self, outcome: Result<Reply, ClientError>) -> ExitCode {
        match outcome {
            Err(error @ (ClientError::Connect { .. } | ClientError::Closed(_))) => {
                unreachable(&self.socket, &error)
            }
            other => crate::agent::failed(&self.socket, other),
        }
    }

    fn ask_run(&self, asked: RunAsked) -> ExitCode {
        let waits = asked.waits;
        let asked = self.request(Command::AskRun {
            launch_token: crate::agent::launch_token(),
            command: asked.command,
            cwd: asked.cwd,
            reason: asked.reason,
        });
        match asked {
            Ok(Reply::RunRequest { request }) if waits => {
                eprintln!("cockpit: asked the owner (request {}); waiting", request.id);
                self.wait_for_run(request.id)
            }
            Ok(Reply::RunRequest { request }) => {
                eprintln!(
                    "cockpit: asked the owner (request {id}); collect the answer with: \
                     cockpit ask run --wait {id}",
                    id = request.id
                );
                ExitCode::from(PENDING)
            }
            other => self.failed(other),
        }
    }

    fn wait_for_run(&self, id: i64) -> ExitCode {
        let deadline = Instant::now() + wait_time();
        loop {
            let status = self.request(Command::AskRunStatus {
                launch_token: crate::agent::launch_token(),
                request: id,
            });
            let request = match status {
                Ok(Reply::RunRequest { request }) => request,
                other => return self.failed(other),
            };
            if let Some(code) = run_answer(&request) {
                return code;
            }
            if Instant::now() >= deadline {
                eprintln!(
                    "cockpit: request {id} is still {}; wait on with: cockpit ask run --wait {id}",
                    state_name(request.state)
                );
                return ExitCode::from(PENDING);
            }
            std::thread::sleep(POLL_EVERY);
        }
    }

    fn ask_guide(&self, guide: String, cwd: String) -> ExitCode {
        let asked = self.request(Command::AskGuide {
            launch_token: crate::agent::launch_token(),
            guide,
            cwd,
        });
        match asked {
            Ok(Reply::Guide { guide }) => {
                eprintln!("cockpit: handed the owner guide {}; waiting", guide.id);
                self.wait_for_guide(guide.id)
            }
            other => self.failed(other),
        }
    }

    fn wait_for_guide(&self, id: i64) -> ExitCode {
        let deadline = Instant::now() + wait_time();
        loop {
            let status = self.request(Command::AskGuideStatus {
                launch_token: crate::agent::launch_token(),
                guide: id,
            });
            let guide = match status {
                Ok(Reply::Guide { guide }) => guide,
                other => return self.failed(other),
            };
            if let Some(code) = guide_answer(&guide) {
                return code;
            }
            if Instant::now() >= deadline {
                eprintln!(
                    "cockpit: the owner is at step {} of guide {id}; wait on with: \
                     cockpit ask guide --wait {id}",
                    guide.current_step + 1
                );
                return ExitCode::from(PENDING);
            }
            std::thread::sleep(POLL_EVERY);
        }
    }
}

fn unreachable(socket: &Path, error: &ClientError) -> ExitCode {
    eprintln!(
        "cockpit: cannot reach the Cockpit daemon at {}: {error}",
        socket.display()
    );
    eprintln!(
        "cockpit: to fix: the owner starts it (cockpit-daemon), or mounts its agent socket \
         directory into this sandbox and sets COCKPIT_AGENT_SOCKET"
    );
    ExitCode::from(UNREACHABLE)
}

fn wait_time() -> Duration {
    std::env::var(WAIT_VARIABLE)
        .ok()
        .and_then(|seconds| seconds.parse().ok())
        .map_or(DEFAULT_WAIT, Duration::from_secs)
}

/// The exit the agent gets for `request`, or `None` while it is undecided or running.
fn run_answer(request: &RunRequestView) -> Option<ExitCode> {
    match request.state {
        RunStateView::Asking | RunStateView::Approved | RunStateView::Running => None,
        RunStateView::Finished => {
            let result = request.result.as_ref()?;
            if result.truncated_bytes > 0 || result.omitted_lines > 0 {
                outln!(
                    "[{} earlier lines and {} bytes not shown]",
                    result.omitted_lines,
                    result.truncated_bytes
                );
            }
            outln!("{}", result.output);
            if result.redacted > 0 {
                eprintln!(
                    "cockpit: {} secrets redacted from the output",
                    result.redacted
                );
            }
            Some(ExitCode::from(u8::try_from(result.exit_code).unwrap_or(1)))
        }
        RunStateView::Declined => {
            eprintln!("cockpit: the owner declined request {}", request.id);
            Some(ExitCode::from(DECLINED))
        }
        RunStateView::Expired => {
            eprintln!(
                "cockpit: request {} expired: nobody waited for its answer; ask again",
                request.id
            );
            Some(ExitCode::FAILURE)
        }
        RunStateView::NotRun | RunStateView::OutcomeUnknown => {
            eprintln!(
                "cockpit: request {} {}: {}",
                request.id,
                state_name(request.state),
                request.note.as_deref().unwrap_or("no reason recorded")
            );
            Some(ExitCode::FAILURE)
        }
    }
}

/// The exit the agent gets for `guide`, or `None` while the owner works through it.
fn guide_answer(guide: &GuideView) -> Option<ExitCode> {
    match guide.state {
        GuideStateView::Asking => None,
        GuideStateView::Done => {
            for (index, step) in guide.steps.iter().enumerate() {
                if let Some(answer) = &step.answer {
                    outln!("step {}: {answer}", index + 1);
                }
                if let Some(request) = step.run_request {
                    outln!(
                        "step {}: filed run request {request}; cockpit ask run --wait {request}",
                        index + 1
                    );
                }
            }
            Some(ExitCode::SUCCESS)
        }
        GuideStateView::Stuck => {
            outln!(
                "stuck at step {}: {}",
                guide.current_step + 1,
                guide.stuck_note.as_deref().unwrap_or("")
            );
            Some(ExitCode::from(STUCK))
        }
        GuideStateView::Closed => {
            eprintln!("cockpit: the owner closed guide {}", guide.id);
            Some(ExitCode::from(DECLINED))
        }
        GuideStateView::Expired => {
            eprintln!(
                "cockpit: guide {} expired: nobody waited for it; ask again",
                guide.id
            );
            Some(ExitCode::FAILURE)
        }
    }
}

pub(crate) fn state_name(state: RunStateView) -> &'static str {
    match state {
        RunStateView::Asking => "asking",
        RunStateView::Expired => "expired",
        RunStateView::Declined => "declined",
        RunStateView::Approved => "approved",
        RunStateView::Running => "running",
        RunStateView::Finished => "finished",
        RunStateView::NotRun => "not run",
        RunStateView::OutcomeUnknown => "of unknown outcome",
    }
}

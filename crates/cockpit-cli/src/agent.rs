//! `cockpit agent report` (an agent, on the agent socket) and `cockpit agent reports` (the
//! owner, on the host-only socket). The daemon identifies the sender; nothing here names a
//! session.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use cockpit_client::{ClientError, Connection};
use cockpit_protocol::{
    AGENT_SOCKET_VARIABLE, AgentReport, AgentState, Command, LAUNCH_TOKEN_VARIABLE, QuestionSource,
    Reply, SessionReport, TurnOutcome, UnblockedBy,
};

/// Long enough for a busy daemon, short enough that a hook never hangs its agent.
const DEADLINE: Duration = Duration::from_secs(10);

/// `--state working|waiting`, `--label`, `--doing`, `--needs-input`, `--dialog`, the
/// value-less `--prompt`, `--outcome completed|failed|quota-stopped`, `--outcome-reason`
/// (with `failed` only), `--conversation` and the value-less `--conversation-ends` (with
/// `--conversation` only), each at most once.
pub(crate) fn report(flags: &[OsString]) -> Result<ExitCode, String> {
    let report = parse_report(flags)?;
    let launch_token = launch_token();
    let socket = match agent_socket() {
        Ok(socket) => socket,
        Err(code) => return Ok(code),
    };
    let command = Command::AgentReport {
        launch_token,
        report,
    };
    Ok(match exchange(&socket, command) {
        Ok(Reply::AgentReported { report }) => {
            outln!("reported for session {}", report.session);
            ExitCode::SUCCESS
        }
        other => failed(&socket, other),
    })
}

/// Prints every session's latest report, one line each.
pub(crate) fn reports(flags: &[OsString]) -> Result<ExitCode, String> {
    if let Some(flag) = flags.first() {
        return Err(format!("unknown argument {}", flag.to_string_lossy()));
    }
    let socket = match socket_dir() {
        Ok(dir) => dir.join(cockpit_config::DAEMON_SOCKET),
        Err(code) => return Ok(code),
    };
    Ok(match exchange(&socket, Command::AgentReports) {
        Ok(Reply::AgentReports { reports }) => {
            for report in &reports {
                outln!("{}", render(report));
            }
            ExitCode::SUCCESS
        }
        other => failed(&socket, other),
    })
}

fn parse_report(flags: &[OsString]) -> Result<AgentReport, String> {
    let mut report = AgentReport::default();
    let mut outcome = None;
    let mut outcome_reason = None;
    let mut flags = flags.iter();
    while let Some(flag) = flags.next() {
        let name = flag.to_string_lossy().into_owned();
        if name == "--prompt" {
            if report.is_new_prompt {
                return Err("--prompt given twice".to_owned());
            }
            report.is_new_prompt = true;
            continue;
        }
        if name == "--conversation-ends" {
            if report.is_conversation_ending {
                return Err("--conversation-ends given twice".to_owned());
            }
            report.is_conversation_ending = true;
            continue;
        }
        let value = flags
            .next()
            .ok_or_else(|| format!("{name} needs a value"))?
            .to_str()
            .ok_or_else(|| format!("{name}: not UTF-8"))?
            .to_owned();
        let field = match name.as_str() {
            "--state" => {
                if report.state.is_some() {
                    return Err("--state given twice".to_owned());
                }
                report.state = Some(
                    AgentState::ALL
                        .into_iter()
                        .find(|state| state.as_str() == value)
                        .ok_or_else(|| format!("--state {value}: use working or waiting"))?,
                );
                continue;
            }
            "--label" => &mut report.label,
            "--doing" => &mut report.doing,
            "--needs-input" => &mut report.needs_input,
            "--outcome" => &mut outcome,
            "--outcome-reason" => &mut outcome_reason,
            "--dialog" => &mut report.dialog,
            "--conversation" => &mut report.conversation,
            _ => return Err(format!("unknown argument {name}")),
        };
        if field.replace(value).is_some() {
            return Err(format!("{name} given twice"));
        }
    }
    report.outcome = turn_outcome(outcome, outcome_reason)?;
    if report.is_conversation_ending && report.conversation.is_none() {
        return Err("--conversation-ends needs --conversation".to_owned());
    }
    // The conversation only says where a report comes from; it reports nothing alone.
    let reported = AgentReport {
        conversation: None,
        is_conversation_ending: false,
        ..report.clone()
    };
    let is_reporting = reported != AgentReport::default();
    if report.is_conversation_ending {
        if is_reporting {
            return Err("--conversation-ends reports nothing else".to_owned());
        }
        return Ok(report);
    }
    if !is_reporting {
        return Err(
            "name at least one of --state, --label, --doing, --needs-input, --dialog, --prompt, \
             --outcome or --conversation-ends"
                .to_owned(),
        );
    }
    Ok(report)
}

fn turn_outcome(
    outcome: Option<String>,
    reason: Option<String>,
) -> Result<Option<TurnOutcome>, String> {
    match (outcome.as_deref(), reason) {
        (None, None) => Ok(None),
        (Some("failed"), reason) => Ok(Some(TurnOutcome::Failed { reason })),
        (Some("completed"), None) => Ok(Some(TurnOutcome::Completed)),
        (Some("quota-stopped"), None) => Ok(Some(TurnOutcome::QuotaStopped)),
        (None | Some("completed" | "quota-stopped"), Some(_)) => {
            Err("--outcome-reason needs --outcome failed".to_owned())
        }
        (Some(other), _) => Err(format!(
            "--outcome {other}: use completed, failed or quota-stopped"
        )),
    }
}

/// The agent socket: `COCKPIT_AGENT_SOCKET` when the sandbox mounts it elsewhere, else its
/// place in the daemon's socket directory.
pub(crate) fn agent_socket() -> Result<PathBuf, ExitCode> {
    match std::env::var_os(AGENT_SOCKET_VARIABLE).filter(|path| !path.is_empty()) {
        Some(path) => Ok(PathBuf::from(path)),
        None => Ok(socket_dir()?
            .join(cockpit_config::AGENT_SOCKET_DIR)
            .join(cockpit_config::AGENT_SOCKET)),
    }
}

/// The launch token this agent was started with, if any.
pub(crate) fn launch_token() -> Option<String> {
    std::env::var(LAUNCH_TOKEN_VARIABLE)
        .ok()
        .filter(|token| !token.is_empty())
}

/// The daemon's socket directory, or exit 1 when the environment cannot place it.
pub(crate) fn socket_dir() -> Result<PathBuf, ExitCode> {
    let environment = cockpit_config::Environment::from_process();
    environment
        .home_dir()
        .and_then(|home| cockpit_config::Layout::resolve(&environment, &home).socket_dir())
        .map_err(|error| {
            eprintln!("cockpit: {error}");
            ExitCode::FAILURE
        })
}

pub(crate) fn exchange(socket: &Path, command: Command) -> Result<Reply, ClientError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a single-threaded runtime starts");
    runtime.block_on(async {
        let connection = Connection::open(socket, DEADLINE).await?;
        connection.request(command, DEADLINE).await
    })
}

/// Exit 1 after saying why, and for a refusal what would lift it.
pub(crate) fn failed(socket: &Path, outcome: Result<Reply, ClientError>) -> ExitCode {
    match outcome {
        Err(ClientError::Daemon(error)) => {
            eprintln!("cockpit: refused: {}", error.message);
            if let Some(unblock) = error.unblock {
                let by = match unblock.by {
                    UnblockedBy::Owner => "the owner",
                    UnblockedBy::Agent => "you",
                    UnblockedBy::Cockpit => "Cockpit",
                    UnblockedBy::Unknown(ref party) => party.as_str(),
                };
                eprintln!("cockpit: to unblock ({by}): {}", unblock.action);
            }
        }
        Err(ClientError::Connect { source, .. }) => {
            eprintln!(
                "cockpit: cannot reach the daemon at {}: {source}",
                socket.display()
            );
        }
        Err(error) => eprintln!("cockpit: {error}"),
        Ok(reply) => eprintln!("cockpit: the daemon answered with an unexpected {reply:?}"),
    }
    ExitCode::FAILURE
}

/// `session state label="…" doing="…" needs-input="…" outcome=… outcome-reason="…"`, leaving
/// out what was never reported; a dialog's question shows as `dialog="…"`.
fn render(report: &SessionReport) -> String {
    let mut line = report.session.clone();
    line.push(' ');
    line.push_str(report.state.map_or("-", AgentState::as_str));
    let question_key = match report.question_source {
        Some(QuestionSource::Dialog) => "dialog",
        Some(QuestionSource::Agent) | None => "needs-input",
    };
    for (key, value) in [
        ("label", &report.label),
        ("doing", &report.doing),
        (question_key, &report.needs_input),
    ] {
        if let Some(value) = value {
            line.push_str(&format!(" {key}={value:?}"));
        }
    }
    match &report.outcome {
        None => {}
        Some(TurnOutcome::Completed) => line.push_str(" outcome=completed"),
        Some(TurnOutcome::QuotaStopped) => line.push_str(" outcome=quota-stopped"),
        Some(TurnOutcome::Failed { reason }) => {
            line.push_str(" outcome=failed");
            if let Some(reason) = reason {
                line.push_str(&format!(" outcome-reason={reason:?}"));
            }
        }
    }
    line
}

//! `cockpit agent report` (an agent, on the agent socket) and `cockpit agent reports` (the
//! owner, on the host-only socket). The daemon identifies the sender; nothing here names a
//! session.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use cockpit_client::{ClientError, Connection};
use cockpit_protocol::{
    AGENT_SOCKET_VARIABLE, AgentReport, AgentState, Command, CompactionPhase, CompactionReport,
    CompactionStatus, CompactionTrigger, ContextUsage, LAUNCH_TOKEN_VARIABLE, QuestionSource,
    Reply, SessionReport, TurnOutcome, Unblock, UnblockedBy,
};

/// Long enough for a busy daemon, short enough that a hook never hangs its agent.
pub(crate) const DEADLINE: Duration = Duration::from_secs(10);

/// `--state working|waiting`, `--label`, `--doing`, `--needs-input`, `--dialog`, the
/// value-less `--prompt`, `--outcome completed|failed|quota-stopped|network-lost`,
/// `--outcome-reason` (with `failed`, and always with `network-lost`), `--conversation` and
/// the value-less `--conversation-ends` (with `--conversation` only), and a context compaction's moment: `--compaction
/// started|ended|failed` with `--compaction-trigger auto|manual`, `--compaction-tokens-before N`
/// and `--compaction-tokens-after N`, which report nothing else (#782), and the harness's
/// context at a turn's end: `--context-tokens N` and `--context-window N`, with `--outcome`
/// only (#479). Each at most once.
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
    let socket = match caller_socket() {
        Ok(socket) => socket,
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
    let mut compaction = None;
    let mut compaction_trigger = None;
    let mut tokens_before = None;
    let mut tokens_after = None;
    let mut context_tokens = None;
    let mut context_window = None;
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
            "--compaction" => &mut compaction,
            "--compaction-trigger" => &mut compaction_trigger,
            "--compaction-tokens-before" => &mut tokens_before,
            "--compaction-tokens-after" => &mut tokens_after,
            "--context-tokens" => &mut context_tokens,
            "--context-window" => &mut context_window,
            "--dialog" => &mut report.dialog,
            "--conversation" => &mut report.conversation,
            _ => return Err(format!("unknown argument {name}")),
        };
        if field.replace(value).is_some() {
            return Err(format!("{name} given twice"));
        }
    }
    report.outcome = turn_outcome(outcome, outcome_reason)?;
    report.compaction =
        compaction_report(compaction, compaction_trigger, tokens_before, tokens_after)?;
    report.context = context_usage(context_tokens, context_window)?;
    if report.context.is_some() && report.outcome.is_none() {
        return Err(
            "--context-tokens needs --outcome: it is the context at a turn's end".to_owned(),
        );
    }
    if report.is_conversation_ending && report.conversation.is_none() {
        return Err("--conversation-ends needs --conversation".to_owned());
    }
    // The conversation only says where a report comes from; it reports nothing alone.
    let reported = AgentReport {
        conversation: None,
        is_conversation_ending: false,
        compaction: None,
        ..report.clone()
    };
    let is_reporting = reported != AgentReport::default();
    if report.compaction.is_some() {
        if is_reporting || report.is_conversation_ending {
            return Err("--compaction reports nothing else".to_owned());
        }
        return Ok(report);
    }
    if report.is_conversation_ending {
        if is_reporting {
            return Err("--conversation-ends reports nothing else".to_owned());
        }
        return Ok(report);
    }
    if !is_reporting {
        return Err(
            "name at least one of --state, --label, --doing, --needs-input, --dialog, --prompt, \
             --outcome, --compaction or --conversation-ends"
                .to_owned(),
        );
    }
    Ok(report)
}

/// The compaction the `--compaction*` flags describe; its trigger and sizes need `--compaction`.
fn compaction_report(
    phase: Option<String>,
    trigger: Option<String>,
    tokens_before: Option<String>,
    tokens_after: Option<String>,
) -> Result<Option<CompactionReport>, String> {
    let Some(phase) = phase else {
        if trigger.is_some() || tokens_before.is_some() || tokens_after.is_some() {
            return Err(
                "--compaction-trigger and --compaction-tokens-* need --compaction".to_owned(),
            );
        }
        return Ok(None);
    };
    let phase = [
        CompactionPhase::Started,
        CompactionPhase::Ended,
        CompactionPhase::Failed,
    ]
    .into_iter()
    .find(|candidate| candidate.as_str() == phase)
    .ok_or_else(|| format!("--compaction {phase}: use started, ended or failed"))?;
    let trigger = trigger
        .map(|trigger| {
            [CompactionTrigger::Auto, CompactionTrigger::Manual]
                .into_iter()
                .find(|candidate| candidate.as_str() == trigger)
                .ok_or_else(|| format!("--compaction-trigger {trigger}: use auto or manual"))
        })
        .transpose()?;
    Ok(Some(CompactionReport {
        phase,
        trigger,
        tokens_before: token_count("--compaction-tokens-before", tokens_before)?,
        tokens_after: token_count("--compaction-tokens-after", tokens_after)?,
    }))
}

/// The harness's context the `--context-*` flags describe; its window needs its tokens.
fn context_usage(
    tokens: Option<String>,
    window: Option<String>,
) -> Result<Option<ContextUsage>, String> {
    let Some(tokens) = token_count("--context-tokens", tokens)? else {
        if window.is_some() {
            return Err("--context-window needs --context-tokens".to_owned());
        }
        return Ok(None);
    };
    Ok(Some(ContextUsage {
        tokens,
        window: token_count("--context-window", window)?,
    }))
}

/// `value` of `flag` as a whole number of tokens.
fn token_count(flag: &str, value: Option<String>) -> Result<Option<u64>, String> {
    value
        .map(|value| {
            value
                .parse::<u64>()
                .map_err(|_| format!("{flag} {value}: use a whole number of tokens"))
        })
        .transpose()
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
        (Some("network-lost"), Some(reason)) => Ok(Some(TurnOutcome::NetworkLost { reason })),
        (Some("network-lost"), None) => {
            Err("--outcome network-lost needs --outcome-reason, the harness's own word".to_owned())
        }
        (None | Some("completed" | "quota-stopped"), Some(_)) => {
            Err("--outcome-reason needs --outcome failed or network-lost".to_owned())
        }
        (Some(other), _) => Err(format!(
            "--outcome {other}: use completed, failed, quota-stopped or network-lost"
        )),
    }
}

/// The agent socket: `TORLEK_AGENT_SOCKET` when the sandbox mounts it elsewhere, else its
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

/// Whether this process runs in a session Torlek launched: every launch sets `TORLEK_SESSION`
/// (`cockpit-sessions`' lifecycle) and a launch token, and a sandbox the agent socket's path;
/// any one of them is enough.
pub(crate) fn is_in_a_session() -> bool {
    [
        "TORLEK_SESSION",
        LAUNCH_TOKEN_VARIABLE,
        AGENT_SOCKET_VARIABLE,
    ]
    .into_iter()
    .any(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()))
}

/// The socket of whoever runs this command: the agent socket inside a session, the host-only
/// socket from the owner's own terminal. The daemon takes any peer on the host-only socket
/// for the owner, and an agent launched with `agents.sandbox = "off"` can reach it, so a
/// session never uses it: its owner verbs get the agent socket's `owner_only` refusal (#729).
pub(crate) fn caller_socket() -> Result<PathBuf, ExitCode> {
    if is_in_a_session() {
        agent_socket()
    } else {
        Ok(socket_dir()?.join(cockpit_config::DAEMON_SOCKET))
    }
}

/// The daemon's socket directory, or exit 1 when the environment cannot place it.
pub(crate) fn socket_dir() -> Result<PathBuf, ExitCode> {
    locate_socket_dir().map_err(|error| {
        eprintln!("torlek: {error}");
        ExitCode::FAILURE
    })
}

/// The daemon's socket directory, or why this environment names none.
pub(crate) fn locate_socket_dir() -> Result<PathBuf, cockpit_config::ConfigError> {
    let environment = crate::profile::environment();
    environment
        .home_dir()
        .and_then(|home| cockpit_config::Layout::resolve(&environment, &home).socket_dir())
}

pub(crate) fn exchange(socket: &Path, command: Command) -> Result<Reply, ClientError> {
    exchange_within(socket, command, DEADLINE)
}

/// [`exchange`] for a request that may wait longer than [`DEADLINE`] for its answer.
pub(crate) fn exchange_within(
    socket: &Path,
    command: Command,
    deadline: Duration,
) -> Result<Reply, ClientError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a single-threaded runtime starts");
    runtime.block_on(async {
        let connection = Connection::open(socket, DEADLINE).await?;
        connection.request(command, deadline).await
    })
}

/// Exit 1 after saying why, and for a refusal what would lift it.
pub(crate) fn failed(socket: &Path, outcome: Result<Reply, ClientError>) -> ExitCode {
    match outcome {
        Err(ClientError::Daemon(error)) => {
            eprintln!("torlek: refused: {}", error.message);
            if let Some(unblock) = &error.unblock {
                print_unblock(unblock);
            }
        }
        Err(ClientError::Connect { source, .. }) => {
            eprintln!(
                "torlek: cannot reach the daemon at {}: {source}",
                socket.display()
            );
        }
        Err(error) => eprintln!("torlek: {error}"),
        Ok(reply) => eprintln!("torlek: the daemon answered with an unexpected {reply:?}"),
    }
    ExitCode::FAILURE
}

/// Says on stderr who lifts a refusal and how.
pub(crate) fn print_unblock(unblock: &Unblock) {
    let by = match &unblock.by {
        UnblockedBy::Owner => "the owner",
        UnblockedBy::Agent => "you",
        UnblockedBy::Cockpit => "Torlek",
        UnblockedBy::Unknown(party) => party.as_str(),
    };
    eprintln!("torlek: to unblock ({by}): {}", unblock.action);
}

/// `session state label="…" doing="…" needs-input="…" outcome=… outcome-reason="…"`, leaving
/// out what was never reported; a dialog's question shows as `dialog="…"`, followed by
/// `also-waiting="…"` for the agent's own question it covers (#250).
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
        ("also-waiting", &report.covered_question),
    ] {
        if let Some(value) = value {
            line.push_str(&format!(" {key}={value:?}"));
        }
    }
    if let Some(compaction) = &report.compaction {
        line.push_str(&format!(
            " compaction={}",
            compaction_status(compaction.status)
        ));
        if let Some(trigger) = compaction.trigger {
            line.push_str(&format!(" compaction-trigger={}", trigger.as_str()));
        }
        for (key, value) in [
            ("compaction-tokens-before", compaction.tokens_before),
            ("compaction-tokens-after", compaction.tokens_after),
            ("compaction-seconds", compaction.seconds),
        ] {
            if let Some(value) = value {
                line.push_str(&format!(" {key}={value}"));
            }
        }
    }
    match &report.outcome {
        None => {}
        Some(TurnOutcome::Completed) => line.push_str(" outcome=completed"),
        Some(TurnOutcome::QuotaStopped) => line.push_str(" outcome=quota-stopped"),
        Some(TurnOutcome::NetworkLost { reason }) => {
            line.push_str(&format!(" outcome=network-lost outcome-reason={reason:?}"));
        }
        Some(TurnOutcome::Failed { reason }) => {
            line.push_str(" outcome=failed");
            if let Some(reason) = reason {
                line.push_str(&format!(" outcome-reason={reason:?}"));
            }
        }
    }
    line
}

fn compaction_status(status: CompactionStatus) -> &'static str {
    match status {
        CompactionStatus::Running => "running",
        CompactionStatus::Completed => "completed",
        CompactionStatus::Failed => "failed",
    }
}

//! `cockpit agent sessions|message|start`: a coordinator session observes, messages and
//! starts other sessions on the agent socket, under the grants the owner gave it with
//! `cockpit session grant` (#410). The daemon identifies the sender and refuses each verb
//! without its grant.
//!
//! Exit codes follow `cockpit ask run`: a start waiting for the owner exits 124 when the wait
//! ends first (`cockpit ask run --wait ID` goes on), 125 when declined, 126 when the daemon
//! cannot be reached.

use std::ffi::OsString;
use std::process::ExitCode;
use std::time::Duration;

use cockpit_protocol::{Command, ObservedSessionView, Reply};

use crate::ask::Agent;
use crate::daemon::{absolute_dir, utf8_words};

/// How often `--watch` reads the sessions again.
const WATCH_EVERY: Duration = Duration::from_secs(2);
/// A start launches an agent, which can take longer than a request usually may.
const START_DEADLINE: Duration = Duration::from_secs(120);

/// `cockpit agent sessions [--watch]`: one line per other session; with `--watch`, again
/// each time a line changes, until interrupted.
pub(crate) fn sessions(arguments: &[OsString]) -> Result<ExitCode, String> {
    let is_watching = match utf8_words(arguments)?.as_slice() {
        [] => false,
        ["--watch"] => true,
        _ => return Err("unknown agent sessions arguments".to_owned()),
    };
    let agent = match Agent::connect() {
        Ok(agent) => agent,
        Err(code) => return Ok(code),
    };
    let mut shown = None;
    loop {
        let listing = match agent.request(Command::AgentSessions {
            launch_token: crate::agent::launch_token(),
        }) {
            Ok(Reply::AgentSessions { sessions }) => listing(&sessions),
            other => return Ok(agent.failed(other)),
        };
        if shown.as_ref() != Some(&listing) {
            if shown.is_some() {
                out!("\n");
            }
            out!("{listing}");
            shown = Some(listing);
        }
        if !is_watching {
            return Ok(ExitCode::SUCCESS);
        }
        std::thread::sleep(WATCH_EVERY);
    }
}

/// `cockpit agent message SESSION TEXT`: queued now, typed at the session's next prompt.
pub(crate) fn message(arguments: &[OsString]) -> Result<ExitCode, String> {
    let (session, text) = match utf8_words(arguments)?.as_slice() {
        [session, text] => ((*session).to_owned(), (*text).to_owned()),
        _ => return Err("agent message takes SESSION and TEXT, the text quoted".to_owned()),
    };
    let agent = match Agent::connect() {
        Ok(agent) => agent,
        Err(code) => return Ok(code),
    };
    Ok(
        match agent.request(Command::AgentMessage {
            launch_token: crate::agent::launch_token(),
            session,
            text,
        }) {
            Ok(Reply::AgentMessageQueued { session, message }) => {
                outln!(
                    "queued message {message} to session {session}; it is typed at its next prompt"
                );
                ExitCode::SUCCESS
            }
            other => agent.failed(other),
        },
    )
}

/// `cockpit agent start DIR [--prompt TEXT] [--clone BRANCH]`: starts at once when the grant
/// needs no confirmation, otherwise asks the owner and waits like `cockpit ask run`.
pub(crate) fn start(arguments: &[OsString]) -> Result<ExitCode, String> {
    let words = utf8_words(arguments)?;
    let [dir, options @ ..] = words.as_slice() else {
        return Err("agent start takes DIR".to_owned());
    };
    let (mut prompt, mut clone) = (None, None);
    let mut options = options.iter();
    while let Some(flag) = options.next() {
        let slot = match *flag {
            "--prompt" => &mut prompt,
            "--clone" => &mut clone,
            _ => return Err(format!("unknown argument {flag}")),
        };
        let value = options
            .next()
            .ok_or_else(|| format!("{flag} needs a value"))?;
        if slot.replace((*value).to_owned()).is_some() {
            return Err(format!("{flag} given twice"));
        }
    }
    let dir = absolute_dir(dir)?;
    let agent = match Agent::connect() {
        Ok(agent) => agent,
        Err(code) => return Ok(code),
    };
    let asked = agent.request_within(
        Command::AgentStart {
            launch_token: crate::agent::launch_token(),
            dir,
            prompt,
            clone,
        },
        START_DEADLINE,
    );
    Ok(match asked {
        Ok(Reply::AgentStarted { session }) => {
            outln!("started session {}", session.id);
            ExitCode::SUCCESS
        }
        Ok(Reply::RunRequest { request }) => {
            eprintln!(
                "cockpit: asked the owner to start it (request {}); waiting",
                request.id
            );
            agent.wait_for_run(request.id)
        }
        other => agent.failed(other),
    })
}

/// `id  state  agent  agent-state  working-dir` then the report's `label=… doing=…
/// needs-input=…` (or `dialog=…`), tab-separated before the report.
fn listing(sessions: &[ObservedSessionView]) -> String {
    sessions
        .iter()
        .map(|session| {
            let report = session.report.as_ref();
            let agent_state = report
                .and_then(|report| report.state)
                .map_or("-", |state| state.as_str());
            let mut line = format!(
                "{}\t{}\t{}\t{agent_state}\t{}",
                session.id, session.state, session.agent, session.working_dir
            );
            if let Some(report) = report {
                let question_key = match report.question_source {
                    Some(cockpit_protocol::QuestionSource::Dialog) => "dialog",
                    Some(cockpit_protocol::QuestionSource::Agent) | None => "needs-input",
                };
                for (key, value) in [
                    ("label", &report.label),
                    ("doing", &report.doing),
                    (question_key, &report.needs_input),
                ] {
                    if let Some(value) = value {
                        line.push_str(&format!("\t{key}={value:?}"));
                    }
                }
            }
            line.push('\n');
            line
        })
        .collect()
}

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

/// `cockpit agent sessions [--watch]`: one line per other session; with `--watch`, then only
/// the line of a session that changed or appeared, and `ID\tended` for one that left the
/// listing, until interrupted (#707).
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
    let mut shown: Vec<(String, String)> = Vec::new();
    loop {
        let listed: Vec<(String, String)> = match agent.request(Command::AgentSessions {
            launch_token: crate::agent::launch_token(),
        }) {
            Ok(Reply::AgentSessions { sessions }) => sessions
                .iter()
                .map(|session| (session.id.clone(), session_line(session)))
                .collect(),
            other => return Ok(agent.failed(other)),
        };
        out!("{}", changes(&shown, &listed));
        shown = listed;
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
            Ok(Reply::AgentMessageQueued {
                session,
                message,
                waits,
            }) => {
                match waits {
                    Some(why) => outln!(
                        "queued message {message} to session {session}; it is not typed yet: {why}"
                    ),
                    None => outln!(
                        "queued message {message} to session {session}; it is typed once its \
                         agent waits at its prompt; if it cannot be typed, its card says why"
                    ),
                }
                ExitCode::SUCCESS
            }
            other => agent.failed(other),
        },
    )
}

/// `cockpit agent start DIR [--agent NAME] [--prompt TEXT] [--clone BRANCH]`: starts at once when the grant
/// needs no confirmation, otherwise asks the owner and waits like `cockpit ask run`.
pub(crate) fn start(arguments: &[OsString]) -> Result<ExitCode, String> {
    let words = utf8_words(arguments)?;
    let [dir, options @ ..] = words.as_slice() else {
        return Err("agent start takes DIR".to_owned());
    };
    let (mut agent_name, mut prompt, mut clone) = (None, None, None);
    let mut options = options.iter();
    while let Some(flag) = options.next() {
        let slot = match *flag {
            "--agent" => &mut agent_name,
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
            agent: agent_name,
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
                "torlek: asked the owner to start it (request {}); waiting",
                request.id
            );
            agent.wait_for_run(request.id)
        }
        other => agent.failed(other),
    })
}

/// The lines of `listed` that `shown` does not hold, then `ID\tended` for each session of
/// `shown` that `listed` no longer has. Both are `(id, line)` pairs.
fn changes(shown: &[(String, String)], listed: &[(String, String)]) -> String {
    let mut printed: String = listed
        .iter()
        .filter(|pair| !shown.contains(pair))
        .map(|(_, line)| line.as_str())
        .collect();
    for (id, _) in shown {
        if !listed.iter().any(|(listed_id, _)| listed_id == id) {
            printed.push_str(&format!("{id}\tended\n"));
        }
    }
    printed
}

/// `id  state  agent  agent-state  working-dir` then the report's `label=… doing=…
/// needs-input=…` (or `dialog=…`), tab-separated before the report.
fn session_line(session: &ObservedSessionView) -> String {
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
}

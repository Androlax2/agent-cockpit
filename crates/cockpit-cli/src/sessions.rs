//! `cockpit session`: start, list and change agent sessions, served by the daemon.

use std::borrow::Cow;
use std::ffi::OsString;
use std::process::ExitCode;

use cockpit_protocol::{Command, Reply, SessionFailure, SessionStateView, SessionView};

use crate::daemon::{
    BULK_DEADLINE, CONTROL_DEADLINE, absolute_dir, request, unexpected, utf8_words,
};

/// A `session` verb's command, or the usage problem that stops it.
pub fn parse(arguments: &[OsString]) -> Result<Command, String> {
    let words = utf8_words(arguments)?;
    let session = |id: &&str| (*id).to_owned();
    Ok(match words.as_slice() {
        ["new", dir, options @ ..] => session_new(dir, options)?,
        ["list"] => Command::SessionList { ended: false },
        ["list", "--ended"] => Command::SessionList { ended: true },
        ["restart", "--all"] => Command::SessionRestartAll,
        ["restart", id] => Command::SessionRestart {
            session: session(id),
        },
        ["sleep", id] => Command::SessionSleep {
            session: session(id),
        },
        ["wake", id] => Command::SessionWake {
            session: session(id),
        },
        ["resume", id] => Command::SessionResume {
            session: session(id),
        },
        ["end", id] => Command::SessionEnd {
            session: session(id),
        },
        ["link", id, url] => Command::SessionLink {
            session: session(id),
            url: (*url).to_owned(),
        },
        _ => return Err("unknown session arguments".to_owned()),
    })
}

/// `new DIR [--agent NAME] [--prompt TEXT] [--clone BRANCH]`.
fn session_new(dir: &str, options: &[&str]) -> Result<Command, String> {
    let (mut agent, mut prompt, mut clone) = (None, None, None);
    let mut options = options.iter();
    while let Some(flag) = options.next() {
        let slot = match *flag {
            "--agent" => &mut agent,
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
    Ok(Command::SessionNew {
        dir: absolute_dir(dir)?,
        agent,
        prompt,
        clone,
    })
}

/// Sends `command` and prints the sessions it answers, one line each. Exit 1 when the
/// daemon refuses, or when `restart --all` could neither restart nor defer every session;
/// a deferred session prints with its pending restart.
pub fn run(command: Command) -> ExitCode {
    let deadline = match command {
        Command::SessionList { .. } => CONTROL_DEADLINE,
        _ => BULK_DEADLINE,
    };
    let reply = match request(command, deadline) {
        Ok(reply) => reply,
        Err(code) => return code,
    };
    match &reply {
        Reply::Session { session } => print!("{}", session_line(session)),
        Reply::Sessions { sessions } => sessions.iter().for_each(|session| {
            print!("{}", session_line(session));
        }),
        Reply::RestartedAll {
            restarted,
            deferred,
            failed,
        } => {
            restarted.iter().chain(deferred).for_each(|session| {
                print!("{}", session_line(session));
            });
            failed.iter().for_each(|failure| {
                eprint!("{}", failure_line(failure));
            });
            if !failed.is_empty() {
                return ExitCode::FAILURE;
            }
        }
        _ => return unexpected(&reply),
    }
    ExitCode::SUCCESS
}

/// `id  state  agent  launch  conversation  working-dir  reason  restart`, tab-separated.
/// `restart` is `-`, `changed:<start files>` when the session runs on old start files, or
/// `pending:<start files>` once `restart --all` deferred its restart.
fn session_line(session: &SessionView) -> String {
    let launch = session
        .launch
        .as_ref()
        .map_or("-", |launch| launch.kind.as_str());
    let changed = session.changed_start_files.join(",");
    let restart = match (session.restart_pending, changed.is_empty()) {
        (true, _) => format!("pending:{changed}"),
        (false, false) => format!("changed:{changed}"),
        (false, true) => "-".to_owned(),
    };
    format!(
        "{}\t{}\t{}\t{launch}\t{}\t{}\t{}\t{restart}\n",
        session.id,
        state_word(&session.state),
        session.agent,
        session.conversation.as_deref().unwrap_or("-"),
        session.working_dir,
        session.reason.as_deref().unwrap_or("-"),
    )
}

fn state_word(state: &SessionStateView) -> Cow<'static, str> {
    Cow::Borrowed(match state {
        SessionStateView::Launching => "launching",
        SessionStateView::Running => "running",
        SessionStateView::Stopping { to } => {
            return Cow::Owned(format!("stopping:{}", to.as_str()));
        }
        SessionStateView::Asleep => "asleep",
        SessionStateView::Ended => "ended",
        SessionStateView::Failed => "failed",
        SessionStateView::Unknown => "unknown",
    })
}

fn failure_line(failure: &SessionFailure) -> String {
    format!(
        "cockpit: {} not restarted: {}: {}\n",
        failure.session,
        failure.error.code.as_str(),
        failure.error.message
    )
}

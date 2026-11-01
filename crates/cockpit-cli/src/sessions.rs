//! `cockpit session`, `cockpit provider` and `cockpit sandbox`: start, list, show and change
//! agent sessions, the providers they start with and the sandbox they run in, served by the
//! daemon.

use std::ffi::OsString;
use std::process::ExitCode;

use cockpit_protocol::{
    Command, CoordinatorAuditView, CoordinatorGrantView, Reply, SessionFailure, SessionView,
};

use crate::daemon::{
    BULK_DEADLINE, CONTROL_DEADLINE, absolute_dir, request, unexpected, utf8_words,
};

/// `provider list`: the providers `session new --agent` takes.
pub fn parse_provider(arguments: &[OsString]) -> Result<Command, String> {
    match utf8_words(arguments)?.as_slice() {
        ["list"] => Ok(Command::ProviderList),
        _ => Err("unknown provider arguments".to_owned()),
    }
}

/// `sandbox show DIR [--agent NAME]`: the sandbox a new session there would get.
pub fn parse_sandbox(arguments: &[OsString]) -> Result<Command, String> {
    match utf8_words(arguments)?.as_slice() {
        ["show", dir] => Ok(Command::SandboxShow {
            dir: absolute_dir(dir)?,
            agent: None,
        }),
        ["show", dir, "--agent", agent] => Ok(Command::SandboxShow {
            dir: absolute_dir(dir)?,
            agent: Some((*agent).to_owned()),
        }),
        _ => Err("unknown sandbox arguments".to_owned()),
    }
}

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
        ["end", "--finished"] => Command::SessionEndFinished { only: None },
        ["end", id] => Command::SessionEnd {
            session: session(id),
        },
        ["show", id] => Command::SessionShow {
            session: session(id),
        },
        ["sandbox", id] => Command::SessionSandbox {
            session: session(id),
        },
        ["link", id, url] => Command::SessionLink {
            session: session(id),
            url: (*url).to_owned(),
        },
        ["grant", id, permission, options @ ..] => session_grant(id, permission, options)?,
        ["revoke", id, permission] => Command::SessionRevoke {
            session: session(id),
            permission: (*permission).to_owned(),
        },
        ["grants", id] => Command::SessionGrants {
            session: session(id),
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

/// `grant SESSION PERMISSION [--per-hour N] [--without-confirmation]`: the daemon checks
/// which permission takes which terms (#410).
fn session_grant(id: &str, permission: &str, options: &[&str]) -> Result<Command, String> {
    let (mut per_hour, mut without_confirmation) = (None, false);
    let mut options = options.iter();
    while let Some(flag) = options.next() {
        match *flag {
            "--without-confirmation" if !without_confirmation => without_confirmation = true,
            "--per-hour" if per_hour.is_none() => {
                let value = options
                    .next()
                    .ok_or_else(|| "--per-hour needs a value".to_owned())?;
                per_hour = Some(
                    value
                        .parse::<u32>()
                        .map_err(|_| format!("--per-hour {value}: a whole number of starts"))?,
                );
            }
            "--without-confirmation" | "--per-hour" => return Err(format!("{flag} given twice")),
            _ => return Err(format!("unknown argument {flag}")),
        }
    }
    Ok(Command::SessionGrant {
        session: id.to_owned(),
        permission: permission.to_owned(),
        without_confirmation,
        per_hour,
    })
}

/// Sends `command` and prints the sessions it answers, one line each. Exit 1 when the
/// daemon refuses, when `restart --all` could neither restart nor defer every session, or
/// when `end --finished` could not end every finished session; a deferred session prints
/// with its pending restart.
pub fn run(command: Command) -> ExitCode {
    let deadline = match command {
        Command::SessionList { .. }
        | Command::SessionGrant { .. }
        | Command::SessionRevoke { .. }
        | Command::SessionGrants { .. }
        | Command::ProviderList
        | Command::SessionSandbox { .. }
        | Command::SandboxShow { .. } => CONTROL_DEADLINE,
        _ => BULK_DEADLINE,
    };
    let reply = match request(command, deadline) {
        Ok(reply) => reply,
        Err(code) => return code,
    };
    match &reply {
        Reply::Session { session } => out!("{}", session_line(session)),
        Reply::Sessions { sessions } => sessions.iter().for_each(|session| {
            out!("{}", session_line(session));
        }),
        Reply::Providers { providers } => providers.iter().for_each(|provider| {
            let resume = if provider.resume {
                "resume"
            } else {
                "no-resume"
            };
            let events = if provider.events {
                "events"
            } else {
                "no-events"
            };
            outln!("{}\t{resume}\t{events}", provider.name);
        }),
        Reply::SessionSandbox { evidence: None } => {
            outln!("sandbox: no launch recorded yet");
        }
        Reply::SessionSandbox {
            evidence: Some(evidence),
        } => evidence.lines().iter().for_each(|line| outln!("{line}")),
        Reply::SandboxProfile { evidence, command } => {
            evidence.lines().iter().for_each(|line| outln!("{line}"));
            outln!("command: {}", command.join(" "));
        }
        Reply::RestartedAll {
            restarted,
            deferred,
            failed,
        } => {
            restarted.iter().chain(deferred).for_each(|session| {
                out!("{}", session_line(session));
            });
            failed.iter().for_each(|failure| {
                eprint!("{}", failure_line(failure, "restarted"));
            });
            if !failed.is_empty() {
                return ExitCode::FAILURE;
            }
        }
        Reply::CoordinatorGrants {
            session: _,
            grants,
            audit,
        } => {
            grants
                .iter()
                .for_each(|grant| outln!("{}", grant_line(grant)));
            audit
                .iter()
                .for_each(|entry| outln!("{}", audit_line(entry)));
        }
        Reply::EndedFinished { ended, failed } => {
            ended.iter().for_each(|session| {
                out!("{}", session_line(session));
            });
            failed.iter().for_each(|failure| {
                eprint!("{}", failure_line(failure, "ended"));
            });
            if !failed.is_empty() {
                return ExitCode::FAILURE;
            }
        }
        _ => return unexpected(&reply),
    }
    ExitCode::SUCCESS
}

/// `id  state  agent  launch  conversation  working-dir  reason  restart  work`,
/// tab-separated. `restart` is `-`, `changed:<start files>` when the session runs on old
/// start files, or `pending:<start files>` once `restart --all` deferred its restart.
/// `work` is `finished` when the task list says its work is done (#157), `-` otherwise.
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
    let work = if session.finished { "finished" } else { "-" };
    format!(
        "{}\t{}\t{}\t{launch}\t{}\t{}\t{}\t{restart}\t{work}\n",
        session.id,
        session.state,
        session.agent,
        session.conversation.as_deref().unwrap_or("-"),
        session.working_dir,
        session.reason.as_deref().unwrap_or("-"),
    )
}

/// `grant  permission  id  terms`, tab-separated; `terms` is `per-hour=N confirmed` or
/// `per-hour=N unconfirmed` for `start-sessions`, `-` otherwise.
fn grant_line(grant: &CoordinatorGrantView) -> String {
    let terms = match (grant.per_hour, grant.needs_confirmation) {
        (Some(per_hour), Some(true)) => format!("per-hour={per_hour} confirmed"),
        (Some(per_hour), Some(false)) => format!("per-hour={per_hour} unconfirmed"),
        _ => "-".to_owned(),
    };
    format!("grant\t{}\t{}\t{terms}", grant.permission, grant.grant)
}

/// `audit  id  at  actor  action  target  detail`, tab-separated, newest first; `-` for what
/// an entry has not.
fn audit_line(entry: &CoordinatorAuditView) -> String {
    format!(
        "audit\t{}\t{}\t{}\t{}\t{}\t{}",
        entry.id,
        entry.at,
        entry.actor,
        entry.action,
        entry.target.as_deref().unwrap_or("-"),
        entry
            .detail
            .as_deref()
            .map_or("-".to_owned(), |detail| format!("{detail:?}")),
    )
}

/// `outcome` is what did not happen to the session: `restarted`, `ended`.
fn failure_line(failure: &SessionFailure, outcome: &str) -> String {
    format!(
        "cockpit: {} not {outcome}: {}: {}\n",
        failure.session,
        failure.error.code.as_str(),
        failure.error.message
    )
}

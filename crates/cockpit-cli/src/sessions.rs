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
        ["send", id, message] => Command::SessionSend {
            session: session(id),
            message: (*message).to_owned(),
        },
        ["cancel", id] => Command::SessionCancel {
            session: session(id),
        },
        ["end", "--finished"] => Command::SessionEndFinished { only: None },
        ["end", id] => Command::SessionEnd {
            session: session(id),
        },
        ["paste-image"] => Command::SessionPasteImage,
        ["show", id] => Command::SessionShow {
            session: session(id),
        },
        ["sandbox", id] => Command::SessionSandbox {
            session: session(id),
        },
        ["sandbox-record", id, "--client", client, options @ ..] => {
            let mut observations = Vec::new();
            let mut options = options.iter();
            while let Some(flag) = options.next() {
                if *flag != "--observed" {
                    return Err(format!("unknown sandbox-record argument {flag}"));
                }
                observations.push(options.next().ok_or("--observed needs text")?.to_string());
            }
            Command::SessionSandboxRecord {
                session: session(id),
                client_identity: (*client).to_owned(),
                observations,
            }
        }
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

/// `new DIR [--agent NAME] [--prompt TEXT] [--clone BRANCH] [--model MODEL] [--effort LEVEL]
/// [--compaction-threshold VALUE]`.
fn session_new(dir: &str, options: &[&str]) -> Result<Command, String> {
    let (mut agent, mut prompt, mut clone) = (None, None, None);
    let (mut model, mut effort, mut compaction_threshold) = (None, None, None);
    let mut options = options.iter();
    while let Some(flag) = options.next() {
        let slot = match *flag {
            "--agent" => &mut agent,
            "--prompt" => &mut prompt,
            "--clone" => &mut clone,
            "--model" => &mut model,
            "--effort" => &mut effort,
            "--compaction-threshold" => &mut compaction_threshold,
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
        model,
        effort,
        compaction_threshold,
    })
}

/// `grant SESSION PERMISSION [--all-projects] [--per-hour N] [--max-active N]
/// [--without-confirmation]`: the daemon checks which permission takes which terms (#410,
/// #739). Without `--all-projects` the grant covers the session's own project (#1131).
fn session_grant(id: &str, permission: &str, options: &[&str]) -> Result<Command, String> {
    let (mut per_hour, mut max_active, mut without_confirmation) = (None, None, false);
    let mut all_projects = false;
    let mut options = options.iter();
    while let Some(flag) = options.next() {
        match *flag {
            "--all-projects" if !all_projects => all_projects = true,
            "--without-confirmation" if !without_confirmation => without_confirmation = true,
            "--per-hour" if per_hour.is_none() => {
                per_hour = Some(count_value(flag, options.next(), "starts")?);
            }
            "--max-active" if max_active.is_none() => {
                max_active = Some(count_value(flag, options.next(), "sessions")?);
            }
            "--all-projects" | "--without-confirmation" | "--per-hour" | "--max-active" => {
                return Err(format!("{flag} given twice"));
            }
            _ => return Err(format!("unknown argument {flag}")),
        }
    }
    Ok(Command::SessionGrant {
        session: id.to_owned(),
        permission: permission.to_owned(),
        all_projects,
        without_confirmation,
        per_hour,
        max_active,
    })
}

/// The whole number of `what` given after `flag`.
fn count_value(flag: &str, value: Option<&&str>, what: &str) -> Result<u32, String> {
    let value = value.ok_or_else(|| format!("{flag} needs a value"))?;
    value
        .parse::<u32>()
        .map_err(|_| format!("{flag} {value}: a whole number of {what}"))
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
        | Command::SessionSandboxRecord { .. }
        | Command::SandboxShow { .. } => CONTROL_DEADLINE,
        _ => BULK_DEADLINE,
    };
    let is_show = matches!(command, Command::SessionShow { .. });
    let reply = match request(command, deadline) {
        Ok(reply) => reply,
        Err(code) => return code,
    };
    match &reply {
        Reply::SessionAction { session, held } => {
            out!("delivered to session {session}\n");
            // The owner's message overtook these; its agent was told they wait (#897).
            for held in held {
                out!(
                    "still held, sent before it: message {} from session {}; it reaches the \
                     agent when its turn ends, and the agent was told it waits\n",
                    held.message,
                    held.from
                );
            }
        }
        Reply::Session { session } if is_show => {
            out!("{}", session_line(session));
            out!("{}", choice_lines(session));
            out!("{}", exit_lines(session));
            out!("{}", over_lines(session));
        }
        Reply::Session { session } => {
            out!("{}", session_line(session));
            out!("{}", exit_lines(session));
            if let Some(needed) = &session.sign_in_needed {
                eprintln!("sign-in: {needed}");
            }
        }
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
        // CMD+V in the viewer runs it (#848): what it did shows in the session.
        Reply::ClipboardImage { .. } => {}
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
/// `reason` ends with the last line an agent that exited by itself printed (#629).
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
    let reason = match (&session.reason, last_output(session)) {
        (Some(reason), Some(line)) => format!("{reason} (last output: {line})"),
        (Some(reason), None) => reason.clone(),
        (None, _) => "-".to_owned(),
    };
    format!(
        "{}\t{}\t{}\t{launch}\t{}\t{}\t{reason}\t{restart}\t{work}\n",
        session.id,
        session.state,
        session.agent,
        session.conversation.as_deref().unwrap_or("-"),
        session.working_dir,
    )
}

/// The last line the session's latest agent printed before it exited by itself (#629).
fn last_output(session: &SessionView) -> Option<&str> {
    session.exit.as_ref()?.output.last().map(String::as_str)
}

/// What `session show` adds for the settings chosen at the start (#783); nothing when the
/// harness's own apply.
fn choice_lines(session: &SessionView) -> String {
    session
        .compaction_threshold
        .as_deref()
        .map(|threshold| format!("compaction threshold: {threshold}\n"))
        .unwrap_or_default()
}

/// How the session's latest agent exited by itself (#629): `exit:` and
/// its status, then `output:` and the last lines it printed, indented. Empty when it did not.
fn exit_lines(session: &SessionView) -> String {
    let Some(exit) = &session.exit else {
        return String::new();
    };
    let mut lines = format!("exit: {}\n", exit.status);
    if !exit.output.is_empty() {
        lines.push_str("output:\n");
        for line in &exit.output {
            lines.push_str(&format!("  {line}\n"));
        }
    }
    lines
}

/// What `session show` adds for an ended or failed session, which has no terminal to show
/// (#630): how to go on with it. Resume needs the conversation its provider named; without
/// one, only a new session goes on in its directory. Empty for a session that is not over.
fn over_lines(session: &SessionView) -> String {
    if !session.state.is_over() {
        return String::new();
    }
    let profile = &crate::profile::active().value;
    match &session.conversation {
        Some(_) => format!(
            "resume: torlek --profile {profile} session resume {}\n",
            session.id
        ),
        None => format!(
            "no conversation to resume: torlek --profile {profile} session new {}\n",
            session.working_dir
        ),
    }
}

/// `grant  permission  id  scope  terms`, tab-separated; `scope` is `project NAME` or
/// `everything` (#1131), `terms` is
/// `per-hour=N max-active=M confirmed` or `... unconfirmed` for `start-sessions`, `-`
/// otherwise.
fn grant_line(grant: &CoordinatorGrantView) -> String {
    let terms = match (grant.per_hour, grant.max_active, grant.needs_confirmation) {
        (Some(per_hour), Some(max_active), Some(needs_confirmation)) => {
            let confirmed = if needs_confirmation {
                "confirmed"
            } else {
                "unconfirmed"
            };
            format!("per-hour={per_hour} max-active={max_active} {confirmed}")
        }
        _ => "-".to_owned(),
    };
    format!(
        "grant\t{}\t{}\t{}\t{terms}",
        grant.permission, grant.grant, grant.scope
    )
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
        "torlek: {} not {outcome}: {}: {}\n",
        failure.session,
        failure.error.code.as_str(),
        failure.error.message
    )
}

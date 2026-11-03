//! `cockpit-claude-hook <cockpit program>`: the in-sandbox bridge Claude Code runs on each
//! hook event a launch subscribes to (`--settings`). It reads the payload on standard input,
//! normalizes it and reports the agent's state, from the conversation the payload names,
//! with `cockpit agent report`, which carries the launch token and finds the agent socket.
//!
//! On `SessionStart`, whatever started the context (a launch, a resume, `/clear`, a
//! compaction), it also prints the session's launch brief (`cockpit brief`) as the hook's
//! `additionalContext`, so the agent sees it without asking (#419). For every other event it
//! prints nothing on standard output, which Claude Code adds to the model's context.
//!
//! Reporting, it exits 1 on any failure and never 2, which would block Claude Code's stop or
//! prompt. `cockpit-claude-hook --version` is the daemon's start check that the
//! bridge runs on this machine (#203); Claude Code never passes it.
//!
//! `cockpit-claude-hook <cockpit program> --inbox` is the waiter Claude Code runs in the
//! background (`asyncRewake`) after a new context or a turn's end (#703): it waits with
//! `cockpit agent inbox` for the next message from another session, then prints it on
//! standard error and exits 2, which wakes the model with it as input of its own, never
//! typed at the owner's prompt. It exits 0 without a message when a newer waiter took over.

use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::process::{Child, Command, ExitCode, Stdio};

use cockpit_claude::hooks;

fn main() -> ExitCode {
    let arguments: Vec<OsString> = std::env::args_os().skip(1).collect();
    let outcome = match arguments.as_slice() {
        [flag] if flag == cockpit_claude::HOOK_VERSION_FLAG => {
            println!("{}", cockpit_claude::hook_version_line());
            Ok(ExitCode::SUCCESS)
        }
        [cockpit] => run(cockpit).map(|()| ExitCode::SUCCESS),
        [cockpit, flag] if flag == hooks::INBOX_FLAG => wait_for_message(cockpit),
        _ => Err(format!(
            "usage: torlek-claude-hook <torlek program> [{}] | {}",
            hooks::INBOX_FLAG,
            cockpit_claude::HOOK_VERSION_FLAG
        )),
    };
    outcome.unwrap_or_else(|reason| {
        eprintln!("torlek-claude-hook: {reason}");
        ExitCode::FAILURE
    })
}

/// Waits for the session's next message; hands it to the model through Claude Code's
/// `asyncRewake` (standard error, exit 2), or exits 0 when a newer waiter took over.
fn wait_for_message(cockpit: &OsStr) -> Result<ExitCode, String> {
    // Claude Code writes the payload whatever it is; nothing in it is needed here.
    std::io::copy(&mut std::io::stdin(), &mut std::io::sink())
        .map_err(|error| format!("cannot read the hook payload: {error}"))?;
    // Its standard error is kept apart: with status 2, all of the hook's goes to the model.
    let output = Command::new(cockpit)
        .args(hooks::INBOX_ARGUMENTS)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("cannot run {}: {error}", cockpit.to_string_lossy()))?;
    if !output.status.success() {
        return Err(format!(
            "`torlek agent inbox` failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let message = String::from_utf8(output.stdout)
        .map_err(|_| "`torlek agent inbox` printed no UTF-8".to_owned())?;
    let message = message.trim();
    if message.is_empty() {
        return Ok(ExitCode::SUCCESS);
    }
    eprintln!("{message}");
    Ok(ExitCode::from(hooks::REWAKE_STATUS))
}

fn run(cockpit: &OsStr) -> Result<(), String> {
    let mut payload = String::new();
    std::io::stdin()
        .read_to_string(&mut payload)
        .map_err(|error| format!("cannot read the hook payload: {error}"))?;
    let event = hooks::normalize(&payload).map_err(|error| error.to_string())?;
    let report = hooks::bridge_arguments(&event).map(|report| {
        let started = start_report(cockpit, &report);
        (report, started)
    });
    // Read while the report runs: both must answer within the hook's one timeout.
    let brief = hooks::starts_context(&event).then(|| read_brief(cockpit));
    let reported = report.map_or(Ok(()), |(report, started)| finish_report(&report, started));
    let brief = brief.transpose();
    if let Ok(Some(brief)) = &brief {
        println!("{}", hooks::brief_context(brief));
    }
    let failures: Vec<String> = [reported.err(), brief.err()]
        .into_iter()
        .flatten()
        .collect();
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

fn start_report(cockpit: &OsStr, report: &[String]) -> Result<Child, String> {
    Command::new(cockpit)
        .args(report)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .spawn()
        .map_err(|error| format!("cannot run {}: {error}", cockpit.to_string_lossy()))
}

fn finish_report(report: &[String], started: Result<Child, String>) -> Result<(), String> {
    let status = started?
        .wait()
        .map_err(|error| format!("`torlek {}` was lost: {error}", report.join(" ")))?;
    if !status.success() {
        return Err(format!("`torlek {}` failed ({status})", report.join(" ")));
    }
    Ok(())
}

/// What `cockpit brief` prints for this session, which the launch token identifies.
fn read_brief(cockpit: &OsStr) -> Result<String, String> {
    let output = Command::new(cockpit)
        .args(hooks::BRIEF_ARGUMENTS)
        .stdin(Stdio::null())
        .stderr(Stdio::inherit())
        .output()
        .map_err(|error| format!("cannot run {}: {error}", cockpit.to_string_lossy()))?;
    if !output.status.success() {
        return Err(format!("`torlek brief` failed ({})", output.status));
    }
    String::from_utf8(output.stdout).map_err(|_| "`torlek brief` printed no UTF-8".to_owned())
}

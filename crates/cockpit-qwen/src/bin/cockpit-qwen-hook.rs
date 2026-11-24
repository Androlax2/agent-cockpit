//! `torlek-qwen-hook <torlek program>`: the in-sandbox bridge Qwen Code runs on each hook
//! event the launch's system-defaults settings subscribe it to
//! (`QWEN_CODE_SYSTEM_DEFAULTS_PATH`). It reads the payload on standard input, normalizes it
//! and reports the agent's state, from the conversation the payload names, with `torlek agent
//! report`, which carries the launch token and finds the agent socket.
//!
//! On `SessionStart`, whatever started the context (a launch, a resume, `/clear`, a
//! compaction), it also prints the session's launch brief (`torlek brief`) as the hook's
//! `additionalContext`, so the agent sees it without asking (#419). For every other event it
//! prints nothing on standard output.
//!
//! It exits 1 on any failure and never 2, which would block Qwen Code's stop or prompt.
//! `torlek-qwen-hook --version` is the daemon's start check that the bridge runs on this
//! machine; Qwen Code never passes it.

use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::process::{Child, Command, ExitCode, Stdio};

use cockpit_qwen::hooks;

fn main() -> ExitCode {
    let arguments: Vec<OsString> = std::env::args_os().skip(1).collect();
    let outcome = match arguments.as_slice() {
        [flag] if flag == cockpit_qwen::HOOK_VERSION_FLAG => {
            println!("{}", cockpit_qwen::hook_version_line());
            Ok(())
        }
        [cockpit] => run(cockpit),
        _ => Err(format!(
            "usage: torlek-qwen-hook <torlek program> | {}",
            cockpit_qwen::HOOK_VERSION_FLAG
        )),
    };
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(reason) => {
            eprintln!("torlek-qwen-hook: {reason}");
            ExitCode::FAILURE
        }
    }
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

/// What `torlek brief` prints for this session, which the launch token identifies.
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

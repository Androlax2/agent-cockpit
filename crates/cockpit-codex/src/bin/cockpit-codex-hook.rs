//! `cockpit-codex-hook <cockpit program>`: the in-sandbox bridge Codex runs on each hook
//! event a launch subscribes to (`--config hooks.*`). It reads the payload on standard input,
//! normalizes it and reports the agent's state, from the thread the payload names, with
//! `cockpit agent report`, which carries the launch token and finds the agent socket.
//!
//! On `SessionStart`, whatever started the context (a new thread, a resume, `/clear`, a
//! compaction), it answers with the session's launch brief (`cockpit brief`) as the hook's
//! `additionalContext`, so the agent sees it without asking (#419). For every other event it
//! prints nothing on standard output, which Codex reads as the hook's answer: a permission
//! request is never approved or denied by it.
//!
//! It exits 1 on any failure and never 2, which would block Codex's prompt or stop. `cockpit-codex-hook --version` is the daemon's
//! start check that the bridge runs on this machine (#203); Codex never passes it.

use std::ffi::OsStr;
use std::io::Read;
use std::path::Path;
use std::process::{Command, ExitCode, Stdio};

use cockpit_codex::hooks;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(reason) => {
            eprintln!("cockpit-codex-hook: {reason}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let mut arguments = std::env::args_os().skip(1);
    let (Some(cockpit), None) = (arguments.next(), arguments.next()) else {
        return Err("usage: cockpit-codex-hook <cockpit program> | --version".to_owned());
    };
    if cockpit == cockpit_codex::HOOK_VERSION_FLAG {
        println!("{}", cockpit_codex::hook_version_line());
        return Ok(());
    }
    let mut payload = String::new();
    std::io::stdin()
        .read_to_string(&mut payload)
        .map_err(|error| format!("cannot read the hook payload: {error}"))?;
    let event = hooks::normalize(&payload).map_err(|error| error.to_string())?;
    let (reported, brief) = std::thread::scope(|scope| {
        let reporting = scope.spawn(|| {
            let state = std::env::var_os(cockpit_codex::reports::STATE_VARIABLE);
            hooks::bridge_arguments(&event).map_or(Ok(()), |report| {
                cockpit_codex::reports::send(
                    &cockpit,
                    state.as_deref().map(Path::new),
                    &report,
                    hooks::clears_conversation(&event),
                )
            })
        });
        // Read while the report runs: both must answer within the hook's one timeout.
        let brief = hooks::starts_context(&event).then(|| read_brief(&cockpit));
        (
            reporting
                .join()
                .unwrap_or_else(|_| Err("the report thread panicked".to_owned())),
            brief,
        )
    });
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

/// What `cockpit brief` prints for this session, which the launch token identifies.
fn read_brief(cockpit: &OsStr) -> Result<String, String> {
    let output = Command::new(cockpit)
        .args(hooks::BRIEF_ARGUMENTS)
        .stdin(Stdio::null())
        .stderr(Stdio::inherit())
        .output()
        .map_err(|error| format!("cannot run {}: {error}", cockpit.to_string_lossy()))?;
    if !output.status.success() {
        return Err(format!("`cockpit brief` failed ({})", output.status));
    }
    String::from_utf8(output.stdout).map_err(|_| "`cockpit brief` printed no UTF-8".to_owned())
}

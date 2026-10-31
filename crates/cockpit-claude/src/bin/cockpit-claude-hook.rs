//! `cockpit-claude-hook <cockpit program>`: the in-sandbox bridge Claude Code runs on each
//! hook event a launch subscribes to (`--settings`). It reads the payload on standard input,
//! normalizes it and reports the agent's state with `cockpit agent report`, which carries
//! the launch token and finds the agent socket.
//!
//! It exits 1 on any failure and never 2, which would block Claude Code's stop or prompt,
//! and it prints nothing on standard output, which Claude Code adds to the model's context
//! for some events.

use std::io::Read;
use std::process::{Command, ExitCode, Stdio};

use cockpit_claude::hooks;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(reason) => {
            eprintln!("cockpit-claude-hook: {reason}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let mut arguments = std::env::args_os().skip(1);
    let (Some(cockpit), None) = (arguments.next(), arguments.next()) else {
        return Err("usage: cockpit-claude-hook <cockpit program>".to_owned());
    };
    let mut payload = String::new();
    std::io::stdin()
        .read_to_string(&mut payload)
        .map_err(|error| format!("cannot read the hook payload: {error}"))?;
    let event = hooks::normalize(&payload).map_err(|error| error.to_string())?;
    let Some(report) = hooks::report_arguments(&event) else {
        return Ok(());
    };
    let mut command = Command::new(&cockpit);
    command
        .args(&report)
        .stdin(Stdio::null())
        .stdout(Stdio::null());
    let status = command
        .status()
        .map_err(|error| format!("cannot run {}: {error}", cockpit.to_string_lossy()))?;
    if !status.success() {
        return Err(format!("`cockpit {}` failed ({status})", report.join(" ")));
    }
    Ok(())
}

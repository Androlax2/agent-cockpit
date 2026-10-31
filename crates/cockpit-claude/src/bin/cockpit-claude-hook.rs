//! `cockpit-claude-hook <cockpit program>`: the in-sandbox bridge Claude Code runs on each
//! hook event a launch subscribes to (`--settings`). It reads the payload on standard input,
//! normalizes it and reports the agent's state with `cockpit agent report`, which carries
//! the launch token and finds the agent socket.
//!
//! It exits 1 on any failure and never 2, which would block Claude Code's stop or prompt,
//! and it prints nothing on standard output, which Claude Code adds to the model's context
//! for some events.

use std::ffi::OsString;
use std::io::Read;
use std::process::{Command, ExitCode, Stdio};

use cockpit_claude::hooks;

/// The variables `cockpit agent report` needs from the agent's launch.
const LAUNCH_VARIABLES: [&str; 2] = ["COCKPIT_LAUNCH_TOKEN", "COCKPIT_AGENT_SOCKET"];

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
    for (name, value) in inherited_launch_variables() {
        command.env(name, value);
    }
    let status = command
        .status()
        .map_err(|error| format!("cannot run {}: {error}", cockpit.to_string_lossy()))?;
    if !status.success() {
        return Err(format!("`cockpit {}` failed ({status})", report.join(" ")));
    }
    Ok(())
}

/// Launch variables this process lacks, from its nearest ancestor that has them. Claude
/// Code (2.1.300 and later) runs hooks under its own daemon with a scrubbed environment;
/// the agent the launcher started still carries them.
fn inherited_launch_variables() -> Vec<(&'static str, OsString)> {
    LAUNCH_VARIABLES
        .into_iter()
        .filter(|name| std::env::var_os(name).is_none_or(|value| value.is_empty()))
        .filter_map(|name| Some((name, ancestor_variable(name)?)))
        .collect()
}

#[cfg(target_os = "linux")]
fn ancestor_variable(name: &str) -> Option<OsString> {
    use std::os::unix::ffi::OsStringExt;

    let prefix = format!("{name}=");
    let mut pid = std::os::unix::process::parent_id();
    while pid > 1 {
        if let Ok(environment) = std::fs::read(format!("/proc/{pid}/environ"))
            && let Some(value) = environment
                .split(|byte| *byte == 0)
                .find_map(|entry| entry.strip_prefix(prefix.as_bytes()))
                .filter(|value| !value.is_empty())
        {
            return Some(OsString::from_vec(value.to_vec()));
        }
        pid = parent_of(pid)?;
    }
    None
}

#[cfg(target_os = "linux")]
fn parent_of(pid: u32) -> Option<u32> {
    std::fs::read_to_string(format!("/proc/{pid}/status"))
        .ok()?
        .lines()
        .find_map(|line| line.strip_prefix("PPid:"))?
        .trim()
        .parse()
        .ok()
}

/// Elsewhere the agent channel refuses every sender as unverifiable (report 11), so there
/// is nothing to recover.
#[cfg(not(target_os = "linux"))]
fn ancestor_variable(_name: &str) -> Option<OsString> {
    None
}

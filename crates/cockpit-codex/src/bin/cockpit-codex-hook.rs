//! `cockpit-codex-hook <cockpit program>`: the in-sandbox bridge Codex runs on each hook
//! event a launch subscribes to (`--config hooks.*`). It reads the payload on standard input,
//! normalizes it and reports the agent's state, from the thread the payload names, with
//! `cockpit agent report`, which carries the launch token and finds the agent socket.
//!
//! It exits 1 on any failure and never 2, which would block Codex's prompt or stop, and it
//! prints nothing on standard output, which Codex reads as the hook's answer: a permission
//! request is never approved or denied by it. `cockpit-codex-hook --version` is the daemon's
//! start check that the bridge runs on this machine (#203); Codex never passes it.

use std::io::Read;
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
    let Some(report) = hooks::bridge_arguments(&event) else {
        return Ok(());
    };
    let status = Command::new(&cockpit)
        .args(&report)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .status()
        .map_err(|error| format!("cannot run {}: {error}", cockpit.to_string_lossy()))?;
    if !status.success() {
        return Err(format!("`cockpit {}` failed ({status})", report.join(" ")));
    }
    Ok(())
}

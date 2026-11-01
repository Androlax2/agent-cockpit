//! `cockpit-opencode-bridge <cockpit program>`: the in-sandbox bridge Cockpit's OpenCode
//! plugin runs for each event it forwards. It reads the event on standard input, normalizes
//! it and reports the agent's state, from the OpenCode session the event names, with
//! `cockpit agent report`, which carries the launch token and finds the agent socket.
//!
//! It exits 1 on any failure and prints nothing on standard output; the plugin ignores both,
//! so OpenCode never stalls on Cockpit. `cockpit-opencode-bridge --version` is the daemon's
//! start check that the bridge runs on this machine; the plugin never passes it.

use std::io::Read;
use std::process::{Command, ExitCode, Stdio};

use cockpit_opencode::events;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(reason) => {
            eprintln!("cockpit-opencode-bridge: {reason}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let mut arguments = std::env::args_os().skip(1);
    let (Some(cockpit), None) = (arguments.next(), arguments.next()) else {
        return Err("usage: cockpit-opencode-bridge <cockpit program> | --version".to_owned());
    };
    if cockpit == cockpit_opencode::BRIDGE_VERSION_FLAG {
        println!("{}", cockpit_opencode::bridge_version_line());
        return Ok(());
    }
    let mut event = String::new();
    std::io::stdin()
        .read_to_string(&mut event)
        .map_err(|error| format!("cannot read the event: {error}"))?;
    let event = events::normalize(&event).map_err(|error| error.to_string())?;
    let Some(report) = events::bridge_arguments(&event) else {
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

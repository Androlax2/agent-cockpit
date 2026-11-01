//! `cockpit desktop-session import`: hands the daemon this shell's display and session bus
//! variables for the agents it launches from now on (#307). A login or compositor start runs
//! it, so a daemon that outlived the graphical session does not give agents a dead display.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::process::ExitCode;

use cockpit_protocol::{Command, DESKTOP_SESSION_VARIABLES, Reply};

use crate::daemon::{CONTROL_DEADLINE, request, unexpected, utf8_words};

/// The `desktop-session` verb's command, from this process's environment.
pub fn parse(arguments: &[OsString]) -> Result<Command, String> {
    match utf8_words(arguments)?.as_slice() {
        ["import"] => Ok(Command::DesktopSessionImport {
            variables: own_variables()?,
        }),
        _ => Err("unknown desktop-session arguments".to_owned()),
    }
}

/// The desktop session variables this process has; an unset one stays out, so later
/// agents get it unset too.
fn own_variables() -> Result<BTreeMap<String, String>, String> {
    let mut variables = BTreeMap::new();
    for name in DESKTOP_SESSION_VARIABLES {
        let Some(value) = std::env::var_os(name) else {
            continue;
        };
        let value = value
            .into_string()
            .map_err(|_| format!("${name} is not UTF-8"))?;
        variables.insert(name.to_owned(), value);
    }
    Ok(variables)
}

pub fn run(command: Command) -> ExitCode {
    match request(command, CONTROL_DEADLINE) {
        Ok(Reply::DesktopSessionImported { withheld: false }) => ExitCode::SUCCESS,
        // Recorded all the same: a login script must not fail because the owner opted out.
        Ok(Reply::DesktopSessionImported { withheld: true }) => {
            eprintln!(
                "cockpit: recorded, but agents.desktop_session = \"withheld\" keeps every agent off the desktop session"
            );
            ExitCode::SUCCESS
        }
        Ok(other) => unexpected(&other),
        Err(code) => code,
    }
}

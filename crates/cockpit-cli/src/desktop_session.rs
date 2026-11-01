//! `cockpit desktop-session import`: hands the daemon this shell's display and session bus
//! variables for the agents it launches from now on (#307). A login or compositor start runs
//! it, so a daemon that outlived the graphical session does not give agents a dead display.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::process::ExitCode;

use cockpit_protocol::{Command, DESKTOP_SESSION_VARIABLES, Reply};

use crate::daemon::{CONTROL_DEADLINE, fail, request, unexpected, utf8_words};

/// The `desktop-session` verbs; only `import` exists.
pub fn parse(arguments: &[OsString]) -> Result<(), String> {
    match utf8_words(arguments)?.as_slice() {
        ["import"] => Ok(()),
        _ => Err("unknown desktop-session arguments".to_owned()),
    }
}

/// Sends this process's desktop session variables; an unset one stays out, so later agents
/// get it unset too.
pub fn import() -> ExitCode {
    let variables = match own_variables() {
        Ok(variables) => variables,
        Err(problem) => return fail(&problem),
    };
    // Legitimate (an agent then gets no display), but from a TTY or SSH login it is more
    // likely a mistake, and it replaces a live session's values.
    if variables.is_empty() {
        eprintln!(
            "cockpit: no desktop session variable is set here; agents launched from now on get none"
        );
    }
    match request(
        Command::DesktopSessionImport { variables },
        CONTROL_DEADLINE,
    ) {
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

fn own_variables() -> Result<BTreeMap<String, String>, String> {
    let mut variables = BTreeMap::new();
    for name in DESKTOP_SESSION_VARIABLES {
        let Some(value) = std::env::var_os(name) else {
            continue;
        };
        let value = value
            .into_string()
            .map_err(|_| format!("${name} is not UTF-8, so it cannot be imported"))?;
        variables.insert(name.to_owned(), value);
    }
    Ok(variables)
}

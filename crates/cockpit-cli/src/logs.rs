//! Timed changes to this profile's running daemon filter, through its host socket.
use std::ffi::OsString;
use std::process::ExitCode;

use crate::daemon::{CONTROL_DEADLINE, request, unexpected, utf8_words};
use cockpit_protocol::{Command, Reply};

pub fn parse(arguments: &[OsString]) -> Result<Command, String> {
    match utf8_words(arguments)?.as_slice() {
        ["level"] => Ok(Command::LogsLevelStatus),
        ["level", "--revert"] => Ok(Command::LogsLevelRevert),
        ["level", directives] if !directives.starts_with('-') => Ok(Command::LogsLevelSet {
            directives: (*directives).into(),
            duration: None,
        }),
        ["level", directives, "--for", duration]
            if !directives.starts_with('-') && !duration.starts_with('-') =>
        {
            Ok(Command::LogsLevelSet {
                directives: (*directives).into(),
                duration: Some((*duration).into()),
            })
        }
        _ => Err("use logs level [DIRECTIVES [--for Ns|Nm] | --revert]".into()),
    }
}

pub fn run(command: Command) -> ExitCode {
    match request(command, CONTROL_DEADLINE) {
        Ok(Reply::LogsLevel {
            directives,
            expires_at_unix_ms,
        }) => {
            outln!("level: {directives}");
            match expires_at_unix_ms {
                Some(expiry) => outln!("override expires at Unix milliseconds: {expiry}"),
                None => outln!("no active override"),
            }
            ExitCode::SUCCESS
        }
        Ok(other) => unexpected(&other),
        Err(code) => code,
    }
}

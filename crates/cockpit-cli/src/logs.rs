//! Timed changes to this profile's running daemon filter, through its host socket.
use std::ffi::OsString;
use std::process::ExitCode;

use chrono::{DateTime, Local, TimeDelta, Utc};

use crate::daemon::{CONTROL_DEADLINE, fail, request, unexpected, utf8_words};
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
            expires_at_unix_ms: None,
            ..
        }) => {
            outln!("level: {directives}");
            outln!("no active override");
            ExitCode::SUCCESS
        }
        Ok(Reply::LogsLevel {
            directives,
            expires_at_unix_ms: Some(expiry),
            reverts_to,
        }) => {
            let Some(expiry) = i64::try_from(expiry)
                .ok()
                .and_then(DateTime::from_timestamp_millis)
            else {
                return fail(&format!(
                    "the daemon sent an unrepresentable override expiry: {expiry}"
                ));
            };
            let clock = expiry.with_timezone(&Local).format("%H:%M:%S");
            let remaining = describe_remaining(expiry - Utc::now());
            match reverts_to {
                Some(baseline) => {
                    outln!(
                        "level: {directives} until {clock} (in {remaining}), then back to {baseline}"
                    )
                }
                None => outln!("level: {directives} until {clock} (in {remaining})"),
            }
            ExitCode::SUCCESS
        }
        Ok(other) => unexpected(&other),
        Err(code) => code,
    }
}

/// Rounds up so an override set `--for 1m` reads "1m", not "59s".
fn describe_remaining(remaining: TimeDelta) -> String {
    let milliseconds = u64::try_from(remaining.num_milliseconds()).unwrap_or(0);
    let seconds = milliseconds.div_ceil(1000);
    match (seconds / 60, seconds % 60) {
        (0, seconds) => format!("{seconds}s"),
        (minutes, 0) => format!("{minutes}m"),
        (minutes, seconds) => format!("{minutes}m {seconds}s"),
    }
}

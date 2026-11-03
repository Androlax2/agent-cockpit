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
            let then = reverts_to
                .map_or_else(String::new, |baseline| format!(", then back to {baseline}"));
            outln!("level: {directives} until {clock} ({remaining}){then}");
            ExitCode::SUCCESS
        }
        Ok(other) => unexpected(&other),
        Err(code) => code,
    }
}

/// Rounds up so an override set `--for 1m` reads "in 1m", not "in 59s". A past expiry
/// still in effect (the daemon's timer missed it, as across a suspend) says so.
fn describe_remaining(remaining: TimeDelta) -> String {
    let Ok(milliseconds) = u64::try_from(remaining.num_milliseconds()) else {
        let overdue = describe_duration((-remaining.num_milliseconds()).unsigned_abs());
        return format!("overdue by {overdue}");
    };
    format!("in {}", describe_duration(milliseconds))
}

fn describe_duration(milliseconds: u64) -> String {
    let seconds = milliseconds.div_ceil(1000);
    match (seconds / 60, seconds % 60) {
        (0, seconds) => format!("{seconds}s"),
        (minutes, 0) => format!("{minutes}m"),
        (minutes, seconds) => format!("{minutes}m {seconds}s"),
    }
}

//! `torlek usage quota`: each provider account's plan windows as the daemon last read them
//! (#58, report 18).

use std::ffi::OsString;
use std::process::ExitCode;

use cockpit_protocol::{Command, QuotaAccountView, QuotaWindowView, Reply};

use crate::daemon::{CONTROL_DEADLINE, request, unexpected, utf8_words};
use crate::status_page::age;

/// A `usage` verb's command, or the usage problem that stops it.
pub fn parse(arguments: &[OsString]) -> Result<Command, String> {
    let words = utf8_words(arguments)?;
    match words.as_slice() {
        ["quota"] => Ok(Command::UsageQuota),
        _ => Err("unknown usage arguments".to_owned()),
    }
}

/// Sends `command` and prints what it answers. Exit 1 when the daemon refuses.
pub fn run(command: Command) -> ExitCode {
    let reply = match request(command, CONTROL_DEADLINE) {
        Ok(reply) => reply,
        Err(code) => return code,
    };
    let Reply::UsageQuota { accounts } = &reply else {
        return unexpected(&reply);
    };
    out!("{}", quota_text(accounts, crate::update::now()));
    ExitCode::SUCCESS
}

/// One line per account (`provider  account  plan  when read  current or stale`, then
/// `blocked` when the provider refuses ordinary use), then one indented line per window and the
/// last failed refresh, if any.
fn quota_text(accounts: &[QuotaAccountView], now: i64) -> String {
    if accounts.is_empty() {
        return "no quota read yet\n".to_owned();
    }
    let mut text = String::new();
    for account in accounts {
        let read = match account.observed_at {
            Some(observed_at) => format!("read {}", age(now - observed_at)),
            None => "never read".to_owned(),
        };
        text.push_str(&format!(
            "{}\t{}\tplan {}\t{read}\t{}{}\n",
            printable(&account.provider),
            printable(&account.account),
            account
                .plan
                .as_deref()
                .map_or("unknown".to_owned(), printable),
            if account.is_current {
                "current"
            } else {
                "stale"
            },
            if account.is_blocked { "\tblocked" } else { "" },
        ));
        for window in &account.windows {
            text.push_str(&window_text(window, now));
        }
        if let Some(failure) = &account.failure {
            let is_before_the_reading = account
                .observed_at
                .is_some_and(|observed_at| failure.at < observed_at);
            text.push_str(&format!(
                "  refresh failed {}{}: {}\n",
                age(now - failure.at),
                if is_before_the_reading {
                    ", before the last reading"
                } else {
                    ""
                },
                printable(&failure.reason)
            ));
        }
    }
    text
}

fn window_text(window: &QuotaWindowView, now: i64) -> String {
    let reset = match window.resets_at {
        Some(resets_at) if resets_at > now => format!("resets in {}", span(resets_at - now)),
        Some(resets_at) => format!("reset {}", age(now - resets_at)),
        None => "reset unknown".to_owned(),
    };
    format!(
        "  {}\t{}%\t{}\t{reset}\n",
        printable(&window.limit),
        window.used_percent,
        window.state.as_str()
    )
}

/// A wait in its two largest units, rounded down, so a reset 4 days 23 h away does not read
/// as 4 days.
fn span(seconds: i64) -> String {
    match seconds {
        ..120 => format!("{seconds} s"),
        120..7200 => format!("{} min", seconds / 60),
        7200..172_800 => format!("{} h {} min", seconds / 3600, seconds % 3600 / 60),
        _ => format!("{} days {} h", seconds / 86_400, seconds % 86_400 / 3600),
    }
}

/// The provider's words are data: a control character (an escape sequence, a newline or a tab
/// that would fake a line or a column) is shown escaped, the rest as it is.
fn printable(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_control() {
                character.escape_debug().to_string()
            } else {
                character.to_string()
            }
        })
        .collect()
}

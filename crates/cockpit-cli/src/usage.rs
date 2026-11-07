//! `torlek usage quota`: each provider account's plan windows as the daemon last read them
//! (#58, report 18).

use std::ffi::OsString;
use std::process::ExitCode;

use cockpit_protocol::{Command, QuotaAccountView, QuotaWindowView, Reply};

use crate::daemon::{CONTROL_DEADLINE, request, unexpected, utf8_words};

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
    out!("{}", quota_text(accounts, unix_now()));
    ExitCode::SUCCESS
}

fn unix_now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// One line per account (`provider  account  plan  when read  current or stale`), then one
/// indented line per window and the last failed refresh, if any.
fn quota_text(accounts: &[QuotaAccountView], now: i64) -> String {
    if accounts.is_empty() {
        return "no quota read yet\n".to_owned();
    }
    let mut text = String::new();
    for account in accounts {
        let read = match account.observed_at {
            Some(observed_at) => format!("read {}", ago(now - observed_at)),
            None => "never read".to_owned(),
        };
        text.push_str(&format!(
            "{}\t{}\tplan {}\t{read}\t{}\n",
            account.provider,
            account.account,
            account.plan.as_deref().unwrap_or("unknown"),
            if account.is_current {
                "current"
            } else {
                "stale"
            },
        ));
        for window in &account.windows {
            text.push_str(&window_text(window, now));
        }
        if let Some(failure) = &account.failure {
            text.push_str(&format!(
                "  refresh failed {}: {}\n",
                ago(now - failure.at),
                failure.reason
            ));
        }
    }
    text
}

fn window_text(window: &QuotaWindowView, now: i64) -> String {
    let reset = match window.resets_at {
        Some(resets_at) if resets_at > now => format!("resets in {}", span(resets_at - now)),
        Some(resets_at) => format!("reset {}", ago(now - resets_at)),
        None => "reset unknown".to_owned(),
    };
    format!(
        "  {}\t{}%\t{}\t{reset}\n",
        window.limit,
        window.used_percent,
        window.state.as_str()
    )
}

fn ago(seconds: i64) -> String {
    if seconds < 10 {
        return "just now".to_owned();
    }
    format!("{} ago", span(seconds))
}

fn span(seconds: i64) -> String {
    match seconds {
        ..120 => format!("{seconds} s"),
        120..7200 => format!("{} min", seconds / 60),
        7200..172_800 => format!("{} h", seconds / 3600),
        _ => format!("{} days", seconds / 86_400),
    }
}

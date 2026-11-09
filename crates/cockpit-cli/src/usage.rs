//! `torlek usage quota`: each provider account's plan windows as the daemon last read them
//! (#58, report 18). `torlek usage cost [--days N]`: what each account's replies cost at the
//! provider's API prices, per UTC day and project (#699). `torlek usage digest [--days N]`: each
//! UTC day's time with the agent, turns, approvals, cost and provider incidents (#700).

use std::ffi::OsString;
use std::process::ExitCode;

use cockpit_protocol::{
    Command, CostArchiveView, DayDigestView, DigestView, IncidentView, ProjectDayView,
    QuotaAccountView, QuotaPaceView, QuotaWindowView, Reply,
};

use crate::daemon::{CONTROL_DEADLINE, request, unexpected, utf8_words};
use crate::status_page::age;

const DEFAULT_DAYS: u32 = 7;
const MOST_DAYS: u32 = 366;
const NANOS_PER_TEN_THOUSANDTH: u64 = 100_000;

/// A `usage` verb's command, or the usage problem that stops it.
pub fn parse(arguments: &[OsString]) -> Result<Command, String> {
    let words = utf8_words(arguments)?;
    match words.as_slice() {
        ["quota"] => Ok(Command::UsageQuota),
        ["cost"] => Ok(Command::UsageCost { days: DEFAULT_DAYS }),
        ["cost", "--days", days] => match days.parse::<u32>() {
            Ok(days) if (1..=MOST_DAYS).contains(&days) => Ok(Command::UsageCost { days }),
            _ => Err(format!("--days is a number from 1 to {MOST_DAYS}")),
        },
        ["digest"] => Ok(Command::UsageDigest { days: DEFAULT_DAYS }),
        ["digest", "--days", days] => match days.parse::<u32>() {
            Ok(days) if (1..=MOST_DAYS).contains(&days) => Ok(Command::UsageDigest { days }),
            _ => Err(format!("--days is a number from 1 to {MOST_DAYS}")),
        },
        _ => Err("unknown usage arguments".to_owned()),
    }
}

/// Sends `command` and prints what it answers. Exit 1 when the daemon refuses.
pub fn run(command: Command) -> ExitCode {
    let reply = match request(command, CONTROL_DEADLINE) {
        Ok(reply) => reply,
        Err(code) => return code,
    };
    match &reply {
        Reply::UsageQuota { accounts } => {
            out!("{}", quota_text(accounts, crate::update::now()));
            ExitCode::SUCCESS
        }
        Reply::UsageCost { cost } => {
            out!("{}", cost_text(cost));
            ExitCode::SUCCESS
        }
        Reply::UsageDigest { digest } => {
            out!("{}", digest_text(digest));
            ExitCode::SUCCESS
        }
        _ => unexpected(&reply),
    }
}

/// One block per UTC day, oldest first: the day's figures on one line, one indented line per
/// project, then the provider incidents of the day. A day with nothing says so.
fn digest_text(digest: &DigestView) -> String {
    let mut text = format!("{} to {} (UTC days)\n", digest.first_day, digest.last_day);
    for day in &digest.days {
        text.push_str(&day_text(day));
    }
    text
}

fn day_text(day: &DayDigestView) -> String {
    let is_quiet =
        day.turns == 0 && day.approvals == 0 && day.replies == 0 && day.seconds_with_agent == 0;
    let mut text = if is_quiet {
        format!("{}\tnothing recorded\n", printable(&day.day))
    } else {
        format!(
            "{}\t{} with the agent\t{}\t{}\t{}\t{}{}\n",
            printable(&day.day),
            duration(day.seconds_with_agent),
            counted(day.sessions, "session", "sessions"),
            counted(day.turns, "turn", "turns"),
            counted(day.approvals, "approval", "approvals"),
            dollars(day.cost_nanos),
            unpriced_note(day.unpriced_replies),
        )
    };
    for project in &day.projects {
        text.push_str(&project_text(project));
    }
    for incident in &day.incidents {
        text.push_str(&incident_text(incident));
    }
    text
}

fn project_text(project: &ProjectDayView) -> String {
    format!(
        "  {}\t{}\t{}\t{}\t{}\t{}{}\n",
        printable(&project.project),
        duration(project.seconds_with_agent),
        counted(project.sessions, "session", "sessions"),
        counted(project.turns, "turn", "turns"),
        counted(project.approvals, "approval", "approvals"),
        dollars(project.cost_nanos),
        unpriced_note(project.unpriced_replies),
    )
}

fn incident_text(incident: &IncidentView) -> String {
    let resolution = match incident.resolved_at {
        Some(resolved_at) => format!("resolved {}", utc_time(resolved_at)),
        None => "not resolved".to_owned(),
    };
    format!(
        "  incident\t{}\t{}\tfrom {}, {resolution}\n",
        printable(&incident.provider),
        printable(&incident.title),
        utc_time(incident.started_at),
    )
}

fn counted(count: u64, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

/// Seconds as hours and minutes, rounded down: `1 h 05 min`, `12 min`, `45 s` under a minute.
fn duration(seconds: u64) -> String {
    match seconds {
        ..60 => format!("{seconds} s"),
        60..3600 => format!("{} min", seconds / 60),
        _ => format!("{} h {:02} min", seconds / 3600, seconds % 3600 / 60),
    }
}

/// A moment as the UTC date and time the digest's days are made of; the seconds count when it
/// is outside the calendar's range.
fn utc_time(timestamp: i64) -> String {
    chrono::DateTime::from_timestamp(timestamp, 0).map_or_else(
        || timestamp.to_string(),
        |at| at.format("%Y-%m-%d %H:%M UTC").to_string(),
    )
}

/// The archive's rows, one line each (`day  provider  account  project  cost  replies`), then
/// the total of every row. A reply of a model with no price is counted and has no cost, so a
/// row that holds one says how many, and a total that would be short says so.
fn cost_text(cost: &CostArchiveView) -> String {
    let mut text = format!(
        "{} to {} (UTC days, at API prices)\n",
        cost.first_day, cost.last_day
    );
    if cost.rows.is_empty() {
        text.push_str("no replies archived\n");
        return text;
    }
    let mut total_nanos = 0_u64;
    let mut unpriced_replies = 0_u64;
    for row in &cost.rows {
        total_nanos = total_nanos.saturating_add(row.cost_nanos);
        unpriced_replies = unpriced_replies.saturating_add(row.unpriced_replies);
        text.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\t{} {}{}\n",
            printable(&row.day),
            printable(&row.provider),
            printable(&row.account),
            printable(&row.project),
            dollars(row.cost_nanos),
            row.replies,
            if row.replies == 1 { "reply" } else { "replies" },
            unpriced_note(row.unpriced_replies),
        ));
    }
    text.push_str(&format!(
        "total\t{}{}\n",
        dollars(total_nanos),
        unpriced_note(unpriced_replies)
    ));
    text
}

fn unpriced_note(unpriced_replies: u64) -> String {
    match unpriced_replies {
        0 => String::new(),
        count => format!(", {count} with no known price, not in the cost"),
    }
}

/// Nano-dollars as dollars to four decimals, the last one rounded half up.
fn dollars(nanos: u64) -> String {
    let ten_thousandths =
        nanos.saturating_add(NANOS_PER_TEN_THOUSANDTH / 2) / NANOS_PER_TEN_THOUSANDTH;
    format!(
        "${}.{:04}",
        ten_thousandths / 10_000,
        ten_thousandths % 10_000
    )
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
        "  {}\t{}%\t{}\t{reset}{}\n",
        printable(&window.limit),
        window.used_percent,
        window.state.as_str(),
        pace_text(window.pace.as_ref(), now)
    )
}

/// Where queued work stands against the window (#412): the share its even pace allows by
/// now, and how long the queue waits when the window is ahead of it.
fn pace_text(pace: Option<&QuotaPaceView>, now: i64) -> String {
    let Some(pace) = pace else {
        return String::new();
    };
    let line = pace.line_percent.map(|line| format!("pace {line}%"));
    if !pace.is_holding {
        return line.map_or_else(String::new, |line| format!("\tqueue {line}"));
    }
    let wait = match pace.resumes_at {
        Some(resumes_at) => format!("queue waits {}", span((resumes_at - now).max(0))),
        None => "queue waits".to_owned(),
    };
    match line {
        Some(line) => format!("\t{wait} ({line})"),
        None => format!("\t{wait}"),
    }
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

//! The provider accounts' plan windows and the queue's pace through them (#412), one plain
//! row per account until Beta 3 designs the view.

use std::time::{SystemTime, UNIX_EPOCH};

use cockpit_protocol::{QuotaAccountView, QuotaPaceView, QuotaWindowStateView, QuotaWindowView};

/// One row per account the daemon has a reading of, its waits counted from `now` (Unix
/// seconds). An account never read takes no row: `torlek usage quota` shows its failure.
pub(crate) fn quota_rows(accounts: &[QuotaAccountView], now: i64) -> Vec<String> {
    accounts
        .iter()
        .filter(|account| !account.windows.is_empty())
        .map(|account| {
            let windows: Vec<String> = account
                .windows
                .iter()
                .map(|window| window_text(window, now))
                .collect();
            format!(
                "{} {}: {}",
                account.provider,
                account.account,
                windows.join(" · ")
            )
        })
        .collect()
}

/// Seconds since 1970 by the wall clock, as the daemon stamps the windows.
pub(crate) fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            i64::try_from(since.as_secs()).unwrap_or(i64::MAX)
        })
}

fn window_text(window: &QuotaWindowView, now: i64) -> String {
    let mut text = format!("{} {}%", window.limit, window.used_percent);
    if window.state != QuotaWindowStateView::Available {
        text.push(' ');
        text.push_str(window.state.as_str());
    }
    if let Some(pace) = &window.pace {
        text.push_str(&pace_text(pace, now));
    }
    text
}

/// The same words as `torlek usage quota`: the line normal-priority work follows now, or how
/// long queued work waits on the window.
fn pace_text(pace: &QuotaPaceView, now: i64) -> String {
    let line = pace.line_percent.map(|line| format!("pace {line}%"));
    if !pace.is_holding {
        return line.map_or_else(String::new, |line| format!(" queue {line}"));
    }
    let wait = match pace.resumes_at {
        Some(resumes_at) => format!(" queue waits {}", span((resumes_at - now).max(0))),
        None => " queue waits".to_owned(),
    };
    match line {
        Some(line) => format!("{wait} ({line})"),
        None => wait,
    }
}

/// A wait in its two largest units, rounded down, as `torlek usage quota` writes it.
fn span(seconds: i64) -> String {
    match seconds {
        ..120 => format!("{seconds} s"),
        120..7200 => format!("{} min", seconds / 60),
        7200..172_800 => format!("{} h {} min", seconds / 3600, seconds % 3600 / 60),
        _ => format!("{} days {} h", seconds / 86_400, seconds % 86_400 / 3600),
    }
}

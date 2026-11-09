//! Usage-aware pacing (#412): queue starts on a provider account follow an even line through
//! each of its plan windows, so a window that full speed would spend early lasts until its
//! reset. Running work is never paused; only what the queue starts waits.

use cockpit_tasks::Priority;

/// How queue starts follow the plan windows of the account a session runs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pacing {
    /// The queue starts work whatever the windows show.
    Off,
    /// The queue may use `100 - reserve_percent` of each window, spread evenly until its
    /// reset: work starts on an account while each of its windows is used below its line,
    /// `elapsed share × (100 - reserve_percent) + burst_percent`, capped at
    /// `100 - reserve_percent`. The burst lets work start when a window opens. Low-priority
    /// work gets no burst, so it waits first; high-priority work ignores the line and stops
    /// at the reserve only. The reserve is the owner's own sessions'.
    Even {
        reserve_percent: u8,
        burst_percent: u8,
    },
}

impl Pacing {
    /// Owner decision 2026-11-09 (#412, option A): on, a 10 % reserve and a 15 % burst.
    pub const DEFAULT: Pacing = Pacing::Even {
        reserve_percent: 10,
        burst_percent: 15,
    };
}

/// Whether a window's last reading can pace, as `cockpit-usage` judged it (#58).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Standing {
    Current,
    /// The provider refuses ordinary use on the account, whatever the window says. It holds
    /// every priority.
    Blocked,
    /// No current reading: it paces nothing, since an old share says nothing of now.
    Stale,
}

/// One plan window of a provider account at its last reading.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageWindow {
    pub provider: String,
    pub account: String,
    pub limit: String,
    /// What a scoped window counts (one model, one feature); `None` for the whole account.
    /// Only whole-account windows pace by their share: scheduling cannot tell which model a
    /// turn will use. Any blocked window holds the account.
    pub scope: Option<String>,
    pub used_percent: f64,
    pub length_seconds: Option<u32>,
    /// Seconds since 1970.
    pub resets_at: Option<i64>,
    pub standing: Standing,
}

/// Why work of some priority does not start on an account now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Paced {
    pub(crate) why: String,
    pub(crate) limited_by: String,
    /// When the window lets it start again; `None` when no reading can say.
    pub(crate) resumes_at: Option<i64>,
}

/// Why a turn of `priority` waits on `provider`'s `account` at `now`; `None` when it may
/// start. `priority` is the item's own: aging orders the queue but never earns a burst. Of
/// several windows holding it, the one that holds it longest explains it.
pub(crate) fn paced(
    pacing: Pacing,
    windows: &[UsageWindow],
    provider: &str,
    account: &str,
    priority: i64,
    now: i64,
) -> Option<Paced> {
    let Pacing::Even {
        reserve_percent,
        burst_percent,
    } = pacing
    else {
        return None;
    };
    let usable = 100.0 - f64::from(reserve_percent);
    let burst = if priority <= Priority::LOW.level() {
        0.0
    } else {
        f64::from(burst_percent)
    };
    windows
        .iter()
        .filter(|window| window.provider == provider && window.account == account)
        .filter_map(|window| {
            let name = || format!("window {} of {provider} account {account}", window.limit);
            match window.standing {
                Standing::Stale => None,
                Standing::Blocked => Some(Paced {
                    why: format!(
                        "the provider refuses ordinary use of {provider} account {account}"
                    ),
                    limited_by: name(),
                    resumes_at: None,
                }),
                // A block refuses every model, so a scoped window says it as well as any.
                Standing::Current if window.scope.is_some() => None,
                Standing::Current => held(window, usable, burst, priority, now, name),
            }
        })
        .max_by_key(|paced| paced.resumes_at.unwrap_or(i64::MAX))
}

/// Where normal-priority queued work stands against one window under the pacing (#412), as
/// `torlek usage quota` shows it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WindowPace {
    /// The share the queue may have used by now; `None` for a window without a length and a
    /// reset, or one the provider blocks, which no line runs through.
    pub line_percent: Option<f64>,
    /// Whether queued work of normal priority waits on this window now.
    pub is_holding: bool,
    /// When it may start again; `None` when it does not wait, or when no reading can say.
    pub resumes_at: Option<i64>,
}

/// `window`'s pace at `now`; `None` when it paces nothing: pacing is off, its reading is
/// stale, or it counts one model only and the provider does not block it.
pub fn window_pace(pacing: Pacing, window: &UsageWindow, now: i64) -> Option<WindowPace> {
    let Pacing::Even {
        reserve_percent,
        burst_percent,
    } = pacing
    else {
        return None;
    };
    let line_percent = match window.standing {
        Standing::Stale => return None,
        Standing::Current if window.scope.is_some() => return None,
        Standing::Blocked => None,
        Standing::Current => {
            let usable = 100.0 - f64::from(reserve_percent);
            even_line(window, usable, f64::from(burst_percent), now)
        }
    };
    let held = paced(
        pacing,
        std::slice::from_ref(window),
        &window.provider,
        &window.account,
        Priority::NORMAL.level(),
        now,
    );
    Some(WindowPace {
        line_percent,
        is_holding: held.is_some(),
        resumes_at: held.and_then(|held| held.resumes_at),
    })
}

/// The share `window` may be used at `now`: `elapsed share × usable + burst`, capped at
/// `usable`; `None` without a length and a reset.
fn even_line(window: &UsageWindow, usable: f64, burst: f64, now: i64) -> Option<f64> {
    let (length, opened_at) = opening(window)?;
    let elapsed = ((now as f64 - opened_at) / length).clamp(0.0, 1.0);
    Some((elapsed * usable + burst).min(usable))
}

/// The window's length and when it opened, in seconds.
fn opening(window: &UsageWindow) -> Option<(f64, f64)> {
    let (Some(length), Some(resets_at)) = (window.length_seconds, window.resets_at) else {
        return None;
    };
    let length = f64::from(length);
    Some((length, resets_at as f64 - length))
}

fn held(
    window: &UsageWindow,
    usable: f64,
    burst: f64,
    priority: i64,
    now: i64,
    name: impl Fn() -> String,
) -> Option<Paced> {
    let used = window.used_percent;
    if used >= usable {
        let name = name();
        return Some(Paced {
            why: format!(
                "{name} is {used:.0} % used, at the {usable:.0} % the queue may use; the rest \
                 is kept for your own sessions"
            ),
            limited_by: name,
            resumes_at: window.resets_at,
        });
    }
    if priority >= Priority::HIGH.level() {
        return None;
    }
    let line = even_line(window, usable, burst, now)?;
    let (length, opened_at) = opening(window)?;
    if used < line {
        return None;
    }
    let name = name();
    // The time the line rises to `used`.
    let resumes_at = (opened_at + length * (used - burst) / usable).ceil() as i64;
    Some(Paced {
        why: format!(
            "{name} is {used:.0} % used, ahead of its even pace to the reset ({line:.0} % now)"
        ),
        limited_by: name,
        resumes_at: Some(resumes_at.max(now)),
    })
}

/// `at` as a wait from `now`, in whole minutes rounded up.
pub(crate) fn wait_until(at: i64, now: i64) -> String {
    let minutes = ((at - now).max(0) + 59) / 60;
    let minutes = minutes.max(1);
    if minutes < 60 {
        return format!("in about {minutes} min");
    }
    format!("in about {} h {} min", minutes / 60, minutes % 60)
}

//! Issue #412 in the TUI: one plain row per provider account above the status line, each
//! window with its share used and the queue's pace through it, from `usage-quota`.

mod common;

use std::time::{Instant, SystemTime, UNIX_EPOCH};

use cockpit_protocol::{
    Command, QuotaAccountView, QuotaFailureView, QuotaPaceView, QuotaWindowStateView,
    QuotaWindowView, Reply,
};
use cockpit_tui::App;
use common::{answer, app_with, screen};

const WIDTH: u16 = 120;

fn unix_now() -> i64 {
    let since_epoch = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    i64::try_from(since_epoch.as_secs()).unwrap()
}

fn account(provider: &str, windows: Vec<QuotaWindowView>) -> QuotaAccountView {
    QuotaAccountView {
        provider: provider.to_owned(),
        account: "default".to_owned(),
        plan: Some("max".to_owned()),
        is_blocked: false,
        observed_at: Some(unix_now() - 60),
        is_current: true,
        failure: None,
        windows,
    }
}

fn window(limit: &str, used_percent: u32, pace: Option<QuotaPaceView>) -> QuotaWindowView {
    QuotaWindowView {
        limit: limit.to_owned(),
        scope: None,
        length_seconds: Some(18_000),
        used_percent,
        resets_at: Some(unix_now() + 3 * 3600),
        state: QuotaWindowStateView::Available,
        pace,
    }
}

fn pace(line_percent: Option<u32>, resumes_at: Option<i64>) -> Option<QuotaPaceView> {
    Some(QuotaPaceView {
        line_percent,
        is_holding: resumes_at.is_some() || line_percent.is_none(),
        resumes_at,
    })
}

/// The screen's last rows, the status line last.
fn bottom(app: &App, now: Instant, rows: usize) -> Vec<String> {
    let screen = screen(app, now, WIDTH);
    screen[screen.len() - rows..].to_vec()
}

fn quota(app: &mut App, accounts: Vec<QuotaAccountView>, now: Instant) {
    answer(
        app,
        Command::UsageQuota,
        Reply::UsageQuota { accounts },
        now,
    );
}

#[test]
fn the_refresh_asks_for_the_accounts_plan_windows() {
    assert!(App::refresh_commands().contains(&Command::UsageQuota));
}

#[test]
fn each_account_read_shows_its_windows_and_the_queue_pace_above_the_status_line() {
    let now = Instant::now();
    let mut app = app_with(&["app"], Vec::new(), now);
    let resumes_at = unix_now() + 12 * 60 + 30;
    quota(
        &mut app,
        vec![
            account(
                "claude",
                vec![
                    window("five-hour", 42, pace(Some(51), None)),
                    window("seven-day", 70, pace(Some(60), Some(resumes_at))),
                ],
            ),
            account("codex", vec![window("five-hour", 3, None)]),
        ],
        now,
    );

    assert_eq!(
        bottom(&app, now, 3),
        [
            "claude default: five-hour 42% queue pace 51% · seven-day 70% queue waits 12 min (pace 60%)",
            "codex default: five-hour 3%",
            "? help",
        ]
    );
}

#[test]
fn a_window_not_available_names_its_state_and_a_wait_no_reading_can_time_has_no_span() {
    let now = Instant::now();
    let mut app = app_with(&["app"], Vec::new(), now);
    let blocked = QuotaAccountView {
        is_blocked: true,
        ..account(
            "claude",
            vec![QuotaWindowView {
                state: QuotaWindowStateView::Blocked,
                ..window("five-hour", 10, pace(None, None))
            }],
        )
    };
    let stale = QuotaAccountView {
        is_current: false,
        ..account(
            "codex",
            vec![QuotaWindowView {
                state: QuotaWindowStateView::Stale,
                ..window("five-hour", 100, None)
            }],
        )
    };
    quota(&mut app, vec![blocked, stale], now);

    assert_eq!(
        bottom(&app, now, 3),
        [
            "claude default: five-hour 10% blocked queue waits",
            "codex default: five-hour 100% stale",
            "? help",
        ]
    );
}

#[test]
fn an_account_never_read_takes_no_row() {
    let now = Instant::now();
    let mut app = app_with(&["app"], Vec::new(), now);
    let never_read = QuotaAccountView {
        observed_at: None,
        is_current: false,
        failure: Some(QuotaFailureView {
            at: unix_now() - 60,
            reason: "Codex's quota sign-in is missing".to_owned(),
        }),
        ..account("codex", Vec::new())
    };
    quota(&mut app, vec![never_read], now);

    let rows = bottom(&app, now, 2);

    assert!(rows[0].starts_with('└'), "{rows:?}");
    assert_eq!(rows[1], "? help");
}

#[test]
fn a_row_wider_than_the_terminal_ends_where_it_was_cut() {
    let now = Instant::now();
    let mut app = app_with(&["app"], Vec::new(), now);
    let windows = [
        "five-hour",
        "seven-day",
        "seven-day-opus",
        "seven-day-sonnet",
    ]
    .into_iter()
    .map(|limit| window(limit, 42, pace(Some(51), None)))
    .collect();
    quota(&mut app, vec![account("claude", windows)], now);

    let row = &bottom(&app, now, 2)[0];

    assert!(row.chars().count() <= usize::from(WIDTH), "{row}");
    assert!(row.ends_with('…'), "{row}");
}

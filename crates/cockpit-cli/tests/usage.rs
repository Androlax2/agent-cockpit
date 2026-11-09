//! `torlek usage quota` driven as a process against a scripted daemon socket. The windows'
//! judgement is proven in `cockpit-usage` and the request in `cockpit-daemon/tests/usage_quota.rs`;
//! these prove the verb's request, its output and its exit status.

use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;
use std::process::Output;
use std::time::{SystemTime, UNIX_EPOCH};

mod scripted;

use cockpit_protocol::{
    Command, CostArchiveView, CostRowView, DayDigestView, DigestView, ErrorCode, IncidentView,
    Outcome, ProjectDayView, ProtocolError, QuotaAccountView, QuotaFailureView, QuotaPaceView,
    QuotaWindowStateView, QuotaWindowView, Reply,
};
use scripted::{scripted_daemon, text};

struct Home {
    dir: tempfile::TempDir,
}

impl Home {
    fn new() -> Home {
        Home {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn runtime_dir(&self) -> PathBuf {
        self.dir.path().join("run")
    }

    fn daemon_socket(&self) -> PathBuf {
        let path = self.runtime_dir().join("torlek-dev/daemon.sock");
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path.parent().unwrap())
            .unwrap();
        path
    }

    async fn cockpit(&self, arguments: &[&str]) -> Output {
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_torlek"));
        command
            .args(arguments)
            .env_clear()
            .env("HOME", self.dir.path())
            .env("XDG_RUNTIME_DIR", self.runtime_dir());
        tokio::task::spawn_blocking(move || command.output().unwrap())
            .await
            .unwrap()
    }
}

fn unix_now() -> i64 {
    let since_epoch = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    i64::try_from(since_epoch.as_secs()).unwrap()
}

fn window(limit: &str, used_percent: u32, resets_at: Option<i64>) -> QuotaWindowView {
    QuotaWindowView {
        limit: limit.to_owned(),
        scope: None,
        length_seconds: None,
        used_percent,
        resets_at,
        state: QuotaWindowStateView::Available,
        pace: None,
    }
}

#[tokio::test]
async fn usage_quota_prints_each_account_then_its_windows_and_last_failure() {
    let home = Home::new();
    let now = unix_now();
    // Minutes and hours past a boundary by more than the time the process takes to start.
    let current = QuotaAccountView {
        provider: "claude".to_owned(),
        account: "default".to_owned(),
        plan: Some("max".to_owned()),
        is_blocked: false,
        observed_at: Some(now - 3 * 60 - 30),
        is_current: true,
        failure: None,
        windows: vec![
            window("five-hour", 42, Some(now + 2 * 3600 + 30 * 60 + 30)),
            QuotaWindowView {
                state: QuotaWindowStateView::Low,
                ..window("seven-day", 81, Some(now + 4 * 86_400 + 23 * 3600 + 1800))
            },
        ],
    };
    let failing = QuotaAccountView {
        provider: "codex".to_owned(),
        account: "default".to_owned(),
        plan: None,
        is_blocked: false,
        observed_at: None,
        is_current: false,
        failure: Some(QuotaFailureView {
            at: now - 5 * 60 - 30,
            reason: "Codex's quota sign-in is missing".to_owned(),
        }),
        windows: Vec::new(),
    };
    let daemon = scripted_daemon(
        &home.daemon_socket(),
        Outcome::Ok(Reply::UsageQuota {
            accounts: vec![current, failing],
        }),
    );

    let output = home.cockpit(&["usage", "quota"]).await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        text(&output.stdout),
        "claude\tdefault\tplan max\tread 3 min ago\tcurrent\n\
         \x20 five-hour\t42%\tavailable\tresets in 2 h 30 min\n\
         \x20 seven-day\t81%\tlow\tresets in 4 days 23 h\n\
         codex\tdefault\tplan unknown\tnever read\tstale\n\
         \x20 refresh failed 5 min ago: Codex's quota sign-in is missing\n"
    );
    assert_eq!(daemon.await.unwrap(), Command::UsageQuota);
}

#[tokio::test]
async fn a_window_whose_reset_passed_or_is_unknown_says_so() {
    let home = Home::new();
    let now = unix_now();
    let daemon = scripted_daemon(
        &home.daemon_socket(),
        Outcome::Ok(Reply::UsageQuota {
            accounts: vec![QuotaAccountView {
                provider: "claude".to_owned(),
                account: "default".to_owned(),
                plan: Some("pro".to_owned()),
                is_blocked: false,
                observed_at: Some(now - 3 * 3600 - 600),
                is_current: false,
                failure: None,
                windows: vec![
                    QuotaWindowView {
                        state: QuotaWindowStateView::Stale,
                        ..window("five-hour", 100, Some(now - 20 * 60 - 30))
                    },
                    window("window-60-minutes", 3, None),
                ],
            }],
        }),
    );

    let output = home.cockpit(&["usage", "quota"]).await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        text(&output.stdout),
        "claude\tdefault\tplan pro\tread 3 h ago\tstale\n\
         \x20 five-hour\t100%\tstale\treset 20 min ago\n\
         \x20 window-60-minutes\t3%\tavailable\treset unknown\n"
    );
    daemon.await.unwrap();
}

#[tokio::test]
async fn each_window_shows_the_queue_pace_and_how_long_queued_work_waits() {
    let home = Home::new();
    let now = unix_now();
    let pace = |line_percent, is_holding, resumes_at| {
        Some(QuotaPaceView {
            line_percent,
            is_holding,
            resumes_at,
        })
    };
    let daemon = scripted_daemon(
        &home.daemon_socket(),
        Outcome::Ok(Reply::UsageQuota {
            accounts: vec![QuotaAccountView {
                provider: "claude".to_owned(),
                account: "default".to_owned(),
                plan: Some("max".to_owned()),
                is_blocked: false,
                observed_at: Some(now - 3 * 60 - 30),
                is_current: true,
                failure: None,
                windows: vec![
                    QuotaWindowView {
                        pace: pace(Some(51), false, None),
                        ..window("five-hour", 42, Some(now + 3 * 3600 + 30))
                    },
                    QuotaWindowView {
                        pace: pace(Some(60), true, Some(now + 12 * 60 + 30)),
                        ..window("seven-day", 70, Some(now + 2 * 86_400 + 1800))
                    },
                    QuotaWindowView {
                        pace: pace(None, true, None),
                        ..window("window-60-minutes", 3, None)
                    },
                ],
            }],
        }),
    );

    let output = home.cockpit(&["usage", "quota"]).await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        text(&output.stdout),
        "claude\tdefault\tplan max\tread 3 min ago\tcurrent\n\
         \x20 five-hour\t42%\tavailable\tresets in 3 h 0 min\tqueue pace 51%\n\
         \x20 seven-day\t70%\tavailable\tresets in 2 days 0 h\tqueue waits 12 min (pace 60%)\n\
         \x20 window-60-minutes\t3%\tavailable\treset unknown\tqueue waits\n"
    );
    daemon.await.unwrap();
}

#[tokio::test]
async fn a_blocked_account_says_so_and_a_failure_before_the_reading_is_labelled() {
    let home = Home::new();
    let now = unix_now();
    let daemon = scripted_daemon(
        &home.daemon_socket(),
        Outcome::Ok(Reply::UsageQuota {
            accounts: vec![QuotaAccountView {
                provider: "codex".to_owned(),
                account: "default".to_owned(),
                plan: Some("plus".to_owned()),
                is_blocked: true,
                observed_at: Some(now - 60),
                is_current: true,
                failure: Some(QuotaFailureView {
                    at: now - 3 * 86_400,
                    reason: "did not answer within 12000 ms".to_owned(),
                }),
                windows: Vec::new(),
            }],
        }),
    );

    let output = home.cockpit(&["usage", "quota"]).await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        text(&output.stdout),
        "codex\tdefault\tplan plus\tread 60 s ago\tcurrent\tblocked\n\
         \x20 refresh failed 3 days ago, before the last reading: did not answer within 12000 ms\n"
    );
    daemon.await.unwrap();
}

#[tokio::test]
async fn the_providers_words_are_printed_escaped() {
    let home = Home::new();
    let now = unix_now();
    let daemon = scripted_daemon(
        &home.daemon_socket(),
        Outcome::Ok(Reply::UsageQuota {
            accounts: vec![QuotaAccountView {
                provider: "codex".to_owned(),
                account: "default".to_owned(),
                plan: Some("plus\u{1b}[2J".to_owned()),
                is_blocked: false,
                observed_at: None,
                is_current: false,
                failure: Some(QuotaFailureView {
                    at: now - 60,
                    reason: "refused\nclaude\tdefault\tplan max".to_owned(),
                }),
                windows: Vec::new(),
            }],
        }),
    );

    let output = home.cockpit(&["usage", "quota"]).await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        text(&output.stdout),
        "codex\tdefault\tplan plus\\u{1b}[2J\tnever read\tstale\n\
         \x20 refresh failed 60 s ago: refused\\nclaude\\tdefault\\tplan max\n"
    );
    daemon.await.unwrap();
}

#[tokio::test]
async fn no_account_read_yet_is_said_plainly() {
    let home = Home::new();
    let daemon = scripted_daemon(
        &home.daemon_socket(),
        Outcome::Ok(Reply::UsageQuota {
            accounts: Vec::new(),
        }),
    );

    let output = home.cockpit(&["usage", "quota"]).await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(text(&output.stdout), "no quota read yet\n");
    daemon.await.unwrap();
}

#[tokio::test]
async fn a_refusal_exits_1_with_the_daemons_message() {
    let home = Home::new();
    let daemon = scripted_daemon(
        &home.daemon_socket(),
        Outcome::Error(ProtocolError {
            code: ErrorCode::Refused,
            message: "this daemon reads no quota".to_owned(),
            unblock: None,
        }),
    );

    let output = home.cockpit(&["usage", "quota"]).await;

    assert_eq!(output.status.code(), Some(1));
    assert!(
        text(&output.stderr).contains("this daemon reads no quota"),
        "{}",
        text(&output.stderr)
    );
    daemon.await.unwrap();
}

fn cost_row(day: &str, cost_nanos: u64, replies: u64, unpriced_replies: u64) -> CostRowView {
    CostRowView {
        day: day.to_owned(),
        provider: "claude".to_owned(),
        account: "default".to_owned(),
        project: "app".to_owned(),
        replies,
        unpriced_replies,
        input_tokens: 0,
        output_tokens: 0,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
        cost_nanos,
    }
}

#[tokio::test]
async fn usage_cost_prints_a_line_per_row_and_the_exact_total() {
    let home = Home::new();
    let daemon = scripted_daemon(
        &home.daemon_socket(),
        Outcome::Ok(Reply::UsageCost {
            cost: CostArchiveView {
                first_day: "2026-11-02".to_owned(),
                last_day: "2026-11-08".to_owned(),
                rows: vec![
                    cost_row("2026-11-02", 122_000_000, 1, 0),
                    cost_row("2026-11-04", 24_850_000, 3, 1),
                ],
            },
        }),
    );

    let output = home.cockpit(&["usage", "cost"]).await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        text(&output.stdout),
        "2026-11-02 to 2026-11-08 (UTC days, at API prices)\n\
         2026-11-02\tclaude\tdefault\tapp\t$0.1220\t1 reply\n\
         2026-11-04\tclaude\tdefault\tapp\t$0.0249\t3 replies, 1 with no known price, not in the cost\n\
         total\t$0.1469, 1 with no known price, not in the cost\n"
    );
    assert_eq!(daemon.await.unwrap(), Command::UsageCost { days: 7 });
}

#[tokio::test]
async fn usage_cost_asks_for_the_days_it_is_given() {
    let home = Home::new();
    let daemon = scripted_daemon(
        &home.daemon_socket(),
        Outcome::Ok(Reply::UsageCost {
            cost: CostArchiveView {
                first_day: "2026-10-10".to_owned(),
                last_day: "2026-11-08".to_owned(),
                rows: Vec::new(),
            },
        }),
    );

    let output = home.cockpit(&["usage", "cost", "--days", "30"]).await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        text(&output.stdout),
        "2026-10-10 to 2026-11-08 (UTC days, at API prices)\nno replies archived\n"
    );
    assert_eq!(daemon.await.unwrap(), Command::UsageCost { days: 30 });
}

#[tokio::test]
async fn usage_cost_rounds_to_four_decimals_half_up() {
    let home = Home::new();
    let daemon = scripted_daemon(
        &home.daemon_socket(),
        Outcome::Ok(Reply::UsageCost {
            cost: CostArchiveView {
                first_day: "2026-11-08".to_owned(),
                last_day: "2026-11-08".to_owned(),
                rows: vec![
                    cost_row("2026-11-08", 12_049, 1, 0),
                    cost_row("2026-11-08", 50_000, 1, 0),
                ],
            },
        }),
    );

    let output = home.cockpit(&["usage", "cost", "--days", "1"]).await;

    let lines: Vec<String> = text(&output.stdout).lines().map(str::to_owned).collect();
    assert!(lines[1].contains("\t$0.0000\t"), "{lines:?}");
    assert!(lines[2].contains("\t$0.0001\t"), "{lines:?}");
    assert!(lines[3].starts_with("total\t$0.0001"), "{lines:?}");
    drop(daemon);
}

#[tokio::test]
async fn usage_cost_refuses_a_day_count_outside_one_to_366_without_asking_the_daemon() {
    let home = Home::new();

    for days in ["0", "367", "many", "-1"] {
        let output = home.cockpit(&["usage", "cost", "--days", days]).await;

        assert!(!output.status.success(), "{days}");
        assert!(
            text(&output.stderr).contains("--days is a number from 1 to 366"),
            "{days}: {}",
            text(&output.stderr)
        );
    }
}

fn quiet_day(day: &str) -> DayDigestView {
    DayDigestView {
        day: day.to_owned(),
        sessions: 0,
        turns: 0,
        approvals: 0,
        seconds_with_agent: 0,
        replies: 0,
        unpriced_replies: 0,
        cost_nanos: 0,
        projects: Vec::new(),
        incidents: Vec::new(),
    }
}

fn project_day(project: &str, seconds_with_agent: u64) -> ProjectDayView {
    ProjectDayView {
        project: project.to_owned(),
        sessions: 1,
        turns: 2,
        approvals: 1,
        seconds_with_agent,
        replies: 4,
        unpriced_replies: 0,
        cost_nanos: 122_000_000,
    }
}

#[tokio::test]
async fn usage_digest_prints_each_day_its_projects_and_incidents() {
    let home = Home::new();
    let busy_day = DayDigestView {
        sessions: 2,
        turns: 3,
        approvals: 1,
        seconds_with_agent: 3_900,
        replies: 5,
        unpriced_replies: 1,
        cost_nanos: 244_000_000,
        projects: vec![project_day("app", 3_600), project_day("tools", 720)],
        incidents: vec![
            IncidentView {
                provider: "claude".to_owned(),
                title: "Elevated errors".to_owned(),
                started_at: 1_794_129_120,
                resolved_at: Some(1_794_132_000),
            },
            IncidentView {
                provider: "codex".to_owned(),
                title: "Slow replies".to_owned(),
                started_at: 1_794_129_120,
                resolved_at: None,
            },
        ],
        ..quiet_day("2026-11-08")
    };
    let daemon = scripted_daemon(
        &home.daemon_socket(),
        Outcome::Ok(Reply::UsageDigest {
            digest: DigestView {
                first_day: "2026-11-07".to_owned(),
                last_day: "2026-11-08".to_owned(),
                days: vec![quiet_day("2026-11-07"), busy_day],
            },
        }),
    );

    let output = home.cockpit(&["usage", "digest"]).await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        text(&output.stdout),
        "2026-11-07 to 2026-11-08 (UTC days)\n\
         2026-11-07\tnothing recorded\n\
         2026-11-08\t1 h 05 min with the agent\t2 sessions\t3 turns\t1 approval\t$0.2440, 1 with no known price, not in the cost\n\
         \x20 app\t1 h 00 min\t1 session\t2 turns\t1 approval\t$0.1220\n\
         \x20 tools\t12 min\t1 session\t2 turns\t1 approval\t$0.1220\n\
         \x20 incident\tclaude\tElevated errors\tfrom 2026-11-08 09:12 UTC, resolved 2026-11-08 10:00 UTC\n\
         \x20 incident\tcodex\tSlow replies\tfrom 2026-11-08 09:12 UTC, not resolved\n"
    );
    assert_eq!(daemon.await.unwrap(), Command::UsageDigest { days: 7 });
}

#[tokio::test]
async fn usage_digest_asks_for_the_days_it_is_given() {
    let home = Home::new();
    let daemon = scripted_daemon(
        &home.daemon_socket(),
        Outcome::Ok(Reply::UsageDigest {
            digest: DigestView {
                first_day: "2026-11-08".to_owned(),
                last_day: "2026-11-08".to_owned(),
                days: vec![quiet_day("2026-11-08")],
            },
        }),
    );

    let output = home.cockpit(&["usage", "digest", "--days", "1"]).await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(daemon.await.unwrap(), Command::UsageDigest { days: 1 });
}

#[tokio::test]
async fn usage_digest_shows_seconds_under_a_minute_and_escapes_the_providers_words() {
    let home = Home::new();
    let day = DayDigestView {
        turns: 1,
        sessions: 1,
        seconds_with_agent: 45,
        incidents: vec![IncidentView {
            provider: "claude".to_owned(),
            title: "down\u{1b}[2J\nfake line".to_owned(),
            started_at: 1_794_129_120,
            resolved_at: None,
        }],
        ..quiet_day("2026-11-08")
    };
    let daemon = scripted_daemon(
        &home.daemon_socket(),
        Outcome::Ok(Reply::UsageDigest {
            digest: DigestView {
                first_day: "2026-11-08".to_owned(),
                last_day: "2026-11-08".to_owned(),
                days: vec![day],
            },
        }),
    );

    let output = home.cockpit(&["usage", "digest", "--days", "1"]).await;

    let printed = text(&output.stdout);
    assert!(printed.contains("\t45 s with the agent\t"), "{printed}");
    assert!(printed.contains("down\\u{1b}[2J\\nfake line"), "{printed}");
    assert_eq!(printed.lines().count(), 3, "{printed}");
    drop(daemon);
}

#[tokio::test]
async fn usage_digest_refuses_a_day_count_outside_one_to_366_without_asking_the_daemon() {
    let home = Home::new();

    for days in ["0", "367", "many", "-1"] {
        let output = home.cockpit(&["usage", "digest", "--days", days]).await;

        assert!(!output.status.success(), "{days}");
        assert!(
            text(&output.stderr).contains("--days is a number from 1 to 366"),
            "{days}: {}",
            text(&output.stderr)
        );
    }
}

#[tokio::test]
async fn usage_digest_does_not_call_a_day_quiet_when_a_turn_from_the_day_before_ran_into_it() {
    let home = Home::new();
    let day = DayDigestView {
        seconds_with_agent: 600,
        ..quiet_day("2026-11-08")
    };
    let daemon = scripted_daemon(
        &home.daemon_socket(),
        Outcome::Ok(Reply::UsageDigest {
            digest: DigestView {
                first_day: "2026-11-08".to_owned(),
                last_day: "2026-11-08".to_owned(),
                days: vec![day],
            },
        }),
    );

    let output = home.cockpit(&["usage", "digest", "--days", "1"]).await;

    let printed = text(&output.stdout);
    assert!(!printed.contains("nothing recorded"), "{printed}");
    assert!(printed.contains("10 min with the agent"), "{printed}");
    drop(daemon);
}

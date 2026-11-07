//! `torlek usage quota` driven as a process against a scripted daemon socket. The windows'
//! judgement is proven in `cockpit-usage` and the request in `cockpit-daemon/tests/usage_quota.rs`;
//! these prove the verb's request, its output and its exit status.

use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;
use std::process::Output;
use std::time::{SystemTime, UNIX_EPOCH};

mod scripted;

use cockpit_protocol::{
    Command, ErrorCode, Outcome, ProtocolError, QuotaAccountView, QuotaFailureView,
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

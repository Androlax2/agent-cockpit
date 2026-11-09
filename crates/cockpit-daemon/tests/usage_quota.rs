//! Issue #58 through the daemon: each provider reporting `quota` is read at the owner's
//! interval in the owner's home, its windows recorded, and a provider that fails is asked
//! again later and later. The owner reads what was recorded with `usage-quota`, and each alert
//! a window fires reaches the change feed that `torlek-notify` follows.
#![cfg(target_os = "linux")]

mod candidate;
mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use cockpit_client::{ClientError, Connection, Subscription};
use cockpit_daemon::{Daemon, Instance, Limits, Options, QuotaPolling, Routes};
use cockpit_protocol::{
    Change, Command, EntityKind, ErrorCode, QuotaAccountView, QuotaAlertView, QuotaPaceView,
    QuotaWindowStateView, QuotaWindowView, Reply,
};
use cockpit_provider_api::fake::FakeProvider;
use cockpit_provider_api::{LimitKey, ProviderError, ProviderName, QuotaReading, QuotaWindow};
use cockpit_usage::{QuotaPolicy, WindowState};
use common::GENEROUS;

const REFRESH: Duration = Duration::from_millis(200);

fn options_in(dir: &Path) -> Options {
    Options {
        runtime_dir: dir.join("run/cockpit"),
        state_dir: dir.join("state/cockpit"),
        limits: Limits::default(),
    }
}

fn five_hour(used_percent: f64) -> QuotaReading {
    let resets_at = common::unix_now() + 3 * 3600;
    let window = QuotaWindow::new(
        LimitKey::new("five-hour").unwrap(),
        None,
        Some(18_000),
        used_percent,
        Some(resets_at),
    )
    .unwrap();
    QuotaReading::new(Some("max".to_owned()), vec![window], false).unwrap()
}

fn refused(reason: &str) -> Result<QuotaReading, ProviderError> {
    Err(ProviderError {
        reason: reason.to_owned(),
    })
}

struct Polled {
    dir: tempfile::TempDir,
    log: PathBuf,
    database: Arc<cockpit_turso::Database>,
}

impl Polled {
    async fn start(answers: Vec<Result<QuotaReading, ProviderError>>) -> (Polled, Daemon) {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("quota-asks");
        let provider = FakeProvider::new(
            ProviderName::new("fake").unwrap(),
            dir.path().join("launches"),
        )
        .reading_quota(answers, log.clone());
        let mut wiring = common::wiring_in(dir.path(), vec![Arc::new(provider)]);
        wiring.quota = Some(QuotaPolling {
            home: dir.path().join("home"),
            refresh: REFRESH,
            policy: QuotaPolicy::default(),
            project_shares: std::collections::BTreeMap::new(),
        });
        let environment = wiring.environment.clone();
        let opened = Arc::new(std::sync::Mutex::new(None));
        let handed = Arc::clone(&opened);
        let daemon = Instance::acquire(options_in(dir.path()))
            .unwrap()
            .start(wiring, move |capabilities| {
                *handed.lock().unwrap() = Some(Arc::clone(&capabilities.database));
                Arc::new(Routes::new(capabilities, environment))
            })
            .await
            .unwrap();
        let database = opened
            .lock()
            .unwrap()
            .take()
            .expect("the routes got the database");
        (Polled { dir, log, database }, daemon)
    }

    async fn asks_once(&self, count: usize) -> Vec<(u128, PathBuf)> {
        common::once_logged(count, || FakeProvider::quota_asks(&self.log)).await
    }

    /// What the owner's `usage-quota` answers while the daemon runs.
    async fn asked(&self, daemon: &Daemon) -> Result<Vec<QuotaAccountView>, ClientError> {
        let client = Connection::open(daemon.socket_path(), GENEROUS)
            .await
            .unwrap();
        match client.request(Command::UsageQuota, GENEROUS).await? {
            Reply::UsageQuota { accounts } => Ok(accounts),
            reply => panic!("{reply:?}"),
        }
    }

    /// The `quota-alert` entities the feed carries, applied batch by batch, until `is_done`
    /// says so; panics when it never does.
    async fn alerts_until(
        &self,
        daemon: &Daemon,
        is_done: impl Fn(&BTreeMap<String, QuotaAlertView>) -> bool,
    ) -> BTreeMap<String, QuotaAlertView> {
        let mut feed = Subscription::new(daemon.socket_path().to_owned(), None, GENEROUS);
        let mut alerts = BTreeMap::new();
        let deadline = tokio::time::Instant::now() + GENEROUS;
        while !is_done(&alerts) {
            let pending = tokio::time::timeout_at(deadline, feed.next_batch())
                .await
                .unwrap_or_else(|_| panic!("the feed never got there: {alerts:#?}"))
                .expect("the feed continues");
            for event in &pending.batch().changes {
                if event.entity.kind != EntityKind::QuotaAlert {
                    continue;
                }
                match &event.change {
                    Change::Upsert { state } => alerts.insert(
                        event.entity.id.clone(),
                        serde_json::from_value(state.clone().into()).unwrap(),
                    ),
                    Change::Tombstone => alerts.remove(&event.entity.id),
                };
            }
            pending.applied();
        }
        alerts
    }

    /// The ids of the `quota-alert` entities the state database holds once the daemon
    /// stopped.
    async fn published_alerts(&self, daemon: Daemon) -> Vec<String> {
        drop(daemon);
        let read = self.database.read().await.unwrap();
        cockpit_messaging::published_ids(&read, "quota-alert")
            .await
            .unwrap()
    }

    /// What the state database holds once the daemon stopped.
    async fn recorded(&self, daemon: Daemon) -> Vec<cockpit_usage::AccountQuota> {
        drop(daemon);
        let read = self.database.read().await.unwrap();
        cockpit_usage::quotas(&read, &QuotaPolicy::default(), common::unix_now())
            .await
            .unwrap()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn each_quota_provider_is_read_in_the_owners_home_and_recorded() {
    let (polled, daemon) = Polled::start(vec![Ok(five_hour(42.0))]).await;

    let asks = polled.asks_once(2).await;
    let recorded = polled.recorded(daemon).await;

    assert!(
        asks.iter()
            .all(|(_, home)| *home == polled.dir.path().join("home")),
        "{asks:?}"
    );
    assert_eq!(recorded.len(), 1, "{recorded:#?}");
    let quota = &recorded[0];
    assert_eq!(
        (quota.provider.as_str(), quota.account.as_str()),
        ("fake", "default")
    );
    assert!(quota.is_current, "{quota:#?}");
    assert_eq!(quota.windows[0].used_percent, 42.0);
    assert_eq!(quota.windows[0].state, WindowState::Available);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_reading_is_recorded_and_each_retry_waits_twice_as_long() {
    let (polled, daemon) = Polled::start(vec![
        refused("the usage endpoint answered HTTP 429"),
        refused("the usage endpoint answered HTTP 429"),
        refused("the usage endpoint answered HTTP 429"),
    ])
    .await;

    let asks = polled.asks_once(3).await;
    let recorded = polled.recorded(daemon).await;

    let waited: Vec<u128> = asks.windows(2).map(|pair| pair[1].0 - pair[0].0).collect();
    let refresh = REFRESH.as_millis();
    assert!(waited[0] >= 2 * refresh, "{waited:?}");
    assert!(waited[1] >= 4 * refresh, "{waited:?}");
    let quota = &recorded[0];
    assert!(!quota.is_current);
    assert_eq!(
        quota
            .failure
            .as_ref()
            .map(|failure| failure.reason.as_str()),
        Some("the usage endpoint answered HTTP 429")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reading_after_failures_is_current_and_the_refresh_returns() {
    let (polled, daemon) = Polled::start(vec![refused("offline"), Ok(five_hour(10.0))]).await;

    let asks = polled.asks_once(4).await;
    let recorded = polled.recorded(daemon).await;

    let after_success = asks[3].0 - asks[2].0;
    assert!(
        after_success < 4 * REFRESH.as_millis(),
        "{after_success} ms"
    );
    assert!(recorded[0].is_current, "{:#?}", recorded[0]);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_owner_reads_each_accounts_windows_as_recorded_and_judged_now() {
    let (polled, daemon) = Polled::start(vec![Ok(five_hour(42.7))]).await;
    // The second ask comes after the first reading was recorded.
    polled.asks_once(2).await;

    let accounts = polled.asked(&daemon).await.unwrap();

    assert_eq!(accounts.len(), 1);
    let account = &accounts[0];
    assert_eq!(
        (
            account.provider.as_str(),
            account.account.as_str(),
            account.plan.as_deref(),
            account.is_blocked,
            account.is_current,
            &account.failure,
        ),
        ("fake", "default", Some("max"), false, true, &None)
    );
    let QuotaWindowView {
        limit,
        scope,
        length_seconds,
        used_percent,
        resets_at,
        state,
        pace,
    } = &account.windows[0];
    assert_eq!(
        (
            limit.as_str(),
            scope,
            *length_seconds,
            *used_percent,
            *state
        ),
        (
            "five-hour",
            &None,
            Some(18_000),
            42,
            QuotaWindowStateView::Available
        )
    );
    assert!(resets_at.is_some_and(|at| at > common::unix_now()));
    // Two of five hours in, the default line is 0.4 × 90 + 15 = 51 %.
    assert_eq!(
        pace,
        &Some(QuotaPaceView {
            line_percent: Some(51),
            is_holding: false,
            resumes_at: None,
        })
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_account_the_provider_blocks_is_answered_blocked_even_without_windows() {
    let blocked = QuotaReading::new(Some("plus".to_owned()), Vec::new(), true).unwrap();
    let (polled, daemon) = Polled::start(vec![Ok(blocked)]).await;
    polled.asks_once(2).await;

    let accounts = polled.asked(&daemon).await.unwrap();

    assert_eq!(
        (accounts[0].is_blocked, accounts[0].windows.len()),
        (true, 0)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_account_whose_every_refresh_failed_is_answered_with_the_reason() {
    let (polled, daemon) = Polled::start(vec![
        refused("the usage endpoint answered HTTP 429"),
        refused("the usage endpoint answered HTTP 429"),
    ])
    .await;
    polled.asks_once(2).await;

    let accounts = polled.asked(&daemon).await.unwrap();

    let account = &accounts[0];
    assert_eq!(
        (
            account.observed_at,
            account.is_current,
            account.windows.len(),
            account
                .failure
                .as_ref()
                .map(|failure| failure.reason.as_str()),
        ),
        (None, false, 0, Some("the usage endpoint answered HTTP 429"))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_daemon_that_reads_no_quota_refuses_the_request() {
    let dir = tempfile::tempdir().unwrap();
    let wiring = common::wiring_in(dir.path(), Vec::new());
    assert!(wiring.quota.is_none(), "the test wiring reads no quota");
    let environment = wiring.environment.clone();
    let daemon = Instance::acquire(options_in(dir.path()))
        .unwrap()
        .start(wiring, move |capabilities| {
            Arc::new(Routes::new(capabilities, environment))
        })
        .await
        .unwrap();
    let client = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();

    let answer = client.request(Command::UsageQuota, GENEROUS).await;

    assert!(
        matches!(&answer, Err(ClientError::Daemon(error))
            if error.code == ErrorCode::Refused && error.message == "this daemon reads no quota"),
        "{answer:?}"
    );
}

const FIVE_HOUR_ALERT: &str = "fake/default/five-hour";

#[tokio::test(flavor = "multi_thread")]
async fn a_window_passing_a_threshold_reaches_the_change_feed_as_its_window() {
    let (polled, daemon) = Polled::start(vec![Ok(five_hour(85.0))]).await;

    let alerts = polled
        .alerts_until(&daemon, |alerts| !alerts.is_empty())
        .await;

    assert_eq!(alerts.keys().collect::<Vec<_>>(), [FIVE_HOUR_ALERT]);
    let alert = &alerts[FIVE_HOUR_ALERT];
    assert_eq!(
        (alert.provider.as_str(), alert.account.as_str()),
        ("fake", "default")
    );
    assert_eq!((alert.threshold, alert.used_percent), (80, 85));
    assert!(alert.resets_at.is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn the_next_threshold_replaces_the_windows_alert_on_the_feed() {
    let (polled, daemon) = Polled::start(vec![Ok(five_hour(85.0)), Ok(five_hour(97.0))]).await;

    let alerts = polled
        .alerts_until(&daemon, |alerts| {
            alerts.values().any(|alert| alert.threshold == 95)
        })
        .await;

    assert_eq!(alerts.keys().collect::<Vec<_>>(), [FIVE_HOUR_ALERT]);
    assert_eq!(alerts[FIVE_HOUR_ALERT].used_percent, 97);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_window_read_below_every_threshold_again_takes_its_alert_off_the_feed() {
    // Five readings past 80 % first, so the feed's snapshot is taken while the alert stands.
    let mut answers = vec![Ok(five_hour(85.0)); 5];
    answers.push(Ok(five_hour(3.0)));
    let (polled, daemon) = Polled::start(answers).await;
    polled
        .alerts_until(&daemon, |alerts| !alerts.is_empty())
        .await;

    polled.asks_once(7).await;
    let published = polled.published_alerts(daemon).await;

    assert_eq!(published, Vec::<String>::new());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_window_below_every_threshold_puts_no_alert_on_the_feed() {
    let (polled, daemon) = Polled::start(vec![Ok(five_hour(42.0))]).await;

    polled.asks_once(2).await;
    let published = polled.published_alerts(daemon).await;

    assert_eq!(published, Vec::<String>::new());
}

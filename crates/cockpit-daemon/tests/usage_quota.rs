//! Issue #58 through the daemon: each provider reporting `quota` is read at the owner's
//! interval in the owner's home, its windows recorded, and a provider that fails is asked
//! again later and later.
#![cfg(target_os = "linux")]

mod candidate;
mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use cockpit_daemon::{Daemon, Instance, Limits, Options, QuotaPolling, Routes};
use cockpit_provider_api::fake::FakeProvider;
use cockpit_provider_api::{LimitKey, ProviderError, ProviderName, QuotaReading, QuotaWindow};
use cockpit_usage::{QuotaPolicy, WindowState};

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

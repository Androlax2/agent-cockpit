//! Reads each provider's plan windows at the owner's interval (#58) and records them, or why
//! they could not be read, through `cockpit-usage`. A provider whose reading fails waits
//! twice as long as before, up to an hour, so a refused sign-in or a rate limit is not asked
//! again at every refresh. The alerts a reading fires are logged; clients read them with the
//! usage request (#58, part 3).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use cockpit_provider_api::{Capability, Provider, ProviderError, QuotaReading};
use cockpit_turso::Database;
use cockpit_usage::{AccountName, Alert, QuotaPolicy};

/// Until accounts arrive (#59), each provider has one: the one its harness is signed in to.
const ACCOUNT: &str = "default";
const LONGEST_FAILURE_WAIT: Duration = Duration::from_secs(60 * 60);

/// How the daemon reads plan windows: the owner's `[usage]` settings, and the owner's home,
/// where each harness keeps its sign-in.
#[derive(Debug, Clone)]
pub struct QuotaPolling {
    /// Never a session's home: a session's sign-in is not the owner's account.
    pub home: PathBuf,
    /// Between two readings of one provider while they succeed.
    pub refresh: Duration,
    pub policy: QuotaPolicy,
}

impl QuotaPolling {
    /// The owner's `[usage]` settings, with `home` the owner's.
    pub fn from_settings(settings: &cockpit_config::UsageSettings, home: PathBuf) -> QuotaPolling {
        let policy = QuotaPolicy::new(
            settings.quota_alert_percents.value.clone(),
            settings.quota_fresh_minutes.value.saturating_mul(60),
        )
        .expect("cockpit-config refuses `[usage]` settings outside the quota policy's rules");
        QuotaPolling {
            home,
            refresh: Duration::from_secs(u64::from(settings.quota_refresh_minutes.value) * 60),
            policy,
        }
    }

    fn wait_after(&self, last_wait: Duration, is_read: bool) -> Duration {
        if is_read {
            return self.refresh;
        }
        (last_wait * 2).min(LONGEST_FAILURE_WAIT.max(self.refresh))
    }
}

/// Reads every provider of `providers` that reports `quota`, first at once, then each on its
/// own schedule, in a task of its own so a slow reading delays no other provider. Returns at
/// once when none does; a panic in one task is the worker's, for its supervisor.
pub(crate) async fn poll_forever(
    database: Arc<Database>,
    providers: Vec<Arc<dyn Provider>>,
    polling: QuotaPolling,
) {
    let polling = Arc::new(polling);
    let mut tasks = tokio::task::JoinSet::new();
    for provider in providers
        .into_iter()
        .filter(|provider| provider.capabilities().supports(Capability::Quota))
    {
        tasks.spawn(poll_one(
            Arc::clone(&database),
            provider,
            Arc::clone(&polling),
        ));
    }
    while let Some(ended) = tasks.join_next().await {
        if let Err(error) = ended
            && error.is_panic()
        {
            std::panic::resume_unwind(error.into_panic());
        }
    }
}

async fn poll_one(
    database: Arc<Database>,
    provider: Arc<dyn Provider>,
    polling: Arc<QuotaPolling>,
) {
    let account = AccountName::new(ACCOUNT).expect("`default` is a valid account name");
    let mut wait = polling.refresh;
    loop {
        let is_read = read_once(&database, &provider, &account, &polling).await;
        wait = polling.wait_after(wait, is_read);
        tokio::time::sleep(wait).await;
    }
}

/// Whether `provider` gave a reading.
async fn read_once(
    database: &Database,
    provider: &Arc<dyn Provider>,
    account: &AccountName,
    polling: &QuotaPolling,
) -> bool {
    let asked = Arc::clone(provider);
    let home = polling.home.clone();
    let name = provider.name();
    let answer = tokio::task::spawn_blocking(move || asked.quota(&home))
        .await
        .unwrap_or_else(|error| {
            tracing::error!(provider = %name, panic = %panic_message(error), "the quota reading panicked");
            Err(ProviderError {
                reason: "the quota reading panicked".to_owned(),
            })
        });
    match record(database, provider, account, &answer, polling).await {
        Err(error) => {
            tracing::error!(provider = %name, error, "recording the plan windows failed");
        }
        Ok(alerts) => {
            for alert in alerts {
                tracing::info!(
                    provider = %name,
                    account = %alert.account.as_str(),
                    limit = alert.limit.as_str(),
                    threshold = alert.threshold,
                    used_percent = alert.used_percent,
                    "a plan window passed an alert threshold"
                );
            }
        }
    }
    match answer {
        Ok(_) => true,
        Err(error) => {
            tracing::warn!(provider = %name, reason = %error, "reading the plan windows failed");
            false
        }
    }
}

fn panic_message(error: tokio::task::JoinError) -> String {
    if !error.is_panic() {
        return error.to_string();
    }
    let payload = error.into_panic();
    payload
        .downcast_ref::<&str>()
        .map(|message| (*message).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "a panic without a message".to_owned())
}

async fn record(
    database: &Database,
    provider: &Arc<dyn Provider>,
    account: &AccountName,
    answer: &Result<QuotaReading, ProviderError>,
    polling: &QuotaPolling,
) -> Result<Vec<Alert>, String> {
    let now = crate::approvals::now();
    let write = database.write().await.map_err(|error| error.to_string())?;
    let alerts = match answer {
        Ok(reading) => cockpit_usage::record_reading(
            &write,
            provider.name(),
            account,
            reading,
            &polling.policy,
            now,
        )
        .await
        .map_err(|error| error.to_string())?,
        Err(error) => {
            cockpit_usage::record_failure(&write, provider.name(), account, &error.reason, now)
                .await
                .map_err(|error| error.to_string())?;
            Vec::new()
        }
    };
    write.commit().await.map_err(|error| error.to_string())?;
    Ok(alerts)
}

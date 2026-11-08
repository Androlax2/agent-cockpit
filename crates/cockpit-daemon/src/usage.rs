//! Reads each provider's plan windows at the owner's interval (#58) and records them, or why
//! they could not be read, through `cockpit-usage`. A provider whose reading fails waits
//! twice as long as before, up to an hour, so a refused sign-in or a rate limit is not asked
//! again at every refresh. The alerts a reading fires are logged and put on the change feed,
//! in the reading's own transaction, as one `quota-alert` entity per window: `torlek-notify`
//! tells the owner each new state. `usage-quota` answers the recorded windows to the owner,
//! judged under the same policy.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use cockpit_protocol::{
    ErrorCode, Outcome, ProtocolError, QuotaAccountView, QuotaAlertView, QuotaFailureView,
    QuotaWindowStateView, QuotaWindowView, Reply,
};
use cockpit_provider_api::{Capability, Provider, ProviderError, QuotaReading};
use cockpit_turso::Database;
use cockpit_usage::{AccountName, AccountQuota, Alert, QuotaPolicy, WindowQuota, WindowState};

/// Until accounts arrive (#59), each provider has one: the one its harness is signed in to.
const ACCOUNT: &str = "default";
const QUOTA_ALERT: &str = "quota-alert";
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
    for alert in &alerts {
        cockpit_messaging::publish(
            &write,
            QUOTA_ALERT,
            &alert_id(alert),
            &crate::session_feed::state_of(&alert_view(alert)),
        )
        .await
        .map_err(|error| error.to_string())?;
    }
    write.commit().await.map_err(|error| error.to_string())?;
    Ok(alerts)
}

/// One entity per window: a later alert of the window replaces the earlier one.
fn alert_id(alert: &Alert) -> String {
    format!(
        "{}/{}/{}",
        alert.provider.as_str(),
        alert.account.as_str(),
        alert.limit.as_str()
    )
}

fn alert_view(alert: &Alert) -> QuotaAlertView {
    QuotaAlertView {
        provider: alert.provider.as_str().to_owned(),
        account: alert.account.as_str().to_owned(),
        limit: alert.limit.as_str().to_owned(),
        scope: alert.scope.clone(),
        threshold: alert.threshold,
        used_percent: whole_percent(alert.used_percent),
        resets_at: alert.resets_at,
        fired_at: alert.fired_at,
    }
}

/// `usage-quota`: every account's windows as last recorded, judged now under `policy`, the
/// worker's own. `None` when this daemon reads no quota: its answer would judge readings an
/// earlier start recorded under a policy nobody set.
pub(crate) async fn report(database: Arc<Database>, policy: Option<QuotaPolicy>) -> Outcome {
    let Some(policy) = policy else {
        return Outcome::Error(ProtocolError {
            code: ErrorCode::Refused,
            message: "this daemon reads no quota".to_owned(),
            unblock: None,
        });
    };
    match recorded_quotas(&database, &policy).await {
        Ok(accounts) => Outcome::Ok(Reply::UsageQuota {
            accounts: accounts.into_iter().map(account_view).collect(),
        }),
        Err(error) => {
            tracing::error!(%error, "reading the recorded quota failed");
            Outcome::Error(ProtocolError {
                code: ErrorCode::Internal,
                message: "the daemon failed; no quota was read".to_owned(),
                unblock: None,
            })
        }
    }
}

async fn recorded_quotas(
    database: &Database,
    policy: &QuotaPolicy,
) -> Result<Vec<AccountQuota>, String> {
    let read = database.read().await.map_err(|error| error.to_string())?;
    cockpit_usage::quotas(&read, policy, crate::approvals::now())
        .await
        .map_err(|error| error.to_string())
}

fn account_view(quota: AccountQuota) -> QuotaAccountView {
    QuotaAccountView {
        provider: quota.provider.as_str().to_owned(),
        account: quota.account.as_str().to_owned(),
        plan: quota.plan,
        is_blocked: quota.is_blocked,
        observed_at: quota.observed_at,
        is_current: quota.is_current,
        failure: quota.failure.map(|failure| QuotaFailureView {
            at: failure.at,
            reason: failure.reason,
        }),
        windows: quota.windows.into_iter().map(window_view).collect(),
    }
}

fn window_view(window: WindowQuota) -> QuotaWindowView {
    QuotaWindowView {
        limit: window.limit.as_str().to_owned(),
        scope: window.scope,
        length_seconds: window.length_seconds,
        used_percent: whole_percent(window.used_percent),
        resets_at: window.resets_at,
        state: match window.state {
            WindowState::Available => QuotaWindowStateView::Available,
            WindowState::Low => QuotaWindowStateView::Low,
            WindowState::Exhausted => QuotaWindowStateView::Exhausted,
            WindowState::Blocked => QuotaWindowStateView::Blocked,
            WindowState::Stale => QuotaWindowStateView::Stale,
        },
    }
}

/// Rounded down, so 100 is shown only once the whole limit is used. A float cast saturates;
/// readings are finite and never negative.
fn whole_percent(used_percent: f64) -> u32 {
    used_percent.floor() as u32
}

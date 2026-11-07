use cockpit_provider_api::{LimitKey, ProviderName, QuotaReading, QuotaWindow};
use cockpit_turso::WriteTransaction;
use cockpit_turso::sql::{Connection, Row, params};

use crate::error::{invalid, invalid_row};
use crate::{
    AccountName, AccountQuota, Alert, Error, Failure, MAX_FAILURE_CHARS, QuotaPolicy, WindowQuota,
    WindowState,
};

/// Records `reading` of `account`, taken at `observed_at`, in place of its last one, and
/// returns the alerts it fires under `policy`, already recorded as fired. Refuses a reading
/// older than the one recorded. A window whose reset time is already past fires nothing.
pub async fn record_reading(
    write: &WriteTransaction<'_>,
    provider: &ProviderName,
    account: &AccountName,
    reading: &QuotaReading,
    policy: &QuotaPolicy,
    observed_at: i64,
) -> Result<Vec<Alert>, Error> {
    let key = (provider.as_str(), account.as_str());
    let recorded = recorded_account(write, provider, account).await?;
    if let Some(recorded_at) = recorded.as_ref().and_then(|row| row.observed_at)
        && observed_at < recorded_at
    {
        return Err(Error::OlderReading {
            observed_at,
            recorded_at,
        });
    }
    write
        .execute(
            "INSERT INTO usage_quota_accounts (provider, account, plan, is_blocked, observed_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (provider, account) DO UPDATE SET plan = excluded.plan,
                is_blocked = excluded.is_blocked, observed_at = excluded.observed_at",
            params![
                key.0,
                key.1,
                reading.plan(),
                i64::from(reading.is_blocked()),
                observed_at
            ],
        )
        .await?;
    write
        .execute(
            "DELETE FROM usage_quota_windows WHERE provider = ?1 AND account = ?2",
            params![key.0, key.1],
        )
        .await?;
    for window in reading.windows() {
        write
            .execute(
                "INSERT INTO usage_quota_windows (provider, account, limit_key, scope,
                    length_seconds, used_percent, resets_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    key.0,
                    key.1,
                    window.limit().as_str(),
                    window.scope(),
                    window.length_seconds().map(i64::from),
                    window.used_percent(),
                    window.resets_at(),
                ],
            )
            .await?;
    }
    record_alerts(write, provider, account, reading, policy, observed_at).await
}

/// Records that refreshing `account`'s quota failed at `failed_at`, keeping its last reading,
/// which stops being current. `reason` is cut to [`MAX_FAILURE_CHARS`] characters. A failure
/// older than the one recorded is ignored.
pub async fn record_failure(
    write: &WriteTransaction<'_>,
    provider: &ProviderName,
    account: &AccountName,
    reason: &str,
    failed_at: i64,
) -> Result<(), Error> {
    let reason: String = reason.trim().chars().take(MAX_FAILURE_CHARS).collect();
    if reason.is_empty() {
        return Err(invalid("failure", "the reason is empty"));
    }
    let key = (provider.as_str(), account.as_str());
    match recorded_account(write, provider, account).await? {
        None => {
            write
                .execute(
                    "INSERT INTO usage_quota_accounts (provider, account, failed_at, failure)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![key.0, key.1, failed_at, reason],
                )
                .await?;
        }
        Some(row) if row.failed_at.is_some_and(|recorded| recorded > failed_at) => {}
        Some(_) => {
            write
                .execute(
                    "UPDATE usage_quota_accounts SET failed_at = ?3, failure = ?4
                     WHERE provider = ?1 AND account = ?2",
                    params![key.0, key.1, failed_at, reason],
                )
                .await?;
        }
    }
    Ok(())
}

/// Every account's windows as last read, judged at `now` under `policy`, by provider and
/// account, each account's windows by limit. One query, so an account and its windows always
/// come from the same reading.
pub async fn quotas(
    connection: &Connection,
    policy: &QuotaPolicy,
    now: i64,
) -> Result<Vec<AccountQuota>, Error> {
    let mut rows = connection
        .query(
            "SELECT account.provider, account.account, account.plan, account.is_blocked,
                account.observed_at, account.failed_at, account.failure, window.limit_key,
                window.scope, window.length_seconds, window.used_percent, window.resets_at
             FROM usage_quota_accounts AS account
             LEFT JOIN usage_quota_windows AS window
                ON window.provider = account.provider AND window.account = account.account
             ORDER BY account.provider, account.account, window.limit_key",
            (),
        )
        .await?;
    let mut quotas: Vec<AccountQuota> = Vec::new();
    while let Some(row) = rows.next().await? {
        let account = account_from(&row)?;
        let is_same_account = quotas.last().is_some_and(|last| {
            last.provider == account.provider && last.account == account.account
        });
        if !is_same_account {
            let is_current = is_current(&account, policy, now);
            quotas.push(AccountQuota {
                is_current,
                ..account
            });
        }
        let Some(quota) = quotas.last_mut() else {
            unreachable!("an account was pushed above");
        };
        if let Some(mut window) = window_from(&row)? {
            window.state = window_state(&window, quota.is_current, quota.is_blocked, policy, now);
            quota.windows.push(window);
        }
    }
    Ok(quotas)
}

struct RecordedAccount {
    observed_at: Option<i64>,
    failed_at: Option<i64>,
}

async fn recorded_account(
    connection: &Connection,
    provider: &ProviderName,
    account: &AccountName,
) -> Result<Option<RecordedAccount>, Error> {
    let mut rows = connection
        .query(
            "SELECT observed_at, failed_at FROM usage_quota_accounts
             WHERE provider = ?1 AND account = ?2",
            params![provider.as_str(), account.as_str()],
        )
        .await?;
    let Some(row) = rows.next().await? else {
        return Ok(None);
    };
    Ok(Some(RecordedAccount {
        observed_at: row.get(0)?,
        failed_at: row.get(1)?,
    }))
}

/// An alert row: `threshold` fired for `limit` in the window instance `window_reset`.
#[derive(Clone)]
struct Fired {
    limit: String,
    threshold: u8,
    window_reset: Option<i64>,
    used_percent: f64,
    fired_at: i64,
}

/// Rewrites the account's alert rows for `reading` and returns the alerts that fire now.
/// A window the reading leaves out, or reports with its reset already past, keeps its rows,
/// so it does not alert again when it comes back. Rows of an earlier window instance and of
/// thresholds the policy no longer has are dropped. A window without a reset time re-arms a
/// threshold once a reading shows it below.
async fn record_alerts(
    write: &WriteTransaction<'_>,
    provider: &ProviderName,
    account: &AccountName,
    reading: &QuotaReading,
    policy: &QuotaPolicy,
    observed_at: i64,
) -> Result<Vec<Alert>, Error> {
    let recorded = fired_alerts(write, provider, account).await?;
    let mut kept: Vec<Fired> = recorded
        .iter()
        .filter(|fired| {
            !reading
                .windows()
                .iter()
                .any(|window| window.limit().as_str() == fired.limit)
        })
        .cloned()
        .collect();
    let mut alerts = Vec::new();
    for window in reading.windows() {
        let of_window = recorded
            .iter()
            .filter(|fired| fired.limit == window.limit().as_str());
        if window.resets_at().is_some_and(|reset| reset <= observed_at) {
            kept.extend(of_window.cloned());
            continue;
        }
        let instance = window.resets_at();
        let this_instance: Vec<&Fired> = of_window
            .filter(|fired| is_same_instance(fired.window_reset, instance))
            .collect();
        let highest_fired = this_instance.iter().map(|fired| fired.threshold).max();
        let mut newly_crossed = None;
        for &threshold in policy.alert_thresholds() {
            let is_crossed = window.used_percent() >= f64::from(threshold);
            let already = this_instance
                .iter()
                .find(|fired| fired.threshold == threshold);
            match already {
                Some(fired) if is_crossed || instance.is_some() => kept.push((*fired).clone()),
                Some(_) => {}
                None if is_crossed => {
                    kept.push(Fired {
                        limit: window.limit().as_str().to_owned(),
                        threshold,
                        window_reset: instance,
                        used_percent: window.used_percent(),
                        fired_at: observed_at,
                    });
                    // A threshold the owner added below one already fired stays silent.
                    if highest_fired.is_none_or(|highest| threshold > highest) {
                        newly_crossed = Some(threshold);
                    }
                }
                None => {}
            }
        }
        if let Some(threshold) = newly_crossed {
            alerts.push(alert(provider, account, window, threshold, observed_at));
        }
    }
    write
        .execute(
            "DELETE FROM usage_quota_alerts WHERE provider = ?1 AND account = ?2",
            params![provider.as_str(), account.as_str()],
        )
        .await?;
    for fired in kept {
        write
            .execute(
                "INSERT INTO usage_quota_alerts (provider, account, limit_key, threshold,
                    window_reset, used_percent, fired_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    provider.as_str(),
                    account.as_str(),
                    fired.limit,
                    i64::from(fired.threshold),
                    fired.window_reset,
                    fired.used_percent,
                    fired.fired_at,
                ],
            )
            .await?;
    }
    Ok(alerts)
}

async fn fired_alerts(
    connection: &Connection,
    provider: &ProviderName,
    account: &AccountName,
) -> Result<Vec<Fired>, Error> {
    let mut rows = connection
        .query(
            "SELECT limit_key, threshold, window_reset, used_percent, fired_at
             FROM usage_quota_alerts WHERE provider = ?1 AND account = ?2",
            params![provider.as_str(), account.as_str()],
        )
        .await?;
    let mut fired = Vec::new();
    while let Some(row) = rows.next().await? {
        let threshold = row.get::<i64>(1)?;
        fired.push(Fired {
            limit: row.get(0)?,
            threshold: u8::try_from(threshold)
                .map_err(|_| invalid_row(format!("alert threshold {threshold}")))?,
            window_reset: row.get(2)?,
            used_percent: row.get(3)?,
            fired_at: row.get(4)?,
        });
    }
    Ok(fired)
}

fn alert(
    provider: &ProviderName,
    account: &AccountName,
    window: &QuotaWindow,
    threshold: u8,
    fired_at: i64,
) -> Alert {
    Alert {
        provider: provider.clone(),
        account: account.clone(),
        limit: window.limit().clone(),
        scope: window.scope().map(str::to_owned),
        threshold,
        used_percent: window.used_percent(),
        resets_at: window.resets_at(),
        fired_at,
    }
}

/// Two reset times at most this far apart name the same window instance. A provider's reset
/// time moves between readings: by a fraction of a second ("23:59:59.63", then "00:00:00.10",
/// which made every reading a new window in the legacy cockpit), or by a second when an
/// adapter computes it from a countdown. The next instance resets hours later.
const SAME_INSTANCE_SECONDS: i64 = 60;

fn is_same_instance(stored: Option<i64>, reset: Option<i64>) -> bool {
    match (stored, reset) {
        (None, None) => true,
        (Some(stored), Some(reset)) => {
            stored.abs_diff(reset) <= SAME_INSTANCE_SECONDS.unsigned_abs()
        }
        _ => false,
    }
}

fn is_current(account: &AccountQuota, policy: &QuotaPolicy, now: i64) -> bool {
    let Some(observed_at) = account.observed_at else {
        return false;
    };
    let is_young = observed_at <= now && now - observed_at <= i64::from(policy.fresh_for_seconds());
    let failed_since = account
        .failure
        .as_ref()
        .is_some_and(|failure| failure.at > observed_at);
    is_young && !failed_since
}

fn window_state(
    window: &WindowQuota,
    is_current: bool,
    is_blocked: bool,
    policy: &QuotaPolicy,
    now: i64,
) -> WindowState {
    if !is_current || window.resets_at.is_some_and(|reset| reset <= now) {
        WindowState::Stale
    } else if is_blocked {
        WindowState::Blocked
    } else if window.used_percent >= 100.0 {
        WindowState::Exhausted
    } else if window.used_percent >= f64::from(policy.low_at()) {
        WindowState::Low
    } else {
        WindowState::Available
    }
}

fn account_from(row: &Row) -> Result<AccountQuota, Error> {
    let provider = row.get::<String>(0)?;
    let account = row.get::<String>(1)?;
    let failure = match (row.get::<Option<i64>>(5)?, row.get::<Option<String>>(6)?) {
        (None, None) => None,
        (Some(at), Some(reason)) => Some(Failure { at, reason }),
        _ => return Err(invalid_row("failure columns disagree")),
    };
    let quota = AccountQuota {
        provider: ProviderName::new(provider)
            .map_err(|error| invalid_row(format!("provider: {error}")))?,
        account: AccountName::new(account)
            .map_err(|error| invalid_row(format!("account: {error}")))?,
        plan: row.get(2)?,
        is_blocked: row.get::<Option<i64>>(3)? == Some(1),
        observed_at: row.get(4)?,
        is_current: false,
        failure,
        windows: Vec::new(),
    };
    Ok(quota)
}

/// The window columns of a [`quotas`] row; `None` for an account without windows.
fn window_from(row: &Row) -> Result<Option<WindowQuota>, Error> {
    let Some(limit) = row.get::<Option<String>>(7)? else {
        return Ok(None);
    };
    let length_seconds = row
        .get::<Option<i64>>(9)?
        .map(|length| {
            u32::try_from(length).map_err(|_| invalid_row(format!("window length {length}")))
        })
        .transpose()?;
    Ok(Some(WindowQuota {
        limit: LimitKey::new(limit).map_err(|error| invalid_row(format!("limit: {error}")))?,
        scope: row.get(8)?,
        length_seconds,
        used_percent: row.get(10)?,
        resets_at: row.get(11)?,
        state: WindowState::Stale,
    }))
}

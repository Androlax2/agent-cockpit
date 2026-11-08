use std::fmt;

use cockpit_provider_api::{LimitKey, ProviderName};

use crate::Error;
use crate::error::invalid;

/// Report 18's alerts: a window passing 80 % and then 95 % of its limit.
pub const DEFAULT_ALERT_THRESHOLDS: [u8; 2] = [80, 95];
/// How long a reading stays current without a newer one: two refreshes of the legacy cockpit's
/// five minutes.
pub const DEFAULT_FRESH_FOR_SECONDS: u32 = 600;
/// Longest failure reason kept for an account.
pub const MAX_FAILURE_CHARS: usize = 500;
const MAX_ACCOUNT_BYTES: usize = 32;
const MAX_THRESHOLDS: usize = 5;

/// An account's name within its provider, as the owner calls it: lower-case ASCII letters,
/// digits and `-`, starting with a letter. An owner may hold two accounts on one provider's
/// subscription (#58); account routing (#59) names them. Never the provider's own account
/// identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AccountName(String);

impl AccountName {
    pub fn new(name: impl Into<String>) -> Result<AccountName, Error> {
        let name = name.into();
        let is_plain = name.len() <= MAX_ACCOUNT_BYTES
            && name.starts_with(|first: char| first.is_ascii_lowercase())
            && name.chars().all(|character| {
                character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
            });
        if !is_plain {
            return Err(invalid(
                "account",
                format!(
                    "{name:?} is not 1 to 32 lower-case ASCII letters, digits and `-`, \
                     starting with a letter"
                ),
            ));
        }
        Ok(AccountName(name))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AccountName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// The owner's settings for judging quota windows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaPolicy {
    alert_thresholds: Vec<u8>,
    fresh_for_seconds: u32,
}

impl QuotaPolicy {
    /// `alert_thresholds` are the used shares, in percent, at which an alert fires once per
    /// window: 1 to 5 of them, strictly ascending, each 1 to 100. The lowest also marks a
    /// window [`WindowState::Low`]. `fresh_for_seconds` is how long a reading stays current.
    pub fn new(alert_thresholds: Vec<u8>, fresh_for_seconds: u32) -> Result<QuotaPolicy, Error> {
        let is_ascending = alert_thresholds.windows(2).all(|pair| pair[0] < pair[1]);
        let is_in_range = alert_thresholds
            .iter()
            .all(|threshold| (1..=100).contains(threshold));
        if alert_thresholds.is_empty()
            || alert_thresholds.len() > MAX_THRESHOLDS
            || !is_ascending
            || !is_in_range
        {
            return Err(invalid(
                "alert thresholds",
                format!(
                    "{alert_thresholds:?} is not 1 to 5 strictly ascending percentages from 1 \
                     to 100"
                ),
            ));
        }
        if fresh_for_seconds == 0 {
            return Err(invalid(
                "fresh for",
                "a reading must stay current at least 1 second",
            ));
        }
        Ok(QuotaPolicy {
            alert_thresholds,
            fresh_for_seconds,
        })
    }

    pub fn alert_thresholds(&self) -> &[u8] {
        &self.alert_thresholds
    }

    pub fn fresh_for_seconds(&self) -> u32 {
        self.fresh_for_seconds
    }

    pub(crate) fn low_at(&self) -> u8 {
        self.alert_thresholds[0]
    }
}

impl Default for QuotaPolicy {
    fn default() -> QuotaPolicy {
        QuotaPolicy {
            alert_thresholds: DEFAULT_ALERT_THRESHOLDS.to_vec(),
            fresh_for_seconds: DEFAULT_FRESH_FOR_SECONDS,
        }
    }
}

/// What a window's last reading means now. Only a current reading can say a window has room:
/// an old reading, a failed refresh after it or a reset time already past make it
/// [`WindowState::Stale`], never available.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowState {
    Available,
    /// Used at or past the policy's lowest alert threshold.
    Low,
    /// Used at or past its whole limit.
    Exhausted,
    /// The provider refuses ordinary use on the account, whatever the window says.
    Blocked,
    Stale,
}

impl WindowState {
    pub fn as_str(self) -> &'static str {
        match self {
            WindowState::Available => "available",
            WindowState::Low => "low",
            WindowState::Exhausted => "exhausted",
            WindowState::Blocked => "blocked",
            WindowState::Stale => "stale",
        }
    }
}

/// A refresh of an account's quota that failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub at: i64,
    pub reason: String,
}

/// An account's plan windows as last read, judged at the time asked.
#[derive(Debug, Clone, PartialEq)]
pub struct AccountQuota {
    pub provider: ProviderName,
    pub account: AccountName,
    /// The plan as the last reading named it.
    pub plan: Option<String>,
    /// Whether the last reading said the provider refuses ordinary use on the account.
    pub is_blocked: bool,
    /// When the last reading was taken; `None` when every refresh so far failed.
    pub observed_at: Option<i64>,
    /// Whether the last reading is younger than the policy allows and no refresh failed
    /// after it.
    pub is_current: bool,
    /// The last failed refresh, whether before or after the last reading.
    pub failure: Option<Failure>,
    pub windows: Vec<WindowQuota>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WindowQuota {
    pub limit: LimitKey,
    pub scope: Option<String>,
    pub length_seconds: Option<u32>,
    pub used_percent: f64,
    pub resets_at: Option<i64>,
    pub state: WindowState,
}

/// A window passed one of the owner's thresholds. It fires once per threshold and window
/// instance: a reading that jumps past several thresholds fires the highest only.
#[derive(Debug, Clone, PartialEq)]
pub struct Alert {
    pub provider: ProviderName,
    pub account: AccountName,
    pub limit: LimitKey,
    pub scope: Option<String>,
    pub threshold: u8,
    pub used_percent: f64,
    pub resets_at: Option<i64>,
    pub fired_at: i64,
}

//! What `usage quota` asks the daemon for (report 18, #58): each provider account's plan
//! windows as last read, judged when asked. And the alert a window fires as it fills, which
//! the daemon puts on the change feed for `torlek-notify`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuotaAccountView {
    pub provider: String,
    pub account: String,
    /// The plan as the last reading named it.
    pub plan: Option<String>,
    /// Whether the last reading said the provider refuses ordinary use on the account,
    /// whatever its windows say.
    pub is_blocked: bool,
    /// Unix seconds; `None` when every refresh so far failed.
    pub observed_at: Option<i64>,
    /// Whether the last reading is younger than the owner's `quota_fresh_minutes` and no
    /// refresh failed after it. Only a current reading can say a window has room.
    pub is_current: bool,
    /// The last failed refresh, before or after the last reading.
    pub failure: Option<QuotaFailureView>,
    /// By limit.
    pub windows: Vec<QuotaWindowView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuotaFailureView {
    /// Unix seconds.
    pub at: i64,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuotaWindowView {
    /// `five-hour`, `seven-day`, `seven-day:<model>` and the like.
    pub limit: String,
    /// The model or surface the limit applies to, as the provider names it.
    pub scope: Option<String>,
    pub length_seconds: Option<u32>,
    /// The share of the limit used, rounded down: 100 is the whole limit, and more is past it.
    pub used_percent: u32,
    /// Unix seconds.
    pub resets_at: Option<i64>,
    pub state: QuotaWindowStateView,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum QuotaWindowStateView {
    Available,
    /// Used at or past the owner's lowest alert threshold.
    Low,
    /// Used at or past its whole limit.
    Exhausted,
    /// The provider refuses ordinary use on the account, whatever the window says.
    Blocked,
    /// Too old to say: an old reading, a failed refresh after it, or a reset already past.
    Stale,
}

impl QuotaWindowStateView {
    pub fn as_str(self) -> &'static str {
        match self {
            QuotaWindowStateView::Available => "available",
            QuotaWindowStateView::Low => "low",
            QuotaWindowStateView::Exhausted => "exhausted",
            QuotaWindowStateView::Blocked => "blocked",
            QuotaWindowStateView::Stale => "stale",
        }
    }
}

/// The state of a [`crate::EntityKind::QuotaAlert`] entity: the highest threshold a window
/// passed in one reading, published once, when it fires.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuotaAlertView {
    pub provider: String,
    pub account: String,
    pub limit: String,
    pub scope: Option<String>,
    /// The owner's alert threshold the window passed, in percent.
    pub threshold: u8,
    /// The share of the limit used when it fired, rounded down.
    pub used_percent: u32,
    /// Unix seconds.
    pub resets_at: Option<i64>,
    /// Unix seconds: when the reading that fired it was taken.
    pub fired_at: i64,
}

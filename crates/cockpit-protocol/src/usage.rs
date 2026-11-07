//! What `usage quota` asks the daemon for (report 18, #58): each provider account's plan
//! windows as last read, judged when asked.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuotaAccountView {
    pub provider: String,
    pub account: String,
    /// The plan as the last reading named it.
    pub plan: Option<String>,
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

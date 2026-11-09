//! What `usage quota` and `usage cost` ask the daemon for (report 18, #58): each provider account's plan
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
    /// Where the queue stands against the window (#412); `None` when it paces nothing:
    /// pacing off, a stale reading, or a window of one model the provider does not block.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pace: Option<QuotaPaceView>,
}

/// The even pace queued work of normal priority keeps through a window (#412).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuotaPaceView {
    /// The share the queue may have used by now, rounded down; `None` for a blocked window or
    /// one without a length and a reset.
    pub line_percent: Option<u32>,
    /// Whether queued work waits on this window now.
    pub is_holding: bool,
    /// Unix seconds: when queued work may start again; `None` when it does not wait, or when
    /// no reading can say.
    pub resumes_at: Option<i64>,
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

/// The cost archive's totals for the days `usage-cost` asked for (report 18, #699).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CostArchiveView {
    /// `YYYY-MM-DD`, UTC: the first and the last day asked for, both included. Providers bill
    /// by UTC day.
    pub first_day: String,
    pub last_day: String,
    /// Oldest day first, then by provider, account and project. A day, account and project
    /// with no replies has no row.
    pub rows: Vec<CostRowView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CostRowView {
    /// `YYYY-MM-DD`, UTC.
    pub day: String,
    pub provider: String,
    pub account: String,
    pub project: String,
    pub replies: u64,
    /// Replies of a model without a known price: in `replies` and the token counts, absent
    /// from `cost_nanos`.
    pub unpriced_replies: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    /// What the replies cost at the provider's API prices, in nano-dollars (a billionth of a
    /// dollar): an integer, so totals add up exactly.
    pub cost_nanos: u64,
}

/// The daily digest's data for the days `usage-digest` asked for (report 18, #700): a client
/// renders it without asking anything else.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DigestView {
    /// `YYYY-MM-DD`, UTC: the first and the last day asked for, both included.
    pub first_day: String,
    pub last_day: String,
    /// Oldest day first; a day with nothing is there with zeros.
    pub days: Vec<DayDigestView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DayDigestView {
    /// `YYYY-MM-DD`, UTC.
    pub day: String,
    /// Distinct sessions that began a turn on the day.
    pub sessions: u64,
    /// Turns that began on the day.
    pub turns: u64,
    /// Approvals the owner granted on the day.
    pub approvals: u64,
    /// Time with the agent, projects together: overlapping stretches are counted once.
    pub seconds_with_agent: u64,
    pub replies: u64,
    /// Replies of a model without a known price: in `replies`, absent from `cost_nanos`.
    pub unpriced_replies: u64,
    /// At the provider's API prices, in nano-dollars: an integer, so totals add up exactly.
    pub cost_nanos: u64,
    /// Each project with anything on the day, by name.
    pub projects: Vec<ProjectDayView>,
    /// The provider incidents under way at some moment of the day, or begun on it.
    pub incidents: Vec<IncidentView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectDayView {
    pub project: String,
    pub sessions: u64,
    pub turns: u64,
    pub approvals: u64,
    pub seconds_with_agent: u64,
    pub replies: u64,
    pub unpriced_replies: u64,
    pub cost_nanos: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IncidentView {
    pub provider: String,
    pub title: String,
    /// Unix seconds.
    pub started_at: i64,
    /// Unix seconds; `None` while the incident was never recorded as resolved.
    pub resolved_at: Option<i64>,
}

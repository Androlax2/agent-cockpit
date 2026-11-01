//! Wire views of verification (#313): a check run's receipt, a claim, and a gate's report on
//! a checkout's current result.

use serde::{Deserialize, Serialize};

/// One recorded run of a declared check: what ran, on what source, over which files, and
/// what it found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckReceiptView {
    pub id: i64,
    pub project: String,
    /// The daemon's execution that ran it.
    pub run: String,
    pub check: String,
    pub command: String,
    pub working_directory: String,
    pub environment: String,
    pub head: String,
    /// A digest of the uncommitted changes, `None` for a clean checkout.
    pub dirty: Option<String>,
    pub artifacts: Vec<CheckArtifactView>,
    pub outcome: CheckOutcomeView,
    pub exit_code: Option<i64>,
    /// The output's tail; `output_bytes` is its full size.
    pub output: String,
    pub output_bytes: u64,
    /// Where the full output is.
    pub output_references: Vec<String>,
    /// Seconds since the Unix epoch.
    pub started_at: i64,
    pub finished_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckArtifactView {
    pub path: String,
    /// SHA-256, lower-case hex, taken before the run started.
    pub digest: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CheckOutcomeView {
    Passed,
    Failed,
    Unavailable,
    Blocked,
    Cancelled,
    /// The run ended without a trustworthy result, or a newer daemon's outcome.
    #[serde(other)]
    Unknown,
}

impl CheckOutcomeView {
    pub fn as_str(self) -> &'static str {
        match self {
            CheckOutcomeView::Passed => "passed",
            CheckOutcomeView::Failed => "failed",
            CheckOutcomeView::Unavailable => "unavailable",
            CheckOutcomeView::Blocked => "blocked",
            CheckOutcomeView::Cancelled => "cancelled",
            CheckOutcomeView::Unknown => "unknown",
        }
    }
}

/// A session's statement that a check passed, with no run behind it: never evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckClaimView {
    pub id: i64,
    pub project: String,
    pub check: String,
    pub head: String,
    pub dirty: Option<String>,
    /// The session the daemon identified.
    pub claimant: String,
    pub statement: String,
    pub claimed_at: i64,
}

/// Where a project's declared checks stand on a checkout's current result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateView {
    pub project: String,
    pub checkout: String,
    pub head: String,
    pub dirty: Option<String>,
    pub outcome: GateOutcomeView,
    /// In the order the project declares them.
    pub checks: Vec<GateCheckView>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GateOutcomeView {
    /// Every check passed on this exact result.
    Passed,
    /// A check failed on this exact result.
    Failed,
    /// Nothing failed, but not every check passed: each names its next action.
    Open,
    #[serde(other)]
    Unknown,
}

impl GateOutcomeView {
    pub fn as_str(self) -> &'static str {
        match self {
            GateOutcomeView::Passed => "passed",
            GateOutcomeView::Failed => "failed",
            GateOutcomeView::Open => "open",
            GateOutcomeView::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateCheckView {
    pub name: String,
    pub status: CheckStatusView,
    /// The newest receipt of exactly this check on this result, stale or not.
    pub receipt: Option<i64>,
    /// What has to happen for the check to pass, `None` once it has.
    pub next_action: Option<String>,
}

/// Where one check stands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub enum CheckStatusView {
    /// A receipt applies; its outcome.
    Checked { outcome: CheckOutcomeView },
    /// Receipts exist, none applies.
    Stale { reason: StaleReasonView },
    /// No receipt on this result, only claims that it passed.
    Unverified,
    /// Neither a receipt nor a claim.
    Missing,
    #[serde(other)]
    Unknown,
}

impl CheckStatusView {
    /// `checked: passed`, `stale: artifact-changed src/lib.rs`, `unverified`.
    pub fn summary(&self) -> String {
        match self {
            CheckStatusView::Checked { outcome } => format!("checked: {}", outcome.as_str()),
            CheckStatusView::Stale { reason } => format!("stale: {}", reason.summary()),
            CheckStatusView::Unverified => "unverified".to_owned(),
            CheckStatusView::Missing => "missing".to_owned(),
            CheckStatusView::Unknown => "unknown".to_owned(),
        }
    }
}

/// Why a check's receipts do not apply to the result asked about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum StaleReasonView {
    /// The check's declaration is no longer the one that ran.
    CheckChanged,
    /// It last ran on another head or other uncommitted changes.
    SourceChanged {
        head: String,
        dirty: Option<String>,
    },
    ArtifactChanged {
        path: String,
    },
    ArtifactMissing {
        path: String,
    },
    ArtifactUnreadable {
        path: String,
        detail: String,
    },
    #[serde(other)]
    Unknown,
}

impl StaleReasonView {
    pub fn summary(&self) -> String {
        match self {
            StaleReasonView::CheckChanged => "check-changed".to_owned(),
            StaleReasonView::SourceChanged { head, .. } => {
                format!("source-changed (ran on {head})")
            }
            StaleReasonView::ArtifactChanged { path } => format!("artifact-changed {path}"),
            StaleReasonView::ArtifactMissing { path } => format!("artifact-missing {path}"),
            StaleReasonView::ArtifactUnreadable { path, detail } => {
                format!("artifact-unreadable {path}: {detail}")
            }
            StaleReasonView::Unknown => "unknown".to_owned(),
        }
    }
}

//! Bug reports about Cockpit itself (#427): drafts the owner reviews and sends, never filed
//! automatically.

use serde::{Deserialize, Serialize};

/// What went wrong, as the fingerprint names it. An agent reports `timeout`, `unreachable`,
/// `internal` or `other`; the daemon `internal`, `panic`, `stall` and `crash`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BugKindView {
    Timeout,
    Unreachable,
    Internal,
    Panic,
    Stall,
    Crash,
    Other,
}

impl BugKindView {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::Unreachable => "unreachable",
            Self::Internal => "internal",
            Self::Panic => "panic",
            Self::Stall => "stall",
            Self::Crash => "crash",
            Self::Other => "other",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BugOriginView {
    Owner,
    Agent,
    Daemon,
}

impl BugOriginView {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Agent => "agent",
            Self::Daemon => "daemon",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BugStatusView {
    Draft,
    Sent,
    Discarded,
}

impl BugStatusView {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Sent => "sent",
            Self::Discarded => "discarded",
        }
    }
}

/// What filing a report did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BugFiledView {
    /// A new draft waits for the owner.
    Created,
    /// Today's draft with the same fingerprint counts one more occurrence.
    Merged,
    /// Today's report with the same fingerprint was sent or discarded; nothing new opened.
    AlreadyHandled,
}

impl BugFiledView {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Merged => "merged",
            Self::AlreadyHandled => "already-handled",
        }
    }
}

/// The support bundle of a draft.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum BugBundleView {
    Pending,
    /// `contents`: one `<bytes>\t<path in the archive>` line per entry.
    Written {
        path: String,
        contents: String,
    },
    Failed {
        problem: String,
    },
}

/// What a send hands the owner: built by the daemon from the draft, without the network.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BugIssueView {
    /// `owner/name` of the repository issues go to.
    pub repository: String,
    pub title: String,
    /// The body `gh issue create` files.
    pub body: String,
    pub new_issue_url: String,
    /// Issues that already carry the fingerprint; `None` for an unspecific one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search_url: Option<String>,
    /// What `gh issue list --search` looks for, with `search_url`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search_query: Option<String>,
    /// The URL cut the description or steps to fit.
    pub is_truncated: bool,
}

/// A report as the owner reviews it. Agents only ever see the number and what filing did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BugReportView {
    pub id: i64,
    pub origin: BugOriginView,
    pub requested_by_owner: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    pub status: BugStatusView,
    pub title: String,
    pub description: String,
    pub steps: String,
    /// `cockpit-<version>/<kind>/<component>`.
    pub fingerprint: String,
    pub environment: String,
    pub bundle: BugBundleView,
    pub occurrences: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue_url: Option<String>,
    pub created_at: i64,
    pub changed_at: i64,
    /// What a send would file; on `bug-report-show` only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue: Option<BugIssueView>,
}

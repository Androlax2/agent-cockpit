//! Wire views of projects and checkouts (#28). Paths travel as UTF-8 text: the daemon refuses
//! to register a path that is not.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectView {
    pub name: String,
    pub root: String,
    /// Whether the owner keeps a GitHub token its sessions get as `GH_TOKEN` (#754), never
    /// the value; absent from an older daemon's reply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github_token: Option<GithubTokenView>,
}

/// A project's GitHub token as Torlek shows it (#754): where it lives, never what it holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum GithubTokenView {
    Configured {
        path: String,
    },
    /// Its sessions get no `GH_TOKEN`; the owner creates `path` to give them one.
    Missing {
        path: String,
    },
    /// The file exists but is refused; its sessions do not launch until it is fixed.
    Unusable {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckoutView {
    /// Never reused: a working copy recreated at the same path gets a new id.
    pub id: i64,
    pub project: String,
    pub kind: CheckoutKindView,
    pub path: String,
    /// Counts the working copies created at `path`, from 1.
    pub incarnation: i64,
    /// The sweep is deleting it.
    pub removing: bool,
    /// The holder occupying it, if any.
    pub occupant: Option<String>,
    pub trust: TrustView,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum CheckoutKindView {
    Main,
    Parallel { branch: String },
    Review { pull_request: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum TrustView {
    Trusted,
    Inherited { from: String },
    Untrusted,
    NeverForReview,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeptCheckout {
    pub checkout: CheckoutView,
    pub reason: KeptReason,
}

/// Why the sweep kept a clone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum KeptReason {
    Occupied { holder: String },
    UncommittedChanges,
    UnpushedCommits,
    Replaced { reason: String },
    Unreadable { reason: String },
}

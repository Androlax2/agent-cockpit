//! Wire views of projects and checkouts (#28). Paths travel as UTF-8 text: the daemon refuses
//! to register a path that is not.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectView {
    pub name: String,
    pub root: String,
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

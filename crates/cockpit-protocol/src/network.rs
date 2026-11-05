//! Wire views of network requests (#674): a destination a sandboxed subject was refused for
//! want of an entry, which the owner allows for the rest of the session, allows always for
//! the project, or denies; and the project grants "allow always" creates.
//!
//! The shape is kept stable for every owner client, the phone (#245) included: a request
//! names its `subject` (kind and id), its `project`, what it asks for (`kind` and `target`)
//! and, once decided, when and through which client. A change in the approvals view stamp
//! (`ViewArea::Approvals`) is the change feed's signal to read them again.

use serde::{Deserialize, Serialize};

use crate::DecidedThroughView;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkRequestView {
    pub id: i64,
    pub subject: NetworkSubjectView,
    pub project: String,
    pub kind: NetworkTargetKindView,
    pub host: String,
    pub port: u16,
    /// `host:port`, as the refusal named it.
    pub target: String,
    pub state: NetworkRequestStateView,
    /// Seconds since 1970.
    pub asked_at: i64,
    /// `None` while asking.
    pub decision: Option<NetworkDecisionView>,
    /// The grant an "allow always" created.
    pub grant: Option<i64>,
}

/// Who was refused: a session today; a check run or a project's containers later (#856,
/// #862).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum NetworkSubjectView {
    Session { id: String },
}

/// What a request asks for: a host and port today; a declared service later (#860).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NetworkTargetKindView {
    Host,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NetworkRequestStateView {
    Asking,
    /// Allowed for the rest of the asking session.
    AllowedOnce,
    /// Allowed always for the project, through `grant`.
    AllowedAlways,
    Denied,
}

/// When the owner decided, and through which client (#657). Only the owner decides, on the
/// host-only socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkDecisionView {
    /// Seconds since 1970.
    pub at: i64,
    /// `None` for a client that did not name itself.
    pub through: Option<DecidedThroughView>,
}

/// A project's "allow always" of a host and port, revocable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkGrantView {
    pub id: i64,
    pub project: String,
    pub host: String,
    pub port: u16,
    pub target: String,
    /// The request whose decision created it.
    pub request: i64,
    pub granted: NetworkDecisionView,
    pub revoked: Option<NetworkDecisionView>,
}

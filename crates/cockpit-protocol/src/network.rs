//! Wire views of network requests (#674): a destination a sandboxed subject was refused for
//! want of an entry, which the owner allows for the rest of the session, allows always for
//! the project, or denies (a check's only always or denied, #352); and the project grants
//! "allow always" creates.
//!
//! The shape is kept stable for every owner client, the phone (#245) included: a request
//! names its `subject` (kind and id), its `project`, what it asks for (`kind`, with `host`
//! and `port` or `service`, and the readable `target`) and, once decided, when and through
//! which client. A change in the approvals view stamp
//! (`ViewArea::Approvals`) is the change feed's signal to read them again.

use serde::{Deserialize, Serialize};

use crate::{DecidedThroughView, ShownText};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkRequestView {
    pub id: i64,
    pub subject: NetworkSubjectView,
    pub project: String,
    #[serde(flatten)]
    pub asks: NetworkTargetView,
    /// `host:port` as the refusal named it, or the service's whole record.
    pub target: String,
    pub state: NetworkRequestStateView,
    /// Seconds since 1970.
    pub asked_at: i64,
    /// `None` while asking.
    pub decision: Option<NetworkDecisionView>,
    /// The grant an "allow always" created.
    pub grant: Option<i64>,
    /// Why the agent asked, under the display rules, for a service it asked for ahead
    /// (#880); `None` for what its launcher refused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shown_reason: Option<ShownText>,
}

/// Who was refused: a session, or a run of a project's check (#352, #862 Q2). A check's
/// request stands for its next runs, which file no second one; its "allow always" is a
/// project grant the sessions get too, and it is never allowed once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum NetworkSubjectView {
    Session { id: String },
    Check { run: String, check: String },
}

/// What a request asks for and a grant allows, by `kind`: a host and port, or a declared
/// service (#880), whose grant is bound to the record `target` names. A host without its
/// `host` or `port` is refused when read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum NetworkTargetView {
    Host { host: String, port: u16 },
    Service { service: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NetworkRequestStateView {
    Asking,
    /// Allowed for the rest of the asking session; never a check's request.
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

/// A project's "allow always" of a host and port or a service's record, revocable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkGrantView {
    pub id: i64,
    pub project: String,
    #[serde(flatten)]
    pub allows: NetworkTargetView,
    /// `host:port`, or the service's whole record.
    pub target: String,
    /// The request whose decision created it; `None` for the github.com grant the owner made
    /// without a request (#1256).
    pub request: Option<i64>,
    pub granted: NetworkDecisionView,
    pub revoked: Option<NetworkDecisionView>,
}

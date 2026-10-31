use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::agent::{AgentReport, SessionReport};
use crate::approvals::{GuideView, RunRequestView};
use crate::change_feed::{ChangeBatch, Cursor, FeedPosition};
use crate::error::{ErrorCode, ProtocolError, bounded};
use crate::projects::{CheckoutView, KeptCheckout, ProjectView};

/// Correlates a [`Response`] with the [`Request`] it answers; chosen by the client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RequestId(pub u64);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub id: RequestId,
    pub command: Command,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Command {
    Ping,
    /// Start the change feed at `from`.
    Subscribe {
        from: FeedPosition,
    },
    /// Agent socket only: report the sender's session state. `launch_token` binds a new
    /// launch; once bound, the sender's process tree alone identifies it.
    AgentReport {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        launch_token: Option<String>,
        report: AgentReport,
    },
    /// Owner only: every session's latest report.
    AgentReports,
    /// Register the Git work tree at `path` (absolute) as a project.
    ProjectAdd {
        path: String,
        name: Option<String>,
    },
    ProjectList,
    /// Checkouts not yet removed, of one project or all.
    CheckoutList {
        project: Option<String>,
    },
    /// A parallel clone of the project working on `branch`.
    CheckoutClone {
        project: String,
        branch: String,
    },
    /// A review clone of the project's GitHub pull request.
    CheckoutReview {
        project: String,
        pull_request: u64,
    },
    CheckoutClaim {
        checkout: i64,
        holder: String,
    },
    CheckoutRelease {
        checkout: i64,
        holder: String,
    },
    /// Delete clones that are unoccupied and hold no work; `dry_run` only reports.
    CheckoutSweep {
        dry_run: bool,
    },
    /// Push the checkout's branch to its GitHub origin.
    CheckoutPush {
        checkout: i64,
    },
    /// Agent socket only: ask the owner to run `command` in `cwd` (absolute) on the host.
    /// `reason` says what it does and why.
    AskRun {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        launch_token: Option<String>,
        command: String,
        cwd: String,
        reason: String,
    },
    /// Agent socket only: hand the owner `guide`, in the `ask-to-guide` format, to do by hand.
    AskGuide {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        launch_token: Option<String>,
        guide: String,
        cwd: String,
    },
    /// Agent socket only: one of the sender's run requests. Asking keeps it from expiring.
    AskRunStatus {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        launch_token: Option<String>,
        request: i64,
    },
    /// Agent socket only: one of the sender's guides. Asking keeps it from expiring.
    AskGuideStatus {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        launch_token: Option<String>,
        guide: i64,
    },
    /// Agent socket only: whether an ask from the sender would reach the owner now.
    AskCheck {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        launch_token: Option<String>,
    },
    /// Owner only: what agents ask now, in the order to answer it.
    ApprovalQueue,
    /// Owner only: one run request, whatever its state.
    ApprovalShow {
        request: i64,
    },
    /// Owner only: run the request's `revision`, whose digest is `digest`. Consumed once.
    ApprovalApprove {
        request: i64,
        revision: i64,
        digest: String,
    },
    ApprovalDecline {
        request: i64,
    },
    /// Owner only: replace the command, creating revision `revision + 1`.
    ApprovalEdit {
        request: i64,
        revision: i64,
        command: String,
    },
    /// Owner only: type `text` and Enter into the running command's terminal.
    ApprovalInput {
        request: i64,
        text: String,
    },
    /// Owner only: one guide, whatever its state.
    GuideShow {
        guide: i64,
    },
    /// Owner only: done with step `step`, the one shown now; `answer` when it asks.
    GuideAnswer {
        guide: i64,
        step: usize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        answer: Option<String>,
    },
    /// Owner only: stuck at step `step`; `note` says why.
    GuideStuck {
        guide: i64,
        step: usize,
        note: String,
    },
    GuideClose {
        guide: i64,
    },
    /// A kind this version does not know. Read only: `decode_request` refuses it as
    /// `unknown-command`, and serializing it fails.
    #[serde(other, skip_serializing)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Response {
    pub id: RequestId,
    pub outcome: Outcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Outcome {
    Ok(Reply),
    Error(ProtocolError),
}

/// What a command produced. A client only receives replies to commands it sent, so this set
/// is closed for each protocol version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Reply {
    Pong,
    Subscribed {
        cursor: Cursor,
    },
    /// The report as recorded, under the session the daemon identified.
    AgentReported {
        report: SessionReport,
    },
    AgentReports {
        reports: Vec<SessionReport>,
    },
    // Checkout views are boxed so every `Outcome` stays small enough to move around.
    ProjectAdded {
        project: ProjectView,
        main: Box<CheckoutView>,
    },
    Projects {
        projects: Vec<ProjectView>,
    },
    Checkouts {
        checkouts: Vec<CheckoutView>,
    },
    Checkout {
        checkout: Box<CheckoutView>,
    },
    Released {
        checkout: i64,
    },
    Swept {
        /// Nothing was deleted; `removed` lists what would have been.
        dry_run: bool,
        removed: Vec<CheckoutView>,
        kept: Vec<KeptCheckout>,
    },
    Pushed {
        checkout: i64,
        branch: String,
        remote_url: String,
    },
    RunRequest {
        request: Box<RunRequestView>,
    },
    Guide {
        guide: Box<GuideView>,
    },
    /// An ask from the sender would reach the owner: it is identified and may ask.
    AskReady {
        session: String,
    },
    /// Run requests first: a running command waiting for input, then asking requests oldest
    /// first, then approved and running ones. Then guides, oldest first.
    ApprovalQueue {
        runs: Vec<RunRequestView>,
        guides: Vec<GuideView>,
    },
}

/// Every frame the daemon sends after the handshake.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ServerFrame {
    Response(Response),
    Changes(ChangeBatch),
    /// The answer to a frame that had no readable request id to answer.
    MalformedFrame(ProtocolError),
}

/// A request frame the daemon cannot carry out, with the id to answer when one was readable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestRejection {
    pub id: Option<RequestId>,
    pub error: ProtocolError,
}

impl RequestRejection {
    /// The frame that tells the client about this rejection.
    pub fn into_frame(self) -> ServerFrame {
        match self.id {
            Some(id) => ServerFrame::Response(Response {
                id,
                outcome: Outcome::Error(self.error),
            }),
            None => ServerFrame::MalformedFrame(self.error),
        }
    }
}

/// Reads one request frame, telling an unknown command apart from a malformed one.
pub fn decode_request(frame: &str) -> Result<Request, RequestRejection> {
    let value: Value = serde_json::from_str(frame).map_err(|error| {
        rejection(
            None,
            ErrorCode::InvalidRequest,
            format!("the frame is not JSON: {error}"),
        )
    })?;
    let id = value
        .get("id")
        .and_then(|id| RequestId::deserialize(id).ok());
    match Request::deserialize(&value) {
        Ok(Request {
            command: Command::Unknown,
            ..
        }) => match value["command"]["kind"].as_str() {
            Some(kind) => Err(rejection(
                id,
                ErrorCode::UnknownCommand,
                format!("this daemon does not know the command `{kind}`"),
            )),
            None => Err(rejection(
                id,
                ErrorCode::InvalidRequest,
                "the command kind is not a string".to_owned(),
            )),
        },
        Ok(request) => Ok(request),
        Err(error) => Err(rejection(
            id,
            ErrorCode::InvalidRequest,
            format!("malformed request: {error}"),
        )),
    }
}

fn rejection(id: Option<RequestId>, code: ErrorCode, message: String) -> RequestRejection {
    RequestRejection {
        id,
        error: ProtocolError::new(code, bounded(message)),
    }
}

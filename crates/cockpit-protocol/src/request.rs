use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::agent::{AgentReport, SessionReport};
use crate::approvals::{GuideView, RunRequestView};
use crate::change_feed::{ChangeBatch, Cursor, FeedPosition};
use crate::error::{ErrorCode, ProtocolError, bounded};
use crate::projects::{CheckoutView, KeptCheckout, ProjectView};
use crate::resources::{ProfileView, ReservationView, ResourceStatusView, VerdictView};
use crate::sessions::{ProviderView, SessionFailure, SessionView};
use crate::tasks::{
    HexBytes, TaskDraftTargetView, TaskDraftView, TaskImageView, TaskKindView, TaskView,
};
use crate::workflows::{
    WorkflowBlockedActionView, WorkflowHandoffView, WorkflowPacketView, WorkflowPresetView,
    WorkflowView,
};

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
    /// Start a session in `dir` (absolute), a directory of a registered project, or in a new
    /// parallel clone of its project on the branch `clone`. `agent` names the provider;
    /// without it the daemon's only provider is used. `prompt` is the first message.
    SessionNew {
        dir: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prompt: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        clone: Option<String>,
    },
    /// Sessions not ended; with `ended`, the ended ones too (recent first is the
    /// client's to sort).
    SessionList {
        #[serde(default)]
        ended: bool,
    },
    /// Stop the agent and launch the same conversation again, in place.
    SessionRestart {
        session: String,
    },
    /// Restart the running sessions whose start files changed since their launch: now when
    /// their agent waits at its prompt, at the end of its turn otherwise (#177).
    SessionRestartAll,
    SessionSleep {
        session: String,
    },
    SessionWake {
        session: String,
    },
    /// Reopen an ended or failed session's conversation.
    SessionResume {
        session: String,
    },
    SessionEnd {
        session: String,
    },
    /// End every session the task list says finished its work (#157, decided in #201).
    /// With `only`, end just those of the named sessions still finished: the ones the owner
    /// saw and confirmed, never one that finished since.
    SessionEndFinished {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        only: Option<Vec<String>>,
    },
    /// Record a design link (an http or https URL) on the session.
    SessionLink {
        session: String,
        url: String,
    },
    /// File an item. The owner names `project`; an agent files in its own project's list
    /// without one, or hands it across to `project`'s. `after` names items it waits on.
    TaskAdd {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        launch_token: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        project: Option<String>,
        title: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        notes: String,
        /// `kind` names the command itself; `task` when absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        item_kind: Option<TaskKindView>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        priority: Option<i64>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        after: Vec<i64>,
    },
    /// Agent socket only: suggest an item; `why` says why. The owner approves or declines it.
    TaskPropose {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        launch_token: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        project: Option<String>,
        title: String,
        why: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        notes: String,
        /// `kind` names the command itself; `task` when absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        item_kind: Option<TaskKindView>,
    },
    /// Agent socket only: report what the setup kept the agent from doing, to the owner's
    /// configured blocker project.
    TaskReportBlocker {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        launch_token: Option<String>,
        title: String,
        why: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        notes: String,
    },
    /// The open items the sender may see, of one project or all; `all` adds done and
    /// declined ones.
    TaskList {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        launch_token: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        project: Option<String>,
        #[serde(default)]
        all: bool,
    },
    TaskShow {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        launch_token: Option<String>,
        item: i64,
    },
    /// Owner only: replace what is given. `intent_revision`, when given, must be current.
    TaskEdit {
        item: i64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        intent_revision: Option<i64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        notes: Option<String>,
        /// `kind` names the command itself; `task` when absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        item_kind: Option<TaskKindView>,
    },
    /// Owner only.
    TaskPriority {
        item: i64,
        priority: i64,
    },
    /// Owner only: make a `todo` item available for a session to take, or take it back.
    TaskQueue {
        item: i64,
        queued: bool,
    },
    /// Owner only.
    TaskApprove {
        item: i64,
    },
    /// Owner only: the title stays declined in its project.
    TaskDecline {
        item: i64,
    },
    TaskStart {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        launch_token: Option<String>,
        item: i64,
    },
    /// `summary` is what the items waiting on it read.
    TaskDone {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        launch_token: Option<String>,
        item: i64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
    },
    /// Owner only: back to `todo`.
    TaskReopen {
        item: i64,
    },
    /// Owner only: `item` waits on `on` before it can start.
    TaskWait {
        item: i64,
        on: Vec<i64>,
    },
    /// Owner only.
    TaskUnwait {
        item: i64,
        on: Vec<i64>,
    },
    /// The owner deletes an item nothing open waits on; an agent what it filed and nobody
    /// started.
    TaskDelete {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        launch_token: Option<String>,
        item: i64,
    },
    /// Owner only: attach an image to the notes; the reply names its number.
    TaskImageAdd {
        item: i64,
        data: HexBytes,
    },
    /// Owner only.
    TaskImage {
        item: i64,
        number: i64,
    },
    /// Owner only: keep unsaved form text. For an item, `base_intent_revision` is the one
    /// the form was opened on.
    TaskDraftSave {
        target: TaskDraftTargetView,
        title: String,
        notes: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        base_intent_revision: Option<i64>,
    },
    /// Owner only.
    TaskDrafts,
    /// Owner only.
    TaskDraftDiscard {
        target: TaskDraftTargetView,
    },
    /// The workflow presets this daemon offers, newest version of each.
    WorkflowPresets,
    /// Owner only: start a workflow on an open item. Without `preset`, the item's project's
    /// `workflow.preset`, else `research` for a research item and `code-change` for others.
    WorkflowStart {
        item: i64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        preset: Option<String>,
    },
    /// The running workflows the sender may see, of one project or all; `all` adds delivered
    /// and cancelled ones.
    WorkflowList {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        launch_token: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        project: Option<String>,
        #[serde(default)]
        all: bool,
    },
    WorkflowShow {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        launch_token: Option<String>,
        workflow: i64,
    },
    /// The task packet of the workflow's current step, rebuilt from durable records.
    WorkflowPacket {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        launch_token: Option<String>,
        workflow: i64,
    },
    /// Agent socket only: take the workflow's ready agent step. The sender's provider must
    /// report what the step declares.
    WorkflowBegin {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        launch_token: Option<String>,
        workflow: i64,
    },
    /// Agent socket only: record progress in the step the sender is doing.
    WorkflowCheckpoint {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        launch_token: Option<String>,
        workflow: i64,
        note: String,
    },
    /// End the current step with its handoff. `waive` (owner only) names requirements the
    /// owner waives; no request can claim a requirement passed.
    WorkflowComplete {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        launch_token: Option<String>,
        workflow: i64,
        handoff: WorkflowHandoffView,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        waive: Vec<String>,
    },
    /// Hand the active step back mid-step. An agent must hand off; the owner may omit it.
    WorkflowRelease {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        launch_token: Option<String>,
        workflow: i64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        handoff: Option<WorkflowHandoffView>,
    },
    WorkflowBlock {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        launch_token: Option<String>,
        workflow: i64,
        action: WorkflowBlockedActionView,
        detail: String,
    },
    WorkflowUnblock {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        launch_token: Option<String>,
        workflow: i64,
    },
    /// Owner only.
    WorkflowCancel {
        workflow: i64,
    },
    /// Owner only: the budget, the profile, what this host enforces, and every reservation
    /// with its limits requested, supported and effective.
    ResourceStatus,
    /// Owner only: switch the profile. Agents cannot award themselves foreground priority.
    ResourceProfile {
        profile: ProfileView,
    },
    /// Owner only: pause the queue (start nothing new) or resume it.
    ResourceQueue {
        paused: bool,
    },
    /// Owner only: whether work of `workload` with this demand would be admitted now, and
    /// why not. Reserves nothing. `required` names controls it must not start without.
    ResourceCheck {
        workload: String,
        cpu_millis: u64,
        memory_mib: u64,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        required: Vec<String>,
    },
    /// Owner only: pause at checkpoint: ask the reservation's running work to stop at its
    /// next supported boundary.
    ResourceCheckpoint {
        reservation: i64,
    },
    /// Owner only: stop execution: terminate the reservation's whole execution and confirm
    /// it was observed ended.
    ResourceStop {
        reservation: i64,
    },
    /// Show the session in the manager's viewer pane, waking it first when it is asleep.
    SessionShow {
        session: String,
    },
    /// The providers `session-new` may name in `agent`: those the daemon was started with.
    ProviderList,
    /// Open the manager tmux session, or find the one already open. `column` (the TUI) runs
    /// in its left pane and `viewer` in the pane beside it until a session is shown there,
    /// both from `dir` (absolute). Words are never parsed by a shell.
    ManagerOpen {
        dir: String,
        column: Vec<String>,
        viewer: Vec<String>,
    },
    /// Give the keys to the manager's column, where the TUI shows what waits on the owner
    /// (#317): the TUI does not run tmux, the daemon does.
    ManagerFocus,
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
    /// The report as recorded, under the session the daemon identified; boxed like the
    /// views below, to keep every `Outcome` small.
    AgentReported {
        report: Box<SessionReport>,
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
    Session {
        session: Box<SessionView>,
    },
    Sessions {
        sessions: Vec<SessionView>,
    },
    /// Only sessions whose start files changed are restarted; busy ones are `deferred`
    /// until their agent waits at its prompt.
    RestartedAll {
        restarted: Vec<SessionView>,
        #[serde(default)]
        deferred: Vec<SessionView>,
        failed: Vec<SessionFailure>,
    },
    /// What `session-end-finished` ended, and each finished session it could not end.
    EndedFinished {
        ended: Vec<SessionView>,
        failed: Vec<SessionFailure>,
    },
    Task {
        task: Box<TaskView>,
    },
    /// In the order work moves (doing, todo, proposed, done, declined), highest priority
    /// first within each.
    Tasks {
        tasks: Vec<TaskView>,
    },
    TaskDeleted {
        item: i64,
    },
    TaskImageAdded {
        item: i64,
        image: TaskImageView,
    },
    TaskImage {
        item: i64,
        image: TaskImageView,
        data: HexBytes,
    },
    TaskDraft {
        draft: TaskDraftView,
    },
    /// Oldest save first.
    TaskDrafts {
        drafts: Vec<TaskDraftView>,
    },
    TaskDraftDiscarded,
    Workflow {
        workflow: Box<WorkflowView>,
    },
    /// Oldest first.
    Workflows {
        workflows: Vec<WorkflowView>,
    },
    WorkflowPacket {
        packet: Box<WorkflowPacketView>,
    },
    WorkflowPresets {
        presets: Vec<WorkflowPresetView>,
    },
    ResourceStatus {
        status: Box<ResourceStatusView>,
    },
    ResourceVerdict {
        verdict: VerdictView,
    },
    Reservation {
        reservation: Box<ReservationView>,
    },
    /// `observed_ended`: the execution was observed empty and the reservation released;
    /// otherwise it keeps its reservation and the stop stays requested.
    ResourceStopped {
        reservation: i64,
        observed_ended: bool,
    },
    Providers {
        providers: Vec<ProviderView>,
    },
    /// The manager session: `attach` are the words that attach a terminal to it; `created`
    /// says whether this request opened it.
    Manager {
        attach: Vec<String>,
        created: bool,
    },
    /// The manager's column has the keys.
    ManagerFocused,
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

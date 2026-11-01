use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::agent::{AgentReport, SessionReport};
use crate::approvals::{GuideView, RunRequestView};
use crate::bug_reports::{BugFiledView, BugKindView, BugReportView};
use crate::change_feed::{ChangeBatch, Cursor, FeedPosition};
use crate::coordination::{CoordinatorAuditView, CoordinatorGrantView, ObservedSessionView};
use crate::error::{ErrorCode, ProtocolError, bounded};
use crate::instructions::{ContextBudgetView, InstructionTopicView};
use crate::projects::{CheckoutView, KeptCheckout, ProjectView};
use crate::queue::{QueueExplanationView, QueuePlacementView};
use crate::readiness::RequirementView;
use crate::resources::{ProfileView, ReservationView, ResourceStatusView, VerdictView};
use crate::sessions::{
    AppliedSettingView, ManagerPaneView, ProviderView, SandboxEvidenceView, SessionFailure,
    SessionView,
};
use crate::tasks::{
    HexBytes, TaskDraftTargetView, TaskDraftView, TaskImageView, TaskKindView, TaskView,
};
use crate::verification::{CheckClaimView, CheckReceiptView, GateView};
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
    /// Agent socket only (#410): the sessions the sender's `observe-sessions` grant covers,
    /// each with its state and latest report.
    AgentSessions {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        launch_token: Option<String>,
    },
    /// Agent socket only (#410): queue `text`, one line, to `session`, delivered at its next
    /// prompt with the sender named. Needs `message-sessions`.
    AgentMessage {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        launch_token: Option<String>,
        session: String,
        text: String,
    },
    /// Agent socket only (#410): start a session in `dir` (absolute), running `agent`, with
    /// `prompt` as its first message, in a new clone on `clone` when given. `agent` is
    /// required once the daemon wires more than one. Needs `start-sessions`; waits in the
    /// `ask run` queue unless the grant says otherwise.
    AgentStart {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        launch_token: Option<String>,
        dir: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prompt: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        clone: Option<String>,
    },
    /// Owner only (#410): give `session` one coordinator grant over every session:
    /// `observe-sessions`, `message-sessions` or `start-sessions`. `without_confirmation` and
    /// `per_hour` are `start-sessions`' terms; `without_confirmation` needs `per_hour`.
    SessionGrant {
        session: String,
        permission: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        without_confirmation: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        per_hour: Option<u32>,
    },
    /// Owner only (#410): revoke `session`'s coordinator grant `permission`.
    SessionRevoke {
        session: String,
        permission: String,
    },
    /// Owner only (#410): `session`'s coordinator grants and the latest audit entries it
    /// made or was the target of.
    SessionGrants {
        session: String,
    },
    /// Agent socket only: report a Cockpit malfunction the sender saw (#427). It opens a
    /// draft for the owner to review, never an issue: one per fingerprint per day unless
    /// `requested_by_owner`, which never lifts the per-session bound.
    AgentReportBug {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        launch_token: Option<String>,
        description: String,
        bug_kind: BugKindView,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        component: Option<String>,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        steps: String,
        #[serde(default)]
        requested_by_owner: bool,
    },
    /// Owner only: open a draft with the version facts and a support bundle attached.
    BugReportNew {
        description: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        steps: String,
    },
    /// Owner only: the drafts waiting; `all` adds the sent and discarded reports.
    BugReportList {
        #[serde(default)]
        all: bool,
    },
    /// Owner only: one report with what a send would file.
    BugReportShow {
        report: i64,
    },
    /// Owner only: replace what is given of a draft.
    BugReportEdit {
        report: i64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        steps: Option<String>,
    },
    /// Owner only: the owner sent the draft from their browser or their own `gh`;
    /// `issue_url` when the send returned the issue.
    BugReportSent {
        report: i64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        issue_url: Option<String>,
    },
    /// Owner only: drop the draft and its support bundle.
    BugReportDiscard {
        report: i64,
    },
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
    /// Owner only: run the project's declared check `check` in its checkout `checkout` (the
    /// project's root when absent) and record its receipt once it ends. Answers once the run
    /// started; `check-receipt` reads the result.
    CheckRun {
        project: String,
        check: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        checkout: Option<String>,
    },
    /// Owner only: the receipt of the run `check-run` started, `None` until it ended.
    CheckReceipt {
        project: String,
        run: String,
    },
    /// Where the project's declared checks stand on the checkout's current result. The owner
    /// names the project (and a checkout, its root when absent); an agent's is the checkout
    /// it works in.
    CheckGate {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        launch_token: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        project: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        checkout: Option<String>,
    },
    /// Agent socket only: the sender says `check` passed on its checkout's current result.
    /// Recorded as a claim, never as evidence: only a run the daemon executed is.
    CheckClaim {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        launch_token: Option<String>,
        check: String,
        statement: String,
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
    /// Owner only: why each open item waits, as the last scheduling pass explained it, in
    /// dispatch order; with `item`, that item's explanation alone.
    QueueExplain {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        item: Option<i64>,
    },
    /// Owner only: run a scheduling pass now instead of waiting for the next one.
    QueueDispatch,
    /// Show the session in the manager's viewer pane, waking it first when it is asleep.
    SessionShow {
        session: String,
    },
    /// Owner only: how the session's latest launch ran: inside which sandbox, with which
    /// rules, or unsandboxed and why (#61).
    SessionSandbox {
        session: String,
    },
    /// Owner only: the sandbox a new session of `agent` (the default provider without one)
    /// in `dir` (absolute) would get, without launching anything (#61). A profile this host
    /// cannot enforce is refused with the rule and the reason.
    SandboxShow {
        dir: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent: Option<String>,
    },
    /// The providers `session-new` may name in `agent`: those the daemon was started with.
    ProviderList,
    /// Owner only: the readiness report (#50): this host's requirements and, with `project`
    /// (an absolute directory), that project's, each with its status and next action.
    Readiness {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        project: Option<String>,
    },
    /// Owner only: the [`crate::DESKTOP_SESSION_VARIABLES`] later launches get, replacing
    /// the ones the daemon had (#307). Only the set ones travel; a name missing here is
    /// unset in later agents. Running agents keep theirs.
    DesktopSessionImport {
        variables: std::collections::BTreeMap<String, String>,
    },
    /// Owner only: re-read the owner's configuration file and apply its session settings
    /// (`agents.desktop_session`, `sessions.idle_sleep_after_minutes`,
    /// `sessions.asleep_expiry_days`) to later launches and sessions passes (#354). An
    /// invalid file is refused and nothing changes. Start files still count from the next
    /// daemon start.
    ConfigReload,
    /// Both sockets: the instruction bundle's topics, each with when to read it (#49).
    InstructionCatalog,
    /// Both sockets: one topic of the instruction bundle, or every topic at once (the one-read
    /// manual `cockpit --skill` prints) without `topic`.
    InstructionsRead {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        topic: Option<String>,
    },
    /// Agent socket only: the sender's launch brief, assembled from what its owners say now.
    AgentBrief {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        launch_token: Option<String>,
    },
    /// Open the manager tmux session, or find the one already open. `column` (the TUI) runs
    /// in its left pane and `viewer` in the pane beside it until a session is shown there,
    /// both from `dir` (absolute). Words are never parsed by a shell.
    ManagerOpen {
        dir: String,
        column: Vec<String>,
        viewer: Vec<String>,
    },
    /// Give the keys to a pane of the manager: the column, where the TUI shows what waits on
    /// the owner (#317), or back to the viewer the column took them from (#345). The TUI
    /// does not run tmux, the daemon does. Without `pane`, as a TUI before #345 sends it,
    /// the column.
    ManagerFocus {
        #[serde(default = "ManagerPaneView::column")]
        pane: ManagerPaneView,
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
    /// The report as recorded, under the session the daemon identified; boxed like the
    /// views below, to keep every `Outcome` small.
    AgentReported {
        report: Box<SessionReport>,
    },
    AgentReports {
        reports: Vec<SessionReport>,
    },
    /// The sessions a coordinator observes, oldest first.
    AgentSessions {
        sessions: Vec<ObservedSessionView>,
    },
    /// The message waits for `session`'s next prompt; `message` is its audit entry.
    AgentMessageQueued {
        session: String,
        message: i64,
    },
    /// A start without confirmation: the session that began.
    AgentStarted {
        session: Box<SessionView>,
    },
    /// A session's coordinator grants and the newest audit entries about it, newest first.
    CoordinatorGrants {
        session: String,
        grants: Vec<CoordinatorGrantView>,
        audit: Vec<CoordinatorAuditView>,
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
    /// The daemon's execution running the check.
    CheckRunStarted {
        run: String,
    },
    CheckReceipt {
        receipt: Option<Box<CheckReceiptView>>,
    },
    CheckClaimed {
        claim: Box<CheckClaimView>,
    },
    CheckGate {
        gate: Box<GateView>,
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
    /// In dispatch order: the queue first, then the items waiting outside it.
    QueueExplanations {
        explanations: Vec<QueueExplanationView>,
    },
    /// What the pass placed and the placements it ended; `waiting` counts the items it
    /// explained.
    QueueDispatched {
        placed: Vec<QueuePlacementView>,
        ended: Vec<i64>,
        waiting: u64,
    },
    Providers {
        providers: Vec<ProviderView>,
    },
    Readiness {
        requirements: Vec<RequirementView>,
    },
    /// What `agent-report-bug` did, and the report it filed into.
    BugReportFiled {
        report: i64,
        filed: BugFiledView,
    },
    BugReports {
        reports: Vec<BugReportView>,
    },
    BugReport {
        report: Box<BugReportView>,
    },
    /// `None`: the session has not launched an agent yet.
    SessionSandbox {
        evidence: Option<Box<SandboxEvidenceView>>,
    },
    /// What a launch would get, and the words it would run, with `<agent>` standing for the
    /// provider's own program and arguments.
    SandboxProfile {
        evidence: Box<SandboxEvidenceView>,
        command: Vec<String>,
    },
    /// The import is recorded. `withheld`: the owner's `agents.desktop_session` keeps
    /// agents off the desktop session, so none of them gets it (#305).
    DesktopSessionImported {
        withheld: bool,
    },
    /// The owner settings the daemon applies now, each with the value it applied before.
    ConfigReloaded {
        settings: Vec<AppliedSettingView>,
    },
    /// The running release's instruction topics; `manifest` is its instruction-manifest id.
    InstructionCatalog {
        version: String,
        manifest: String,
        topics: Vec<InstructionTopicView>,
    },
    Instructions {
        version: String,
        manifest: String,
        text: String,
    },
    AgentBrief {
        brief: String,
        manifest: String,
        budget: ContextBudgetView,
    },
    /// The manager session: `attach` are the words that attach a terminal to it; `created`
    /// says whether this request opened it.
    Manager {
        attach: Vec<String>,
        created: bool,
    },
    /// The pane `manager-focus` named has the keys; `took_from_viewer` says whether the
    /// viewer had them just before.
    ManagerFocused {
        took_from_viewer: bool,
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
    decode_request_with_kind(frame).map(|(request, _)| request)
}

/// [`decode_request`] with the command's `kind` as the frame names it (`todo-add`), read in
/// the same pass, so the daemon can name a request in its diagnostics without re-reading it.
pub fn decode_request_with_kind(frame: &str) -> Result<(Request, String), RequestRejection> {
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
        // Serde also decodes a tagged enum from a sequence (`["ping"]`); only the object form
        // is the protocol, and only it names its kind.
        Ok(request) => match value["command"]["kind"].as_str() {
            Some(kind) => Ok((request, kind.to_owned())),
            None => Err(rejection(
                id,
                ErrorCode::InvalidRequest,
                "the command is not an object with a `kind`".to_owned(),
            )),
        },
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

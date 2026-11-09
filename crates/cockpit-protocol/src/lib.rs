#![doc = include_str!("../README.md")]

mod accounts;
mod agent;
mod approvals;
mod bug_reports;
mod change_feed;
mod coordination;
mod credentials;
mod error;
mod handshake;
mod instructions;
mod network;
mod projects;
mod queue;
mod readiness;
mod request;
mod resources;
mod sessions;
mod storage;
mod tasks;
mod usage;
mod verification;
mod versions;
mod workflows;

pub use accounts::LoginGrantView;
pub use agent::{
    AGENT_SOCKET_VARIABLE, AgentReport, AgentState, CompactionPhase, CompactionReport,
    CompactionStatus, CompactionTrigger, CompactionView, ContextUsage, LAUNCH_TOKEN_VARIABLE,
    QuestionSource, SessionReport, TurnOutcome,
};
pub use approvals::{
    DecidedThroughView, FoldedView, GuideStateView, GuideStepView, GuideView, HiddenView,
    RunDecisionView, RunRequestView, RunResultView, RunStateView, ShownText,
};
pub use bug_reports::{
    BugBundleView, BugFiledView, BugIssueView, BugKindView, BugOriginView, BugReportView,
    BugStatusView,
};
pub use change_feed::{
    Change, ChangeBatch, ChangeEvent, CommitRevision, Cursor, DatabaseEpoch, DiskView, EntityKind,
    EntityRef, EntityRevision, FeedPosition, ViewArea, ViewStamp,
};
pub use coordination::{
    CoordinatorAuditView, CoordinatorGrantView, HeldMessageView, InboundMessageView, InboxView,
    ObservedSessionView,
};
pub use credentials::{
    CredentialFileView, CredentialStateView, CredentialTarget, CredentialView, SecretText,
};
pub use error::{ErrorCode, ProtocolError, Unblock, UnblockedBy};
pub use handshake::{
    Capability, HandshakeReply, Hello, ProtocolVersion, Refusal, RefusalReason, SUPPORTED_VERSIONS,
    Welcome, decode_hello, negotiate,
};
pub use instructions::{
    ContextBudgetView, InstalledSkillView, InstructionTopicView, MCP_INSTRUCTIONS,
    PackDeliveryView, PackSkillStateView, PackSkillView, SkillPackView,
};
pub use network::{
    NetworkDecisionView, NetworkGrantView, NetworkRequestStateView, NetworkRequestView,
    NetworkSubjectView, NetworkTargetKindView,
};
pub use projects::{
    CheckoutKindView, CheckoutView, GithubTokenView, KeptCheckout, KeptReason, ProjectView,
    TrustView,
};
pub use queue::{QueueExplanationView, QueuePlacementView, QueueSessionNoteView, QueueStepView};
pub use readiness::{ReadinessStatusView, RequirementView};
pub use request::{
    Command, Outcome, Reply, Request, RequestId, RequestRejection, Response, ServerFrame,
    decode_request, decode_request_with_kind,
};
pub use resources::{
    ControlSupportView, DemandView, EffectiveView, LimitView, ObservedView, ProfileView,
    ReservationView, ResourceStatusView, VerdictView,
};
pub use sessions::{
    AgentExitView, AppliedSettingView, DESKTOP_SESSION_VARIABLES, ExitStatusView, LaunchKindView,
    LaunchView, ManagerPaneView, ProviderView, QuotaStopCauseView, QuotaStopStatusView,
    QuotaStopView, SandboxEnforcementView, SandboxEvidenceView, SandboxRuleView, SessionFailure,
    SessionStateView, SessionView, StopTargetView,
};
pub use storage::{
    BackupView, CloneStorageView, DatabaseStorageView, EndSweepView, LogFolderStorageView,
    OutputRetentionView, PayloadRetentionView, PruneVerdictView, SessionTempStorageView,
    StorageAreasView, StoragePrunePlanView, StoragePrunePreviewView, StoragePruneStatusView,
    TargetSizeView,
};
pub use tasks::{
    HexBytes, TaskBudgetSpentView, TaskBudgetView, TaskDraftTargetView, TaskDraftView,
    TaskImageView, TaskKindView, TaskNextView, TaskStatusView, TaskView, TaskWaitView,
};
pub use usage::{
    CostArchiveView, CostRowView, DayDigestView, DigestView, IncidentView, ProjectDayView,
    QuotaAccountView, QuotaAlertView, QuotaFailureView, QuotaPaceView, QuotaWindowStateView,
    QuotaWindowView,
};
pub use verification::{
    CheckArtifactView, CheckClaimView, CheckOutcomeView, CheckReceiptView, CheckStatusView,
    GateCheckView, GateOutcomeView, GateView, PendingCheckView, StaleReasonView,
};
pub use versions::{LaunchBundleView, SchemaOwnerView, SessionBundleView, VersionReportView};
pub use workflows::{
    WorkflowBlockedActionView, WorkflowBlockedView, WorkflowCheckpointView, WorkflowGoalView,
    WorkflowHandoffRecordView, WorkflowHandoffView, WorkflowNextView, WorkflowPacketView,
    WorkflowPresetStepView, WorkflowPresetView, WorkflowStatusView, WorkflowStepStatusView,
    WorkflowStepView, WorkflowView,
};

/// Longest frame either side may send, newline included. Frames are one JSON value per line,
/// UTF-8, ended by `\n`. Large enough for a snapshot of a personal installation's state,
/// small enough that a runaway peer cannot exhaust memory.
pub const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

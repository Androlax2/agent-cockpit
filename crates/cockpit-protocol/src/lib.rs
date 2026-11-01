#![doc = include_str!("../README.md")]

mod agent;
mod approvals;
mod bug_reports;
mod change_feed;
mod coordination;
mod error;
mod handshake;
mod instructions;
mod projects;
mod queue;
mod readiness;
mod request;
mod resources;
mod sessions;
mod tasks;
mod verification;
mod workflows;

pub use agent::{
    AGENT_SOCKET_VARIABLE, AgentReport, AgentState, LAUNCH_TOKEN_VARIABLE, QuestionSource,
    SessionReport, TurnOutcome,
};
pub use approvals::{
    FoldedView, GuideStateView, GuideStepView, GuideView, HiddenView, RunRequestView,
    RunResultView, RunStateView, ShownText,
};
pub use bug_reports::{
    BugBundleView, BugFiledView, BugIssueView, BugKindView, BugOriginView, BugReportView,
    BugStatusView,
};
pub use change_feed::{
    Change, ChangeBatch, ChangeEvent, CommitRevision, Cursor, DatabaseEpoch, EntityKind, EntityRef,
    EntityRevision, FeedPosition,
};
pub use coordination::{
    CoordinatorAuditView, CoordinatorGrantView, InboundMessageView, ObservedSessionView,
};
pub use error::{ErrorCode, ProtocolError, Unblock, UnblockedBy};
pub use handshake::{
    Capability, HandshakeReply, Hello, ProtocolVersion, Refusal, RefusalReason, SUPPORTED_VERSIONS,
    Welcome, decode_hello, negotiate,
};
pub use instructions::{ContextBudgetView, InstructionTopicView};
pub use projects::{
    CheckoutKindView, CheckoutView, KeptCheckout, KeptReason, ProjectView, TrustView,
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
    AppliedSettingView, DESKTOP_SESSION_VARIABLES, LaunchKindView, LaunchView, ManagerPaneView,
    ProviderView, SandboxEnforcementView, SandboxEvidenceView, SandboxRuleView, SessionFailure,
    SessionStateView, SessionView, StopTargetView,
};
pub use tasks::{
    HexBytes, TaskDraftTargetView, TaskDraftView, TaskImageView, TaskKindView, TaskNextView,
    TaskStatusView, TaskView, TaskWaitView,
};
pub use verification::{
    CheckArtifactView, CheckClaimView, CheckOutcomeView, CheckReceiptView, CheckStatusView,
    GateCheckView, GateOutcomeView, GateView, StaleReasonView,
};
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

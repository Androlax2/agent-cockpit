#![doc = include_str!("../README.md")]

mod agent;
mod approvals;
mod change_feed;
mod error;
mod handshake;
mod projects;
mod request;
mod resources;
mod sessions;
mod tasks;

pub use agent::{
    AGENT_SOCKET_VARIABLE, AgentReport, AgentState, LAUNCH_TOKEN_VARIABLE, SessionReport,
    TurnOutcome,
};
pub use approvals::{
    FoldedView, GuideStateView, GuideStepView, GuideView, HiddenView, RunRequestView,
    RunResultView, RunStateView, ShownText,
};
pub use change_feed::{
    Change, ChangeBatch, ChangeEvent, CommitRevision, Cursor, DatabaseEpoch, EntityKind, EntityRef,
    EntityRevision, FeedPosition,
};
pub use error::{ErrorCode, ProtocolError, Unblock, UnblockedBy};
pub use handshake::{
    Capability, HandshakeReply, Hello, ProtocolVersion, Refusal, RefusalReason, SUPPORTED_VERSIONS,
    Welcome, decode_hello, negotiate,
};
pub use projects::{
    CheckoutKindView, CheckoutView, KeptCheckout, KeptReason, ProjectView, TrustView,
};
pub use request::{
    Command, Outcome, Reply, Request, RequestId, RequestRejection, Response, ServerFrame,
    decode_request,
};
pub use resources::{
    ControlSupportView, DemandView, EffectiveView, LimitView, ObservedView, ProfileView,
    ReservationView, ResourceStatusView, VerdictView,
};
pub use sessions::{
    LaunchKindView, LaunchView, ProviderView, SessionFailure, SessionStateView, SessionView,
    StopTargetView,
};
pub use tasks::{
    HexBytes, TaskDraftTargetView, TaskDraftView, TaskImageView, TaskKindView, TaskNextView,
    TaskStatusView, TaskView, TaskWaitView,
};

/// Longest frame either side may send, newline included. Frames are one JSON value per line,
/// UTF-8, ended by `\n`. Large enough for a snapshot of a personal installation's state,
/// small enough that a runaway peer cannot exhaust memory.
pub const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

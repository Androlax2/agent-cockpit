#![doc = include_str!("../README.md")]

mod compactions;
mod conversations;
mod declarations;
mod delivery_context;
mod error;
mod exits;
mod home_observer;
mod lifecycle;
mod model;
mod questions;
mod quota_stops;
mod reports;
mod retention;
mod store;

pub use cockpit_config::DesktopSession;
pub use compactions::{Compaction, CompactionStatus, latest_compaction};
pub use declarations::{RefusedDeclaration, refused_declarations};
pub use delivery_context::{ConnectSessionSocket, DeliveryContext};
pub use error::Error;
pub use home_observer::{HomeChange, HomeObserver};
pub use lifecycle::visible_line;
pub use lifecycle::{Sessions, SignInPlan};
pub use model::{
    Addressed, AgentExit, Ender, ExitStatus, Launch, LaunchBundle, LaunchKind, RestartAll,
    Restored, Session, SessionBundle, SessionId, SessionState, Settings, SleepRefusal,
    StartRequest, StopTarget, TickReport, WakeCause,
};
pub use questions::{
    QUESTION_HOLD_SECONDS, answer_question, coordinator_scope, escalate_question, question_holder,
};
pub use quota_stops::{
    QuotaStop, QuotaStopResolution, StopCause, TRANSIENT_LIMIT_FAILURES,
    TRANSIENT_LIMIT_WINDOW_SECONDS, TransientLimits, claim_quota_resume, confirm_quota_stop,
    drop_quota_stop, hand_quota_stop_to_owner, is_network_stopped, latest_quota_stops,
    open_quota_stops, release_quota_resume,
};
pub use reports::{
    AgentState, QuestionSource, Recorded, Report, SessionReport, current_context, latest_report,
    latest_reports, report, report_counting_limits,
};
pub use store::{MIGRATIONS, checkout_sessions, maintenance_sessions, session, sessions};

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
compile_error!("cockpit-sessions supports Linux (WSL2 included) and macOS (report 01)");

pub use retention::{
    LaunchPayload, RetirablePayload, retirable_launch_payloads, retirable_sessions,
    retire_launch_payload,
};
pub use store::protects_output;

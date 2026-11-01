#![doc = include_str!("../README.md")]

mod conversations;
mod error;
mod lifecycle;
mod model;
mod reports;
mod store;

pub use cockpit_config::DesktopSession;
pub use error::Error;
pub use lifecycle::Sessions;
pub use model::{
    Launch, LaunchKind, RestartAll, Restored, Session, SessionId, SessionState, Settings,
    SleepRefusal, StartRequest, StopTarget, TickReport, WakeCause,
};
pub use reports::{AgentState, QuestionSource, Report, SessionReport, latest_reports, report};
pub use store::MIGRATIONS;

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
compile_error!("cockpit-sessions supports Linux (WSL2 included) and macOS (report 01)");

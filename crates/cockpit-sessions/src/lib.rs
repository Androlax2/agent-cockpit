#![doc = include_str!("../README.md")]

mod error;
mod lifecycle;
mod model;
mod store;

pub use error::Error;
pub use lifecycle::Sessions;
pub use model::{
    Launch, LaunchKind, Restored, Session, SessionId, SessionState, Settings, SleepRefusal,
    StartRequest, StopTarget, TickReport,
};
pub use store::MIGRATIONS;

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
compile_error!("cockpit-sessions supports Linux (WSL2 included) and macOS (report 01)");

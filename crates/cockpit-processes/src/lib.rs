#![doc = include_str!("../README.md")]

mod error;
mod model;
mod output;
mod platform;
mod session;
mod store;
mod supervisor;
mod systemd;

pub use error::Error;
pub use model::{
    Containment, EndReason, Escalation, Execution, ExecutionId, HostedLaunch, Identity, LaunchSpec,
    Phase, Reconciled, Signal, Terminal, Termination,
};
pub use output::{OutputKind, OutputLimits, OutputRead, OutputStream};
pub use session::run_session_leader;
pub use store::{MIGRATIONS, execution, occupying_executions};
pub use supervisor::{LaunchContainment, Launched, Supervisor};
pub use systemd::{SystemdTools, find_in_path};

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
compile_error!("cockpit-processes supports Linux (WSL2 included) and macOS (report 01)");

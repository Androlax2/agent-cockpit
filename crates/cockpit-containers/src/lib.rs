#![doc = include_str!("../README.md")]

mod engine;
mod identity;
mod info;
mod setup;
#[cfg(feature = "persistence")]
mod store;

pub use engine::{
    Account, EngineUser, PASSWD_FILE, UNIT_TEMPLATE, account_named, account_with_uid,
};
pub use identity::{
    Answer, EngineState, Expected, RecordedEngine, Refusal, Remedy, SOCKET_DEADLINE, ask_socket,
    verify,
};
pub use info::{DOCKER_DEADLINE, EngineInfo, probe_docker};
pub use setup::{SetupPlan, setup_script, write_setup_script};
#[cfg(feature = "persistence")]
pub use store::{MIGRATIONS, record_engine, recorded_engine};

#![doc = include_str!("../README.md")]

mod engine;
mod identity;
mod info;
mod masks;
mod masks_install;
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
pub use masks::{
    ENGINES_DIR, MASKS_DEADLINE, MASKS_SOCKET, MASKS_SOCKET_UNIT, MasksHelper, MasksUnready,
};
pub use masks_install::{
    MASKS_SERVICE_UNIT, MasksInstall, PACKAGED_HELPER, PACKAGED_UNIT_DIR, SET_UP_HELPER,
    SET_UP_UNIT_DIR, masks_install, masks_removal_script, masks_setup_script, masks_units,
};
pub use setup::{SetupPlan, setup_script, write_setup_script};
#[cfg(feature = "persistence")]
pub use store::{MIGRATIONS, record_engine, recorded_engine};

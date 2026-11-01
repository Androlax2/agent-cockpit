//! The profile this `cockpit` serves (#196): `--profile NAME` before the verb, else
//! `$COCKPIT_PROFILE`, else the command's name (`cockpit-next`), else its build's own.
//! Selected once in `main`, before any verb runs, and read by every verb that places a file or
//! reaches the daemon, so no verb can resolve another profile than the one `doctor` shows.

use std::ffi::OsStr;
use std::path::PathBuf;
use std::sync::OnceLock;

use cockpit_config::{Build, Environment, Profile, ProfileChoice, Sourced};

static ACTIVE: OnceLock<Selected> = OnceLock::new();

struct Selected {
    profile: Sourced<Profile>,
    build: Build,
}

/// Resolves the profile for this process; `command_name` is `argv[0]`.
pub(crate) fn select(flag: Option<&str>, command_name: Option<&OsStr>) -> Result<(), String> {
    let program = this_program()?;
    let build = Build::of_program(&program);
    let variable = std::env::var_os(cockpit_config::PROFILE_VARIABLE);
    let choice = ProfileChoice {
        flag,
        variable: variable.as_deref(),
        command_name,
    };
    let profile =
        cockpit_config::resolve_profile(choice, build).map_err(|error| error.to_string())?;
    ACTIVE
        .set(Selected { profile, build })
        .map_err(|_| "the profile is selected once".to_owned())
}

fn selected() -> &'static Selected {
    ACTIVE
        .get()
        .expect("main selects the profile before any verb runs")
}

pub(crate) fn active() -> &'static Sourced<Profile> {
    &selected().profile
}

pub(crate) fn build() -> Build {
    selected().build
}

/// The process environment with the active profile: what every verb resolves paths from.
pub(crate) fn environment() -> Environment {
    Environment::from_process(active().clone())
}

/// This program's own path.
pub(crate) fn this_program() -> Result<PathBuf, String> {
    std::env::current_exe().map_err(|error| format!("cannot find this program's path: {error}"))
}

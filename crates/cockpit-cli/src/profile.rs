//! The profile this `cockpit` serves (#196): `--profile NAME` before the verb, else the
//! command's name (`torlek-next`), else `$TORLEK_PROFILE`, else its build's own.
//! Selected once in `main`, before any verb runs, and read by every verb that places a file or
//! reaches the daemon, so no verb can resolve another profile than the one `doctor` shows.

use std::ffi::OsStr;
use std::path::PathBuf;
use std::sync::OnceLock;

use cockpit_config::{Build, Environment, Profile, ProfileChoice, Source, Sourced};
use cockpit_protocol::RequirementView;

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

/// The command an owner types to reach the active profile, for every hint a verb prints:
/// `torlek-next` when that name selected it, bare `torlek` when nothing selected a profile
/// (the build's own is what bare `torlek` reaches), else `torlek --profile NAME`. A hint that
/// said `torlek` under profile `next` would act on the default installation instead.
pub(crate) fn command() -> String {
    command_for(active())
}

/// The command that starts the active profile's daemon.
pub(crate) fn daemon_command() -> String {
    daemon_command_for(active())
}

fn command_for(profile: &Sourced<Profile>) -> String {
    match profile.source {
        Source::CommandName => profile.value.qualified_name(),
        Source::ProductDefault | Source::DevelopmentBuild { .. } => "torlek".to_owned(),
        _ => format!("torlek --profile {}", profile.value),
    }
}

fn daemon_command_for(profile: &Sourced<Profile>) -> String {
    match profile.source {
        Source::ProductDefault | Source::DevelopmentBuild { .. } => "torlek-daemon".to_owned(),
        _ => format!("torlek-daemon --profile {}", profile.value),
    }
}

/// `text` with the commands the daemon wrote between backticks (`torlek doctor`,
/// `torlek-daemon`) renamed for the active profile. The daemon cannot name them itself: how
/// the owner reached the profile is the client's to know.
pub(crate) fn localize(text: &str) -> String {
    localize_for(active(), text)
}

fn localize_for(profile: &Sourced<Profile>, text: &str) -> String {
    let command = command_for(profile);
    text.replace(
        "`torlek-daemon`",
        &format!("`{}`", daemon_command_for(profile)),
    )
    .replace("`torlek ", &format!("`{command} "))
    .replace("`torlek`", &format!("`{command}`"))
}

/// A requirement the daemon reported, with its detail and its next step localized.
pub(crate) fn localize_requirement(requirement: RequirementView) -> RequirementView {
    RequirementView {
        detail: localize(&requirement.detail),
        next: requirement.next.as_deref().map(localize),
        ..requirement
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sourced(name: &str, source: Source) -> Sourced<Profile> {
        Sourced::new(Profile::parse(name).unwrap(), source)
    }

    #[test]
    fn a_profile_chosen_by_flag_or_variable_is_named_in_every_command() {
        for source in [
            Source::Flag("--profile"),
            Source::Environment("TORLEK_PROFILE"),
        ] {
            let next = sourced("next", source);

            assert_eq!(command_for(&next), "torlek --profile next");
            assert_eq!(daemon_command_for(&next), "torlek-daemon --profile next");
        }
    }

    #[test]
    fn a_profile_chosen_by_the_command_name_is_that_command() {
        let next = sourced("next", Source::CommandName);

        assert_eq!(command_for(&next), "torlek-next");
    }

    #[test]
    fn a_profile_nothing_chose_is_reached_by_the_bare_command() {
        for source in [
            Source::ProductDefault,
            Source::DevelopmentBuild { ignored: None },
        ] {
            let own = sourced("torlek", source);

            assert_eq!(command_for(&own), "torlek");
            assert_eq!(daemon_command_for(&own), "torlek-daemon");
        }
    }

    #[test]
    fn only_commands_between_backticks_are_renamed() {
        let next = sourced("next", Source::CommandName);

        assert_eq!(
            localize_for(
                &next,
                "start `torlek-daemon`, then `torlek doctor`; `torlek` is not torlek-daemon or `claude`"
            ),
            "start `torlek-daemon --profile next`, then `torlek-next doctor`; `torlek-next` is not torlek-daemon or `claude`"
        );
    }
}

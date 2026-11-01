//! Where Cockpit keeps its files: the XDG base directories, each with the reason it was chosen,
//! and under each the active profile's own directory (`cockpit` or `cockpit-<name>`).

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::{ConfigError, PROFILE_VARIABLE, Profile, Source, Sourced};

/// The process environment variables configuration reads, captured once at the boundary so
/// tests and the daemon pass an explicit environment instead of mutating the process one,
/// with the profile the program resolved ([`crate::resolve_profile`]).
#[derive(Clone, Debug)]
pub struct Environment {
    pub home: Option<OsString>,
    pub xdg_config_home: Option<OsString>,
    pub xdg_state_home: Option<OsString>,
    pub xdg_cache_home: Option<OsString>,
    pub xdg_runtime_dir: Option<OsString>,
    pub profile: Sourced<Profile>,
}

/// No variable set and the default profile: what the in-process tests start from.
impl Default for Environment {
    fn default() -> Self {
        Self {
            home: None,
            xdg_config_home: None,
            xdg_state_home: None,
            xdg_cache_home: None,
            xdg_runtime_dir: None,
            profile: Sourced::new(Profile::default_profile(), Source::ProductDefault),
        }
    }
}

impl Environment {
    pub fn from_process(profile: Sourced<Profile>) -> Self {
        Self {
            home: std::env::var_os("HOME"),
            xdg_config_home: std::env::var_os("XDG_CONFIG_HOME"),
            xdg_state_home: std::env::var_os("XDG_STATE_HOME"),
            xdg_cache_home: std::env::var_os("XDG_CACHE_HOME"),
            xdg_runtime_dir: std::env::var_os("XDG_RUNTIME_DIR"),
            profile,
        }
    }

    /// The variables that place Cockpit's directories: the XDG ones where set, and the
    /// profile always. A process Cockpit launches gets them, so its `cockpit` resolves this
    /// layout (#268) and reports to this profile's daemon (#196).
    pub fn layout_variables(&self) -> Vec<(&'static str, OsString)> {
        let mut variables: Vec<(&'static str, OsString)> = [
            ("XDG_CONFIG_HOME", &self.xdg_config_home),
            ("XDG_STATE_HOME", &self.xdg_state_home),
            ("XDG_CACHE_HOME", &self.xdg_cache_home),
            ("XDG_RUNTIME_DIR", &self.xdg_runtime_dir),
        ]
        .into_iter()
        .filter_map(|(name, value)| Some((name, value.clone()?)))
        .collect();
        variables.push((PROFILE_VARIABLE, self.profile.value.name().into()));
        variables
    }

    /// `$HOME`, which every default below `~` needs: unset or relative fails instead of
    /// guessing a directory.
    pub fn home_dir(&self) -> Result<PathBuf, ConfigError> {
        match &self.home {
            Some(home) if Path::new(home).is_absolute() => Ok(PathBuf::from(home)),
            Some(home) => Err(ConfigError::Home(format!(
                "HOME is not an absolute path: {}",
                Path::new(home).display()
            ))),
            None => Err(ConfigError::Home("HOME is not set".to_string())),
        }
    }
}

/// Cockpit's directories. `runtime_dir` is absent when `XDG_RUNTIME_DIR` is unset (macOS, some
/// containers): the XDG spec leaves the fallback to the program that needs the sockets.
#[derive(Clone, Debug)]
pub struct Layout {
    pub profile: Sourced<Profile>,
    pub config_dir: Sourced<PathBuf>,
    pub state_dir: Sourced<PathBuf>,
    pub cache_dir: Sourced<PathBuf>,
    pub runtime_dir: Sourced<Option<PathBuf>>,
}

impl Layout {
    /// `home` is the validated `$HOME` ([`Environment::home_dir`]).
    pub fn resolve(environment: &Environment, home: &Path) -> Self {
        let profile_dir = environment.profile.value.qualified_name();
        let profile_dir = profile_dir.as_str();
        Self {
            profile: environment.profile.clone(),
            config_dir: base_dir(
                profile_dir,
                "XDG_CONFIG_HOME",
                &environment.xdg_config_home,
                home.join(".config"),
            ),
            state_dir: base_dir(
                profile_dir,
                "XDG_STATE_HOME",
                &environment.xdg_state_home,
                home.join(".local/state"),
            ),
            cache_dir: base_dir(
                profile_dir,
                "XDG_CACHE_HOME",
                &environment.xdg_cache_home,
                home.join(".cache"),
            ),
            runtime_dir: match xdg_value(&environment.xdg_runtime_dir) {
                XdgValue::Absolute(dir) => Sourced::new(
                    Some(dir.join(profile_dir)),
                    Source::Environment("XDG_RUNTIME_DIR"),
                ),
                XdgValue::Unset => Sourced::new(None, Source::EnvironmentUnset("XDG_RUNTIME_DIR")),
                XdgValue::Ignored => {
                    Sourced::new(None, Source::InvalidEnvironment("XDG_RUNTIME_DIR"))
                }
            },
        }
    }

    pub fn user_config_file(&self) -> PathBuf {
        self.config_dir.value.join("config.toml")
    }

    /// Where the daemon's sockets and instance lock live: `$XDG_RUNTIME_DIR/<profile dir>`, or
    /// `<state dir>/run` without `XDG_RUNTIME_DIR` (macOS, some containers), a fallback the
    /// XDG spec leaves to the program. A relative `XDG_RUNTIME_DIR` is refused rather than
    /// replaced by the fallback, so the daemon and its clients never disagree on the place.
    pub fn socket_dir(&self) -> Result<PathBuf, ConfigError> {
        if let Source::InvalidEnvironment(_) = self.runtime_dir.source {
            return Err(ConfigError::Invalid {
                key: "paths.runtime_dir".to_owned(),
                source: self.runtime_dir.source.clone(),
                reason: "has no value to place the daemon's sockets in".to_owned(),
            });
        }
        Ok(self
            .runtime_dir
            .value
            .clone()
            .unwrap_or_else(|| self.state_dir.value.join("run")))
    }
}

/// The host-only socket's file in [`Layout::socket_dir`]: the owner's, never a sandbox's.
pub const DAEMON_SOCKET: &str = "daemon.sock";

/// The agent socket's directory in [`Layout::socket_dir`]. It is the socket's alone, so a
/// sandbox can be given that directory without the host-only socket beside it.
pub const AGENT_SOCKET_DIR: &str = "agent";

/// The agent socket's file in [`AGENT_SOCKET_DIR`].
pub const AGENT_SOCKET: &str = "agent.sock";

enum XdgValue {
    Absolute(PathBuf),
    Unset,
    Ignored,
}

/// The XDG spec: an empty variable counts as unset, and a relative one is invalid and ignored.
fn xdg_value(value: &Option<OsString>) -> XdgValue {
    match value {
        None => XdgValue::Unset,
        Some(value) if value.is_empty() => XdgValue::Unset,
        Some(value) if Path::new(value).is_absolute() => XdgValue::Absolute(PathBuf::from(value)),
        Some(_) => XdgValue::Ignored,
    }
}

fn base_dir(
    profile_dir: &str,
    variable: &'static str,
    value: &Option<OsString>,
    home_default: PathBuf,
) -> Sourced<PathBuf> {
    let default_dir = home_default.join(profile_dir);
    match xdg_value(value) {
        XdgValue::Absolute(dir) => {
            Sourced::new(dir.join(profile_dir), Source::Environment(variable))
        }
        XdgValue::Unset => Sourced::new(default_dir, Source::ProductDefault),
        XdgValue::Ignored => Sourced::new(default_dir, Source::IgnoredEnvironment(variable)),
    }
}

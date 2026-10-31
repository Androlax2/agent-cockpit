//! Where Cockpit keeps its files: the XDG base directories, each with the reason it was chosen.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::{ConfigError, Source, Sourced};

/// The process environment variables configuration reads, captured once at the boundary so
/// tests and the daemon pass an explicit environment instead of mutating the process one.
#[derive(Clone, Debug, Default)]
pub struct Environment {
    pub home: Option<OsString>,
    pub xdg_config_home: Option<OsString>,
    pub xdg_state_home: Option<OsString>,
    pub xdg_cache_home: Option<OsString>,
    pub xdg_runtime_dir: Option<OsString>,
}

impl Environment {
    pub fn from_process() -> Self {
        Self {
            home: std::env::var_os("HOME"),
            xdg_config_home: std::env::var_os("XDG_CONFIG_HOME"),
            xdg_state_home: std::env::var_os("XDG_STATE_HOME"),
            xdg_cache_home: std::env::var_os("XDG_CACHE_HOME"),
            xdg_runtime_dir: std::env::var_os("XDG_RUNTIME_DIR"),
        }
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
    pub config_dir: Sourced<PathBuf>,
    pub state_dir: Sourced<PathBuf>,
    pub cache_dir: Sourced<PathBuf>,
    pub runtime_dir: Sourced<Option<PathBuf>>,
}

impl Layout {
    /// `home` is the validated `$HOME` ([`Environment::home_dir`]).
    pub fn resolve(environment: &Environment, home: &Path) -> Self {
        Self {
            config_dir: base_dir(
                "XDG_CONFIG_HOME",
                &environment.xdg_config_home,
                home.join(".config"),
            ),
            state_dir: base_dir(
                "XDG_STATE_HOME",
                &environment.xdg_state_home,
                home.join(".local/state"),
            ),
            cache_dir: base_dir(
                "XDG_CACHE_HOME",
                &environment.xdg_cache_home,
                home.join(".cache"),
            ),
            runtime_dir: match xdg_value(&environment.xdg_runtime_dir) {
                XdgValue::Absolute(dir) => Sourced::new(
                    Some(dir.join("cockpit")),
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

    /// Where the daemon's sockets and instance lock live: `$XDG_RUNTIME_DIR/cockpit`, or
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
    variable: &'static str,
    value: &Option<OsString>,
    home_default: PathBuf,
) -> Sourced<PathBuf> {
    let default_dir = home_default.join("cockpit");
    match xdg_value(value) {
        XdgValue::Absolute(dir) => Sourced::new(dir.join("cockpit"), Source::Environment(variable)),
        XdgValue::Unset => Sourced::new(default_dir, Source::ProductDefault),
        XdgValue::Ignored => Sourced::new(default_dir, Source::IgnoredEnvironment(variable)),
    }
}

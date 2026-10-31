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
    pub fn resolve(environment: &Environment) -> Result<Self, ConfigError> {
        let home = environment.home_dir()?;
        Ok(Self {
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
            runtime_dir: match xdg_value("XDG_RUNTIME_DIR", &environment.xdg_runtime_dir) {
                XdgValue::Absolute(dir) => Sourced::new(
                    Some(dir.join("cockpit")),
                    Source::Environment("XDG_RUNTIME_DIR"),
                ),
                XdgValue::Unset => Sourced::new(None, Source::EnvironmentUnset("XDG_RUNTIME_DIR")),
                XdgValue::Ignored(variable) => {
                    Sourced::new(None, Source::IgnoredEnvironment(variable))
                }
            },
        })
    }

    pub fn user_config_file(&self) -> PathBuf {
        self.config_dir.value.join("config.toml")
    }
}

enum XdgValue {
    Absolute(PathBuf),
    Unset,
    Ignored(&'static str),
}

/// The XDG spec: an empty variable counts as unset, and a relative one is invalid and ignored.
fn xdg_value(variable: &'static str, value: &Option<OsString>) -> XdgValue {
    match value {
        None => XdgValue::Unset,
        Some(value) if value.is_empty() => XdgValue::Unset,
        Some(value) if Path::new(value).is_absolute() => XdgValue::Absolute(PathBuf::from(value)),
        Some(_) => XdgValue::Ignored(variable),
    }
}

fn base_dir(
    variable: &'static str,
    value: &Option<OsString>,
    home_default: PathBuf,
) -> Sourced<PathBuf> {
    match xdg_value(variable, value) {
        XdgValue::Absolute(dir) => Sourced::new(dir.join("cockpit"), Source::Environment(variable)),
        XdgValue::Unset => Sourced::new(home_default.join("cockpit"), Source::ProductDefault),
        XdgValue::Ignored(variable) => Sourced::new(
            home_default.join("cockpit"),
            Source::IgnoredEnvironment(variable),
        ),
    }
}

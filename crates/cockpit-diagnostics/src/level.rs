use std::fmt;
use std::str::FromStr;
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::thread;
use std::time::{Duration, SystemTime};

use tracing::Dispatch;
use tracing_subscriber::{EnvFilter, Registry, reload};

use crate::Error;

/// A dedicated variable, so Cockpit never inherits an unrelated `RUST_LOG`.
pub const LOG_LEVEL_VAR: &str = "COCKPIT_LOG_LEVEL";
pub const DEFAULT_OVERRIDE: Duration = Duration::from_secs(5 * 60);
pub const MAX_OVERRIDE: Duration = Duration::from_secs(15 * 60);

const LEVELS: [&str; 6] = ["off", "error", "warn", "info", "debug", "trace"];

/// A level (`debug`) or per-component directives (`scheduling=debug,tmux=trace`).
///
/// A component names a Cockpit crate without its prefix: `tmux` filters `cockpit_tmux`.
/// Anything else, such as a bare word `EnvFilter` would read as a target, is refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Directives {
    input: String,
    filter: String,
}

impl Directives {
    /// Reads [`LOG_LEVEL_VAR`]; unset means `info`, an unreadable value is an error.
    pub fn from_env() -> Result<Self, Error> {
        match std::env::var(LOG_LEVEL_VAR) {
            Ok(value) => Self::from_env_value(Some(&value)),
            Err(std::env::VarError::NotPresent) => Self::from_env_value(None),
            Err(std::env::VarError::NotUnicode(_)) => Err(Error::InvalidDirectives {
                input: String::from("<not unicode>"),
                reason: format!("{LOG_LEVEL_VAR} is not valid UTF-8"),
            }),
        }
    }

    pub fn from_env_value(value: Option<&str>) -> Result<Self, Error> {
        value.unwrap_or("info").parse()
    }

    pub(crate) fn env_filter(&self) -> EnvFilter {
        EnvFilter::builder()
            .parse(&self.filter)
            .expect("directives are validated when parsed")
    }
}

impl FromStr for Directives {
    type Err = Error;

    fn from_str(input: &str) -> Result<Self, Error> {
        let invalid = |reason: String| Error::InvalidDirectives {
            input: input.to_owned(),
            reason,
        };
        let mut filter_parts = Vec::new();
        for part in input.split(',') {
            let (component, level) = match part.split_once('=') {
                Some((component, level)) => (Some(component), level),
                None => (None, part),
            };
            if !LEVELS.contains(&level) {
                return Err(invalid(format!(
                    "{level:?} is not one of {}",
                    LEVELS.join(", ")
                )));
            }
            let Some(component) = component else {
                filter_parts.push(level.to_owned());
                continue;
            };
            let is_component = !component.is_empty()
                && component.chars().all(|character| {
                    character.is_ascii_alphanumeric() || "_-:".contains(character)
                });
            if !is_component {
                return Err(invalid(format!("{component:?} is not a component name")));
            }
            let target = component.replace('-', "_");
            let target = if target.starts_with("cockpit_") {
                target
            } else {
                format!("cockpit_{target}")
            };
            filter_parts.push(format!("{target}={level}"));
        }
        let filter = filter_parts.join(",");
        EnvFilter::builder()
            .parse(&filter)
            .map_err(|error| invalid(error.to_string()))?;
        Ok(Self {
            input: input.to_owned(),
            filter,
        })
    }
}

impl fmt::Display for Directives {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.input)
    }
}

/// A requested, time-boxed level change: `cockpit logs level <directives> [--for <duration>]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LevelOverride {
    directives: Directives,
    duration: Duration,
}

impl LevelOverride {
    pub fn new(directives: Directives, duration: Duration) -> Result<Self, Error> {
        if duration.is_zero() {
            return Err(Error::InvalidDuration {
                input: format!("{}ms", duration.as_millis()),
            });
        }
        if duration > MAX_OVERRIDE {
            return Err(Error::OverrideTooLong {
                requested: duration,
                max: MAX_OVERRIDE,
            });
        }
        Ok(Self {
            directives,
            duration,
        })
    }

    /// Reads the CLI's words: `duration` is `<n>s` or `<n>m`, [`DEFAULT_OVERRIDE`] when absent.
    pub fn parse(directives: &str, duration: Option<&str>) -> Result<Self, Error> {
        let duration = duration.map_or(Ok(DEFAULT_OVERRIDE), parse_duration)?;
        Self::new(directives.parse()?, duration)
    }

    pub fn duration(&self) -> Duration {
        self.duration
    }
}

fn parse_duration(input: &str) -> Result<Duration, Error> {
    let invalid = || Error::InvalidDuration {
        input: input.to_owned(),
    };
    let (count, seconds_per_unit) = if let Some(count) = input.strip_suffix('s') {
        (count, 1)
    } else if let Some(count) = input.strip_suffix('m') {
        (count, 60)
    } else {
        return Err(invalid());
    };
    if count.is_empty() || !count.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid());
    }
    let count: u64 = count.parse().map_err(|_| invalid())?;
    let seconds = count
        .checked_mul(seconds_per_unit)
        .filter(|seconds| *seconds > 0)
        .ok_or_else(invalid)?;
    Ok(Duration::from_secs(seconds))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveOverride {
    pub directives: Directives,
    pub expires_at: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LevelStatus {
    /// The directives in effect now.
    pub directives: Directives,
    pub active_override: Option<ActiveOverride>,
}

/// Changes the running filter. Overrides revert on their own when they expire, and a
/// restart always starts from the process's own directives: nothing is persisted.
#[derive(Debug, Clone)]
pub struct LevelControl {
    shared: Arc<Shared>,
}

#[derive(Debug)]
struct Shared {
    filter: reload::Handle<EnvFilter, Registry>,
    /// Level changes are logged through the same pipeline, from whichever thread makes them.
    dispatch: Dispatch,
    state: Mutex<State>,
}

#[derive(Debug)]
struct State {
    baseline: Directives,
    active_override: Option<ActiveOverride>,
    /// Lets an expiry timer tell whether its override was replaced or reverted meanwhile.
    generation: u64,
}

impl LevelControl {
    pub(crate) fn new(
        baseline: Directives,
        filter: reload::Handle<EnvFilter, Registry>,
        dispatch: Dispatch,
    ) -> Self {
        let state = State {
            baseline,
            active_override: None,
            generation: 0,
        };
        Self {
            shared: Arc::new(Shared {
                filter,
                dispatch,
                state: Mutex::new(state),
            }),
        }
    }

    /// Applies `level_override` until it expires, replacing any active override.
    pub fn set_override(&self, level_override: LevelOverride) -> Result<ActiveOverride, Error> {
        let mut state = self.shared.lock_state();
        self.shared.reload(&level_override.directives)?;
        state.generation += 1;
        let active = ActiveOverride {
            directives: level_override.directives,
            expires_at: SystemTime::now() + level_override.duration,
        };
        state.active_override = Some(active.clone());
        let generation = state.generation;
        drop(state);

        let shared = Arc::downgrade(&self.shared);
        thread::Builder::new()
            .name(String::from("cockpit-log-level-expiry"))
            .spawn(move || expire_after(shared, level_override.duration, generation))
            .map_err(|error| Error::Reload {
                reason: format!("cannot start the expiry timer: {error}"),
            })?;
        tracing::dispatcher::with_default(&self.shared.dispatch, || {
            tracing::info!(
                directives = %active.directives,
                expires_in_seconds = level_override.duration.as_secs(),
                "log level raised"
            );
        });
        Ok(active)
    }

    /// Returns to the starting directives now; a no-op without an active override.
    pub fn revert(&self) -> Result<(), Error> {
        self.shared.revert_if(|_| true)
    }

    pub fn status(&self) -> LevelStatus {
        let state = self.shared.lock_state();
        let directives = state
            .active_override
            .as_ref()
            .map_or(&state.baseline, |active| &active.directives);
        LevelStatus {
            directives: directives.clone(),
            active_override: state.active_override.clone(),
        }
    }
}

fn expire_after(shared: Weak<Shared>, duration: Duration, generation: u64) {
    thread::sleep(duration);
    let Some(shared) = shared.upgrade() else {
        return;
    };
    if let Err(error) = shared.revert_if(|state| state.generation == generation) {
        tracing::dispatcher::with_default(&shared.dispatch, || {
            tracing::error!(%error, "log level override did not revert");
        });
    }
}

impl Shared {
    fn lock_state(&self) -> MutexGuard<'_, State> {
        // The state stays consistent across a panic: every field is written after the reload.
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn reload(&self, directives: &Directives) -> Result<(), Error> {
        self.filter
            .reload(directives.env_filter())
            .map_err(|error| Error::Reload {
                reason: error.to_string(),
            })
    }

    fn revert_if(&self, should_revert: impl FnOnce(&State) -> bool) -> Result<(), Error> {
        let mut state = self.lock_state();
        let Some(active) = state.active_override.clone() else {
            return Ok(());
        };
        if !should_revert(&state) {
            return Ok(());
        }
        // Logged before the reload so the record passes the raised filter it describes.
        tracing::dispatcher::with_default(&self.dispatch, || {
            tracing::info!(from = %active.directives, to = %state.baseline, "log level reverted");
        });
        self.reload(&state.baseline)?;
        state.generation += 1;
        state.active_override = None;
        Ok(())
    }
}

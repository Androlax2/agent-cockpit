use std::fmt;
use std::io;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// `COCKPIT_LOG_LEVEL` or an override is not a level or `component=level` list.
    InvalidDirectives {
        input: String,
        reason: String,
    },
    /// An override duration is not `<n>s` or `<n>m` with `n > 0`.
    InvalidDuration {
        input: String,
    },
    /// Overrides are time-boxed; longer requests are refused, not shortened.
    OverrideTooLong {
        requested: Duration,
        max: Duration,
    },
    InvalidLimits {
        reason: &'static str,
    },
    /// A process, section or log directory name that is not one plain path component.
    InvalidName {
        kind: &'static str,
        name: String,
    },
    /// Another running process already writes this log directory.
    LogDirInUse {
        path: PathBuf,
    },
    Io {
        action: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    BundleTooLarge {
        bytes: u64,
        max: u64,
    },
    /// The subscriber that owned the level filter is gone.
    Reload {
        reason: String,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDirectives { input, reason } => {
                write!(formatter, "invalid log level {input:?}: {reason}")
            }
            Self::InvalidDuration { input } => {
                write!(
                    formatter,
                    "invalid duration {input:?}: expected seconds or minutes such as 90s or 15m"
                )
            }
            Self::OverrideTooLong { requested, max } => write!(
                formatter,
                "a log level override lasts at most {} minutes; {} seconds requested",
                max.as_secs() / 60,
                requested.as_secs()
            ),
            Self::InvalidLimits { reason } => {
                write!(formatter, "invalid log file limits: {reason}")
            }
            Self::InvalidName { kind, name } => write!(
                formatter,
                "invalid {kind} name {name:?}: must be a plain name"
            ),
            Self::LogDirInUse { path } => write!(
                formatter,
                "another process already logs to {}; give each process its own name",
                path.display()
            ),
            Self::Io {
                action,
                path,
                source,
            } => write!(formatter, "cannot {action} {}: {source}", path.display()),
            Self::BundleTooLarge { bytes, max } => write!(
                formatter,
                "support bundle would be {bytes} bytes, over its {max}-byte bound; narrow it with --since"
            ),
            Self::Reload { reason } => write!(formatter, "cannot change the log level: {reason}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

pub(crate) fn io_error(
    action: &'static str,
    path: impl Into<PathBuf>,
) -> impl FnOnce(io::Error) -> Error {
    move |source| Error::Io {
        action,
        path: path.into(),
        source,
    }
}

pub(crate) fn check_plain_name(kind: &'static str, name: &str) -> Result<(), Error> {
    let is_plain = !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\0']);
    if is_plain {
        Ok(())
    } else {
        Err(Error::InvalidName {
            kind,
            name: name.to_owned(),
        })
    }
}

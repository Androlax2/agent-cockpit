use std::fmt;
use std::io;
use std::path::PathBuf;

/// Why the daemon did not start. Each names the file or setting at fault.
#[derive(Debug)]
pub enum StartError {
    Config(cockpit_config::ConfigError),
    /// A directory the daemon keeps private (sockets, state) is not private to this user.
    PrivateDir {
        path: PathBuf,
        reason: String,
    },
    /// Another daemon of this user holds the instance lock.
    AlreadyRunning {
        lock_path: PathBuf,
    },
    Io {
        action: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    /// The configured [`crate::Limits`] would let the daemon serve nothing.
    InvalidLimits(&'static str),
    /// The Tokio runtime could not start.
    Runtime(io::Error),
    Diagnostics(cockpit_diagnostics::Error),
    Database(cockpit_turso::Error),
    /// Reconciling executions against the real processes failed; nothing was dispatched.
    Reconcile(cockpit_processes::Error),
}

impl StartError {
    pub(crate) fn io(
        action: &'static str,
        path: &std::path::Path,
    ) -> impl FnOnce(io::Error) -> Self {
        let path = path.to_path_buf();
        move |source| Self::Io {
            action,
            path,
            source,
        }
    }
}

impl fmt::Display for StartError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(error) => write!(formatter, "{error}"),
            Self::PrivateDir { path, reason } => {
                write!(formatter, "directory {}: {reason}", path.display())
            }
            Self::AlreadyRunning { lock_path } => write!(
                formatter,
                "another cockpit daemon of this user is already running (it holds {})",
                lock_path.display()
            ),
            Self::Io {
                action,
                path,
                source,
            } => write!(formatter, "cannot {action} {}: {source}", path.display()),
            Self::InvalidLimits(reason) => write!(formatter, "invalid limits: {reason}"),
            Self::Runtime(error) => write!(formatter, "cannot start the async runtime: {error}"),
            Self::Diagnostics(error) => write!(formatter, "{error}"),
            Self::Database(error) => write!(formatter, "state database: {error}"),
            Self::Reconcile(error) => write!(formatter, "reconciling executions: {error}"),
        }
    }
}

impl std::error::Error for StartError {}

impl From<cockpit_diagnostics::Error> for StartError {
    fn from(error: cockpit_diagnostics::Error) -> Self {
        Self::Diagnostics(error)
    }
}

impl From<cockpit_turso::Error> for StartError {
    fn from(error: cockpit_turso::Error) -> Self {
        Self::Database(error)
    }
}

impl From<cockpit_processes::Error> for StartError {
    fn from(error: cockpit_processes::Error) -> Self {
        Self::Reconcile(error)
    }
}

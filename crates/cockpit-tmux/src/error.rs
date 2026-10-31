use std::ffi::OsString;
use std::fmt;
use std::io;
use std::path::PathBuf;

use crate::version::MINIMUM_VERSION;

/// Why a tmux operation failed. Each variant names the command, file or value at fault.
#[derive(Debug)]
pub enum Error {
    /// The tmux binary could not be run at all (missing, not executable).
    TmuxNotRunnable { binary: PathBuf, source: io::Error },
    /// `tmux -V` printed something this crate cannot read as a version.
    UnrecognizedVersion { binary: PathBuf, printed: String },
    /// The installed tmux is older than [`MINIMUM_VERSION`].
    TmuxTooOld { binary: PathBuf, found: String },
    /// A tmux command exited unsuccessfully.
    CommandFailed {
        arguments: Vec<OsString>,
        status: Option<i32>,
        stderr: String,
    },
    /// A tmux command printed output this crate did not expect.
    UnexpectedOutput {
        arguments: Vec<OsString>,
        output: String,
    },
    /// A value given to this crate cannot be passed to tmux safely.
    InvalidArgument { what: &'static str, value: String },
    Io {
        action: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    /// The control-mode client ended; no further event will arrive.
    ObserverClosed,
}

impl Error {
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

fn joined(arguments: &[OsString]) -> String {
    arguments
        .iter()
        .map(|argument| argument.to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ")
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TmuxNotRunnable { binary, source } => write!(
                formatter,
                "tmux is required but `{}` could not be run: {source}",
                binary.display()
            ),
            Self::UnrecognizedVersion { binary, printed } => write!(
                formatter,
                "`{} -V` printed {printed:?}, which is not a tmux version this build can check",
                binary.display()
            ),
            Self::TmuxTooOld { binary, found } => write!(
                formatter,
                "`{}` is tmux {found}; Cockpit needs tmux {MINIMUM_VERSION} or newer",
                binary.display()
            ),
            Self::CommandFailed {
                arguments,
                status,
                stderr,
            } => {
                let status = status.map_or("failed".to_string(), |code| format!("exited {code}"));
                write!(
                    formatter,
                    "tmux {} {status}: {}",
                    joined(arguments),
                    stderr.trim_end()
                )
            }
            Self::UnexpectedOutput { arguments, output } => write!(
                formatter,
                "tmux {} printed unexpected output {output:?}",
                joined(arguments)
            ),
            Self::InvalidArgument { what, value } => write!(formatter, "invalid {what}: {value:?}"),
            Self::Io {
                action,
                path,
                source,
            } => write!(formatter, "{action} {}: {source}", path.display()),
            Self::ObserverClosed => write!(formatter, "the tmux control-mode client has exited"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::TmuxNotRunnable { source, .. } | Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

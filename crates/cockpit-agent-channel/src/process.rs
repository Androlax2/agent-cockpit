//! Which processes a peer descends from, as the kernel reports them.

use std::fmt;

/// One process, told apart from any later process that reuses its pid: the boot it ran in,
/// its pid and its start time in clock ticks since that boot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessIdentity {
    pub boot: String,
    pub pid: u32,
    pub start: u64,
}

/// Why a peer's ancestry could not be read. The channel then identifies nobody.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessError {
    /// The process exited before its ancestry was read.
    Gone { pid: u32 },
    /// The kernel's process table could not be read or parsed.
    Unreadable { reason: String },
    /// Reading a process tree is implemented for Linux only; report 11 keeps macOS `validate`.
    Unsupported,
}

impl fmt::Display for ProcessError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProcessError::Gone { pid } => write!(formatter, "process {pid} exited"),
            ProcessError::Unreadable { reason } => {
                write!(formatter, "the process table is unreadable: {reason}")
            }
            ProcessError::Unsupported => formatter
                .write_str("verifying a process tree is implemented for Linux only on this build"),
        }
    }
}

impl std::error::Error for ProcessError {}

/// Deepest tree walked; a longer chain means the table changed under the walk.
const MAX_DEPTH: usize = 4096;

/// `pid` and each of its ancestors, nearest first, ending at the first process without a
/// parent. The walk stops early at a parent that started after its child: that pid was
/// reused after the real parent exited, so nothing above it is this process's ancestry.
/// Blocking: it reads the kernel's process table.
pub fn ancestry(pid: u32) -> Result<Vec<ProcessIdentity>, ProcessError> {
    platform::ancestry(pid)
}

/// The identity of the process `pid`. Blocking, like [`ancestry`].
pub fn identity_of(pid: u32) -> Result<ProcessIdentity, ProcessError> {
    platform::identity_of(pid)
}

#[cfg(target_os = "linux")]
mod platform {
    use std::fs;
    use std::io::ErrorKind;

    use super::{MAX_DEPTH, ProcessError, ProcessIdentity};

    pub(super) fn ancestry(pid: u32) -> Result<Vec<ProcessIdentity>, ProcessError> {
        let boot = boot_id()?;
        let mut chain = Vec::new();
        let (mut current, mut parent) = stat(pid, &boot)?;
        loop {
            chain.push(current);
            if parent == 0 || chain.len() == MAX_DEPTH {
                return Ok(chain);
            }
            let Ok((ancestor, grandparent)) = stat(parent, &boot) else {
                // The parent exited after the child was read: the chain ends there.
                return Ok(chain);
            };
            if ancestor.start > chain.last().expect("the chain is never empty").start {
                return Ok(chain);
            }
            (current, parent) = (ancestor, grandparent);
        }
    }

    pub(super) fn identity_of(pid: u32) -> Result<ProcessIdentity, ProcessError> {
        Ok(stat(pid, &boot_id()?)?.0)
    }

    fn boot_id() -> Result<String, ProcessError> {
        fs::read_to_string("/proc/sys/kernel/random/boot_id")
            .map(|id| id.trim().to_owned())
            .map_err(|error| ProcessError::Unreadable {
                reason: format!("boot id: {error}"),
            })
    }

    /// The process's identity and its parent's pid, from `/proc/<pid>/stat` (proc(5)).
    fn stat(pid: u32, boot: &str) -> Result<(ProcessIdentity, u32), ProcessError> {
        let text = match fs::read_to_string(format!("/proc/{pid}/stat")) {
            Ok(text) => text,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                return Err(ProcessError::Gone { pid });
            }
            Err(error) => {
                return Err(ProcessError::Unreadable {
                    reason: format!("process {pid}: {error}"),
                });
            }
        };
        let unparsable = || ProcessError::Unreadable {
            reason: format!("process {pid}: unexpected stat line"),
        };
        // The command name, field 2, may hold spaces and parentheses: fields resume after
        // the last `)`. Field 3 is the state, 4 the parent pid, 22 the start time.
        let (_, fields) = text.rsplit_once(')').ok_or_else(unparsable)?;
        let fields: Vec<&str> = fields.split_whitespace().collect();
        let field = |number: usize| fields.get(number - 3).ok_or_else(unparsable);
        let parent = field(4)?.parse().map_err(|_| unparsable())?;
        let start = field(22)?.parse().map_err(|_| unparsable())?;
        Ok((
            ProcessIdentity {
                boot: boot.to_owned(),
                pid,
                start,
            },
            parent,
        ))
    }
}

#[cfg(not(target_os = "linux"))]
mod platform {
    use super::{ProcessError, ProcessIdentity};

    pub(super) fn ancestry(_pid: u32) -> Result<Vec<ProcessIdentity>, ProcessError> {
        Err(ProcessError::Unsupported)
    }

    pub(super) fn identity_of(_pid: u32) -> Result<ProcessIdentity, ProcessError> {
        Err(ProcessError::Unsupported)
    }
}

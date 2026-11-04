//! Whether greywall can write the seccomp filter it loads at every Linux launch (#628).
//!
//! The pinned greywall writes it to `$TMPDIR/greywall-seccomp`, `/tmp/greywall-seccomp`
//! without `TMPDIR`. In `/tmp` that is one folder for every user of the host, owner-only,
//! owned by whoever ran greywall first: every other user's sandbox exits at its start, and a
//! folder others can write lets them replace the filter. Every launch now sets `TMPDIR` to
//! the session's own temp folder (#766), so the folder is per user and per session; the
//! profile denies the agent writes to it. The check writes a file there as greywall would,
//! so a failure shows before a session starts.

use std::ffi::{OsStr, OsString};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// The folder greywall's runtime keeps its seccomp filters in.
const SECCOMP_FOLDER: &str = "greywall-seccomp";

/// Why greywall cannot write its seccomp filter, and what the owner does next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilterWriteFailure {
    pub reason: String,
    pub next: String,
}

impl std::fmt::Display for FilterWriteFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}; {}", self.reason, self.next)
    }
}

/// The folder greywall writes its seccomp filter to under `environment`, as Go's
/// `os.TempDir` picks it: the last non-empty `TMPDIR`, else `/tmp`.
pub fn seccomp_filter_dir(environment: &[(OsString, OsString)]) -> PathBuf {
    let temp = environment
        .iter()
        .rev()
        .find(|(name, _)| name == "TMPDIR")
        .map(|(_, value)| value.as_os_str())
        .filter(|value| !value.is_empty())
        .unwrap_or(OsStr::new("/tmp"));
    Path::new(temp).join(SECCOMP_FOLDER)
}

/// Writes and removes a file in `dir` as greywall writes its filter there. An absent folder
/// passes: greywall creates it owner-only for this user.
pub(crate) fn check_filter_write(dir: &Path) -> Result<(), FilterWriteFailure> {
    let shown = dir.display();
    let found = match std::fs::symlink_metadata(dir) {
        Ok(found) => found,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(FilterWriteFailure {
                reason: format!("greywall cannot read its seccomp folder {shown}: {error}"),
                next: format!("make {shown} readable to this user or remove it"),
            });
        }
    };
    let own_uid = std::fs::metadata("/proc/self")
        .map_err(|error| FilterWriteFailure {
            reason: format!("this user's id is unknown: /proc/self: {error}"),
            next: "run the daemon with /proc mounted".to_owned(),
        })?
        .uid();
    let mode = found.permissions().mode() & 0o7777;
    let described = format!("{shown} (owner uid {}, mode {mode:o})", found.uid());
    let refuse = |problem: String| {
        let next = if found.uid() == own_uid {
            format!("remove {shown}; greywall recreates it owner-only at the next launch")
        } else {
            format!(
                "the pinned greywall keeps one seccomp folder for every user of this host \
                 (#628); until a greywall release writes it per user, have uid {} remove \
                 {shown} while none of its sandboxes starts, or run sandboxed agents as that \
                 user",
                found.uid()
            )
        };
        FilterWriteFailure {
            reason: format!("greywall's seccomp folder {described} {problem}"),
            next,
        }
    };
    if !found.is_dir() {
        return Err(refuse("is not a folder".to_owned()));
    }
    if found.uid() != own_uid {
        return Err(refuse(
            "is another user's, who could replace the filter before the sandbox loads it"
                .to_owned(),
        ));
    }
    if mode & 0o022 != 0 {
        return Err(refuse(
            "is writable by other users, who could replace the filter before the sandbox \
             loads it"
                .to_owned(),
        ));
    }
    // A random name of its own, created exclusively: concurrent checks never collide, and
    // a link planted at the name is never followed. Removed when dropped.
    tempfile::Builder::new()
        .prefix(".torlek-filter-probe-")
        .tempfile_in(dir)
        .map(drop)
        .map_err(|error| refuse(format!("cannot be written: {error}")))
}

//! Whether greywall can write the seccomp filter it loads at every Linux launch (#628).
//!
//! The pinned greywall writes it to `$TMPDIR/greywall-seccomp`, `/tmp/greywall-seccomp`
//! without `TMPDIR`: one folder for every user of the host, owner-only, owned by whoever ran
//! greywall first. Every other user's sandbox then exits at its start, and a folder owned by
//! another user lets that user replace the filter. The check writes a file there as greywall
//! would, so the failure shows before a session starts.

use std::ffi::{OsStr, OsString};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
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
pub(crate) fn seccomp_filter_dir(environment: &[(OsString, OsString)]) -> PathBuf {
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
    let found = match std::fs::symlink_metadata(dir) {
        Ok(found) => found,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(FilterWriteFailure {
                reason: format!("greywall cannot read {}: {error}", dir.display()),
                next: format!("make {} readable to this user or remove it", dir.display()),
            });
        }
    };
    let described = format!(
        "{} (owner uid {}, mode {:o})",
        dir.display(),
        found.uid(),
        found.permissions().mode() & 0o7777
    );
    let shared = |reason: String| FilterWriteFailure {
        reason,
        next: format!(
            "the pinned greywall keeps one seccomp folder for every user of this host (#628); \
             until a greywall release writes it per user, have uid {} remove {} while none of \
             its sandboxes starts, or run sandboxed agents as that user",
            found.uid(),
            dir.display()
        ),
    };
    if !found.is_dir() {
        return Err(shared(format!(
            "greywall writes its seccomp filter in {described}, which is not a folder"
        )));
    }
    let probe = dir.join(format!(".torlek-filter-probe-{}", std::process::id()));
    // `create_new` never follows a link planted at the probe's name.
    let written = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&probe)
        .and_then(|file| file.metadata());
    let written = match written {
        Ok(written) => written,
        Err(error) => {
            return Err(shared(format!(
                "greywall cannot write its seccomp filter in {described}: {error}; every \
                 sandboxed agent exits at its start"
            )));
        }
    };
    if let Err(error) = std::fs::remove_file(&probe) {
        tracing::warn!(%error, probe = %probe.display(), "the seccomp folder probe stays");
    }
    if written.uid() != found.uid() {
        return Err(shared(format!(
            "greywall writes its seccomp filter in {described}, another user's folder, \
             where that user could replace the filter before the sandbox loads it"
        )));
    }
    Ok(())
}

//! "Runs a file this session can edit, as root" (#1383) at the daemon: each revision of a
//! command request is checked against the asking session's writes before the owner reads it,
//! in the transaction that filed or edited it; the owner's views read each recorded file as it
//! is now; and an approved run is refused when one no longer has its recorded hash. The rule
//! itself is `cockpit_approvals::root_runs`.

use std::path::Path;

use cockpit_approvals::{Lookup, RootFile, RootRuns, RunRequest, RunState, SessionWrites};
use cockpit_authorization::SessionId;
use cockpit_protocol::{FileNowView, RootFileView, RootRunsView};
use cockpit_sessions::Sessions;
use cockpit_turso::WriteTransaction;

use super::{Failed, now, shown};

const DEFAULT_PATH: &str = "/bin:/usr/bin";

/// Checks what the revision `run` now has runs as root that its session can change, and
/// records it with the request.
pub(super) async fn record(
    write: &WriteTransaction<'_>,
    sessions: &Sessions,
    run: &RunRequest,
) -> Result<RunRequest, Failed> {
    if run.start.is_some() {
        return Ok(run.clone());
    }
    let runs = check(sessions, &run.session, &run.command, &run.cwd).await;
    Ok(cockpit_approvals::record_root_runs(write, run.id, run.revision, &runs, now()).await?)
}

/// What `command`, run in `cwd`, runs as root that `session` can change, read now. The run
/// gets the daemon's own environment, so its `PATH` and `HOME` resolve the command's words.
async fn check(sessions: &Sessions, session: &SessionId, command: &str, cwd: &Path) -> RootRuns {
    let writes = session_writes(sessions, session);
    let command = command.to_owned();
    let cwd = cwd.to_path_buf();
    tokio::task::spawn_blocking(move || {
        // Without `PATH`, a shell and sudo search the C library's default, glibc's
        // `confstr(_CS_PATH)`.
        let search_path = std::env::var_os("PATH").unwrap_or_else(|| DEFAULT_PATH.into());
        let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
        let lookup = Lookup {
            cwd: &cwd,
            search_path: Some(&search_path),
            home: home.as_deref(),
        };
        cockpit_approvals::root_runs(&command, lookup, &writes)
    })
    .await
    .expect("checking what a command runs as root does not panic")
}

/// Where `session` can write: the folders its sandbox's launch spec binds read-write, while
/// that spec is staged; else everything the owner's user can write, which holds every
/// sandbox's folders too.
fn session_writes(sessions: &Sessions, session: &SessionId) -> SessionWrites {
    let Some(Ok(folder)) = sessions.sandbox().session_mount_sources(session.as_str()) else {
        return SessionWrites::Unsandboxed;
    };
    let spec_path = folder.join(cockpit_sandbox::SPEC_FILE);
    let text = match std::fs::read(&spec_path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return SessionWrites::Unsandboxed;
        }
        Err(error) => {
            tracing::warn!(%error, path = %spec_path.display(), "cannot read a session's launch spec; checking its sudo runs as unsandboxed");
            return SessionWrites::Unsandboxed;
        }
    };
    match serde_json::from_slice::<cockpit_sandbox::LaunchSpec>(&text) {
        Ok(spec) => SessionWrites::Folders(spec.host_writes()),
        Err(error) => {
            tracing::warn!(%error, path = %spec_path.display(), "cannot parse a session's launch spec; checking its sudo runs as unsandboxed");
            SessionWrites::Unsandboxed
        }
    }
}

/// The recorded check as the owner reads it; while the request asks, with each file read
/// now. `None` when the revision was not checked.
pub(super) async fn owner_view(run: &RunRequest) -> Option<RootRunsView> {
    let runs = run.root_runs.clone()?;
    let cwd = run.cwd.clone();
    let reads_now = run.state == RunState::Asking;
    let view = tokio::task::spawn_blocking(move || RootRunsView {
        editable: runs
            .editable
            .iter()
            .map(|file| file_view(file, &cwd, reads_now))
            .collect(),
        cannot_tell: runs.cannot_tell,
    })
    .await
    .expect("reading the files a command runs as root does not panic");
    Some(view)
}

fn file_view(file: &RootFile, cwd: &Path, reads_now: bool) -> RootFileView {
    let now = reads_now
        .then(|| cockpit_approvals::file_now(&file.path))
        .and_then(|read| {
            // Debug, not warn: the owner's views poll, and the view shows no current contents.
            read.inspect_err(|error| tracing::debug!(%error, "cannot read a file sudo runs"))
                .ok()
        })
        .map(|now| FileNowView {
            sha256: now.sha256,
            head: now.head.as_deref().map(shown),
            more_lines: now.more_lines,
        });
    RootFileView {
        path: file.path.to_string_lossy().into_owned(),
        shown_path: shown(&path_from(&file.path, cwd)),
        sha256: file.sha256.clone(),
        now,
    }
}

/// `path` as the owner reads it: `./x.sh` inside the request's directory, else absolute.
fn path_from(path: &Path, cwd: &Path) -> String {
    match path.strip_prefix(cwd) {
        Ok(inside) => format!("./{}", inside.display()),
        Err(_) => path.display().to_string(),
    }
}

/// Why an approved run must not start: a file its sudo runs, recorded when the owner read the
/// request, no longer has its recorded hash.
pub(super) async fn changed_after_approval(run: &RunRequest) -> Option<String> {
    let runs = run.root_runs.clone()?;
    let cwd = run.cwd.clone();
    tokio::task::spawn_blocking(move || {
        cockpit_approvals::changed_file(&runs.editable)
            .map(|path| format!("{} changed after you approved", path_from(path, &cwd)))
    })
    .await
    .expect("hashing the files a command runs as root does not panic")
}

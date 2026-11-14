//! The checks a command request gets before the owner reads it, at the daemon: each revision
//! is checked in the transaction that filed or edited it, for what its sudo runs that the
//! asking session can edit (#1383, `cockpit_approvals::root_runs`) and by the rule scan
//! (#1388, `cockpit_approvals::scan`), which lists the first among its findings. The owner's
//! views read each file sudo runs as it is now, and their digest names what they read; an
//! approval must name the files as they are when it arrives, and a run is refused when one no
//! longer has its recorded hash.

use std::path::Path;
use std::time::Duration;

use cockpit_approvals::{
    Contents, FileNow, Finding, Lookup, RootFile, RootRuns, RunRequest, RunState, SessionWrites,
    Severity,
};
use cockpit_authorization::SessionId;
use cockpit_protocol::{
    FileNowView, FindingSeverityView, FindingView, RootFileView, RootRunsView, ShownText,
};
use cockpit_sessions::Sessions;
use cockpit_turso::WriteTransaction;

use super::{Failed, now, shown};

const DEFAULT_PATH: &str = "/bin:/usr/bin";

/// How long the checks or a read of the files wait: a huge file or a stalled filesystem must
/// not hold the write transaction, the queue or a run past it. Files are opened without
/// waiting on a FIFO (`cockpit_approvals::file_now`); a read past the bound is left to end
/// on its own thread.
const READ_TIMEOUT: Duration = Duration::from_secs(10);

/// `work` on a blocking thread, or `None` once [`READ_TIMEOUT`] passed.
async fn bounded<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    match tokio::time::timeout(READ_TIMEOUT, tokio::task::spawn_blocking(work)).await {
        Ok(done) => Some(done.expect("reading the files a command runs as root does not panic")),
        Err(_) => {
            tracing::warn!(
                "reading the files a command's sudo runs took longer than {READ_TIMEOUT:?}"
            );
            None
        }
    }
}

/// Checks the revision `run` now has: what it runs as root that its session can change, and
/// the rule scan's findings; records both with the request.
pub(super) async fn record(
    write: &WriteTransaction<'_>,
    sessions: &Sessions,
    run: &RunRequest,
) -> Result<RunRequest, Failed> {
    if run.start.is_some() {
        return Ok(run.clone());
    }
    let clone = run.project.as_ref().map(|project| project.root.clone());
    let (runs, findings) = check(sessions, &run.session, &run.command, &run.cwd, clone).await;
    Ok(
        cockpit_approvals::record_checks(write, run.id, run.revision, &runs, &findings, now())
            .await?,
    )
}

/// What `command`, run in `cwd`, runs as root that `session` can change, and what the scan
/// finds in it, read now; `clone` is the session's checkout. The run gets the daemon's own
/// environment, so its `PATH` and `HOME` resolve the command's words.
async fn check(
    sessions: &Sessions,
    session: &SessionId,
    command: &str,
    cwd: &Path,
    clone: Option<std::path::PathBuf>,
) -> (RootRuns, Vec<Finding>) {
    let writes = session_writes(sessions, session);
    let command = std::sync::Arc::<str>::from(command);
    let cwd = std::sync::Arc::<Path>::from(cwd);
    // Each check has its own bound, so a scan that does not end in time leaves the files
    // sudo runs pinned by their hashes.
    let runs = bounded({
        let (command, cwd) = (command.clone(), cwd.clone());
        move || {
            with_lookup(&cwd, |lookup| {
                cockpit_approvals::root_runs(&command, lookup, &writes)
            })
        }
    })
    .await
    .unwrap_or(RootRuns {
        editable: Vec::new(),
        cannot_tell: true,
    });
    let findings = bounded({
        let (runs, cwd) = (runs.clone(), cwd.clone());
        move || {
            with_lookup(&cwd, |lookup| {
                cockpit_approvals::scan(&command, lookup, clone.as_deref(), &runs)
            })
        }
    })
    .await
    .unwrap_or_else(|| {
        let mut findings = cockpit_approvals::sudo_findings(&runs, &cwd);
        findings.push(cockpit_approvals::scan_unfinished());
        findings
    });
    (runs, findings)
}

/// `work` given the daemon's own environment, whose `PATH` and `HOME` the run gets.
fn with_lookup<T>(cwd: &Path, work: impl FnOnce(Lookup<'_>) -> T) -> T {
    // Without `PATH`, a shell and sudo search the C library's default, glibc's
    // `confstr(_CS_PATH)`.
    let search_path = std::env::var_os("PATH").unwrap_or_else(|| DEFAULT_PATH.into());
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    work(Lookup {
        cwd,
        search_path: Some(&search_path),
        home: home.as_deref(),
    })
}

/// The recorded findings as the owner reads them. A revision checked before the scan existed
/// gets the sudo findings its `root_runs` holds; one never checked, `None`.
pub(super) fn findings_view(run: &RunRequest) -> Option<Vec<FindingView>> {
    let findings = match (&run.findings, &run.root_runs) {
        (Some(findings), _) => findings.clone(),
        (None, Some(runs)) => cockpit_approvals::sudo_findings(runs, &run.cwd),
        (None, None) => return None,
    };
    Some(
        findings
            .iter()
            .map(|finding| FindingView {
                rule: finding.rule.clone(),
                severity: match finding.severity {
                    Severity::High => FindingSeverityView::High,
                    Severity::Warn => FindingSeverityView::Warn,
                },
                reason: finding.reason.clone(),
                excerpt: shown(&finding.excerpt),
                file: finding
                    .file
                    .as_deref()
                    .map(|file| shown_path(file, &run.cwd)),
                line: finding.line.map(|line| line as u64),
            })
            .collect(),
    )
}

fn shown_path(path: &Path, cwd: &Path) -> ShownText {
    shown(&cockpit_approvals::path_from(path, cwd))
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

/// The recorded check as the owner reads it, and the digest an approval of what it shows
/// names. While the request asks, each file is read now. `None` when the revision was not
/// checked.
pub(super) async fn owner_view(run: &RunRequest) -> Option<(RootRunsView, String)> {
    let runs = run.root_runs.clone()?;
    if run.state != RunState::Asking {
        let view = RootRunsView {
            editable: runs
                .editable
                .iter()
                .map(|file| file_view(file, &run.cwd, None))
                .collect(),
            cannot_tell: runs.cannot_tell,
        };
        return Some((view, run.digest()));
    }
    let paths: Vec<_> = runs.editable.iter().map(|file| file.path.clone()).collect();
    let reads = bounded(move || {
        paths
            .iter()
            .map(|path| {
                cockpit_approvals::file_now(path)
                    // Debug, not warn: the owner's views poll; the view says it is unreadable.
                    .inspect_err(|error| tracing::debug!(%error, path = %path.display(), "cannot read a file sudo runs"))
                    .ok()
            })
            .collect::<Vec<_>>()
    })
    .await
    .unwrap_or_else(|| vec![None; runs.editable.len()]);
    let contents: Vec<Contents> = reads.iter().map(contents_of).collect();
    let view = RootRunsView {
        editable: runs
            .editable
            .iter()
            .zip(reads)
            .map(|(file, read)| file_view(file, &run.cwd, Some(read)))
            .collect(),
        cannot_tell: runs.cannot_tell,
    };
    Some((
        view,
        cockpit_approvals::digest_as_read(&run.digest(), &contents),
    ))
}

fn contents_of(read: &Option<FileNow>) -> Contents {
    match read {
        Some(FileNow {
            sha256: Some(sha256),
            ..
        }) => Contents::Hash(sha256.clone()),
        Some(FileNow { sha256: None, .. }) => Contents::Absent,
        None => Contents::Unreadable,
    }
}

/// `read`: `None` when not read, `Some(None)` when it could not be.
fn file_view(file: &RootFile, cwd: &Path, read: Option<Option<FileNow>>) -> RootFileView {
    let now = read.map(|read| match read {
        Some(now) => FileNowView {
            sha256: now.sha256,
            head: now.head.as_deref().map(shown),
            more_lines: now.more_lines,
            unreadable: false,
        },
        None => FileNowView {
            sha256: None,
            head: None,
            more_lines: 0,
            unreadable: true,
        },
    });
    RootFileView {
        path: file.path.to_string_lossy().into_owned(),
        shown_path: shown_path(&file.path, cwd),
        sha256: file.sha256.clone(),
        now,
    }
}

/// The files `run`'s sudo runs as an approval arriving now finds them.
pub(super) struct AsApproved {
    /// What the owner's view must have named for the approval to be of these contents.
    pub(super) digest: String,
    /// Why the approved run must not start: a file no longer has its recorded hash.
    pub(super) changed: Option<String>,
}

/// The editable files `run`'s sudo runs, read for an approval; `None` when there are none.
pub(super) async fn as_approved(run: &RunRequest) -> Option<AsApproved> {
    let runs = run
        .root_runs
        .clone()
        .filter(|runs| !runs.editable.is_empty())?;
    let paths: Vec<_> = runs.editable.iter().map(|file| file.path.clone()).collect();
    let contents = bounded(move || paths.iter().map(|path| Contents::read(path)).collect())
        .await
        .unwrap_or_else(|| vec![Contents::Unreadable; runs.editable.len()]);
    let changed = runs
        .editable
        .iter()
        .zip(&contents)
        .find(|(file, now)| **now != Contents::recorded(file))
        .map(|(file, _)| {
            format!(
                "{} changed since it was asked",
                cockpit_approvals::path_from(&file.path, &run.cwd)
            )
        });
    Some(AsApproved {
        digest: cockpit_approvals::digest_as_read(&run.digest(), &contents),
        changed,
    })
}

/// Why an approved run must not start: a file its sudo runs no longer has its recorded hash.
pub(super) async fn changed_after_approval(run: &RunRequest) -> Option<String> {
    let runs = run.root_runs.clone()?;
    let cwd = run.cwd.clone();
    bounded(move || {
        cockpit_approvals::changed_file(&runs.editable).map(|path| {
            format!(
                "{} changed after you approved",
                cockpit_approvals::path_from(path, &cwd)
            )
        })
    })
    .await
    .unwrap_or_else(|| Some("the files its sudo runs could not be read in time".to_owned()))
}

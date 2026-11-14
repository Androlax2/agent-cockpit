//! The checks a command request gets before the owner reads it, at the daemon: each revision
//! is checked in the transaction that filed or edited it, for what its sudo runs that the
//! asking session can edit (#1383, `cockpit_approvals::root_runs`), by the rule scan
//! (#1388, `cockpit_approvals::scan`), which lists the first among its findings, and for the
//! files any part of it runs that the session can edit, pinned by their hashes (#1387,
//! `cockpit_approvals::pinned_files`). The owner's views read each pinned file and each file
//! sudo runs as it is now, and their digest names what they read; an approval must name the
//! files as they are when it arrives, and a run is refused when one no longer has its
//! recorded hash.

use std::path::{Path, PathBuf};
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
        Ok(done) => Some(done.expect("reading the files a command runs does not panic")),
        Err(_) => {
            tracing::warn!("reading the files a command runs took longer than {READ_TIMEOUT:?}");
            None
        }
    }
}

/// Checks the revision `run` now has: what it runs as root that its session can change, the
/// rule scan's findings and the files it runs that the session can change; records them with
/// the request.
pub(super) async fn record(
    write: &WriteTransaction<'_>,
    sessions: &Sessions,
    run: &RunRequest,
) -> Result<RunRequest, Failed> {
    if run.start.is_some() {
        return Ok(run.clone());
    }
    let clone = run.project.as_ref().map(|project| project.root.clone());
    let checks = check(sessions, &run.session, &run.command, &run.cwd, clone).await;
    Ok(cockpit_approvals::record_checks(
        write,
        run.id,
        run.revision,
        &checks.root_runs,
        &checks.findings,
        &checks.pinned,
        now(),
    )
    .await?)
}

/// What one revision's checks found.
struct Checks {
    root_runs: RootRuns,
    findings: Vec<Finding>,
    pinned: Vec<RootFile>,
}

/// What `command`, run in `cwd`, runs as root that `session` can change, what the scan finds
/// in it and the files it runs that the session can change, read now; `clone` is the
/// session's checkout. The run gets the daemon's own environment, so its `PATH` and `HOME`
/// resolve the command's words.
async fn check(
    sessions: &Sessions,
    session: &SessionId,
    command: &str,
    cwd: &Path,
    clone: Option<PathBuf>,
) -> Checks {
    let writes = session_writes(sessions, session);
    let command = std::sync::Arc::<str>::from(command);
    let cwd = std::sync::Arc::<Path>::from(cwd);
    // Each check has its own bound, so a scan that does not end in time leaves the files
    // pinned by their hashes. Pinning and the sudo check run side by side: the write
    // transaction waits for the longest, not their sum.
    let (pinned, runs) = tokio::join!(
        bounded({
            let (command, cwd, writes) = (command.clone(), cwd.clone(), writes.clone());
            move || {
                with_lookup(&cwd, |lookup| {
                    cockpit_approvals::pinned_files(&command, lookup, &writes)
                })
            }
        }),
        bounded({
            let (command, cwd) = (command.clone(), cwd.clone());
            move || {
                with_lookup(&cwd, |lookup| {
                    cockpit_approvals::root_runs(&command, lookup, &writes)
                })
            }
        }),
    );
    let runs = runs.unwrap_or(RootRuns {
        editable: Vec::new(),
        cannot_tell: true,
    });
    let mut findings = bounded({
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
    let is_told_cannot_tell = |findings: &[Finding], incomplete: &Finding| {
        findings
            .iter()
            .any(|finding| finding.rule == incomplete.rule)
    };
    let pinned = match pinned {
        Some(pinned) => {
            let incomplete = cockpit_approvals::pinning_incomplete(true);
            if pinned.cannot_tell && !is_told_cannot_tell(&findings, &incomplete) {
                findings.push(incomplete);
            }
            pinned.files
        }
        None => {
            findings.push(cockpit_approvals::pinning_incomplete(false));
            Vec::new()
        }
    };
    Checks {
        root_runs: runs,
        findings,
        pinned,
    }
}

/// `work` given the daemon's own environment, whose `PATH` and `HOME` the run gets.
fn with_lookup<T>(cwd: &Path, work: impl FnOnce(Lookup<'_>) -> T) -> T {
    // Without `PATH`, a shell and sudo search the C library's default, glibc's
    // `confstr(_CS_PATH)`.
    let search_path = std::env::var_os("PATH").unwrap_or_else(|| DEFAULT_PATH.into());
    let home = std::env::var_os("HOME").map(PathBuf::from);
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

/// The recorded checks as the owner reads them: the files sudo runs, the pinned files, and the
/// digest an approval of what they show names. While the request asks, each file is read now,
/// once for both. `None` when the revision was not checked.
pub(super) async fn owner_view(run: &RunRequest) -> Option<OwnerView> {
    if run.root_runs.is_none() && run.pinned.is_none() {
        return None;
    }
    let views = |read_of: &dyn Fn(&Path) -> Option<Option<FileNow>>| {
        let files = |files: &[RootFile]| {
            files
                .iter()
                .map(|file| file_view(file, &run.cwd, read_of(&file.path)))
                .collect()
        };
        let root_runs = run.root_runs.as_ref().map(|runs| RootRunsView {
            editable: files(&runs.editable),
            cannot_tell: runs.cannot_tell,
        });
        (root_runs, run.pinned.as_deref().map(files))
    };
    if run.state != RunState::Asking {
        let (root_runs, pinned) = views(&|_| None);
        return Some(OwnerView {
            root_runs,
            pinned,
            digest: run.digest(),
        });
    }
    let paths: Vec<PathBuf> = run
        .bound_files()
        .into_iter()
        .map(|file| file.path)
        .collect();
    let count = paths.len();
    let reads = bounded(move || {
        paths
            .into_iter()
            .map(|path| {
                let read = cockpit_approvals::file_now(&path)
                    // Debug, not warn: the owner's views poll; the view says it is unreadable.
                    .inspect_err(|error| tracing::debug!(%error, path = %path.display(), "cannot read a file a command runs"))
                    .ok();
                (path, read)
            })
            .collect::<Vec<_>>()
    })
    .await;
    let Some(reads) = reads else {
        let (root_runs, pinned) = views(&|_| Some(None));
        return Some(OwnerView {
            root_runs,
            pinned,
            digest: cockpit_approvals::digest_as_read(
                &run.digest(),
                &vec![Contents::Unreadable; count],
            ),
        });
    };
    let contents: Vec<Contents> = reads.iter().map(|(_, read)| contents_of(read)).collect();
    let read_by_path: std::collections::HashMap<&Path, &Option<FileNow>> = reads
        .iter()
        .map(|(path, read)| (path.as_path(), read))
        .collect();
    let read_of = |path: &Path| read_by_path.get(path).map(|read| (*read).clone());
    let (root_runs, pinned) = views(&read_of);
    Some(OwnerView {
        root_runs,
        pinned,
        digest: cockpit_approvals::digest_as_read(&run.digest(), &contents),
    })
}

/// A request's checks as the owner's views show them.
pub(super) struct OwnerView {
    pub(super) root_runs: Option<RootRunsView>,
    pub(super) pinned: Option<Vec<RootFileView>>,
    pub(super) digest: String,
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

/// `file` as recorded, not read now: what an "always" approval (#1387) is bound to.
pub(super) fn recorded_file_view(file: &RootFile, cwd: &Path) -> RootFileView {
    file_view(file, cwd, None)
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

/// The files `run` is bound to as an approval arriving now finds them.
pub(super) struct AsApproved {
    /// What the owner's view must have named for the approval to be of these contents.
    pub(super) digest: String,
    /// Why the approved run must not start: a file no longer has its recorded hash.
    pub(super) changed: Option<String>,
}

/// The files `run` is bound to, read for an approval; `None` when there are none.
pub(super) async fn as_approved(run: &RunRequest) -> Option<AsApproved> {
    let bound = run.bound_files();
    if bound.is_empty() {
        return None;
    }
    let paths: Vec<_> = bound.iter().map(|file| file.path.clone()).collect();
    let contents = bounded(move || paths.iter().map(|path| Contents::read(path)).collect())
        .await
        .unwrap_or_else(|| vec![Contents::Unreadable; bound.len()]);
    let changed = bound
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

/// Why an approved run must not start: a file it runs no longer has its recorded hash.
pub(super) async fn changed_after_approval(run: &RunRequest) -> Option<String> {
    let bound = run.bound_files();
    if bound.is_empty() {
        return None;
    }
    let cwd = run.cwd.clone();
    bounded(move || {
        cockpit_approvals::changed_file(&bound).map(|path| {
            format!(
                "{} changed after you approved",
                cockpit_approvals::path_from(path, &cwd)
            )
        })
    })
    .await
    .unwrap_or_else(|| Some("the files it runs could not be read in time".to_owned()))
}

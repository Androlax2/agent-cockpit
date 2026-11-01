//! Routes bug reports to `cockpit-bug-reports` and turns the faults the daemon sees into
//! drafts (#427). An agent reports on the agent socket, where `cockpit-agent-channel`
//! identifies it; the owner reviews, edits, sends and discards on the host-only socket.
//! Nothing here files an issue: a send happens in the owner's browser or `gh`.
//!
//! Each new draft gets a support bundle written through `cockpit-diagnostics` after the draft
//! is committed, outside the transaction, so a bundle that cannot be written never loses the
//! report: the draft then says why it has none.

use std::collections::{HashMap, VecDeque};
use std::fs;
use std::io::ErrorKind;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cockpit_bug_reports::{Bundle, Edit, Facts, Filed, Kind, NewReport, Report, ReportId, Source};
use cockpit_protocol::{
    BugBundleView, BugFiledView, BugIssueView, BugKindView, BugOriginView, BugReportView,
    BugStatusView, Command, ErrorCode, ProtocolError, Reply, Unblock, UnblockedBy,
};
use cockpit_turso::Database;
use tokio::sync::mpsc;

use crate::approvals::{Answer, Failed, Sender, job, now};
use crate::routes::{Peer, Route, WorkClass};

/// `<state dir>/bug-reports`: one support bundle per draft, 0700 like the state directory.
pub(crate) const BUG_REPORTS_DIR: &str = "bug-reports";
/// Present while a daemon runs; one left behind at start means the last run did not stop
/// cleanly.
const RUNNING_MARKER: &str = "daemon.running";
/// Faults waiting to be filed; past it they are dropped and logged, never waited for.
const FAULT_QUEUE: usize = 64;
/// An `internal` answer this many times for one command within [`INTERNAL_WINDOW`] is a bug,
/// not a passing failure.
const INTERNAL_REPEATS: usize = 3;
const INTERNAL_WINDOW: Duration = Duration::from_secs(10 * 60);
/// The long-lived processes whose logs a bundle carries.
const LOGGED_PROCESSES: [&str; 2] = ["daemon", "tui"];

/// What the daemon needs to file and serve reports.
#[derive(Clone)]
pub(crate) struct Context {
    pub(crate) database: Arc<Database>,
    /// `$XDG_STATE_HOME/cockpit`: the bundles' directory and, beside it, the logs.
    pub(crate) state_dir: PathBuf,
    pub(crate) facts: Arc<Facts>,
}

/// A fault the daemon saw in its own operation. It names only Cockpit's own parts: the
/// command kind, the lane, the worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Fault {
    /// A request's work panicked; `command` is its kind.
    RequestPanic { command: String },
    /// A long-lived worker panicked and was restarted.
    WorkerPanic { worker: &'static str },
    /// A request ran past its lane's budget and was still running.
    Stall {
        lane: &'static str,
        command: String,
        budget: Duration,
    },
    /// A request answered `internal`.
    Internal { command: String },
    /// The last run left its running marker: it did not stop cleanly.
    Crash,
}

/// Where faults go. Reporting never blocks the caller: a full queue drops the fault.
#[derive(Clone, Debug)]
pub(crate) struct Faults(Option<mpsc::Sender<Fault>>);

impl Faults {
    /// A sink and the receiver [`detect_forever`] files from.
    pub(crate) fn channel() -> (Faults, mpsc::Receiver<Fault>) {
        let (sender, receiver) = mpsc::channel(FAULT_QUEUE);
        (Faults(Some(sender)), receiver)
    }

    /// Reports nothing: for the worker that files faults, whose own panic must not feed it.
    pub(crate) fn none() -> Faults {
        Faults(None)
    }

    pub(crate) fn report(&self, fault: Fault) {
        let Some(sender) = &self.0 else {
            return;
        };
        if let Err(error) = sender.try_send(fault) {
            tracing::warn!(%error, "a fault was not queued for a bug report draft");
        }
    }
}

/// The facts every draft carries: the daemon's version, and the text `cockpit version
/// --all` would print of the platform and providers. Reasons a part is unavailable are left
/// out, since they may name the owner's paths.
pub(crate) fn facts(
    platform: &str,
    tmux: &Result<String, String>,
    providers: &[cockpit_readiness::ProviderFacts],
) -> Facts {
    let version = env!("CARGO_PKG_VERSION").to_owned();
    let revision = option_env!("COCKPIT_SOURCE_REVISION").unwrap_or("unknown: not a release build");
    let providers = if providers.is_empty() {
        "none".to_owned()
    } else {
        providers
            .iter()
            .map(|provider| match &provider.found {
                Ok(found) => format!(
                    "{} {}{}",
                    provider.name,
                    found.version.as_deref().unwrap_or("unknown version"),
                    if found.tested { " (tested)" } else { "" }
                ),
                Err(_) => format!("{} unavailable", provider.name),
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    let environment = format!(
        "cockpit {version} (revision {revision})\nplatform: {platform} ({os} {arch})\ntmux: {tmux}\nproviders: {providers}",
        os = std::env::consts::OS,
        arch = std::env::consts::ARCH,
        tmux = tmux.as_deref().unwrap_or("unavailable"),
    );
    Facts {
        version,
        environment,
    }
}

/// Whether the last run left its marker, then a new marker for this run. Called once the
/// instance lock is held, so no other daemon owns the marker.
pub(crate) fn mark_running(state_dir: &Path) -> std::io::Result<bool> {
    let marker = state_dir.join(RUNNING_MARKER);
    let crashed = match fs::symlink_metadata(&marker) {
        Ok(_) => true,
        Err(error) if error.kind() == ErrorKind::NotFound => false,
        Err(error) => return Err(error),
    };
    fs::write(&marker, format!("{}\n", std::process::id()))?;
    Ok(crashed)
}

/// Removes this run's marker: the daemon stops cleanly.
pub(crate) fn mark_stopped(state_dir: &Path) {
    let marker = state_dir.join(RUNNING_MARKER);
    if let Err(error) = fs::remove_file(&marker)
        && error.kind() != ErrorKind::NotFound
    {
        tracing::warn!(%error, marker = %marker.display(), "the running marker stays; the next start will report a crash");
    }
}

/// Files each fault the daemon reports, until the sink is gone. Repeated `internal`
/// answers count per command; one fault of the other kinds is a draft already.
pub(crate) async fn detect_forever(
    faults: Arc<tokio::sync::Mutex<mpsc::Receiver<Fault>>>,
    context: Context,
) {
    let mut faults = faults.lock().await;
    let mut internal: HashMap<String, VecDeque<Instant>> = HashMap::new();
    while let Some(fault) = faults.recv().await {
        let Some(new) = detected(fault, &mut internal, Instant::now()) else {
            continue;
        };
        match file(&context, new).await {
            Ok(filed) => tracing::info!(
                report = filed.report().id.0,
                filed = filed_view(&filed).as_str(),
                fingerprint = %filed.report().fingerprint.text(),
                "filed a fault as a bug report draft"
            ),
            Err(Failed(error)) => {
                tracing::error!(error = %error.message, "a fault could not be filed as a bug report draft");
            }
        }
    }
}

/// The draft a fault opens, or `None` while repeated `internal` answers stay under the bar.
fn detected(
    fault: Fault,
    internal: &mut HashMap<String, VecDeque<Instant>>,
    at: Instant,
) -> Option<NewReport> {
    let (kind, component, description) = match fault {
        Fault::RequestPanic { command } => (
            Kind::Panic,
            command.clone(),
            format!(
                "The daemon panicked while handling a `{command}` request. It answered \
                 `internal` and carried on."
            ),
        ),
        Fault::WorkerPanic { worker } => (
            Kind::Panic,
            format!("worker/{worker}"),
            format!("The daemon's `{worker}` worker panicked; it was restarted."),
        ),
        Fault::Stall {
            lane,
            command,
            budget,
        } => (
            Kind::Stall,
            format!("{lane}/{command}"),
            format!(
                "A `{command}` request ran past the {lane} lane's budget of {} s and was still \
                 running.",
                budget.as_secs()
            ),
        ),
        Fault::Internal { command } => {
            let seen = internal.entry(command.clone()).or_default();
            seen.push_back(at);
            while seen
                .front()
                .is_some_and(|first| at.duration_since(*first) > INTERNAL_WINDOW)
            {
                seen.pop_front();
            }
            if seen.len() < INTERNAL_REPEATS {
                return None;
            }
            seen.clear();
            (
                Kind::Internal,
                command.clone(),
                format!(
                    "`{command}` requests answered `internal` {INTERNAL_REPEATS} times within \
                     {} minutes.",
                    INTERNAL_WINDOW.as_secs() / 60
                ),
            )
        }
        Fault::Crash => (
            Kind::Crash,
            "daemon".to_owned(),
            "The daemon's last run ended without a clean stop: a crash, a kill or a power loss \
             ends it this way. Its log in the support bundle shows what it did last."
                .to_owned(),
        ),
    };
    Some(NewReport {
        source: Source::Daemon,
        kind,
        component: Some(component),
        title: None,
        description,
        steps: String::new(),
    })
}

/// The route for a bug report command; `routes.rs` sends only those here. An agent's report
/// runs in the bulk lane like every agent request (#32); the owner's verbs in control,
/// except a new draft, whose bundle reads the logs.
pub(crate) fn route(command: Command, peer: Peer, context: Context) -> Route {
    match (command, peer) {
        (
            Command::AgentReportBug {
                launch_token,
                description,
                bug_kind,
                component,
                steps,
                requested_by_owner,
            },
            Peer::Agent { connector },
        ) => Route::Run {
            class: WorkClass::Bulk,
            job: job(async move {
                let kind = agent_kind(bug_kind)?;
                let sender = Sender::read(connector).await;
                let write = context.database.write().await?;
                let session = sender.identify(&write, launch_token).await?;
                let new = NewReport {
                    source: Source::Agent {
                        session: session.as_str().to_owned(),
                        requested_by_owner,
                    },
                    kind,
                    component,
                    title: None,
                    description,
                    steps,
                };
                let filed = cockpit_bug_reports::file(&write, &context.facts, new, now())
                    .await
                    .map_err(failed)?;
                write.commit().await?;
                let filed = with_bundle(&context, filed).await?;
                Ok(Reply::BugReportFiled {
                    report: filed.report().id.0,
                    filed: filed_view(&filed),
                })
            }),
        },
        (command, Peer::Owner) => {
            let class = match command {
                Command::BugReportNew { .. } => WorkClass::Bulk,
                _ => WorkClass::Control,
            };
            Route::Run {
                class,
                job: job(owner(command, context)),
            }
        }
        (command, Peer::Agent { .. }) => {
            unreachable!("routes.rs keeps {command:?} off the agent socket")
        }
    }
}

async fn owner(command: Command, context: Context) -> Answer {
    match command {
        Command::BugReportNew {
            description,
            title,
            steps,
        } => {
            let new = NewReport {
                source: Source::Owner,
                kind: Kind::Other,
                component: None,
                title,
                description,
                steps,
            };
            let filed = file(&context, new).await?;
            Ok(Reply::BugReport {
                report: Box::new(view(filed.report(), false)),
            })
        }
        Command::BugReportList { all } => {
            let read = context.database.read().await?;
            let reports = cockpit_bug_reports::list(&read, all)
                .await
                .map_err(failed)?;
            Ok(Reply::BugReports {
                reports: reports.iter().map(|report| view(report, false)).collect(),
            })
        }
        Command::BugReportShow { report } => {
            let read = context.database.read().await?;
            let report = cockpit_bug_reports::report(&read, ReportId(report))
                .await
                .map_err(failed)?;
            Ok(Reply::BugReport {
                report: Box::new(view(&report, true)),
            })
        }
        Command::BugReportEdit {
            report,
            title,
            description,
            steps,
        } => {
            let edit = Edit {
                title,
                description,
                steps,
            };
            let write = context.database.write().await?;
            let report = cockpit_bug_reports::edit(&write, ReportId(report), &edit, now())
                .await
                .map_err(failed)?;
            write.commit().await?;
            Ok(Reply::BugReport {
                report: Box::new(view(&report, true)),
            })
        }
        Command::BugReportSent { report, issue_url } => {
            let write = context.database.write().await?;
            let report = cockpit_bug_reports::mark_sent(
                &write,
                ReportId(report),
                issue_url.as_deref(),
                now(),
            )
            .await
            .map_err(failed)?;
            write.commit().await?;
            Ok(Reply::BugReport {
                report: Box::new(view(&report, false)),
            })
        }
        Command::BugReportDiscard { report } => {
            let write = context.database.write().await?;
            let report = cockpit_bug_reports::discard(&write, ReportId(report), now())
                .await
                .map_err(failed)?;
            write.commit().await?;
            if let Bundle::Written { path, .. } = &report.bundle {
                remove_bundle(Path::new(path)).await;
            }
            Ok(Reply::BugReport {
                report: Box::new(view(&report, false)),
            })
        }
        other => unreachable!("routes.rs sends only bug report verbs here, not {other:?}"),
    }
}

/// Files `new` in a transaction of its own, then writes a new draft's bundle.
async fn file(context: &Context, new: NewReport) -> Result<Filed, Failed> {
    let write = context.database.write().await?;
    let filed = cockpit_bug_reports::file(&write, &context.facts, new, now())
        .await
        .map_err(failed)?;
    write.commit().await?;
    with_bundle(context, filed).await
}

/// A new draft with its bundle recorded, written or failed; other answers as they are.
async fn with_bundle(context: &Context, filed: Filed) -> Result<Filed, Failed> {
    let Filed::Created(report) = filed else {
        return Ok(filed);
    };
    let state_dir = context.state_dir.clone();
    let environment = context.facts.environment.clone();
    let id = report.id;
    let bundle = tokio::task::spawn_blocking(move || write_bundle(&state_dir, id, &environment))
        .await
        .unwrap_or_else(|error| Bundle::Failed {
            problem: format!("writing the bundle failed: {error}"),
        });
    if let Bundle::Failed { problem } = &bundle {
        tracing::warn!(
            report = id.0,
            problem,
            "a bug report draft goes without its bundle"
        );
    }
    let write = context.database.write().await?;
    let report = cockpit_bug_reports::attach_bundle(&write, id, &bundle, now())
        .await
        .map_err(failed)?;
    write.commit().await?;
    Ok(Filed::Created(report))
}

/// Writes the draft's support bundle: Cockpit's own logs (report 12 keeps prompts, agent
/// output, repository contents, commands and secrets out of them), the facts the draft
/// carries, and the platform facts the bundle adds itself.
fn write_bundle(state_dir: &Path, id: ReportId, environment: &str) -> Bundle {
    let written = || -> Result<Bundle, String> {
        let state_home = state_dir
            .parent()
            .ok_or("the state directory has no parent")?;
        let mut log_dirs = Vec::new();
        for process in LOGGED_PROCESSES {
            let dir = cockpit_diagnostics::log_dir_in(state_home, process)
                .map_err(|error| error.to_string())?;
            if dir.is_dir() {
                log_dirs.push(dir);
            }
        }
        let mut request = cockpit_diagnostics::BundleRequest::new(log_dirs);
        request.sections.push(
            cockpit_diagnostics::BundleSection::new(
                "version.txt",
                format!("{environment}\n").into_bytes(),
            )
            .map_err(|error| error.to_string())?,
        );
        let plan = cockpit_diagnostics::plan_bundle(&request).map_err(|error| error.to_string())?;
        let dir = state_dir.join(BUG_REPORTS_DIR);
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)
            .map_err(|error| format!("cannot create {}: {error}", dir.display()))?;
        let path = dir.join(format!("cockpit-bug-report-{}.tar", id.0));
        plan.write(&path).map_err(|error| error.to_string())?;
        let contents = plan
            .contents()
            .iter()
            .map(|item| format!("{}\t{}", item.bytes, item.path))
            .collect::<Vec<_>>()
            .join("\n");
        let path = path
            .to_str()
            .ok_or("the bundle path is not UTF-8")?
            .to_owned();
        Ok(Bundle::Written { path, contents })
    };
    written().unwrap_or_else(|problem| Bundle::Failed { problem })
}

async fn remove_bundle(path: &Path) {
    let path = path.to_path_buf();
    let removed = tokio::task::spawn_blocking(move || match fs::remove_file(&path) {
        Err(error) if error.kind() != ErrorKind::NotFound => Err((path, error)),
        _ => Ok(()),
    })
    .await;
    if let Ok(Err((path, error))) = removed {
        tracing::warn!(%error, bundle = %path.display(), "a discarded report's bundle stays");
    }
}

/// The kinds an agent reports; the daemon's own (panic, stall, crash) are its to observe.
fn agent_kind(kind: BugKindView) -> Result<Kind, Failed> {
    match kind {
        BugKindView::Timeout => Ok(Kind::Timeout),
        BugKindView::Unreachable => Ok(Kind::Unreachable),
        BugKindView::Internal => Ok(Kind::Internal),
        BugKindView::Other => Ok(Kind::Other),
        BugKindView::Panic | BugKindView::Stall | BugKindView::Crash => {
            Err(Failed(ProtocolError {
                code: ErrorCode::InvalidArgument,
                message: format!("bug kind {}: the daemon reports it itself", kind.as_str()),
                unblock: Some(Unblock {
                    action: "report it as timeout, unreachable, internal or other".to_owned(),
                    by: UnblockedBy::Agent,
                }),
            }))
        }
    }
}

fn failed(error: cockpit_bug_reports::Error) -> Failed {
    match error {
        cockpit_bug_reports::Error::Refused(refusal) => {
            let code = match &refusal {
                cockpit_bug_reports::Refusal::InvalidRequest { .. } => ErrorCode::InvalidArgument,
                cockpit_bug_reports::Refusal::NotFound { .. } => ErrorCode::NotFound,
                cockpit_bug_reports::Refusal::WrongStatus { .. } => ErrorCode::Conflict,
                cockpit_bug_reports::Refusal::SessionLimit { .. } => ErrorCode::Refused,
            };
            Failed(ProtocolError {
                code,
                message: refusal.to_string(),
                unblock: Some(Unblock {
                    action: refusal.next_action(),
                    by: if refusal.is_owners_to_lift() {
                        UnblockedBy::Owner
                    } else {
                        UnblockedBy::Agent
                    },
                }),
            })
        }
        error => {
            tracing::error!(%error, "a bug report request failed");
            Failed(ProtocolError::new(
                ErrorCode::Internal,
                "the daemon failed; the request may or may not have taken effect",
            ))
        }
    }
}

fn filed_view(filed: &Filed) -> BugFiledView {
    match filed {
        Filed::Created(_) => BugFiledView::Created,
        Filed::Merged(_) => BugFiledView::Merged,
        Filed::AlreadyHandled(_) => BugFiledView::AlreadyHandled,
    }
}

fn view(report: &Report, with_issue: bool) -> BugReportView {
    let issue = with_issue.then(|| {
        let issue = cockpit_bug_reports::issue(report);
        BugIssueView {
            repository: cockpit_bug_reports::REPOSITORY.to_owned(),
            title: issue.title,
            body: issue.body,
            new_issue_url: issue.new_issue_url,
            search_query: issue
                .search_url
                .is_some()
                .then(|| cockpit_bug_reports::search_query(report)),
            search_url: issue.search_url,
            is_truncated: issue.is_truncated,
        }
    });
    BugReportView {
        id: report.id.0,
        origin: match report.origin {
            cockpit_bug_reports::Origin::Owner => BugOriginView::Owner,
            cockpit_bug_reports::Origin::Agent => BugOriginView::Agent,
            cockpit_bug_reports::Origin::Daemon => BugOriginView::Daemon,
        },
        requested_by_owner: report.requested_by_owner,
        session: report.session.clone(),
        status: match report.status {
            cockpit_bug_reports::Status::Draft => BugStatusView::Draft,
            cockpit_bug_reports::Status::Sent => BugStatusView::Sent,
            cockpit_bug_reports::Status::Discarded => BugStatusView::Discarded,
        },
        title: report.title.clone(),
        description: report.description.clone(),
        steps: report.steps.clone(),
        fingerprint: report.fingerprint.text(),
        environment: report.environment.clone(),
        bundle: match &report.bundle {
            Bundle::Pending => BugBundleView::Pending,
            Bundle::Written { path, contents } => BugBundleView::Written {
                path: path.clone(),
                contents: contents.clone(),
            },
            Bundle::Failed { problem } => BugBundleView::Failed {
                problem: problem.clone(),
            },
        },
        occurrences: report.occurrences,
        issue_url: report.issue_url.clone(),
        created_at: report.created_at,
        changed_at: report.changed_at,
        issue,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_internal_answers_open_a_draft_at_the_third_within_the_window() {
        let mut internal = HashMap::new();
        let start = Instant::now();
        let fault = || Fault::Internal {
            command: "todo-add".to_owned(),
        };

        assert!(detected(fault(), &mut internal, start).is_none());
        assert!(detected(fault(), &mut internal, start + Duration::from_secs(1)).is_none());
        let third = detected(fault(), &mut internal, start + Duration::from_secs(2)).unwrap();

        assert_eq!(third.kind, Kind::Internal);
        assert_eq!(third.component.as_deref(), Some("todo-add"));
    }

    #[test]
    fn internal_answers_further_apart_than_the_window_never_add_up() {
        let mut internal = HashMap::new();
        let start = Instant::now();
        let fault = || Fault::Internal {
            command: "todo-add".to_owned(),
        };

        for step in 0..5 {
            let at = start + (INTERNAL_WINDOW / 2 + Duration::from_secs(1)) * step;
            assert!(
                detected(fault(), &mut internal, at).is_none(),
                "step {step}"
            );
        }
    }
}

//! View stamps on the change feed (#706): after each commit the daemon moves the stamp of every
//! [`ViewArea`] whose tables the commit touched, so a client asks for `project-list`,
//! `agent-reports`, the approval queue, the tasks or the workflows when they changed rather than
//! every second. The stamp carries no data: the answer to the request is still the truth.
//!
//! What a commit touched comes from Turso's change capture (`commits_after`), not from the
//! views, so a pass costs one read of the captured rows. Published like `bug_report_feed`
//! publishes drafts: in a transaction of its own right after the commit, by one task. The
//! stamp's own commit touches `messaging_entities`, which belongs to no area, so a pass never
//! wakes another.
//!
//! The views also depend on things no commit records (the runner's memory, the clock: a
//! question stops being held by a coordinator after a while), so clients keep a slow poll.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use cockpit_protocol::{ViewArea, ViewStamp};
use cockpit_turso::Database;

use crate::session_feed::state_of;

const VIEW_STAMP: &str = "view-stamp";
/// How long a pass that failed waits before it runs again, commit or not.
const RETRY: Duration = Duration::from_secs(1);
/// Commits read from the capture at a time.
const COMMITS_PER_READ: usize = 256;

/// The area that shows what the table `table` holds, if any.
fn area_of(table: &str) -> Option<ViewArea> {
    match table {
        "projects" | "projects_with_removal" => Some(ViewArea::Projects),
        // The reports also show who holds a question: the asking session and its coordinator.
        "sessions" | "authorization_grants" => Some(ViewArea::AgentReports),
        _ if table.starts_with("session_") => Some(ViewArea::AgentReports),
        _ if table.starts_with("approvals_") => Some(ViewArea::Approvals),
        _ if table.starts_with("tasks_") || table.starts_with("scheduling_") => {
            Some(ViewArea::Tasks)
        }
        _ if table.starts_with("workflows_") => Some(ViewArea::Workflows),
        _ => None,
    }
}

/// Publishes at start and after every commit. Runs until aborted.
pub(crate) async fn publish_forever(database: Arc<Database>) {
    // Watched before the first pass, so a commit during it runs another.
    let mut commits = database.watch_commits();
    // The last commit read; `None` before the first pass, which stamps every area: commits
    // may have touched them while the daemon was down.
    let mut read_through: Option<i64> = None;
    loop {
        if let Err(problem) = pass(&database, &mut read_through).await {
            tracing::warn!(problem, "publishing view stamps failed; retrying");
            tokio::time::sleep(RETRY).await;
            continue;
        }
        if commits.changed().await.is_err() {
            return;
        }
    }
}

async fn pass(database: &Database, read_through: &mut Option<i64>) -> Result<(), String> {
    let (newest, touched) = {
        let read = database.read().await.map_err(|error| error.to_string())?;
        let newest = read.revision().await.map_err(|error| error.to_string())?;
        let mut touched = BTreeMap::new();
        match *read_through {
            None => touched.extend(ViewArea::ALL.map(|area| (area, newest))),
            Some(mut after) => {
                while after < newest {
                    let commits = read
                        .commits_after(after, COMMITS_PER_READ)
                        .await
                        .map_err(|error| error.to_string())?;
                    let Some(last) = commits.last() else { break };
                    after = last.revision;
                    for commit in &commits {
                        for change in &commit.changes {
                            if let Some(area) = area_of(&change.table) {
                                touched.insert(area, commit.revision);
                            }
                        }
                    }
                }
            }
        }
        (newest, touched)
    };
    if !touched.is_empty() {
        write(database, &touched)
            .await
            .map_err(|error| error.to_string())?;
    }
    *read_through = Some(newest);
    Ok(())
}

async fn write(
    database: &Database,
    touched: &BTreeMap<ViewArea, i64>,
) -> Result<(), cockpit_messaging::Error> {
    let write = database.write().await?;
    let mut has_changed = false;
    for (area, revision) in touched {
        let stamp = ViewStamp {
            changed_at: u64::try_from(*revision).unwrap_or_default(),
        };
        let published =
            cockpit_messaging::publish(&write, VIEW_STAMP, area.id(), &state_of(&stamp)).await?;
        has_changed |= matches!(published, cockpit_messaging::Published::Changed { .. });
    }
    if has_changed {
        write.commit().await?;
    } else {
        write.rollback().await?;
    }
    Ok(())
}

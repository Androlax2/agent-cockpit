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
        // Network requests wait in the owner's queue beside run requests (#674).
        _ if table.starts_with("sandbox_network_") => Some(ViewArea::Approvals),
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
                    let commits = match read.commits_after(after, COMMITS_PER_READ).await {
                        Ok(commits) => commits,
                        Err(error) => {
                            // What the capture cannot tell is assumed changed: the retry
                            // stamps every area instead of reading it again.
                            *read_through = None;
                            return Err(error.to_string());
                        }
                    };
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

#[cfg(test)]
mod tests {
    use super::area_of;

    /// Tables no polled view reads. A table that is neither here nor in an area fails
    /// `every_table_is_in_an_area_or_shown_by_none`, so a new table is classified on purpose.
    const SHOWN_BY_NO_AREA: &[&str] = &[
        // Launches read the quota stops to choose an account; no view shows them (#59).
        "account_quota_stops",
        "agent_channel_launches",
        "authorization_audit",
        "authorization_revision",
        // Drafts have a feed kind of their own (`bug_report_feed`).
        "bug_reports",
        // Checkouts are on the feed as `checkout` entities (`session_feed`).
        "checkout_occupancy",
        "project_checkouts",
        "cockpit_database",
        "cockpit_migrations",
        "messaging_entities",
        "messaging_envelopes",
        "messaging_feed_retention",
        "messaging_receipts",
        "process_executions",
        "process_output_sets",
        "project_end_sweeps",
        "project_target_prune_guards",
        "project_target_prunes",
        "resource_reservations",
        "resource_settings",
        "sandbox_launch_lines",
        // Login grants are listed by `torlek account grants`; no polled view shows them (#59).
        "sandbox_login_grants",
        "sandbox_launches",
        // The digest reads these when the owner asks; no view polls them (#700).
        "usage_approvals",
        // `usage-cost` reads the cost archive when the owner asks; no view polls it (#699).
        "usage_cost_cursors",
        "usage_cost_replies",
        "usage_incidents",
        "usage_open_turns",
        // `usage-quota` reads the plan windows when the owner asks; no view polls them.
        "usage_quota_accounts",
        "usage_quota_alerts",
        "usage_quota_windows",
        "usage_turns",
        "verification_artifacts",
        "verification_claims",
        "verification_output_references",
        "verification_pending_artifacts",
        "verification_receipts",
        "verification_run_identities",
    ];

    /// The engine's own tables: counters and the change capture.
    const ENGINE_TABLE_PREFIXES: &[&str] = &["__turso_internal_", "sqlite_", "turso_cdc"];

    #[tokio::test]
    async fn every_table_is_in_an_area_or_shown_by_none() {
        let directory = tempfile::tempdir().unwrap();
        let database = cockpit_turso::Database::open(
            &directory.path().join("state.db"),
            &crate::MIGRATION_SETS,
        )
        .await
        .unwrap();
        let read = database.read().await.unwrap();
        let mut rows = read
            .query("SELECT name FROM sqlite_schema WHERE type = 'table'", ())
            .await
            .unwrap();
        let mut unclassified = Vec::new();
        while let Some(row) = rows.next().await.unwrap() {
            let table: String = row.get(0).unwrap();
            let is_classified = area_of(&table).is_some()
                || SHOWN_BY_NO_AREA.contains(&table.as_str())
                || ENGINE_TABLE_PREFIXES
                    .iter()
                    .any(|prefix| table.starts_with(prefix));
            if !is_classified {
                unclassified.push(table);
            }
        }

        unclassified.sort();
        assert!(
            unclassified.is_empty(),
            "map these tables to a ViewArea in area_of, or list them in SHOWN_BY_NO_AREA: {unclassified:?}"
        );
    }
}

//! Bug report drafts on the change feed (#471): after each commit the daemon publishes every
//! draft waiting for the owner as a `bug-report` entity and retracts the ones sent or
//! discarded, so the TUI shows them without polling. The daemon is the only producer of the
//! kind, and only the owner's socket subscribes to the feed.
//!
//! Published the way `session_feed` publishes sessions: in a transaction of its own right after
//! the commit that changed a draft, by one task, in order. A pass that finds what the last one
//! wrote takes no write transaction, so it does not wake itself.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use cockpit_turso::Database;

use crate::session_feed::{States, replace, state_of};

const BUG_REPORT: &str = "bug-report";
/// How long a pass that failed waits before it runs again, commit or not.
const RETRY: Duration = Duration::from_secs(1);

/// Publishes at start and after every commit. Runs until aborted.
pub(crate) async fn publish_forever(database: Arc<Database>) {
    // Watched before the first pass, so a commit during it runs another.
    let mut commits = database.watch_commits();
    let mut written: Option<States> = None;
    loop {
        let outcome = match draft_states(&database).await {
            Err(problem) => Err(format!("cannot read the drafts: {problem}")),
            Ok(drafts) if written.as_ref() == Some(&drafts) => Ok(()),
            Ok(drafts) => write(&database, &drafts)
                .await
                .map(|()| written = Some(drafts))
                .map_err(|error| error.to_string()),
        };
        if let Err(problem) = outcome {
            tracing::warn!(problem, "publishing bug report drafts failed; retrying");
            tokio::time::sleep(RETRY).await;
            continue;
        }
        if commits.changed().await.is_err() {
            return;
        }
    }
}

/// The drafts as `bug-report-list` answers them.
async fn draft_states(database: &Database) -> Result<States, String> {
    let read = database.read().await.map_err(|error| error.to_string())?;
    let drafts = cockpit_bug_reports::list(&read, false)
        .await
        .map_err(|error| error.to_string())?;
    Ok(drafts
        .iter()
        .map(|report| {
            let view = crate::bug_reports::view(report, false);
            (view.id.to_string(), state_of(&view))
        })
        .collect())
}

async fn write(database: &Database, drafts: &States) -> Result<(), cockpit_messaging::Error> {
    let write = database.write().await?;
    if replace(&write, BUG_REPORT, drafts, &BTreeSet::new()).await? {
        write.commit().await?;
    } else {
        write.rollback().await?;
    }
    Ok(())
}

//! Database retention (#406): sessions that ended or failed and command runs that finished
//! longer ago than the owner's retention lose their payloads, never their identity or audit
//! rows. The same worker checkpoints the WAL and keeps the newest backups. Decided in
//! docs/decisions/2026-11-04-406-database-retention.md.
use crate::routes::WorkClass;
use crate::storage::Context;
use cockpit_protocol::{
    BackupView, DatabaseStorageView, ErrorCode, Outcome, PayloadRetentionView, ProtocolError, Reply,
};
use std::num::NonZeroU64;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// Sessions retired per write transaction, so a pass never holds the writer for long.
const SESSION_PAGE: u32 = 64;
/// Run outputs cleared per write transaction.
const RUN_PAGE: u32 = 256;
/// Sessions one pass considers; the rest wait for the next pass and the reply says so.
const PASS_LIMIT: u64 = 16_384;
const DAY_SECONDS: i64 = 24 * 60 * 60;
const BACKUPS_DIR: &str = "backups";

/// The owner's `[storage] database_*` and `feed_history_changes` settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DatabasePolicy {
    pub(crate) retention_days: u32,
    pub(crate) automatic: bool,
    pub(crate) interval: Duration,
    pub(crate) backup_interval: Duration,
    /// 0 takes no backup and leaves the ones there.
    pub(crate) backups_kept: u32,
    pub(crate) backups_dir: PathBuf,
    pub(crate) feed_changes_kept: NonZeroU64,
}

impl DatabasePolicy {
    pub(crate) fn from_settings(
        settings: &cockpit_config::StorageSettings,
        state_dir: &std::path::Path,
    ) -> Self {
        Self {
            retention_days: settings.database_retention_days.value,
            automatic: settings.database_prune_automatic.value,
            interval: Duration::from_secs(
                u64::from(settings.database_maintenance_interval_minutes.value) * 60,
            ),
            backup_interval: Duration::from_secs(
                u64::from(settings.database_backup_interval_hours.value) * 60 * 60,
            ),
            backups_kept: settings.database_backups_kept.value,
            backups_dir: state_dir.join(BACKUPS_DIR),
            feed_changes_kept: NonZeroU64::new(settings.feed_history_changes.value)
                .expect("cockpit-config refuses a zero feed_history_changes"),
        }
    }

    /// What ended at or before this moment, in seconds since the Unix epoch, is retired.
    fn cutoff(&self, now: i64) -> i64 {
        now - i64::from(self.retention_days) * DAY_SECONDS
    }
}

fn internal(error: impl std::fmt::Display) -> ProtocolError {
    ProtocolError::new(ErrorCode::Internal, error.to_string())
}

/// Reports, and with `apply` retires, what the owner's policy selects.
pub(crate) async fn pass(context: &Context, apply: bool) -> Result<Reply, ProtocolError> {
    let policy = &context.retention;
    let now = crate::approvals::now();
    let cutoff = policy.cutoff(now);
    let (mut sessions, mut truncated) = selected_sessions(context, cutoff).await?;
    let mut run_outputs = selected_run_outputs(context, cutoff).await?;
    let mut failures = Vec::new();
    let mut wal_checkpoint_busy = false;
    if apply {
        let (retired, error) = retire_sessions(context, cutoff).await;
        sessions.retired = retired.retired;
        sessions.retired_bytes = retired.retired_bytes;
        failures.extend(error);
        let (retired, left_some, error) = retire_run_outputs(context, cutoff, now).await;
        run_outputs.retired = retired.retired;
        run_outputs.retired_bytes = retired.retired_bytes;
        truncated |= left_some;
        failures.extend(error);
        match context.database.checkpoint().await {
            Ok(checkpoint) => wal_checkpoint_busy = checkpoint.is_busy,
            Err(error) => failures.push(format!("WAL checkpoint: {error}")),
        }
    }
    let sizes = context.database.file_sizes().map_err(internal)?;
    let feed_changes = {
        let read = context.database.read().await.map_err(internal)?;
        read.retained_changes().await.map_err(internal)?
    };
    let backups = match cockpit_turso::backups(&policy.backups_dir) {
        Ok(backups) => backups.iter().map(backup_view).collect(),
        Err(error) => {
            failures.push(format!("backups: {error}"));
            Vec::new()
        }
    };
    Ok(Reply::StorageDatabase {
        database: Box::new(DatabaseStorageView {
            file_bytes: sizes.main_bytes,
            wal_bytes: sizes.wal_bytes,
            retention_days: policy.retention_days,
            sessions,
            run_outputs,
            feed_changes,
            feed_changes_kept: policy.feed_changes_kept.get(),
            backups,
            backups_kept: policy.backups_kept,
            truncated,
            wal_checkpoint_busy,
            failures,
        }),
        applied: apply,
    })
}

fn backup_view(backup: &cockpit_turso::Backup) -> BackupView {
    BackupView {
        path: backup.path.display().to_string(),
        taken_at: backup.taken_at,
        bytes: backup.bytes,
    }
}

/// The sessions whose launch payloads the policy selects, in one read snapshot, up to
/// [`PASS_LIMIT`]; and whether more were left out.
async fn selected_sessions(
    context: &Context,
    cutoff: i64,
) -> Result<(PayloadRetentionView, bool), ProtocolError> {
    let read = context.database.read().await.map_err(internal)?;
    let mut selected = PayloadRetentionView::default();
    let mut after = String::new();
    loop {
        let page = cockpit_sessions::retirable_launch_payloads(&read, cutoff, &after, SESSION_PAGE)
            .await
            .map_err(internal)?;
        for retirable in &page {
            if selected.selected == PASS_LIMIT {
                return Ok((selected, true));
            }
            selected.selected += 1;
            selected.selected_bytes += retirable.payload.bytes;
        }
        match page.last() {
            Some(last) if page.len() == SESSION_PAGE as usize => after.clone_from(&last.session),
            _ => return Ok((selected, false)),
        }
    }
}

async fn selected_run_outputs(
    context: &Context,
    cutoff: i64,
) -> Result<PayloadRetentionView, ProtocolError> {
    let read = context.database.read().await.map_err(internal)?;
    let outputs = cockpit_approvals::retirable_run_outputs(&read, cutoff)
        .await
        .map_err(internal)?;
    Ok(PayloadRetentionView {
        selected: outputs.runs,
        selected_bytes: outputs.bytes,
        ..PayloadRetentionView::default()
    })
}

/// Retires the selected sessions' payloads a page per write transaction, each rechecked under
/// the writer; up to [`PASS_LIMIT`]. Returns what went, and why it stopped early if it did.
async fn retire_sessions(context: &Context, cutoff: i64) -> (PayloadRetentionView, Option<String>) {
    let mut retired = PayloadRetentionView::default();
    let mut after = String::new();
    let mut considered = 0;
    loop {
        let page = match retire_session_page(context, cutoff, &after).await {
            Ok(page) => page,
            Err(error) => return (retired, Some(format!("sessions: {error}"))),
        };
        for payload in page.retired {
            retired.retired += 1;
            retired.retired_bytes += payload.bytes;
        }
        considered += page.considered;
        match page.last {
            Some(last) if page.considered == u64::from(SESSION_PAGE) && considered < PASS_LIMIT => {
                after = last;
            }
            _ => return (retired, None),
        }
    }
}

struct SessionPage {
    considered: u64,
    last: Option<String>,
    retired: Vec<cockpit_sessions::LaunchPayload>,
}

async fn retire_session_page(
    context: &Context,
    cutoff: i64,
    after: &str,
) -> Result<SessionPage, String> {
    let write = context
        .database
        .write_without_images()
        .await
        .map_err(|e| e.to_string())?;
    let listed = cockpit_sessions::retirable_sessions(&write, cutoff, after, SESSION_PAGE)
        .await
        .map_err(|e| e.to_string())?;
    let mut retired = Vec::new();
    for session in &listed {
        if let Some(payload) = cockpit_sessions::retire_launch_payload(&write, session, cutoff)
            .await
            .map_err(|e| e.to_string())?
        {
            retired.push(payload);
        }
    }
    write.commit().await.map_err(|e| e.to_string())?;
    Ok(SessionPage {
        considered: listed.len() as u64,
        last: listed.last().cloned(),
        retired,
    })
}

/// Clears the selected runs' output a page per write transaction until none is left, or
/// [`PASS_LIMIT`] are cleared. Returns what went, whether the bound left some, and why it
/// stopped early if it did.
async fn retire_run_outputs(
    context: &Context,
    cutoff: i64,
    now: i64,
) -> (PayloadRetentionView, bool, Option<String>) {
    let mut retired = PayloadRetentionView::default();
    while retired.retired < PASS_LIMIT {
        let page = async {
            let write = context
                .database
                .write_without_images()
                .await
                .map_err(|e| e.to_string())?;
            let page = cockpit_approvals::retire_run_outputs(&write, cutoff, now, RUN_PAGE)
                .await
                .map_err(|e| e.to_string())?;
            write.commit().await.map_err(|e| e.to_string())?;
            Ok::<_, String>(page)
        }
        .await;
        match page {
            Ok(page) if page.runs == 0 => return (retired, false, None),
            Ok(page) => {
                retired.retired += page.runs;
                retired.retired_bytes += page.bytes;
            }
            Err(error) => return (retired, false, Some(format!("run outputs: {error}"))),
        }
    }
    (retired, true, None)
}

/// Takes a backup when the newest one is older than the backup interval, then keeps the
/// newest `backups_kept`. Returns the backup taken, if one was.
async fn back_up_if_due(
    context: &Context,
    now: u64,
) -> Result<Option<cockpit_turso::Backup>, cockpit_turso::Error> {
    let policy = &context.retention;
    if policy.backups_kept == 0 {
        return Ok(None);
    }
    let newest = cockpit_turso::backups(&policy.backups_dir)?
        .first()
        .map(|backup| backup.taken_at);
    // A backup dated after now (the clock went back) does not make the next one wait.
    let is_due = newest.is_none_or(|taken_at| {
        taken_at > now || now - taken_at >= policy.backup_interval.as_secs()
    });
    if !is_due {
        return Ok(None);
    }
    let backup = context
        .database
        .backup_into(&policy.backups_dir, now)
        .await?;
    let kept = usize::try_from(policy.backups_kept).unwrap_or(usize::MAX);
    for removed in cockpit_turso::remove_backups_beyond(&policy.backups_dir, kept)? {
        tracing::info!(backup = %removed.path.display(), "removed an old database backup");
    }
    Ok(Some(backup))
}

/// At start and every `database_maintenance_interval_minutes`: checkpoints the WAL, takes a
/// backup when one is due, and runs the owner's opt-in pass on the Bulk lane. The checkpoint
/// and the backup take no lane place, as the feed's history discard takes none: holding a
/// Bulk place at daemon start would refuse the owner's first feed subscription. A backup
/// copies the whole database, so it waits while the low-disk guard reads the disk as low.
pub(crate) async fn maintain_forever(
    context: Context,
    shared: Arc<crate::server::Shared>,
    disk: Arc<cockpit_resource_admission::Backend>,
) {
    let mut ticks = tokio::time::interval(context.retention.interval);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticks.tick().await;
        match context.database.checkpoint().await {
            Ok(checkpoint) if checkpoint.is_busy => {
                tracing::debug!(
                    ?checkpoint,
                    "the WAL checkpoint was busy; the next one finishes it"
                );
            }
            Ok(_) => {}
            Err(error) => tracing::warn!(%error, "WAL checkpoint failed; retrying next period"),
        }
        let is_disk_low = disk
            .disk_space()
            .is_some_and(cockpit_resource_admission::DiskSpace::is_low);
        if is_disk_low {
            tracing::warn!("database backup waits: free disk space is below the floor");
        }
        let due = if is_disk_low {
            Ok(None)
        } else {
            back_up_if_due(&context, crate::approvals::now().unsigned_abs()).await
        };
        match due {
            Ok(Some(backup)) => tracing::info!(
                backup = %backup.path.display(),
                bytes = backup.bytes,
                "backed up the state database"
            ),
            Ok(None) => {}
            Err(error) => tracing::warn!(%error, "database backup failed; retrying next period"),
        }
        if context.retention.automatic {
            prune_automatically(&context, &shared).await;
        }
    }
}

async fn prune_automatically(context: &Context, shared: &crate::server::Shared) {
    let worker = context.clone();
    let work = Box::pin(async move {
        match pass(&worker, true).await {
            Ok(reply) => Outcome::Ok(reply),
            Err(error) => Outcome::Error(error),
        }
    });
    let outcome =
        match shared
            .lanes
            .admit(WorkClass::Bulk, "storage-database-automatic".into(), work)
        {
            Ok(work) => work.await,
            Err(outcome) => outcome,
        };
    match outcome {
        Outcome::Ok(Reply::StorageDatabase { database, .. }) => {
            if database.sessions.retired > 0 || database.run_outputs.retired > 0 {
                tracing::info!(
                    sessions = database.sessions.retired,
                    session_bytes = database.sessions.retired_bytes,
                    run_outputs = database.run_outputs.retired,
                    run_output_bytes = database.run_outputs.retired_bytes,
                    "retired old database payloads"
                );
            }
            for failure in &database.failures {
                tracing::warn!(%failure, "automatic database retention stopped early");
            }
        }
        Outcome::Ok(_) => {}
        Outcome::Error(error) => {
            tracing::warn!(error = %error.message, "automatic database retention deferred");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cutoff_is_the_retention_before_now() {
        let policy = DatabasePolicy {
            retention_days: 30,
            automatic: false,
            interval: Duration::from_secs(3600),
            backup_interval: Duration::from_secs(86_400),
            backups_kept: 7,
            backups_dir: PathBuf::from("/s/backups"),
            feed_changes_kept: NonZeroU64::new(1).unwrap(),
        };
        assert_eq!(policy.cutoff(100 * DAY_SECONDS), 70 * DAY_SECONDS);
    }
}

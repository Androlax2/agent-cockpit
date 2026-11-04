//! Owner-only storage policy; loading a value does not activate a cleanup operation.
use crate::{ConfigError, Layer, Source, Sourced};
use serde::Deserialize;
use toml::Spanned;

#[derive(Debug, Clone)]
pub struct StorageSettings {
    pub automatic: Sourced<bool>,
    pub projects: Sourced<Vec<String>>,
    pub confirm_above_mib: Sourced<u64>,
    pub interval_minutes: Sourced<u32>,
    pub base_ref: Sourced<Option<String>>,
    /// Check output older than this many days is retired (#406).
    pub output_retention_days: Sourced<u32>,
    /// Per project: past this, the oldest check output beyond the grace period is retired.
    pub output_budget_mib: Sourced<u64>,
    pub output_budget_grace_hours: Sourced<u32>,
    pub output_prune_automatic: Sourced<bool>,
    pub output_prune_projects: Sourced<Vec<String>>,
    pub output_prune_interval_minutes: Sourced<u32>,
    /// Sweep a clone when the session that occupied it ends (#767); on by default.
    pub sweep_on_session_end: Sourced<bool>,
    /// Free space below which launches and heavy jobs wait; 0 turns the guard off.
    pub low_disk_floor_mib: Sourced<u64>,
    /// Ended sessions and finished approval runs untouched for this many days lose their
    /// payloads; their identity and audit rows stay (#406).
    pub database_retention_days: Sourced<u32>,
    pub database_prune_automatic: Sourced<bool>,
    /// How often the WAL is checkpointed, a due backup taken and the opted-in pass run.
    pub database_maintenance_interval_minutes: Sourced<u32>,
    pub database_backup_interval_hours: Sourced<u32>,
    /// Newest backups kept under `<state>/backups`; 0 takes none.
    pub database_backups_kept: Sourced<u32>,
    /// Captured change rows kept for clients that resume; an older cursor gets a snapshot.
    pub feed_history_changes: Sourced<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StorageTable {
    target_prune_automatic: Option<Spanned<bool>>,
    target_prune_projects: Option<Spanned<Vec<String>>>,
    target_prune_confirm_above_mib: Option<Spanned<u64>>,
    target_prune_interval_minutes: Option<Spanned<u32>>,
    target_prune_base_ref: Option<Spanned<String>>,
    output_retention_days: Option<Spanned<u32>>,
    output_budget_mib: Option<Spanned<u64>>,
    output_budget_grace_hours: Option<Spanned<u32>>,
    output_prune_automatic: Option<Spanned<bool>>,
    output_prune_projects: Option<Spanned<Vec<String>>>,
    output_prune_interval_minutes: Option<Spanned<u32>>,
    sweep_on_session_end: Option<Spanned<bool>>,
    low_disk_floor_mib: Option<Spanned<u64>>,
    database_retention_days: Option<Spanned<u32>>,
    database_prune_automatic: Option<Spanned<bool>>,
    database_maintenance_interval_minutes: Option<Spanned<u32>>,
    database_backup_interval_hours: Option<Spanned<u32>>,
    database_backups_kept: Option<Spanned<u32>>,
    feed_history_changes: Option<Spanned<u64>>,
}

pub(super) fn settings(layers: &[Layer]) -> Result<StorageSettings, ConfigError> {
    let mut result = StorageSettings {
        automatic: Sourced::new(false, Source::ProductDefault),
        projects: Sourced::new(Vec::new(), Source::ProductDefault),
        confirm_above_mib: Sourced::new(1024, Source::ProductDefault),
        interval_minutes: Sourced::new(60, Source::ProductDefault),
        base_ref: Sourced::new(None, Source::ProductDefault),
        output_retention_days: Sourced::new(14, Source::ProductDefault),
        output_budget_mib: Sourced::new(1024, Source::ProductDefault),
        output_budget_grace_hours: Sourced::new(24, Source::ProductDefault),
        output_prune_automatic: Sourced::new(false, Source::ProductDefault),
        output_prune_projects: Sourced::new(Vec::new(), Source::ProductDefault),
        output_prune_interval_minutes: Sourced::new(60, Source::ProductDefault),
        sweep_on_session_end: Sourced::new(true, Source::ProductDefault),
        low_disk_floor_mib: Sourced::new(10 * 1024, Source::ProductDefault),
        database_retention_days: Sourced::new(30, Source::ProductDefault),
        database_prune_automatic: Sourced::new(false, Source::ProductDefault),
        database_maintenance_interval_minutes: Sourced::new(60, Source::ProductDefault),
        database_backup_interval_hours: Sourced::new(24, Source::ProductDefault),
        database_backups_kept: Sourced::new(7, Source::ProductDefault),
        feed_history_changes: Sourced::new(100_000, Source::ProductDefault),
    };
    for layer in layers {
        let Some(table) = &layer.settings.storage else {
            continue;
        };
        let table = table.get_ref();
        if let Some(value) = &table.target_prune_automatic {
            result.automatic = Sourced::new(*value.get_ref(), layer.source_at(value.span()));
        }
        if let Some(value) = &table.target_prune_projects {
            result.projects = Sourced::new(value.get_ref().clone(), layer.source_at(value.span()));
        }
        if let Some(value) = &table.target_prune_confirm_above_mib {
            result.confirm_above_mib =
                Sourced::new(*value.get_ref(), layer.source_at(value.span()));
        }
        if let Some(value) = &table.target_prune_interval_minutes {
            result.interval_minutes = Sourced::new(*value.get_ref(), layer.source_at(value.span()));
        }
        if let Some(value) = &table.target_prune_base_ref {
            result.base_ref =
                Sourced::new(Some(value.get_ref().clone()), layer.source_at(value.span()));
        }
        if let Some(value) = &table.output_retention_days {
            result.output_retention_days =
                Sourced::new(*value.get_ref(), layer.source_at(value.span()));
        }
        if let Some(value) = &table.output_budget_mib {
            result.output_budget_mib =
                Sourced::new(*value.get_ref(), layer.source_at(value.span()));
        }
        if let Some(value) = &table.output_budget_grace_hours {
            result.output_budget_grace_hours =
                Sourced::new(*value.get_ref(), layer.source_at(value.span()));
        }
        if let Some(value) = &table.output_prune_automatic {
            result.output_prune_automatic =
                Sourced::new(*value.get_ref(), layer.source_at(value.span()));
        }
        if let Some(value) = &table.output_prune_projects {
            result.output_prune_projects =
                Sourced::new(value.get_ref().clone(), layer.source_at(value.span()));
        }
        if let Some(value) = &table.output_prune_interval_minutes {
            result.output_prune_interval_minutes =
                Sourced::new(*value.get_ref(), layer.source_at(value.span()));
        }
        if let Some(value) = &table.sweep_on_session_end {
            result.sweep_on_session_end =
                Sourced::new(*value.get_ref(), layer.source_at(value.span()));
        }
        if let Some(value) = &table.low_disk_floor_mib {
            result.low_disk_floor_mib =
                Sourced::new(*value.get_ref(), layer.source_at(value.span()));
        }
        if let Some(value) = &table.database_retention_days {
            result.database_retention_days =
                Sourced::new(*value.get_ref(), layer.source_at(value.span()));
        }
        if let Some(value) = &table.database_prune_automatic {
            result.database_prune_automatic =
                Sourced::new(*value.get_ref(), layer.source_at(value.span()));
        }
        if let Some(value) = &table.database_maintenance_interval_minutes {
            result.database_maintenance_interval_minutes =
                Sourced::new(*value.get_ref(), layer.source_at(value.span()));
        }
        if let Some(value) = &table.database_backup_interval_hours {
            result.database_backup_interval_hours =
                Sourced::new(*value.get_ref(), layer.source_at(value.span()));
        }
        if let Some(value) = &table.database_backups_kept {
            result.database_backups_kept =
                Sourced::new(*value.get_ref(), layer.source_at(value.span()));
        }
        if let Some(value) = &table.feed_history_changes {
            result.feed_history_changes =
                Sourced::new(*value.get_ref(), layer.source_at(value.span()));
        }
    }
    let invalid = |key: &str, source: &Source, reason: &str| ConfigError::Invalid {
        key: format!("storage.{key}"),
        source: source.clone(),
        reason: reason.to_owned(),
    };
    if !is_stored_mib(result.confirm_above_mib.value) {
        return Err(invalid(
            "target_prune_confirm_above_mib",
            &result.confirm_above_mib.source,
            "must be positive and fit the stored byte range",
        ));
    }
    if result
        .low_disk_floor_mib
        .value
        .checked_mul(1024 * 1024)
        .is_none_or(|bytes| i64::try_from(bytes).is_err())
    {
        return Err(invalid(
            "low_disk_floor_mib",
            &result.low_disk_floor_mib.source,
            "must fit the byte range of a disk size",
        ));
    }
    if result.interval_minutes.value == 0 {
        return Err(invalid(
            "target_prune_interval_minutes",
            &result.interval_minutes.source,
            "must be at least 1 minute",
        ));
    }
    for (key, projects) in [
        ("target_prune_projects", &result.projects),
        ("output_prune_projects", &result.output_prune_projects),
    ] {
        if !is_project_allowlist(&projects.value) {
            return Err(invalid(
                key,
                &projects.source,
                "must list at most 128 plain registered project names",
            ));
        }
    }
    if result.output_retention_days.value == 0 {
        return Err(invalid(
            "output_retention_days",
            &result.output_retention_days.source,
            "must be at least 1 day",
        ));
    }
    if !is_stored_mib(result.output_budget_mib.value) {
        return Err(invalid(
            "output_budget_mib",
            &result.output_budget_mib.source,
            "must be positive and fit the stored byte range",
        ));
    }
    if result.output_prune_interval_minutes.value == 0 {
        return Err(invalid(
            "output_prune_interval_minutes",
            &result.output_prune_interval_minutes.source,
            "must be at least 1 minute",
        ));
    }
    for (key, value, reason) in [
        (
            "database_retention_days",
            &result.database_retention_days,
            "must be at least 1 day",
        ),
        (
            "database_maintenance_interval_minutes",
            &result.database_maintenance_interval_minutes,
            "must be at least 1 minute",
        ),
        (
            "database_backup_interval_hours",
            &result.database_backup_interval_hours,
            "must be at least 1 hour",
        ),
    ] {
        if value.value == 0 {
            return Err(invalid(key, &value.source, reason));
        }
    }
    if result.database_backups_kept.value > MAX_BACKUPS_KEPT {
        return Err(invalid(
            "database_backups_kept",
            &result.database_backups_kept.source,
            "must be at most 1000",
        ));
    }
    if result.feed_history_changes.value == 0
        || i64::try_from(result.feed_history_changes.value).is_err()
    {
        return Err(invalid(
            "feed_history_changes",
            &result.feed_history_changes.source,
            "must be positive and fit the stored revision range",
        ));
    }
    if let Some(base) = &result.base_ref.value
        && (!base.starts_with("refs/")
            || base.len() > 1024
            || base.chars().any(|c| c.is_control() || c.is_whitespace()))
    {
        return Err(invalid(
            "target_prune_base_ref",
            &result.base_ref.source,
            "must be an explicit full LOCAL ref, such as refs/heads/main",
        ));
    }
    Ok(result)
}

const MAX_BACKUPS_KEPT: u32 = 1000;

fn is_stored_mib(mib: u64) -> bool {
    mib != 0
        && mib
            .checked_mul(1024 * 1024)
            .is_some_and(|bytes| i64::try_from(bytes).is_ok())
}

fn is_project_allowlist(projects: &[String]) -> bool {
    projects.len() <= 128
        && projects.iter().all(|name| {
            !name.is_empty()
                && name.len() <= 64
                && !name.starts_with(['.', '-'])
                && name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        })
}
